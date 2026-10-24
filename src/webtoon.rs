//! Webtoons: vertical-scroll comics without pages or gutter grids.
//!
//! Consecutive images of the same width (the slices an episode is saved as) are joined into
//! one strip. The strip is cut at horizontal stretches of plain background, the space between
//! panels; each piece is shown at the screen's width. A piece taller than the screen is split
//! at its quietest row, so no line of text is cut through.

use crate::output::load_image;
use anyhow::Result;
use image::{GrayImage, RgbImage};
use std::path::{Path, PathBuf};

/// Pages taller than this (height / width) are treated as a webtoon without `--webtoon`.
pub const AUTO_ASPECT: f32 = 3.0;

/// A row is background when this share of its pixels is within `TOLERANCE` of the row's median.
const PLAIN_SHARE: f32 = 0.985;
const TOLERANCE: i16 = 12;

pub struct Pieces {
    pub paths: Vec<PathBuf>,
    pub chapters: Vec<String>,
}

/// True when the pages look like webtoon strips (much taller than wide).
pub fn looks_like_webtoon(paths: &[PathBuf]) -> bool {
    let mut ratios: Vec<f32> = paths
        .iter()
        .filter_map(|p| image::image_dimensions(p).ok())
        .map(|(w, h)| h as f32 / w.max(1) as f32)
        .collect();
    if ratios.is_empty() {
        return false;
    }
    ratios.sort_by(f32::total_cmp);
    ratios[ratios.len() / 2] > AUTO_ASPECT
}

/// Cut the pages into pieces, written as PNGs to `dir`. `screen_aspect` is the usable screen
/// height / width: a piece shown at screen width may be at most `width * screen_aspect` tall.
pub fn cut(paths: &[PathBuf], chapters: &[String], dir: &Path, screen_aspect: f32) -> Result<Pieces> {
    std::fs::create_dir_all(dir)?;
    let mut out = Pieces { paths: Vec::new(), chapters: Vec::new() };
    let mut i = 0;
    while i < paths.len() {
        // One strip: consecutive readable images of the same width in the same chapter.
        let first = load_image(&paths[i])?.to_rgb8();
        let width = first.width();
        let mut slices = vec![first];
        let mut j = i + 1;
        while j < paths.len() && chapters[j] == chapters[i] {
            match image::image_dimensions(&paths[j]) {
                Ok((w, _)) if w == width => slices.push(load_image(&paths[j])?.to_rgb8()),
                _ => break,
            }
            j += 1;
        }
        let strip = join(&slices);
        drop(slices);
        let max_h = ((width as f32 * screen_aspect) as u32).max(16);
        for (y0, y1) in pieces(&strip, max_h) {
            let piece = image::imageops::crop_imm(&strip, 0, y0, width, y1 - y0).to_image();
            let path = dir.join(format!("{:05}.png", out.paths.len()));
            piece.save(&path)?;
            out.paths.push(path);
            out.chapters.push(chapters[i].clone());
        }
        i = j;
    }
    Ok(out)
}

fn join(slices: &[RgbImage]) -> RgbImage {
    let w = slices[0].width();
    let h = slices.iter().map(|s| s.height()).sum();
    let mut strip = RgbImage::new(w, h);
    let mut y = 0;
    for s in slices {
        image::imageops::replace(&mut strip, s, 0, y as i64);
        y += s.height();
    }
    strip
}

/// Row ranges `[y0, y1)` of the strip's pieces: content between background gaps, each at most
/// `max_h` tall.
fn pieces(strip: &RgbImage, max_h: u32) -> Vec<(u32, u32)> {
    let gray: GrayImage = image::DynamicImage::ImageRgb8(strip.clone()).to_luma8();
    let busy: Vec<f32> = (0..gray.height()).map(|y| row_busy(&gray, y)).collect();
    let plain = |y: usize| busy[y] <= 1.0 - PLAIN_SHARE;
    // A gap must be this tall: thinner plain bands are inside panels (a sky, a pause).
    let min_gap = (strip.width() / 50).max(8) as usize;

    let mut content = Vec::new();
    let mut y = 0;
    let n = busy.len();
    while y < n {
        if plain(y) {
            y += 1;
            continue;
        }
        // Content runs until a plain band at least min_gap tall.
        let start = y;
        let mut end = y;
        while y < n {
            if plain(y) {
                let gap_start = y;
                while y < n && plain(y) {
                    y += 1;
                }
                if y - gap_start >= min_gap || y == n {
                    break;
                }
            } else {
                y += 1;
                end = y;
            }
        }
        // Skip specks (scan dirt, a stray line).
        if end - start >= 4 {
            content.push((start, end));
        }
    }

    let mut out = Vec::new();
    for (mut y0, y1) in content {
        while y1 - y0 > max_h as usize {
            let cut = quietest_row(&busy, y0 + max_h as usize * 6 / 10, y0 + max_h as usize);
            out.push((y0 as u32, cut as u32));
            y0 = cut;
        }
        out.push((y0 as u32, y1 as u32));
    }
    out
}

/// The row in `[lo, hi)` with the least detail, preferring the lowest of equal rows so pieces
/// stay as tall as allowed.
fn quietest_row(busy: &[f32], lo: usize, hi: usize) -> usize {
    (lo..hi).rev().min_by(|&a, &b| busy[a].total_cmp(&busy[b])).unwrap_or(hi)
}

/// Share of a row's pixels that differ from the row's median by more than `TOLERANCE`.
fn row_busy(gray: &GrayImage, y: u32) -> f32 {
    let w = gray.width() as usize;
    let row = &gray.as_raw()[y as usize * w..(y as usize + 1) * w];
    let mut hist = [0u32; 256];
    for &v in row {
        hist[v as usize] += 1;
    }
    let mut acc = 0;
    let mut median = 0i16;
    for (v, &c) in hist.iter().enumerate() {
        acc += c as usize;
        if acc * 2 >= w {
            median = v as i16;
            break;
        }
    }
    let off = row.iter().filter(|&&v| (v as i16 - median).abs() > TOLERANCE).count();
    off as f32 / w as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    /// A strip with two "panels" (noisy blocks) on a coloured background.
    fn strip(gap: u32) -> RgbImage {
        let (w, panel) = (200, 300);
        let mut img = RgbImage::from_pixel(w, 20 + panel + gap + panel + 20, Rgb([40, 60, 200]));
        for (top, _) in [(20, 0), (20 + panel + gap, 1)] {
            for y in top..top + panel {
                for x in 10..w - 10 {
                    let v = ((x * 7 + y * 13) % 256) as u8;
                    img.put_pixel(x, y, Rgb([v, v, v]));
                }
            }
        }
        img
    }

    #[test]
    fn cuts_at_background_gaps() {
        let p = pieces(&strip(40), 10_000);
        assert_eq!(p, [(20, 320), (360, 660)]);
    }

    #[test]
    fn thin_plain_bands_do_not_cut() {
        assert_eq!(pieces(&strip(3), 10_000).len(), 1);
    }

    #[test]
    fn tall_pieces_are_split_to_fit() {
        let p = pieces(&strip(40), 200);
        assert!(p.iter().all(|(a, b)| b - a <= 200), "{p:?}");
        assert_eq!(p.first().unwrap().0, 20);
        assert_eq!(p.last().unwrap().1, 660);
    }
}
