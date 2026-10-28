//! Page images out of a Kindle book: AZW3 (KF8), MOBI and AZW.
//!
//! These are PalmDB files: a list of records, where record 0 holds the MOBI header, the next ones
//! the book's (usually compressed) text, and the images follow as one plain JPEG/PNG/GIF record
//! each. The text says in which order the pages show the images (`kindle:embed:000A` in KF8,
//! `recindex="00010"` in older MOBI), so pages come out in reading order, without the thumbnail
//! or images the text never shows. Without readable text, images keep their record order.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// One record of the file.
fn record<'a>(data: &'a [u8], offsets: &[usize], i: usize) -> &'a [u8] {
    let start = offsets[i].min(data.len());
    let end = offsets.get(i + 1).copied().unwrap_or(data.len()).clamp(start, data.len());
    &data[start..end]
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|s| u16::from_be_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// The image in a record and its file extension, if the record is one.
fn image_of(rec: &[u8]) -> Option<(&[u8], &'static str)> {
    // "CRES": a high-resolution image behind a 12-byte header.
    let rec = if rec.starts_with(b"CRES") && rec.len() > 12 { &rec[12..] } else { rec };
    if rec.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some((rec, "jpg"))
    } else if rec.starts_with(b"\x89PNG") {
        Some((rec, "png"))
    } else if rec.starts_with(b"GIF8") {
        Some((rec, "gif"))
    } else {
        None
    }
}

/// The book's MOBI header (record 0 of a section): text layout and the first image's record.
struct Header {
    compression: u16,
    text_records: usize,
    encrypted: bool,
    first_image: usize,
    /// Which kinds of trailing data each text record ends with.
    extra_flags: u16,
    /// EXTH entries: (type, value) for the numeric ones.
    exth: Vec<(u32, u32)>,
}

impl Header {
    fn read(rec0: &[u8]) -> Option<Header> {
        if rec0.get(16..20)? != b"MOBI" {
            return None;
        }
        let len = u32_at(rec0, 20)? as usize;
        let extra_flags = if len >= 0xE4 { u16_at(rec0, 16 + 0xE2).unwrap_or(0) } else { 0 };
        let mut exth = Vec::new();
        if u32_at(rec0, 16 + 0x70).unwrap_or(0) & 0x40 != 0 {
            let at = 16 + len;
            if rec0.get(at..at + 4) == Some(b"EXTH") {
                let count = u32_at(rec0, at + 8).unwrap_or(0);
                let mut p = at + 12;
                for _ in 0..count {
                    let (Some(kind), Some(size)) = (u32_at(rec0, p), u32_at(rec0, p + 4)) else { break };
                    if size == 12
                        && let Some(v) = u32_at(rec0, p + 8)
                    {
                        exth.push((kind, v));
                    }
                    p += (size as usize).max(8);
                }
            }
        }
        Some(Header {
            compression: u16_at(rec0, 0)?,
            text_records: u16_at(rec0, 8)? as usize,
            encrypted: u16_at(rec0, 12)? != 0,
            first_image: u32_at(rec0, 16 + 0x5C)? as usize,
            extra_flags,
            exth,
        })
    }

    fn exth(&self, kind: u32) -> Option<u32> {
        self.exth.iter().find(|e| e.0 == kind).map(|e| e.1)
    }
}

/// A text record without its trailing entries (sizes stored backwards at its end).
fn strip_trailing(mut rec: &[u8], flags: u16) -> &[u8] {
    for bit in 1..16 {
        if flags & (1 << bit) == 0 {
            continue;
        }
        // A backward variable-width integer: 7 bits per byte, the first byte has the high bit.
        let mut size = 0usize;
        for (shift, &b) in rec.iter().rev().take(4).enumerate() {
            size |= ((b & 0x7F) as usize) << (7 * shift);
            if b & 0x80 != 0 {
                break;
            }
        }
        rec = &rec[..rec.len().saturating_sub(size)];
    }
    if flags & 1 != 0
        && let Some(&last) = rec.last()
    {
        rec = &rec[..rec.len().saturating_sub((last & 3) as usize + 1)];
    }
    rec
}

/// PalmDOC (LZ77) decompression.
fn palmdoc(input: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(input.len() * 2);
    let mut i = 0;
    while i < input.len() {
        let c = input[i];
        i += 1;
        match c {
            0x01..=0x08 => {
                let n = (c as usize).min(input.len() - i);
                out.extend_from_slice(&input[i..i + n]);
                i += n;
            }
            0x80..=0xBF => {
                let Some(&next) = input.get(i) else { break };
                i += 1;
                let pair = ((c as usize) << 8 | next as usize) & 0x3FFF;
                let (dist, len) = (pair >> 3, (pair & 7) + 3);
                if dist == 0 || dist > out.len() {
                    continue;
                }
                for _ in 0..len {
                    out.push(out[out.len() - dist]);
                }
            }
            0xC0..=0xFF => {
                out.push(b' ');
                out.push(c ^ 0x80);
            }
            _ => out.push(c),
        }
    }
    out
}

/// The image numbers (1-based, counted from the first image record) in the order the text shows
/// them, each once.
fn shown_images(text: &[u8]) -> Vec<usize> {
    let text = String::from_utf8_lossy(text);
    let mut out: Vec<usize> = Vec::new();
    let mut add = |n: Option<usize>| {
        if let Some(n) = n.filter(|&n| n > 0)
            && !out.contains(&n)
        {
            out.push(n);
        }
    };
    let mut rest = text.as_ref();
    loop {
        let kf8 = rest.find("kindle:embed:");
        let mobi = rest.find("recindex=");
        let (at, skip) = match (kf8, mobi) {
            (Some(a), Some(b)) if b < a => (b, "recindex=".len()),
            (Some(a), _) => (a, "kindle:embed:".len()),
            (None, Some(b)) => (b, "recindex=".len()),
            (None, None) => break,
        };
        rest = &rest[at + skip..];
        if skip == "kindle:embed:".len() {
            // Base 32, digits 0-9 then A-V.
            let digits: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).take(4).collect();
            add(usize::from_str_radix(&digits, 32).ok());
        } else {
            let digits: String = rest.trim_start_matches(['"', '\'']).chars().take_while(char::is_ascii_digit).collect();
            add(digits.parse().ok());
        }
    }
    out
}

/// Write the pages of the Kindle book at `path` into `dir`, in reading order.
pub fn extract(path: &Path, dir: &Path) -> Result<Vec<PathBuf>> {
    let data = std::fs::read(path)?;
    let name = path.display();
    if data.len() < 78 || &data[60..68] != b"BOOKMOBI" {
        bail!("{name} is not a Kindle book (AZW3/MOBI)");
    }
    let count = u16_at(&data, 76).unwrap_or(0) as usize;
    let offsets: Vec<usize> = (0..count).filter_map(|i| u32_at(&data, 78 + 8 * i).map(|o| o as usize)).collect();
    let Some(head) = Header::read(record(&data, &offsets, 0)) else { bail!("{name} has no MOBI header") };
    if head.encrypted {
        bail!("{name} is DRM-protected: only DRM-free Kindle books can be repacked");
    }
    // A joint MOBI + KF8 file: the KF8 part (EXTH 121 gives its header record) has the text that
    // pages use; the images are shared.
    let (text_at, text_head) = match head.exth(121).map(|i| i as usize).filter(|&i| i < offsets.len()) {
        Some(i) => match Header::read(record(&data, &offsets, i)) {
            Some(h) => (i, h),
            None => (0, Header::read(record(&data, &offsets, 0)).unwrap()),
        },
        None => (0, Header::read(record(&data, &offsets, 0)).unwrap()),
    };

    let first = head.first_image;
    let images: Vec<Option<(&[u8], &str)>> = (first..offsets.len()).map(|i| image_of(record(&data, &offsets, i))).collect();
    let mut text = Vec::new();
    if matches!(text_head.compression, 1 | 2) {
        for i in text_at + 1..=(text_at + text_head.text_records).min(offsets.len() - 1) {
            let rec = strip_trailing(record(&data, &offsets, i), text_head.extra_flags);
            if text_head.compression == 2 {
                text.extend(palmdoc(rec));
            } else {
                text.extend_from_slice(rec);
            }
        }
    }
    let shown: Vec<usize> = shown_images(&text).into_iter().filter(|&n| images.get(n - 1).is_some_and(Option::is_some)).collect();
    let order: Vec<usize> = if shown.is_empty() {
        // Every image in record order, without the thumbnail (EXTH 202, counted from the first image).
        let thumb = head.exth(202).map(|t| t as usize);
        (0..images.len()).filter(|&i| images[i].is_some() && Some(i) != thumb).collect()
    } else {
        shown.iter().map(|n| n - 1).collect()
    };
    if order.is_empty() {
        bail!("no images inside {name}");
    }
    std::fs::create_dir_all(dir)?;
    let mut out = Vec::with_capacity(order.len());
    for (page, &i) in order.iter().enumerate() {
        let (bytes, ext) = images[i].unwrap();
        let dest = dir.join(format!("{page:05}.{ext}"));
        std::fs::write(&dest, bytes)?;
        out.push(dest);
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal KF8-style book: record 0 (MOBI header with an EXTH thumbnail entry), uncompressed
    /// text records, then the images.
    pub(crate) fn book(text: &[&[u8]], images: &[Vec<u8>], thumb: Option<u32>, encrypted: bool) -> Vec<u8> {
        let mut rec0 = vec![0u8; 16 + 0xE8];
        rec0[0..2].copy_from_slice(&1u16.to_be_bytes()); // no compression
        rec0[8..10].copy_from_slice(&(text.len() as u16).to_be_bytes());
        rec0[12..14].copy_from_slice(&(encrypted as u16).to_be_bytes());
        rec0[16..20].copy_from_slice(b"MOBI");
        rec0[20..24].copy_from_slice(&0xE8u32.to_be_bytes());
        rec0[16 + 0x5C..16 + 0x60].copy_from_slice(&(1 + text.len() as u32).to_be_bytes());
        if let Some(t) = thumb {
            rec0[16 + 0x70..16 + 0x74].copy_from_slice(&0x40u32.to_be_bytes());
            rec0.extend_from_slice(b"EXTH");
            rec0.extend_from_slice(&24u32.to_be_bytes());
            rec0.extend_from_slice(&1u32.to_be_bytes());
            rec0.extend_from_slice(&202u32.to_be_bytes());
            rec0.extend_from_slice(&12u32.to_be_bytes());
            rec0.extend_from_slice(&t.to_be_bytes());
        }
        let mut records: Vec<Vec<u8>> = vec![rec0];
        records.extend(text.iter().map(|t| t.to_vec()));
        records.extend(images.iter().cloned());
        records.push(b"FLIS\0\0\0\x08".to_vec());
        let mut out = vec![0u8; 78];
        out[60..68].copy_from_slice(b"BOOKMOBI");
        out[76..78].copy_from_slice(&(records.len() as u16).to_be_bytes());
        let mut at = 78 + 8 * records.len() + 2;
        for (i, r) in records.iter().enumerate() {
            out.extend_from_slice(&(at as u32).to_be_bytes());
            out.extend_from_slice(&[0, 0, 0, i as u8]);
            at += r.len();
        }
        out.extend_from_slice(&[0, 0]);
        for r in &records {
            out.extend_from_slice(r);
        }
        out
    }

    pub(crate) fn png(w: u32, shade: u8) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        image::GrayImage::from_pixel(w, 10, image::Luma([shade])).write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    fn widths(pages: &[PathBuf]) -> Vec<u32> {
        pages.iter().map(|p| image::image_dimensions(p).unwrap().0).collect()
    }

    // Pages come in the order the text shows them, not the order the images are stored in, and
    // an image the text never shows (the thumbnail) is left out.
    #[test]
    fn pages_follow_the_text() {
        let dir = tempfile::tempdir().unwrap();
        let images = vec![png(10, 0), png(20, 0), png(30, 0), png(5, 0)];
        let text: &[&[u8]] = &[b"<img src=\"kindle:embed:0003?mime=image/png\"/><img src=\"kindle:em", b"bed:0001\"/><img src=\"kindle:embed:0002\"/>"];
        let path = dir.path().join("b.azw3");
        std::fs::write(&path, book(text, &images, Some(3), false)).unwrap();
        // The second reference is split across two text records.
        assert_eq!(widths(&extract(&path, &dir.path().join("out")).unwrap()), vec![30, 10, 20]);
    }

    // Without references in the text, every image but the thumbnail, in record order.
    #[test]
    fn without_text_images_keep_their_order() {
        let dir = tempfile::tempdir().unwrap();
        let images = vec![png(10, 0), png(20, 0), png(5, 0), png(30, 0)];
        let path = dir.path().join("b.azw3");
        std::fs::write(&path, book(&[b"<p>no pictures</p>"], &images, Some(2), false)).unwrap();
        assert_eq!(widths(&extract(&path, &dir.path().join("out")).unwrap()), vec![10, 20, 30]);
    }

    #[test]
    fn drm_and_other_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.azw3");
        std::fs::write(&path, book(&[], &[png(10, 0)], None, true)).unwrap();
        assert!(format!("{:#}", extract(&path, dir.path()).unwrap_err()).contains("DRM"));
        std::fs::write(&path, b"not a book").unwrap();
        assert!(extract(&path, dir.path()).is_err());
    }

    #[test]
    fn palmdoc_decompresses() {
        // "ab", a copy of 3 bytes from 2 back ("aba"), " c" as one byte, two literal bytes.
        let pair = 0x8000u16 | (2 << 3);
        let input = [b'a', b'b', (pair >> 8) as u8, pair as u8, b'c' ^ 0x80 | 0x80, 0x02, 0xC0, b'x'];
        assert_eq!(palmdoc(&input), b"ababa c\xC0x");
    }

    #[test]
    fn mobi_references_are_read() {
        assert_eq!(shown_images(b"<img recindex=\"00002\"><img recindex='00001'><img recindex=\"00002\">"), vec![2, 1]);
    }

    #[test]
    fn trailing_entries_are_stripped() {
        // At the end a 2-byte trailing entry (its size byte 0x82 last), before it 2 multibyte bytes.
        assert_eq!(strip_trailing(b"textY\x01Z\x82", 0b11), b"text");
    }
}
