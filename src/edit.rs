//! Fixing the detected panels by hand before the book is cut.
//!
//! The panels of every page can be written to a JSON file (`--detect-only`) and read back
//! (`--panels`), or edited in the browser in the same run (`--edit`): move and resize panels, draw
//! new ones, delete, merge, split, set the reading order, keep a page whole. The cut then uses the
//! edited panels.

use crate::detect::{Panel, Rect, Shape};
use crate::review::{open_browser, read_request, respond, source_jpeg};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

const PAGE: &str = include_str!("editor.html");

/// The panels of a book, as written to and read from a panel file.
#[derive(Serialize, Deserialize)]
pub struct PanelFile {
    pub version: u32,
    pub pages: Vec<FilePage>,
}

#[derive(Serialize, Deserialize)]
pub struct FilePage {
    /// The page image's file name (a check that the file belongs to the book).
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub panels: Vec<FilePanel>,
}

#[derive(Serialize, Deserialize)]
pub struct FilePanel {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// The exact outline, for a panel that is not a plain rectangle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<Shape>,
}

/// One page as the editor and the panel file see it.
pub struct EditPage<'a> {
    pub source: &'a Path,
    pub width: u32,
    pub height: u32,
    pub panels: &'a mut Vec<Panel>,
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Write the panels of the book to `path`.
pub fn write(path: &Path, pages: &[(PathBuf, u32, u32, Vec<Panel>)]) -> Result<()> {
    let file = PanelFile {
        version: 1,
        pages: pages
            .iter()
            .map(|(source, w, h, panels)| FilePage {
                name: file_name(source),
                width: *w,
                height: *h,
                panels: panels.iter().map(|p| FilePanel { x: p.rect.x, y: p.rect.y, w: p.rect.w, h: p.rect.h, shape: p.shape.clone() }).collect(),
            })
            .collect(),
    };
    std::fs::write(path, serde_json::to_vec_pretty(&file)?).with_context(|| format!("writing {}", path.display()))
}

/// Read a panel file for a book whose pages are `sources` (in order), giving each page's panels.
pub fn read(path: &Path, sources: &[(PathBuf, u32, u32)]) -> Result<Vec<Vec<Panel>>> {
    let text = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let file: PanelFile = serde_json::from_slice(&text).with_context(|| format!("{} is not a panel file", path.display()))?;
    if file.pages.len() != sources.len() {
        bail!("{} has {} page(s), the book {}", path.display(), file.pages.len(), sources.len());
    }
    let mut out = Vec::with_capacity(sources.len());
    for (i, (page, (source, w, h))) in file.pages.into_iter().zip(sources).enumerate() {
        if (page.width, page.height) != (*w, *h) {
            bail!("page {} of {} is {}x{}, the book's is {w}x{h} ({})", i + 1, path.display(), page.width, page.height, file_name(source));
        }
        out.push(
            page.panels
                .into_iter()
                .filter(|p| p.w > 0 && p.h > 0 && p.x < *w && p.y < *h)
                .map(|p| {
                    let rect = Rect { x: p.x, y: p.y, w: p.w.min(w - p.x), h: p.h.min(h - p.y) };
                    Panel::new(rect, p.shape, Vec::new())
                })
                .collect(),
        );
    }
    Ok(out)
}

/// Whether the panel file at `path` is for the book whose pages are `sources` (in order): the same
/// number of pages, of the same sizes. Only the image headers are read. Says why when it isn't.
pub fn fits(path: &Path, sources: &[PathBuf]) -> bool {
    let file = match std::fs::read(path).map_err(anyhow::Error::from).and_then(|t| Ok(serde_json::from_slice::<PanelFile>(&t)?)) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("warning: ignoring the saved panels in {}: {e:#}", path.display());
            return false;
        }
    };
    let size = |p: &Path| image::ImageReader::open(p).ok()?.with_guessed_format().ok()?.into_dimensions().ok();
    let why = if file.pages.len() != sources.len() {
        Some(format!("they are for {} page(s), the book has {}", file.pages.len(), sources.len()))
    } else {
        file.pages.iter().zip(sources).position(|(page, s)| size(s) != Some((page.width, page.height))).map(|i| format!("page {} has another size", i + 1))
    };
    match why {
        None => {
            println!("Using the panels fixed by hand earlier ({})", path.display());
            true
        }
        Some(why) => {
            println!("The panels saved in {} don't fit this book ({why}): detecting them again", path.display());
            false
        }
    }
}

/// A panel as the browser sends it back: its box, and the index of the detected panel it is
/// (unchanged) when it is one.
#[derive(Deserialize)]
struct Edited {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    #[serde(default)]
    orig: Option<usize>,
    /// The panel's traced outline (page coordinates), for a panel that is not a rectangle.
    #[serde(default)]
    outline: Vec<[f64; 2]>,
}

#[derive(Deserialize)]
struct EditedPage {
    panels: Vec<Edited>,
}

/// The edited panels of a page: an unchanged detected panel keeps its outline and details; a
/// moved, resized or new one is a rectangle.
fn apply(edited: &EditedPage, detected: &[Panel], w: u32, h: u32) -> Vec<Panel> {
    edited
        .panels
        .iter()
        .filter_map(|e| {
            let x0 = e.x.round().clamp(0.0, w as f64) as u32;
            let y0 = e.y.round().clamp(0.0, h as f64) as u32;
            let x1 = (e.x + e.w).round().clamp(0.0, w as f64) as u32;
            let y1 = (e.y + e.h).round().clamp(0.0, h as f64) as u32;
            if x1 <= x0 + 1 || y1 <= y0 + 1 {
                return None;
            }
            let rect = Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
            if e.outline.len() >= 3 {
                return Some(Panel::new(rect, Some(traced(&rect, &e.outline)), Vec::new()));
            }
            Some(match e.orig.and_then(|i| detected.get(i)) {
                Some(d) if d.rect == rect => d.clone(),
                _ => Panel::new(rect, None, Vec::new()),
            })
        })
        .collect()
}

/// The shape of a panel traced in the editor: inside both its box and the outline, at a quarter of
/// the page resolution.
fn traced(r: &Rect, outline: &[[f64; 2]]) -> Shape {
    const SCALE: u32 = 4;
    let (mw, mh) = ((r.w / SCALE).max(1) as usize, (r.h / SCALE).max(1) as usize);
    let mut m = crate::detect::Mask::new(mw, mh);
    for y in 0..mh {
        let py = (r.y + y as u32 * SCALE + SCALE / 2) as f64;
        // Crossings of this row with the outline's edges: inside between pairs (even-odd).
        let mut xs: Vec<f64> = (0..outline.len())
            .filter_map(|i| {
                let ([x0, y0], [x1, y1]) = (outline[i], outline[(i + 1) % outline.len()]);
                ((y0 <= py) != (y1 <= py)).then(|| x0 + (py - y0) * (x1 - x0) / (y1 - y0))
            })
            .collect();
        xs.sort_by(f64::total_cmp);
        for x in 0..mw {
            let px = (r.x + x as u32 * SCALE + SCALE / 2) as f64;
            m.data[y * mw + x] = xs.iter().filter(|&&c| c < px).count() % 2 == 1;
        }
    }
    let s = SCALE as f32;
    Shape::from_mask(&m, r.x as f32, r.y as f32, s, s)
}

/// The editor's view of the book.
fn state(input: &Path, rtl: bool, pages: &[EditPage]) -> String {
    #[derive(Serialize)]
    struct P<'a> {
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        doubtful: bool,
        shaped: bool,
        issues: Vec<&'a str>,
    }
    #[derive(Serialize)]
    struct Pg<'a> {
        page: usize,
        name: String,
        width: u32,
        height: u32,
        panels: Vec<P<'a>>,
    }
    #[derive(Serialize)]
    struct S<'a> {
        input: String,
        rtl: bool,
        pages: Vec<Pg<'a>>,
    }
    let s = S {
        input: input.display().to_string(),
        rtl,
        pages: pages
            .iter()
            .enumerate()
            .map(|(i, p)| Pg {
                page: i + 1,
                name: file_name(p.source),
                width: p.width,
                height: p.height,
                panels: p
                    .panels
                    .iter()
                    .map(|q| P {
                        x: q.rect.x,
                        y: q.rect.y,
                        w: q.rect.w,
                        h: q.rect.h,
                        doubtful: q.is_low(),
                        shaped: q.shape.is_some(),
                        issues: q.issues.iter().map(|i| i.label()).collect(),
                    })
                    .collect(),
            })
            .collect(),
    };
    serde_json::to_string(&s).unwrap_or_default()
}

/// Edit the panels in the browser; returns when the editor's Cut button is pressed, with each
/// page's panels replaced by the edited ones.
/// `browser`: open the editor in the browser (else a front end shows it, from the `@@edit` line).
pub fn serve(input: &Path, rtl: bool, browser: bool, pages: &mut [EditPage]) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").context("starting the panel editor")?;
    let url = format!("http://{}/", listener.local_addr()?);
    if browser {
        println!("Fix the panels at {url}, then click Cut the book");
        open_browser(&url);
    } else {
        println!("@@edit {url}");
    }
    // The detected panels, kept to match unchanged ones against.
    let detected: Vec<Vec<Panel>> = pages.iter().map(|p| p.panels.clone()).collect();
    let mut state_json = state(input, rtl, pages);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let req = match read_request(&stream) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("warning: panel editor: {e:#}");
                continue;
            }
        };
        let page_no = |s: &str| s.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).filter(|&i| i < pages.len());
        let reply = match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/") => respond(stream, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()),
            ("GET", "/api/state") => respond(stream, "200 OK", "application/json", state_json.as_bytes()),
            ("GET", p) if p.starts_with("/image/") => match page_no(&p[7..]) {
                Some(i) => match source_jpeg(pages[i].source) {
                    Ok(jpeg) => respond(stream, "200 OK", "image/jpeg", &jpeg),
                    Err(e) => respond(stream, "500 Internal Server Error", "text/plain", format!("{e:#}").as_bytes()),
                },
                None => respond(stream, "404 Not Found", "text/plain", b"no such page"),
            },
            // The whole book's panels: [{panels: [...]}, ...], one entry per page.
            ("POST", "/api/save" | "/api/cut") => match serde_json::from_slice::<Vec<EditedPage>>(&req.body) {
                Ok(edited) if edited.len() == pages.len() => {
                    for ((page, e), d) in pages.iter_mut().zip(&edited).zip(&detected) {
                        *page.panels = apply(e, d, page.width, page.height);
                    }
                    state_json = state(input, rtl, pages);
                    let cut = req.path == "/api/cut";
                    respond(stream, "200 OK", "application/json", b"{\"ok\":true}")?;
                    if cut {
                        break;
                    }
                    Ok(())
                }
                Ok(_) => respond(stream, "400 Bad Request", "text/plain", b"page count does not match"),
                Err(e) => respond(stream, "400 Bad Request", "text/plain", format!("{e}").as_bytes()),
            },
            _ => respond(stream, "404 Not Found", "text/plain", b"not found"),
        };
        if let Err(e) = reply {
            eprintln!("warning: panel editor: {e:#}");
        }
    }
    let n: usize = pages.iter().map(|p| p.panels.len()).sum();
    println!("Edited panels: {n} panel(s) on {} page(s)", pages.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Issue;

    #[test]
    fn panel_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("panels.json");
        let src = PathBuf::from("p1.png");
        let panels = vec![Panel::new(Rect { x: 1, y: 2, w: 30, h: 40 }, None, vec![Issue::Split])];
        write(&path, &[(src.clone(), 100, 200, panels)]).unwrap();
        let back = read(&path, &[(src.clone(), 100, 200)]).unwrap();
        assert_eq!(back[0][0].rect, Rect { x: 1, y: 2, w: 30, h: 40 });
        assert!(read(&path, &[(src, 100, 201)]).is_err(), "size mismatch");
    }

    // Saved panels are used only for the pages they were fixed on.
    #[test]
    fn kept_panels_fit_only_their_pages() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.png"), dir.path().join("b.png"));
        image::GrayImage::new(100, 200).save(&a).unwrap();
        image::GrayImage::new(100, 150).save(&b).unwrap();
        let path = dir.path().join("panels.json");
        write(&path, &[(a.clone(), 100, 200, vec![])]).unwrap();
        assert!(fits(&path, std::slice::from_ref(&a)));
        assert!(!fits(&path, std::slice::from_ref(&b)), "another page size");
        assert!(!fits(&path, &[a.clone(), a.clone()]), "another page count");
        std::fs::write(&path, "not json").unwrap();
        assert!(!fits(&path, &[a]), "not a panel file");
    }

    #[test]
    fn unchanged_panels_keep_their_details() {
        let detected = vec![Panel::new(Rect { x: 10, y: 10, w: 50, h: 50 }, None, vec![Issue::Split])];
        let e = |x: f64, orig| Edited { x, y: 10.0, w: 50.0, h: 50.0, orig, outline: vec![] };
        let out = apply(&EditedPage { panels: vec![e(10.0, Some(0)), e(70.0, Some(0)), e(5.0, None), Edited { x: 0.0, y: 0.0, w: 0.5, h: 9.0, orig: None, outline: vec![] }] }, &detected, 200, 200);
        assert_eq!(out.len(), 3, "the empty box is dropped");
        assert_eq!(out[0].issues, vec![Issue::Split], "unchanged");
        assert!(out[1].issues.is_empty() && out[1].rect.x == 70, "moved");
    }

    #[test]
    fn a_traced_panel_is_cut_along_its_outline() {
        // A panel traced as an L: its top-right part is left out.
        let outline = vec![[0.0, 0.0], [200.0, 0.0], [200.0, 300.0], [400.0, 300.0], [400.0, 600.0], [0.0, 600.0]];
        let traced = Edited { x: 0.0, y: 0.0, w: 400.0, h: 600.0, orig: None, outline };
        let out = apply(&EditedPage { panels: vec![traced] }, &[], 400, 600);
        let s = out[0].shape.as_ref().expect("an outline");
        assert!(s.alpha(100.0, 100.0) > 0.5 && s.alpha(300.0, 500.0) > 0.5, "inside the outline");
        assert!(s.alpha(300.0, 100.0) < 0.5, "outside it");
    }
}
