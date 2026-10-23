mod detect;
mod devices;
mod epub;
mod input;
mod layout;
mod ocr;
mod output;
mod progress;
mod refine;
mod spreads;
mod webtoon;

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use indicatif::ParallelProgressIterator;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq)]
enum Format {
    /// PDF sized to the e-reader screen (copy over USB; works on every e-reader).
    Pdf,
    /// Comic book zip of JPEG pages (KOReader, Kindle Comic Converter, etc.).
    Cbz,
    /// Fixed-layout EPUB (Kobo, PocketBook, Tolino, Boox, Apple Books; Send to Kindle).
    Epub,
    /// Fixed-layout EPUB saved as .kepub.epub for Kobo's faster reader.
    Kepub,
    /// Kindle book (KF8), converted from the EPUB with calibre's ebook-convert.
    Azw3,
    /// A folder of JPEG pages.
    Images,
}

/// Split comic pages into panels and repack them so they are readable on an e-reader without zooming.
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// Comic to convert: .pdf, .cbz/.zip, .cbr/.cb7, an image, or a folder of images.
    input: Option<PathBuf>,

    /// Output path (default: <input>_repanel.<ext>, next to the input).
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// E-reader model (see --list-devices). Asked interactively when omitted.
    #[arg(short, long)]
    device: Option<String>,

    /// List known e-reader models and exit.
    #[arg(long)]
    list_devices: bool,

    /// Custom screen width in pixels (with --height and --ppi instead of --device).
    #[arg(long, requires_all = ["height", "ppi"])]
    width: Option<u32>,
    #[arg(long, requires_all = ["width", "ppi"])]
    height: Option<u32>,
    #[arg(long, requires_all = ["width", "height"])]
    ppi: Option<u32>,
    /// Keep colour (default: only for colour e-readers).
    #[arg(long)]
    color: bool,

    #[arg(short, long, value_enum, default_value_t = Format::Pdf)]
    format: Format,

    /// Manga mode: read panels right to left.
    #[arg(long)]
    rtl: bool,

    /// Minimum on-screen panel size relative to the printed comic (1.0 = print size).
    /// Higher = bigger text, fewer panels per page.
    #[arg(short = 's', long, default_value_t = 0.8)]
    readable_scale: f32,

    /// Height of the printed source page in inches (US comic 10.2, manga ~7.2, bande dessinée ~11.7).
    #[arg(long, default_value_t = 10.1875)]
    print_height: f32,

    /// Crop every panel to its bounding box, even when it isn't rectangular.
    /// By default slanted or irregular panels are cut along their exact outline.
    #[arg(long)]
    rect_crop: bool,

    /// Language of the comic's text, e.g. en, fr, de, es, ja, fa (or a Tesseract code).
    /// Asked interactively together with the e-reader model; defaults to English.
    #[arg(short, long)]
    lang: Option<String>,

    /// Don't add the invisible OCR text layer used for dictionary lookup.
    #[arg(long)]
    no_ocr: bool,

    /// Skip the second detection round for doubtful panels.
    #[arg(long)]
    no_refine: bool,

    /// Rotate wide panels to landscape when that makes them notably larger.
    #[arg(long)]
    rotate: bool,

    /// Never mix panels from different source pages on one e-reader page.
    #[arg(long)]
    page_break: bool,

    /// Show each original page whole before its panels.
    #[arg(long)]
    overview: bool,

    /// Webtoon (vertical scroll comic): join the slices, cut between panels at background gaps
    /// and show them at screen width. Chosen automatically for pages over 3x taller than wide.
    #[arg(long)]
    webtoon: bool,

    /// Keep two-page spreads (landscape scans of an open book) whole instead of splitting them
    /// at the centre gutter into two pages.
    #[arg(long)]
    keep_spreads: bool,

    /// Split the first page into panels too, instead of keeping it whole as the cover.
    #[arg(long)]
    no_cover: bool,

    /// Keep each page's levels; by default the paper is made white and the darkest ink black.
    #[arg(long)]
    no_auto_levels: bool,

    /// Book title (default: the input file name).
    #[arg(long)]
    title: Option<String>,

    /// Book author, written into the PDF/EPUB/AZW3 metadata.
    #[arg(long)]
    author: Option<String>,

    /// Screen margin in pixels.
    #[arg(long, default_value_t = 8)]
    margin: u32,

    /// Space between panels in pixels.
    #[arg(long, default_value_t = 14)]
    gap: u32,

    /// Maximum factor panels are enlarged to fill leftover page space.
    #[arg(long, default_value_t = 2.0)]
    max_upscale: f32,

    /// Gamma applied to output (>1 darkens; 1.0 = unchanged). E-ink often benefits from ~1.4.
    #[arg(long, default_value_t = 1.0)]
    gamma: f32,

    /// JPEG quality of output pages (30-100). Lower makes much smaller files; e-ink shows little
    /// difference down to about 70.
    #[arg(short, long, default_value_t = 88, value_parser = clap::value_parser!(u8).range(30..=100))]
    quality: u8,

    /// Ink detection threshold (luma difference from the paper colour).
    #[arg(long, default_value_t = 48)]
    threshold: u8,

    /// Split panels joined by balloons along lines covered at most this fraction (0-1).
    /// Higher splits more aggressively.
    #[arg(long, default_value_t = 0.35)]
    split_coverage: f32,

    /// Drop detected regions smaller than this fraction of the page area.
    #[arg(long, default_value_t = 0.01)]
    min_panel_area: f32,

    /// Resolution used to rasterise PDF pages.
    #[arg(long, default_value_t = 300)]
    pdf_dpi: u32,

    /// Write each source page with the detected panels outlined into this folder.
    #[arg(long)]
    debug_dir: Option<PathBuf>,

    /// Print progress as `@@progress <stage>|<pos>|<len>` lines on stdout (for front ends).
    #[arg(long, hide = true)]
    progress_lines: bool,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    if args.progress_lines {
        progress::enable_lines();
    }
    if args.list_devices {
        devices::print_list();
        return Ok(());
    }
    let Some(input) = args.input.clone() else {
        bail!("missing INPUT (see --help)");
    };
    if !(0.1..=4.0).contains(&args.readable_scale) {
        bail!("--readable-scale must be between 0.1 and 4.0");
    }

    let device = match (&args.device, args.width) {
        (Some(id), _) => devices::find(id)
            .with_context(|| format!("unknown device '{id}' (see --list-devices)"))?,
        (None, Some(width)) => devices::Device {
            brand: "custom".into(),
            id: format!("{width}x{}", args.height.unwrap()),
            name: "custom".into(),
            width,
            height: args.height.unwrap(),
            ppi: args.ppi.unwrap(),
            color: args.color,
        },
        (None, None) => devices::prompt()?,
    };
    let color = device.color || args.color;
    println!("Target: {} ({}x{} @ {} ppi)", device.name, device.width, device.height, device.ppi);
    let prompted = args.device.is_none() && args.width.is_none();
    let lang = match &args.lang {
        Some(l) => l.clone(),
        None if prompted && !args.no_ocr && args.format == Format::Pdf => ocr::prompt()?,
        None => "en".into(),
    };
    // OCR text layer: PDF only, and only when Tesseract has the language.
    let ocr_lang = if args.no_ocr || args.format != Format::Pdf {
        None
    } else {
        match ocr::resolve(&lang) {
            Ok((code, name)) => {
                println!("Text layer: {name} (dictionary lookup enabled)");
                Some(code)
            }
            // A language the user picked must work: silently writing a PDF without
            // text would only show up later as a dictionary that does nothing.
            Err(e) if args.lang.is_some() || prompted => {
                bail!("{e:#}\n(or pass --no-ocr to convert without the dictionary text layer)")
            }
            Err(e) => {
                eprintln!("warning: {e:#}; continuing without a text layer");
                None
            }
        }
    };

    let start = Instant::now();
    let tmp = tempfile::tempdir()?;
    let pb = progress::spinner("Loading", "");
    let input::Pages { paths: mut pages, mut chapters } = input::collect_pages(&input, tmp.path(), args.pdf_dpi, &pb)?;
    pb.finish_and_clear();
    println!("Loaded {} source page(s)", pages.len());
    let webtoon = args.webtoon || webtoon::looks_like_webtoon(&pages);
    if webtoon {
        let pb = progress::spinner("Loading", "cutting the webtoon strip");
        let usable = |v: u32| v.saturating_sub(2 * args.margin).max(1) as f32;
        let aspect = usable(device.height) / usable(device.width);
        let cut = webtoon::cut(&pages, &chapters, &tmp.path().join("webtoon"), aspect)?;
        pb.finish_and_clear();
        println!("Webtoon: cut into {} piece(s) between panels", cut.paths.len());
        (pages, chapters) = (cut.paths, cut.chapters);
    } else if !args.keep_spreads {
        let (split, split_chapters, n) = spreads::split(&pages, &chapters, &tmp.path().join("spreads"), args.rtl)?;
        if n > 0 {
            println!("Split {n} two-page spread(s) into single pages");
            (pages, chapters) = (split, split_chapters);
        }
    }

    // 1. Detect panels on every page in parallel.
    let det = detect::DetectOptions {
        threshold: args.threshold,
        split_coverage: args.split_coverage,
        min_panel_area: args.min_panel_area,
        padding: 0.004,
        exact_shapes: !args.rect_crop,
        rtl: args.rtl,
    };
    if let Some(d) = &args.debug_dir {
        std::fs::create_dir_all(d)?;
    }
    let pb = progress::bar("Detecting", pages.len(), if ocr_lang.is_some() { "pages (panels + text)" } else { "pages" });
    struct Analysed {
        path: PathBuf,
        width: u32,
        height: u32,
        panels: Vec<detect::Panel>,
        words: Vec<ocr::Word>,
        chapter: String,
        levels: Option<(u8, u8)>,
    }
    // A page that can't be read is skipped with a warning instead of failing the book.
    let analysed: Vec<Option<Analysed>> = pages
        .par_iter()
        .zip(&chapters)
        .progress_with(pb.clone())
        .map(|(p, chapter)| -> Result<Option<Analysed>> {
            let img = match output::load_image(p) {
                Ok(img) => img,
                Err(e) => {
                    pb.suspend(|| eprintln!("warning: skipping unreadable page {}: {e:#}", p.display()));
                    return Ok(None);
                }
            };
            // A webtoon piece is one panel (or a few stacked ones) already.
            let panels = if webtoon {
                let rect = detect::Rect { x: 0, y: 0, w: img.width(), h: img.height() };
                vec![detect::Panel::new(rect, None, Vec::new())]
            } else {
                let mut panels = detect::detect_panels(&img, &det);
                detect::drop_edge_strips(&mut panels, img.width(), img.height());
                panels
            };
            let words = match &ocr_lang {
                Some(lang) => ocr::recognize(&img, lang).with_context(|| format!("OCR of {}", p.display()))?,
                None => Vec::new(),
            };
            let levels = if args.no_auto_levels { None } else { output::auto_levels(&img) };
            Ok(Some(Analysed {
                path: p.clone(),
                width: img.width(),
                height: img.height(),
                panels,
                words,
                chapter: chapter.clone(),
                levels,
            }))
        })
        .collect::<Result<_>>()?;
    pb.finish_and_clear();
    let mut analysed: Vec<Analysed> = analysed.into_iter().flatten().collect();
    if analysed.is_empty() {
        bail!("no readable pages");
    }

    // Book-level check: a panel much bigger than the book's typical one, on a page with
    // other panels, may be several panels merged.
    let mut fractions: Vec<f32> = analysed
        .iter()
        .flat_map(|a| a.panels.iter().map(move |p| p.rect.area() as f32 / (a.width * a.height) as f32))
        .collect();
    fractions.sort_by(f32::total_cmp);
    let median = fractions[fractions.len() / 2];
    for a in &mut analysed {
        let page_area = (a.width * a.height) as f32;
        if a.panels.len() > 1 {
            for p in &mut a.panels {
                let ratio = p.rect.area() as f32 / page_area / median;
                if ratio > 2.5 {
                    p.add_issue(detect::Issue::Oversized);
                } else if ratio > 1.5 {
                    p.add_issue(detect::Issue::Large);
                }
            }
        }
    }
    let n_panels: usize = analysed.iter().map(|a| a.panels.len()).sum();
    let doubtful = |a: &[Analysed]| a.iter().flat_map(|a| &a.panels).filter(|p| p.is_low()).count();
    let low_before = doubtful(&analysed);
    if ocr_lang.is_some() {
        let n_words: usize = analysed.iter().map(|a| a.words.len()).sum();
        println!("Detected {n_panels} panel(s) ({low_before} doubtful), recognised {n_words} word(s)");
    } else {
        println!("Detected {n_panels} panel(s) ({low_before} doubtful)");
    }

    // Second round on pages with doubtful panels.
    if !args.no_refine && low_before > 0 {
        let todo: Vec<usize> = (0..analysed.len()).filter(|&i| std::env::var("CS_FORCE").is_ok() || analysed[i].panels.iter().any(|p| p.is_low())).collect();
        let pb = progress::bar("Refining", todo.len(), "pages");
        let results: Vec<(usize, Vec<detect::Panel>, refine::Stats)> = todo
            .par_iter()
            .progress_with(pb.clone())
            .map(|&i| -> Result<_> {
                let img = output::load_image(&analysed[i].path)?;
                let (panels, stats) = refine::refine_page(&img, analysed[i].panels.clone(), args.rtl, det.padding);
                Ok((i, panels, stats))
            })
            .collect::<Result<_>>()?;
        pb.finish_and_clear();
        let mut total = refine::Stats::default();
        for (i, panels, st) in results {
            analysed[i].panels = panels;
            let (pw, ph) = (analysed[i].width, analysed[i].height);
            detect::drop_edge_strips(&mut analysed[i].panels, pw, ph);
            total.split += st.split;
            total.pieces += st.pieces;
            total.merged += st.merged;
            total.dropped += st.dropped;
        }
        println!(
            "Second round: split {} panel(s) into {}, merged {} fragment(s), dropped {} sliver(s); {} still doubtful",
            total.split,
            total.pieces,
            total.merged,
            total.dropped,
            doubtful(&analysed)
        );
    }

    if let Some(d) = &args.debug_dir {
        let pb = progress::bar("Debug", analysed.len(), "pages");
        analysed.par_iter().enumerate().progress_with(pb.clone()).try_for_each(|(i, a)| -> Result<()> {
            let img = output::load_image(&a.path)?;
            detect::debug_image(&img, &a.panels).save(d.join(format!("page{:04}.jpg", i + 1)))?;
            detect::debug_masks(img.width(), img.height(), &a.panels).save(d.join(format!("page{:04}_masks.png", i + 1)))?;
            Ok(())
        })?;
        pb.finish_and_clear();
        std::fs::write(d.join("report.txt"), report(&analysed.iter().map(|a| (&a.path, &a.panels)).collect::<Vec<_>>()))?;
    }

    let pages: Vec<PathBuf> = analysed.iter().map(|a| a.path.clone()).collect();
    let mut sources = Vec::new();
    let mut shapes = Vec::new();
    let mut words = Vec::new();
    let mut levels = Vec::new();
    let mut rects = Vec::new();
    let mut chapters = Vec::new();
    for a in analysed {
        levels.push(a.levels);
        rects.push(a.panels.iter().map(|p| p.rect).collect::<Vec<_>>());
        chapters.push(a.chapter);
        sources.push(layout::SourcePage { width: a.width, height: a.height, fit_width: webtoon, panels: a.panels.iter().map(|p| p.rect).collect() });
        shapes.push(a.panels.into_iter().map(|p| p.shape).collect::<Vec<_>>());
        words.push(a.words);
    }

    // 2. Pack panels into e-reader pages.
    let lo = layout::LayoutOptions {
        screen_w: device.width,
        screen_h: device.height,
        ppi: device.ppi,
        margin: args.margin,
        gap: args.gap,
        readable_scale: args.readable_scale,
        print_height_in: args.print_height,
        max_upscale: args.max_upscale.max(1.0),
        rotate: args.rotate,
        page_break: args.page_break,
        overview: args.overview,
        // A single page is the whole comic, not a cover, and a webtoon has no cover page.
        cover: !args.no_cover && sources.len() > 1 && !webtoon,
        rtl: args.rtl,
    };
    let out_pages = layout::layout(&sources, &lo);

    // 3. Render.
    let ro = output::RenderOptions {
        width: device.width,
        height: device.height,
        color,
        gamma: args.gamma,
        quality: args.quality,
    };
    let pb = progress::bar("Rendering", out_pages.len(), "pages");
    let rendered: Vec<Vec<u8>> = out_pages
        .par_iter()
        .progress_with(pb.clone())
        .map(|p| output::render_page(p, &pages, &rects, &shapes, &levels, &ro))
        .collect::<Result<_>>()?;
    pb.finish_and_clear();

    let out_path = args.output.clone().unwrap_or_else(|| default_output(&input, args.format));
    let title = args.title.as_deref().unwrap_or_else(|| input.file_stem().and_then(|s| s.to_str()).unwrap_or("comic"));
    let author = args.author.as_deref();
    let toc = table_of_contents(&out_pages, &chapters, lo.cover);
    let pb = progress::spinner("Writing", &out_path.display().to_string());
    match args.format {
        Format::Pdf => {
            let texts: Vec<_> = out_pages.iter().map(|p| ocr::place_words(p, &words, &shapes)).collect();
            let tag = ocr_lang.as_ref().map(|_| ocr::bcp47(&lang));
            let info = output::DocInfo { title, author, lang: tag.as_deref(), toc: &toc };
            output::write_pdf(&out_path, &rendered, &texts, &ro, device.ppi, &info)?
        }
        Format::Cbz => output::write_cbz(&out_path, &rendered)?,
        Format::Epub | Format::Kepub | Format::Azw3 => {
            let tag = ocr::bcp47(&lang);
            let book = epub::Book {
                title,
                author,
                toc: &toc,
                lang: &tag,
                width: device.width,
                height: device.height,
                rtl: args.rtl,
            };
            if args.format == Format::Azw3 {
                epub::write_azw3(&out_path, &rendered, &book)?
            } else {
                epub::write_epub(&out_path, &rendered, &book)?
            }
        }
        Format::Images => output::write_images(&out_path, &rendered)?,
    }
    pb.finish_and_clear();
    println!(
        "Wrote {} page(s) to {} in {:.1}s",
        rendered.len(),
        out_path.display(),
        start.elapsed().as_secs_f32()
    );
    Ok(())
}

/// Bookmarks: one per chapter when the pages came from two or more folders, otherwise one per
/// source page. Each points to the first output page showing that source page.
/// The cover gets its own entry.
fn table_of_contents(out_pages: &[layout::OutPage], chapters: &[String], cover: bool) -> Vec<output::TocEntry> {
    let mut first = vec![usize::MAX; chapters.len()];
    for (i, p) in out_pages.iter().enumerate() {
        for it in &p.items {
            first[it.page] = first[it.page].min(i);
        }
    }
    let by_chapter = chapters.iter().collect::<std::collections::HashSet<_>>().len() > 1;
    let mut toc: Vec<output::TocEntry> = Vec::new();
    let mut last: Option<&str> = None;
    for (pi, &page) in first.iter().enumerate() {
        if page == usize::MAX {
            continue;
        }
        if cover && pi == 0 {
            // The first chapter's entry then points to its first page after the cover.
            toc.push(output::TocEntry { title: "Cover".into(), page });
            continue;
        }
        let title = if by_chapter {
            if last == Some(chapters[pi].as_str()) {
                continue;
            }
            if chapters[pi].is_empty() { "Start".to_string() } else { chapters[pi].clone() }
        } else {
            format!("Page {}", pi + 1)
        };
        last = Some(&chapters[pi]);
        toc.push(output::TocEntry { title, page });
    }
    toc
}

/// Per-page list of panels with confidence and reasons, doubtful ones marked.
fn report(pages: &[(&PathBuf, &Vec<detect::Panel>)]) -> String {
    let mut out = String::new();
    for (i, (path, panels)) in pages.iter().enumerate() {
        let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
        out.push_str(&format!("page {} ({name}): {} panel(s)\n", i + 1, panels.len()));
        for (k, p) in panels.iter().enumerate() {
            let why: Vec<&str> = p.issues.iter().map(|i| i.label()).collect();
            let mark = if p.is_low() { "  DOUBTFUL" } else { "" };
            out.push_str(&format!(
                "  #{:<2} conf {:.2}  {}x{} at {},{}{}{}\n",
                k + 1,
                p.confidence,
                p.rect.w,
                p.rect.h,
                p.rect.x,
                p.rect.y,
                if why.is_empty() { String::new() } else { format!("  [{}]", why.join(", ")) },
                mark
            ));
        }
    }
    out
}

fn default_output(input: &Path, format: Format) -> PathBuf {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("comic");
    let name = match format {
        Format::Pdf => format!("{stem}_repanel.pdf"),
        Format::Cbz => format!("{stem}_repanel.cbz"),
        Format::Epub => format!("{stem}_repanel.epub"),
        Format::Kepub => format!("{stem}_repanel.kepub.epub"),
        Format::Azw3 => format!("{stem}_repanel.azw3"),
        Format::Images => format!("{stem}_repanel"),
    };
    input.parent().unwrap_or(Path::new(".")).join(name)
}
