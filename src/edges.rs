//! Edge map of a page: where brightness changes sharply, and in which direction.
//!
//! Tests on dark pixels ("a frame line is dark on light paper") fail on dark pages, tinted
//! paper and grey scans. A frame line is the same everywhere as two strong, parallel edges, so
//! frame tests ask the edge map instead.

use image::GrayImage;

/// Gradient strength (Sobel, 0..=1020) that counts as an edge.
const STRONG: i32 = 160;

pub struct Edges {
    w: usize,
    h: usize,
    gx: Vec<i16>,
    gy: Vec<i16>,
}

impl Edges {
    pub fn new(gray: &GrayImage) -> Edges {
        let (w, h) = (gray.width() as usize, gray.height() as usize);
        let px = |x: usize, y: usize| gray.as_raw()[y * w + x] as i32;
        let mut gx = vec![0i16; w * h];
        let mut gy = vec![0i16; w * h];
        for y in 1..h.saturating_sub(1) {
            for x in 1..w.saturating_sub(1) {
                let (a, b, c) = (px(x - 1, y - 1), px(x, y - 1), px(x + 1, y - 1));
                let (d, f) = (px(x - 1, y), px(x + 1, y));
                let (g, hh, i) = (px(x - 1, y + 1), px(x, y + 1), px(x + 1, y + 1));
                gx[y * w + x] = ((c + 2 * f + i) - (a + 2 * d + g)) as i16;
                gy[y * w + x] = ((g + 2 * hh + i) - (a + 2 * b + c)) as i16;
            }
        }
        Edges { w, h, gx, gy }
    }

    /// Whether (`x`, `y`) lies on an edge running horizontally (a change from one row to the
    /// next), or vertically when `horizontal` is false.
    pub fn along(&self, x: i64, y: i64, horizontal: bool) -> bool {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return false;
        }
        let i = y as usize * self.w + x as usize;
        let (gx, gy) = (self.gx[i].unsigned_abs() as i32, self.gy[i].unsigned_abs() as i32);
        if horizontal { gy >= STRONG && gy > gx } else { gx >= STRONG && gx > gy }
    }

    /// Share of positions `lo..hi` along a straight line at `at` (a row when `horizontal`, else a
    /// column) lying on an edge of that direction, allowing `drift` pixels either way.
    pub fn line(&self, horizontal: bool, at: i64, lo: u32, hi: u32, drift: i64) -> f32 {
        if hi <= lo {
            return 0.0;
        }
        let n = (lo..hi)
            .filter(|&t| (-drift..=drift).any(|e| if horizontal { self.along(t as i64, at + e, true) } else { self.along(at + e, t as i64, false) }))
            .count();
        n as f32 / (hi - lo) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    #[test]
    fn finds_frame_lines_on_light_and_dark_pages() {
        // A light frame on a dark page, and a dark frame on a light one.
        for (paper, ink) in [(255u8, 0u8), (20, 230)] {
            let g = GrayImage::from_fn(200, 200, |x, y| Luma([if (50..53).contains(&y) && (20..180).contains(&x) { ink } else { paper }]));
            let e = Edges::new(&g);
            assert!(e.line(true, 50, 30, 170, 1) > 0.9, "top edge of the line");
            assert!(e.line(true, 100, 30, 170, 1) < 0.1, "blank row");
            assert!(e.line(false, 100, 30, 170, 1) < 0.1, "no vertical line");
        }
    }
}
