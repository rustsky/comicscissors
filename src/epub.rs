//! Fixed-layout EPUB 3 (also saved as KEPUB for Kobo), and AZW3 for Kindle made from it
//! with calibre's `ebook-convert`.
//!
//! Every page is one screen-sized JPEG on its own fixed-layout page, with the words OCR found
//! laid over it as invisible text, so the reader's dictionary can look them up (the PDF's
//! text layer, in HTML). calibre writes reflowable
//! AZW3 only, so the fixed-layout records are added to its header afterwards; Kindles then show
//! the pages full screen, without margins.

use crate::ocr::PlacedWord;
use crate::output::TocEntry;
use anyhow::{Context, Result, bail};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Book<'a> {
    pub title: &'a str,
    pub author: Option<&'a str>,
    /// Chapters or pages to list in the table of contents (empty: just the start).
    pub toc: &'a [TocEntry],
    /// BCP 47 language tag.
    pub lang: &'a str,
    pub width: u32,
    pub height: u32,
    pub rtl: bool,
    /// The words on each page (none: no text layer).
    pub texts: &'a [Vec<PlacedWord>],
}

pub fn write_epub(path: &Path, pages: &[Vec<u8>], b: &Book) -> Result<()> {
    let mut zip = zip::ZipWriter::new(fs::File::create(path)?);
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // The mimetype must come first, uncompressed.
    zip.start_file("mimetype", stored)?;
    zip.write_all(b"application/epub+zip")?;
    zip.start_file("META-INF/container.xml", deflated)?;
    zip.write_all(CONTAINER.as_bytes())?;

    let title = xml_escape(b.title);
    let (w, h) = (b.width, b.height);
    for (i, jpeg) in pages.iter().enumerate() {
        let n = i + 1;
        zip.start_file(format!("OEBPS/images/{n:05}.jpg"), stored)?;
        zip.write_all(jpeg)?;
        zip.start_file(format!("OEBPS/pages/{n:05}.xhtml"), deflated)?;
        write!(
            zip,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="{lang}" lang="{lang}">
<head>
<title>{title} {n}</title>
<meta name="viewport" content="width={w}, height={h}"/>
<style>html, body {{ margin: 0; padding: 0; width: {w}px; height: {h}px; }} img {{ display: block; width: {w}px; height: {h}px; }}
div.page {{ position: relative; width: {w}px; height: {h}px; }}
span.w {{ position: absolute; display: block; overflow: hidden; white-space: nowrap; line-height: 1; margin: 0; padding: 0; color: transparent; color: rgba(0,0,0,0); }}</style>
</head>
<body><div class="page"><img src="../images/{n:05}.jpg" alt="{title} {n}"/>{text}</div></body>
</html>
"#,
            lang = b.lang,
            text = b.texts.get(i).map(|t| text_layer(t)).unwrap_or_default()
        )?;
    }

    let mut toc: Vec<(String, usize)> =
        b.toc.iter().filter(|e| e.page < pages.len()).map(|e| (xml_escape(&e.title), e.page + 1)).collect();
    if toc.is_empty() {
        toc.push((title.clone(), 1));
    }
    let nav_items: String =
        toc.iter().map(|(t, n)| format!("<li><a href=\"pages/{n:05}.xhtml\">{t}</a></li>\n")).collect();
    let nav_points: String = toc
        .iter()
        .enumerate()
        .map(|(k, (t, n))| {
            format!(
                "<navPoint id=\"n{}\" playOrder=\"{}\"><navLabel><text>{t}</text></navLabel>\
                 <content src=\"pages/{n:05}.xhtml\"/></navPoint>\n",
                k + 1,
                k + 1
            )
        })
        .collect();

    zip.start_file("OEBPS/nav.xhtml", deflated)?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="{lang}" lang="{lang}">
<head><title>{title}</title></head>
<body><nav epub:type="toc" id="toc"><ol>
{nav_items}</ol></nav></body>
</html>
"#,
        lang = b.lang
    )?;

    let id = book_id();
    zip.start_file("OEBPS/toc.ncx", deflated)?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
<head><meta name="dtb:uid" content="{id}"/></head>
<docTitle><text>{title}</text></docTitle>
<navMap>
{nav_points}</navMap>
</ncx>
"#
    )?;

    let mut manifest = String::new();
    let mut spine = String::new();
    for n in 1..=pages.len() {
        let cover = if n == 1 { r#" properties="cover-image""# } else { "" };
        manifest.push_str(&format!(
            "<item id=\"p{n}\" href=\"pages/{n:05}.xhtml\" media-type=\"application/xhtml+xml\"/>\n\
             <item id=\"img{n}\" href=\"images/{n:05}.jpg\" media-type=\"image/jpeg\"{cover}/>\n"
        ));
        spine.push_str(&format!("<itemref idref=\"p{n}\"/>\n"));
    }
    let (direction, mode) = if b.rtl { ("rtl", "horizontal-rl") } else { ("ltr", "horizontal-lr") };
    zip.start_file("OEBPS/content.opf", deflated)?;
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid" prefix="rendition: http://www.idpf.org/vocab/rendition/#">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:identifier id="bookid">{id}</dc:identifier>
<dc:title>{title}</dc:title>
<dc:language>{lang}</dc:language>
{creator}<dc:contributor>comic-scissors</dc:contributor>
<meta property="dcterms:modified">{modified}</meta>
<meta property="rendition:layout">pre-paginated</meta>
<meta property="rendition:orientation">portrait</meta>
<meta property="rendition:spread">none</meta>
<meta name="cover" content="img1"/>
<meta name="fixed-layout" content="true"/>
<meta name="original-resolution" content="{w}x{h}"/>
<meta name="book-type" content="comic"/>
<meta name="primary-writing-mode" content="{mode}"/>
<meta name="zero-gutter" content="true"/>
<meta name="zero-margin" content="true"/>
<meta name="orientation-lock" content="portrait"/>
<meta name="region-mag" content="false"/>
</metadata>
<manifest>
<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
{manifest}</manifest>
<spine toc="ncx" page-progression-direction="{direction}">
{spine}</spine>
</package>
"#,
        lang = b.lang,
        modified = utc_now(),
        creator = b.author.map(|a| format!("<dc:creator>{}</dc:creator>\n", xml_escape(a))).unwrap_or_default()
    )?;

    // Lets front ends (the Calibre plugin) tell our output from an original book.
    zip.set_comment("comic-scissors")?;
    zip.finish()?;
    Ok(())
}

/// The words of a page as invisible text over its image: each word a box over the word in the
/// picture, so a tap or a selection there gives the word. Spaces between them keep words apart
/// for the dictionary.
fn text_layer(words: &[PlacedWord]) -> String {
    let mut out = String::new();
    for word in words.iter().filter(|w| !w.text.trim().is_empty()) {
        let chars = word.text.chars().count().max(1) as f32;
        // Letters are about half as wide as high: shrink the font when the word wouldn't fit its box.
        let size = word.size.min(word.advance / (0.55 * chars)).max(1.0);
        // The box's top-left from the baseline start; a word running downwards is turned a
        // quarter clockwise around that corner.
        let style = if word.down {
            format!(
                "left:{:.0}px;top:{:.0}px;width:{:.0}px;height:{:.0}px;font-size:{size:.1}px;transform:rotate(90deg);transform-origin:0 0;-webkit-transform:rotate(90deg);-webkit-transform-origin:0 0",
                word.x + word.size * 0.85,
                word.y,
                word.advance,
                word.size
            )
        } else {
            format!(
                "left:{:.0}px;top:{:.0}px;width:{:.0}px;height:{:.0}px;font-size:{size:.1}px",
                word.x,
                word.y - word.size * 0.85,
                word.advance,
                word.size
            )
        };
        out.push_str(&format!("<span class=\"w\" style=\"{style}\">{}</span> ", xml_escape(&word.text)));
    }
    out
}

/// Write the pages as an EPUB next to `path`, then convert it to AZW3 with calibre.
pub fn write_azw3(path: &Path, pages: &[Vec<u8>], b: &Book) -> Result<()> {
    let convert = ebook_convert().context(
        "AZW3 output needs calibre's ebook-convert: install calibre (https://calibre-ebook.com) \
         or put ebook-convert on PATH",
    )?;
    let dir = tempfile::tempdir()?;
    let epub = dir.path().join("book.epub");
    write_epub(&epub, pages, b)?;
    let out = Command::new(&convert)
        .arg(&epub)
        .arg(path)
        .arg("--no-inline-toc")
        // Marks the AZW3 as ours (EXTH contributor), like the PDF producer and CBZ/EPUB comment.
        .args(["--book-producer", "comic-scissors"])
        // The only profile that never resizes images: pages are already sized for the screen.
        .args(["--output-profile", "tablet"])
        // Keep the text layer's font sizes in pixels, so each word fills its box, and the pages
        // without the default margins.
        .arg("--disable-font-rescaling")
        .args(["--margin-left", "0", "--margin-right", "0", "--margin-top", "0", "--margin-bottom", "0"])
        .output()
        .with_context(|| format!("running {}", convert.display()))?;
    if !out.status.success() {
        let log = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = log.lines().rev().take(15).collect();
        bail!("ebook-convert failed:\n{}", tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
    }
    let data = fs::read(path)?;
    let fixed = mark_fixed_layout(&data, b.width, b.height).context("adding fixed-layout records to the AZW3")?;
    fs::write(path, fixed)?;
    Ok(())
}

/// Add the EXTH records kindlegen writes for comics, fixed-layout (122) and
/// original-resolution (307), to a KF8-only MOBI file.
///
/// Layout: a Palm database header with the record offset list, then record 0 holding the
/// PalmDOC header (16 bytes), the MOBI header, the EXTH block and the book's full name.
fn mark_fixed_layout(data: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let u16_at = |o: usize| -> Result<u16> {
        Ok(u16::from_be_bytes(data.get(o..o + 2).context("truncated")?.try_into()?))
    };
    let u32_at = |o: usize| -> Result<u32> {
        Ok(u32::from_be_bytes(data.get(o..o + 4).context("truncated")?.try_into()?))
    };
    if data.get(60..68) != Some(b"BOOKMOBI") {
        bail!("not a MOBI file");
    }
    let n_records = u16_at(76)? as usize;
    let rec_offset = |i: usize| -> Result<usize> { Ok(u32_at(78 + 8 * i)? as usize) };
    let r0 = rec_offset(0)?;
    let mobi = r0 + 16;
    if data.get(mobi..mobi + 4) != Some(b"MOBI") {
        bail!("no MOBI header");
    }
    if u32_at(mobi + 0x70)? & 0x40 == 0 {
        bail!("no EXTH block");
    }
    let exth = mobi + u32_at(mobi + 4)? as usize;
    if data.get(exth..exth + 4) != Some(b"EXTH") {
        bail!("EXTH block not found");
    }
    // The full name follows the (padded) EXTH block; its offset is relative to record 0.
    let name_offset = u32_at(mobi + 0x44)? as usize;
    let exth_end = r0 + name_offset;

    let mut records: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut pos = exth + 12;
    for _ in 0..u32_at(exth + 8)? {
        let (kind, len) = (u32_at(pos)?, u32_at(pos + 4)? as usize);
        let body = data.get(pos + 8..pos + len).context("truncated EXTH record")?;
        if kind != 122 && kind != 307 {
            records.push((kind, body.to_vec()));
        }
        pos += len;
    }
    records.push((122, b"true".to_vec()));
    records.push((307, format!("{width}x{height}").into_bytes()));

    let mut block = Vec::new();
    for (kind, body) in &records {
        block.extend_from_slice(&kind.to_be_bytes());
        block.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
        block.extend_from_slice(body);
    }
    let mut new_exth = b"EXTH".to_vec();
    new_exth.extend_from_slice(&(block.len() as u32 + 12).to_be_bytes());
    new_exth.extend_from_slice(&(records.len() as u32).to_be_bytes());
    new_exth.extend_from_slice(&block);
    while new_exth.len() % 4 != 0 {
        new_exth.push(0);
    }
    let delta = (exth + new_exth.len()) as i64 - exth_end as i64;

    let mut out = Vec::with_capacity(data.len() + new_exth.len());
    out.extend_from_slice(&data[..exth]);
    out.extend_from_slice(&new_exth);
    out.extend_from_slice(&data[exth_end..]);
    let new_name = (name_offset as i64 + delta) as u32;
    out[mobi + 0x44..mobi + 0x48].copy_from_slice(&new_name.to_be_bytes());
    // Every record after record 0 moves by the change in size.
    for i in 1..n_records {
        let o = (rec_offset(i)? as i64 + delta) as u32;
        out[78 + 8 * i..82 + 8 * i].copy_from_slice(&o.to_be_bytes());
    }
    Ok(out)
}

/// calibre's ebook-convert: on PATH, or in calibre's default install location.
fn ebook_convert() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ebook-convert.exe" } else { "ebook-convert" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if cfg!(target_os = "macos") {
        dirs.push("/Applications/calibre.app/Contents/MacOS".into());
    } else if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(p) = std::env::var_os(var) {
                dirs.push(PathBuf::from(p).join("Calibre2"));
            }
        }
    } else {
        dirs.push("/opt/calibre".into());
    }
    dirs.into_iter().map(|d| d.join(exe)).find(|p| p.is_file())
}

/// A random-looking urn:uuid (version 4 layout) from the clock and the process id.
fn book_id() -> String {
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;
    let mut x = t ^ ((std::process::id() as u64) << 32) ^ 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        // splitmix64
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let (a, b) = (next(), next());
    format!(
        "urn:uuid:{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0xfff,
        0x8000 | (b >> 48) & 0x3fff,
        b & 0xffff_ffff_ffff
    )
}

/// Current time as an EPUB `dcterms:modified` value, e.g. 2026-10-18T09:30:00Z.
fn utc_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const CONTAINER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn epub_has_mimetype_first_and_fixed_layout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.epub");
        let toc = [TocEntry { title: "Chapter 1".into(), page: 0 }, TocEntry { title: "Chapter 2".into(), page: 1 }];
        let book = Book { title: "Tom & Jerry", author: Some("A & B"), toc: &toc, lang: "en", width: 1264, height: 1680, rtl: true, texts: &[] };
        write_epub(&path, &[vec![0xff, 0xd8], vec![0xff, 0xd8]], &book).unwrap();
        let mut z = zip::ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(z.by_index(0).unwrap().name(), "mimetype");
        assert_eq!(z.comment(), b"comic-scissors");
        let mut opf = String::new();
        z.by_name("OEBPS/content.opf").unwrap().read_to_string(&mut opf).unwrap();
        assert!(opf.contains("<dc:title>Tom &amp; Jerry</dc:title>"));
        assert!(opf.contains("<dc:creator>A &amp; B</dc:creator>"));
        let mut nav = String::new();
        z.by_name("OEBPS/nav.xhtml").unwrap().read_to_string(&mut nav).unwrap();
        assert!(nav.contains(r#"<a href="pages/00002.xhtml">Chapter 2</a>"#));
        assert!(opf.contains("pre-paginated") && opf.contains(r#"content="1264x1680""#));
        assert!(opf.contains(r#"page-progression-direction="rtl""#));
        assert!(opf.contains(r#"href="pages/00002.xhtml""#));
    }

    /// A minimal KF8 file: Palm header, record 0 (PalmDOC + MOBI header + EXTH + name), record 1.
    fn tiny_mobi() -> Vec<u8> {
        let mobi_len = 0xe8u32;
        let mut exth = b"EXTH".to_vec();
        exth.extend_from_slice(&(12u32 + 13).to_be_bytes());
        exth.extend_from_slice(&1u32.to_be_bytes());
        exth.extend_from_slice(&524u32.to_be_bytes());
        exth.extend_from_slice(&(8u32 + 5).to_be_bytes());
        exth.extend_from_slice(b"en-GB");
        exth.extend_from_slice(&[0, 0, 0]);
        let mut rec0 = vec![0u8; 16];
        let mut mobi = vec![0u8; mobi_len as usize];
        mobi[..4].copy_from_slice(b"MOBI");
        mobi[4..8].copy_from_slice(&mobi_len.to_be_bytes());
        mobi[0x70..0x74].copy_from_slice(&0x40u32.to_be_bytes());
        let name_offset = 16 + mobi_len + exth.len() as u32;
        mobi[0x44..0x48].copy_from_slice(&name_offset.to_be_bytes());
        mobi[0x48..0x4c].copy_from_slice(&4u32.to_be_bytes());
        rec0.extend_from_slice(&mobi);
        rec0.extend_from_slice(&exth);
        rec0.extend_from_slice(b"book\0\0\0\0");
        let mut d = vec![0u8; 78];
        d[60..68].copy_from_slice(b"BOOKMOBI");
        d[76..78].copy_from_slice(&2u16.to_be_bytes());
        let r0 = 78 + 16 + 2;
        let r1 = r0 + rec0.len();
        d.extend_from_slice(&(r0 as u32).to_be_bytes());
        d.extend_from_slice(&[0; 4]);
        d.extend_from_slice(&(r1 as u32).to_be_bytes());
        d.extend_from_slice(&[0, 0, 0, 1]);
        d.extend_from_slice(&[0, 0]);
        d.extend_from_slice(&rec0);
        d.extend_from_slice(b"RECORD-ONE");
        d
    }

    #[test]
    fn azw3_gets_fixed_layout_records() {
        let out = mark_fixed_layout(&tiny_mobi(), 1236, 1648).unwrap();
        let u32_at = |o: usize| u32::from_be_bytes(out[o..o + 4].try_into().unwrap()) as usize;
        let (r0, r1) = (u32_at(78), u32_at(86));
        assert_eq!(&out[r1..], b"RECORD-ONE", "record 1 offset follows the move");
        let name = r0 + u32_at(r0 + 16 + 0x44);
        assert_eq!(&out[name..name + 4], b"book");
        let exth = r0 + 16 + 0xe8;
        assert_eq!(u32_at(exth + 8), 3);
        let text = String::from_utf8_lossy(&out[exth..name]);
        assert!(text.contains("en-GB") && text.contains("true") && text.contains("1236x1648"));
        // Running it again replaces the records instead of adding more.
        let again = mark_fixed_layout(&out, 1236, 1648).unwrap();
        assert_eq!(again, out);
    }

    // Each word becomes transparent text over its lettering; a word running down the page is turned.
    #[test]
    fn words_are_laid_over_the_page() {
        let w = |text: &str, down| PlacedWord { x: 100.0, y: 200.0, advance: 120.0, size: 30.0, down, text: text.into() };
        let html = text_layer(&[w("look<up>", false), w("down", true), w(" ", false)]);
        assert_eq!(html.matches("<span").count(), 2, "{html}");
        assert!(html.contains("left:100px;top:174px;width:120px;height:30px") && html.contains(">look&lt;up&gt;</span> "), "{html}");
        assert!(html.contains("left:126px;top:200px") && html.contains("rotate(90deg)"), "{html}");
    }

    #[test]
    fn utc_date_is_well_formed() {
        let s = utc_now();
        assert_eq!(s.len(), 20);
        assert!(s.starts_with("20") && s.ends_with('Z'));
    }
}
