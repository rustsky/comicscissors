//! Review of a debug run in the browser.
//!
//! After a conversion with `--debug-dir`, a small local web page shows every page with its
//! detected panels. Pages the detection got wrong are tagged there: what went wrong, a note, and
//! optionally the boxes of the panels that should have been found. The tags are saved to
//! `review.json` in the debug folder, and the original of each tagged page is copied to
//! `sources/` (pages unpacked from a CBZ or PDF live in a temporary folder that is gone after the
//! run), so the failures can be turned into regression tests later.

use crate::detect::Panel;
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

const PAGE: &str = include_str!("review.html");

/// One source page as the review shows it.
pub struct Page<'a> {
    pub source: &'a Path,
    pub width: u32,
    pub height: u32,
    pub panels: &'a [Panel],
}

/// Serve the review page until it is closed with Done (or the process is interrupted).
pub fn serve(dir: &Path, input: &Path, pages: &[Page]) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").context("starting the review server")?;
    let url = format!("http://{}/", listener.local_addr()?);
    println!("Review the detection at {url}");
    println!("Tag the pages that came out wrong, then click Done (tags are saved as you go to {}).", dir.join("review.json").display());
    open_browser(&url);
    let state = state_json(dir, input, pages);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        match handle(stream, dir, pages, &state) {
            Ok(true) => break,
            Ok(false) => {}
            Err(e) => eprintln!("warning: review server: {e:#}"),
        }
    }
    println!("Review saved to {}", dir.join("review.json").display());
    Ok(())
}

/// One HTTP request: method, path, query string and body.
pub(crate) struct Request {
    pub method: String,
    pub path: String,
    pub query: String,
    pub body: Vec<u8>,
}

/// Read one request from a browser.
pub(crate) fn read_request(stream: &TcpStream) -> Result<Request> {
    // A browser may open a connection it never uses: don't wait on it forever.
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let mut length = 0;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = header.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Ok(Request { method: method.into(), path: path.into(), query: query.into(), body })
}

/// Send a response and close.
pub(crate) fn respond(mut stream: TcpStream, status: &str, kind: &str, content: &[u8]) -> Result<()> {
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", content.len())?;
    stream.write_all(content)?;
    Ok(())
}

/// Answer one request. Returns true when the review is done.
fn handle(stream: TcpStream, dir: &Path, pages: &[Page], state: &str) -> Result<bool> {
    let req = read_request(&stream)?;
    let (method, path, query, body) = (req.method.as_str(), req.path.as_str(), req.query.as_str(), req.body);
    let mut done = false;
    let (status, kind, content): (&str, &str, Vec<u8>) = match (method, path) {
        ("GET", "/") => ("200 OK", "text/html; charset=utf-8", PAGE.as_bytes().to_vec()),
        ("GET", "/api/state") => ("200 OK", "application/json", state.as_bytes().to_vec()),
        ("GET", p) if p.starts_with("/debug/") => match page_index(&p[7..], pages) {
            Some(i) => ("200 OK", "image/jpeg", std::fs::read(dir.join(debug_name(i)))?),
            None => ("404 Not Found", "text/plain", b"no such page".to_vec()),
        },
        ("GET", p) if p.starts_with("/source/") => match page_index(&p[8..], pages) {
            Some(i) => ("200 OK", "image/jpeg", source_jpeg(pages[i].source)?),
            None => ("404 Not Found", "text/plain", b"no such page".to_vec()),
        },
        ("POST", "/api/save" | "/api/done") => {
            // `failed=3,7`: the 1-based pages tagged, whose originals are kept.
            let failed = query.strip_prefix("failed=").unwrap_or("");
            for n in failed.split(',').filter_map(|s| s.parse::<usize>().ok()) {
                if let Some(page) = n.checked_sub(1).and_then(|i| pages.get(i)) {
                    keep_source(dir, n, page.source)?;
                }
            }
            std::fs::write(dir.join("review.json"), &body)?;
            done = path == "/api/done";
            ("200 OK", "application/json", b"{\"ok\":true}".to_vec())
        }
        _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
    };
    respond(stream, status, kind, &content)?;
    Ok(done)
}

fn page_index(n: &str, pages: &[Page]) -> Option<usize> {
    n.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).filter(|&i| i < pages.len())
}

/// The debug image `main` writes for page `i` (0-based).
fn debug_name(i: usize) -> String {
    format!("page{:04}.jpg", i + 1)
}

/// Copy of the original page `n` (1-based) under the debug folder, relative to it.
fn source_copy(n: usize, source: &Path) -> PathBuf {
    let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("png");
    Path::new("sources").join(format!("page{n:04}.{ext}"))
}

fn keep_source(dir: &Path, n: usize, source: &Path) -> Result<()> {
    let to = dir.join(source_copy(n, source));
    if !to.exists() {
        std::fs::create_dir_all(dir.join("sources"))?;
        std::fs::copy(source, &to).with_context(|| format!("copying {}", source.display()))?;
    }
    Ok(())
}

/// The page without outlines, as JPEG (the original may be TIFF or another format browsers
/// don't show).
pub(crate) fn source_jpeg(source: &Path) -> Result<Vec<u8>> {
    let img = crate::output::load_image(source)?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img.to_rgb8()).write_to(&mut out, image::ImageFormat::Jpeg)?;
    Ok(out.into_inner())
}

/// Everything the page needs: the pages with their panels, and an earlier review of this folder
/// to continue from.
fn state_json(dir: &Path, input: &Path, pages: &[Page]) -> String {
    let pages: Vec<String> = pages
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let panels: Vec<String> = p
                .panels
                .iter()
                .map(|q| {
                    let issues: Vec<String> = q.issues.iter().map(|i| json_str(i.label())).collect();
                    format!(
                        "{{\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"confidence\":{:.2},\"doubtful\":{},\"issues\":[{}]}}",
                        q.rect.x,
                        q.rect.y,
                        q.rect.w,
                        q.rect.h,
                        q.confidence,
                        q.is_low(),
                        issues.join(",")
                    )
                })
                .collect();
            format!(
                "{{\"page\":{},\"source\":{},\"source_copy\":{},\"debug_image\":{},\"width\":{},\"height\":{},\"detected\":[{}]}}",
                i + 1,
                json_str(&p.source.display().to_string()),
                json_str(&source_copy(i + 1, p.source).display().to_string()),
                json_str(&debug_name(i)),
                p.width,
                p.height,
                panels.join(",")
            )
        })
        .collect();
    let previous = std::fs::read_to_string(dir.join("review.json")).ok().filter(|s| s.trim_start().starts_with('{'));
    format!(
        "{{\"input\":{},\"debug_dir\":{},\"pages\":[{}],\"previous\":{}}}",
        json_str(&input.display().to_string()),
        json_str(&dir.display().to_string()),
        pages.join(","),
        previous.as_deref().unwrap_or("null")
    )
}

pub(crate) fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) fn open_browser(url: &str) {
    let result = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if result.is_err() {
        println!("(open {url} in a browser)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json_str("a \"b\"\\c\nd\u{1}"), r#""a \"b\"\\c\nd\u0001""#);
    }

    // The review server answers the page, the state and a save, and stops on Done.
    #[test]
    fn serves_state_and_saves_the_review() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("p1.png");
        image::GrayImage::new(40, 40).save(&src).unwrap();
        let panels = vec![Panel::new(crate::detect::Rect { x: 1, y: 2, w: 30, h: 20 }, None, vec![])];
        let pages = [Page { source: &src, width: 40, height: 40, panels: &panels }];
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let request = move |req: String| {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(req.as_bytes()).unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        };
        let client = std::thread::spawn(move || {
            let state = request("GET /api/state HTTP/1.1\r\n\r\n".into());
            let body = "{\"pages\":[{\"page\":1}]}";
            let done = request(format!("POST /api/done?failed=1 HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}", body.len()));
            (state, done)
        });
        let state = state_json(dir.path(), Path::new("book.cbz"), &pages);
        for stream in listener.incoming() {
            if handle(stream.unwrap(), dir.path(), &pages, &state).unwrap() {
                break;
            }
        }
        let (state, done) = client.join().unwrap();
        assert!(state.contains("\"detected\":[{\"x\":1,\"y\":2,\"w\":30,\"h\":20"), "{state}");
        assert!(done.starts_with("HTTP/1.1 200"), "{done}");
        assert_eq!(std::fs::read_to_string(dir.path().join("review.json")).unwrap(), "{\"pages\":[{\"page\":1}]}");
        assert!(dir.path().join("sources/page0001.png").exists());
    }
}
