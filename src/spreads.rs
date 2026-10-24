//! Two-page spreads: a scan of an open book (or a CBZ page holding two pages) is split at its
//! centre gutter, so panels are read page by page: right page first for manga.
//!
//! Only landscape pages with a clear, plain vertical band near the middle are split. A splash
//! whose art runs across the middle has no such band and stays whole.

use crate::output::load_image;
use anyhow::Result;
use image::GrayImage;
use rayon::prelude::*;
use std::path::{Path, PathBuf};

/// Pages wider than this (width / height) may be spreads.
const MIN_ASPECT: f32 = 1.15;
/// The gutter is searched in this central share of the width.
const SEARCH: (f32, f32) = (0.38, 0.62);
/// A column is plain when at most this share of its pixels differs from the column's median.
const PLAIN: f32 = 0.06;
const TOLERANCE: i16 = 24;

/// Split spreads into their pages, written as PNGs to `dir`. Returns the new page list and
/// the chapter of each page.
pub fn split(paths: &[PathBuf], chapters: &[String], dir: &Path, rtl: bool) -> Result<(Vec<PathBuf>, Vec<String>, usize)> {
    std::fs::create_dir_all(dir)?;
    let parts: Vec<Vec<PathBuf>> = paths
        .par_iter()
        .enumerate()
        .map(|(i, p)| split_page(p, &dir.join(format!("{i:05}")), rtl))
        .collect::<Result<_>>()?;
    let mut out = (Vec::new(), Vec::new(), 0);
    for (pages, chapter) in parts.into_iter().zip(chapters) {
        if pages.len() > 1 {
            out.2 += 1;
        }
        for p in pages {
            out.0.push(p);
            out.1.push(chapter.clone());
        }
    }
    Ok(out)
}

/// The page itself, or its two halves in reading order. Unreadable pages are passed through
/// (the detection stage reports them).
fn split_page(path: &Path, stem: &Path, rtl: bool) -> Result<Vec<PathBuf>> {
    let keep = Ok(vec![path.to_path_buf()]);
    let Ok((w, h)) = image::image_dimensions(path) else { return keep };
    if (w as f32) < MIN_ASPECT * h as f32 {
        return keep;
    }
    let Ok(img) = load_image(path) else { return keep };
    // Columns are compared on a small copy: the gutter is wide, and this is fast.
    let scale = (800.0 / w as f32).min(1.0);
    let small = image::imageops::resize(
        &img.to_luma8(),
        ((w as f32 * scale) as u32).max(1),
        ((h as f32 * scale) as u32).max(1),
        image::imageops::FilterType::Triangle,
    );
    let Some(x) = gutter(&small) else { return keep };
    let x = ((x as f32 / scale) as u32).clamp(1, w - 1);
    let left = img.crop_imm(0, 0, x, h);
    let right = img.crop_imm(x, 0, w - x, h);
    let halves = if rtl { [right, left] } else { [left, right] };
    let mut out = Vec::new();
    for (k, half) in halves.iter().enumerate() {
        let p = stem.with_extension(format!("{k}.png"));
        half.save(&p)?;
        out.push(p);
    }
    Ok(out)
}

/// Column of the centre gutter: the middle of the widest run of plain columns in the central
/// search band, if there is one.
fn gutter(g: &GrayImage) -> Option<u32> {
    let (w, h) = g.dimensions();
    let (lo, hi) = ((w as f32 * SEARCH.0) as u32, (w as f32 * SEARCH.1) as u32);
    // Skip the top and bottom tenth: a title or page numbers can sit across the middle.
    let (y0, y1) = (h / 10, h - h / 10);
    let plain = |x: u32| {
        let mut col: Vec<u8> = (y0..y1).map(|y| g.get_pixel(x, y)[0]).collect();
        let n = col.len().max(1);
        let mid = n / 2;
        let median = *col.select_nth_unstable(mid).1 as i16;
        let off = col.iter().filter(|&&v| (v as i16 - median).abs() > TOLERANCE).count();
        (off as f32 / n as f32) <= PLAIN
    };
    let mut best: Option<(u32, u32)> = None;
    let mut run_start = None;
    for x in lo..=hi {
        let p = x < hi && plain(x);
        match (p, run_start) {
            (true, None) => run_start = Some(x),
            (false, Some(s)) => {
                if best.is_none_or(|(a, b)| x - s > b - a) {
                    best = Some((s, x));
                }
                run_start = None;
            }
            _ => {}
        }
    }
    // A real gutter is a band, not a single plain column inside the art.
    let (a, b) = best?;
    (b - a >= (w / 200).max(2)).then_some((a + b) / 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    /// Two "pages" of noisy art with a plain band between them, `gap` columns wide.
    fn spread(gap: u32) -> GrayImage {
        let (w, h) = (800, 500);
        let mid = w / 2;
        GrayImage::from_fn(w, h, |x, y| {
            if x + gap / 2 >= mid && x < mid + gap - gap / 2 {
                Luma([230])
            } else {
                Luma([((x * 31 + y * 17) % 200) as u8])
            }
        })
    }

    #[test]
    fn finds_the_centre_gutter() {
        let x = gutter(&spread(30)).unwrap();
        assert!((390..410).contains(&x), "{x}");
    }

    #[test]
    fn art_across_the_middle_is_not_split() {
        assert_eq!(gutter(&spread(0)), None);
    }
}
