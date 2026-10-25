//! Panels traced from a page's frame lines.
//!
//! 1. Straight lines, level or slanted up to ±12°, are found on the edge map.
//! 2. Gutters are paper joined to the page margin (or long thin channels of paper whose ends a
//!    balloon closes off). Only frame lines are kept: a line with gutter along one side, or one
//!    ending on such lines at both ends (the sides of an inset, with art on both sides). Straight
//!    lines in the art (a railing, the edge of white sky) have enclosed paper beside them, not
//!    gutter. Frame lines in line with each other across a short gap (a balloon breaking the
//!    frame) are joined.
//! 3. The page border, the gutters and the frame lines are walls; the spaces between them are
//!    flood-filled: each is a cell. Cells with art are panels, their outline the cell's shape (a
//!    trapezoid, an L).
//! 4. The page's [`Pattern`] is read from the panels' shapes and how they sit.

use crate::detect::Rect;
use crate::edges::Edges;
use crate::pattern::Pattern;
use image::{GrayImage, imageops::FilterType};

/// Longest side the frames are traced at.
const WORK: u32 = 1000;
/// Steepest slant tried (tan 12°) and its step.
const MAX_SLOPE: f32 = 0.21;
const SLOPE_STEP: f32 = 0.0175;
/// A line is this long at least (share of the page side along it).
const MIN_LEN: f32 = 0.08;
/// Gaps in a line up to this long are bridged.
const MAX_BREAK: u32 = 8;
/// Share of a line's length on the edge: frame lines are continuous.
const DENSE: f32 = 0.85;

/// A straight line: `horizontal` ones run left to right at `y = c + slope * (x - mid)`, vertical
/// ones top to bottom at `x = c + slope * (y - mid)`, over `lo..hi` along their length.
#[derive(Clone, Copy, Debug)]
pub struct Line {
    pub horizontal: bool,
    pub slope: f32,
    pub c: f32,
    pub lo: u32,
    pub hi: u32,
    /// Edge pixels found along it.
    hits: u32,
}

impl Line {
    /// The point of the line at `t` along it, as (x, y).
    pub fn at(&self, t: f32, mid: f32) -> (f32, f32) {
        let u = self.c + self.slope * (t - mid);
        if self.horizontal { (t, u) } else { (u, t) }
    }
}

/// A traced panel.
#[derive(Clone, Debug)]
pub struct Cell {
    /// Box in page pixels.
    pub rect: Rect,
    /// Share of the box the cell fills, and of its convex hull.
    pub box_fill: f32,
    pub hull_fill: f32,
}

/// The frames of a page traced at work resolution.
pub struct Trace {
    pub w: u32,
    pub h: u32,
    /// Page pixels per work pixel.
    pub scale: f32,
    pub lines: Vec<Line>,
    pub frames: Vec<Line>,
    /// Gutter per work pixel.
    pub gutter: Vec<bool>,
    pub cells: Vec<Cell>,
    /// Cell label per work pixel (0: frame or gutter).
    pub labels: Vec<u32>,
}

pub fn trace(gray: &GrayImage) -> Trace {
    let (pw, ph) = gray.dimensions();
    let s = (WORK as f32 / pw.max(ph) as f32).min(1.0);
    let small = if s < 1.0 {
        image::imageops::resize(gray, ((pw as f32 * s) as u32).max(1), ((ph as f32 * s) as u32).max(1), FilterType::Triangle)
    } else {
        gray.clone()
    };
    let (w, h) = small.dimensions();
    let bg = crate::detect::background_luma(&small);
    let edges = Edges::new(&small);
    let lines = find_lines(&edges, w, h);
    // Two passes: frames found against the gutters, then the gutters found again with the frames
    // as barriers (paper inside a framed panel whose frame has a gap is no gutter).
    let gutter = gutters(&small, bg, None);
    let frames = join_collinear(keep_frames(&small, bg, &gutter, &lines, w, h), w, h);
    let mut walls = draw_frames(&frames, w, h);
    wall_bleeding_sides(&small, bg, &frames, &mut walls);
    let gutter = gutters(&small, bg, Some(&walls));
    let (labels, cells) = cells(&small, bg, &gutter, &walls, w, h, 1.0 / s);
    Trace { w, h, scale: 1.0 / s, lines, frames, gutter, cells, labels }
}

/// Gutter pixels: paper joined to the page border, or in a long thin channel of paper (`walls`:
/// pixels paper cannot pass).
fn gutters(gray: &GrayImage, bg: u8, walls: Option<&[bool]>) -> Vec<bool> {
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    let paper: Vec<bool> =
        gray.as_raw().iter().enumerate().map(|(i, &v)| (v as i16 - bg as i16).abs() <= 40 && !walls.is_some_and(|m| m[i])).collect();
    let mut out = vec![false; w * h];
    let mut seen = vec![false; w * h];
    let mut stack = Vec::new();
    let short = w.min(h) as f32;
    for start in 0..w * h {
        if !paper[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut pix = Vec::new();
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        while let Some(i) = stack.pop() {
            pix.push(i);
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                if nx < w && ny < h {
                    let j = ny * w + nx;
                    if paper[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        // (Within a pixel of the border: the border itself may be drawn as a wall.)
        let border = x0 <= 1 || y0 <= 1 || x1 + 2 >= w || y1 + 2 >= h;
        // (Not across a bleeding side walled off three pixels deep: see `wall_bleeding_sides`.)
        let long = (x1 - x0 + 1).max(y1 - y0 + 1) as f32;
        let thin_long = long >= 0.15 * short && pix.len() as f32 / long <= 0.012 * short;
        if border || thin_long {
            for i in pix {
                out[i] = true;
            }
        }
    }
    out
}

/// Frame lines in line with each other (same direction, slope and offset) with a gap up to 12% of
/// the page side between them: one frame broken by a balloon or art crossing it.
fn join_collinear(mut frames: Vec<Line>, w: u32, h: u32) -> Vec<Line> {
    frames.sort_by(|a, b| (a.horizontal, a.lo).cmp(&(b.horizontal, b.lo)));
    let mut out: Vec<Line> = Vec::new();
    for f in frames {
        let len = if f.horizontal { w } else { h } as f32;
        let joined = out.iter_mut().rev().find(|o| {
            o.horizontal == f.horizontal
                && (o.slope - f.slope).abs() <= 2.0 * SLOPE_STEP
                && (o.c - f.c).abs() <= 4.0
                && f.lo as f32 <= o.hi as f32 + 0.12 * len
        });
        match joined {
            Some(o) => {
                o.hi = o.hi.max(f.hi);
                o.hits += f.hits;
            }
            None => out.push(f),
        }
    }
    out
}

/// Straight lines on the edge map, each the best of its neighbours (same line found at nearby
/// slopes and offsets).
fn find_lines(edges: &Edges, w: u32, h: u32) -> Vec<Line> {
    let mut found = Vec::new();
    for horizontal in [true, false] {
        let (len, across) = if horizontal { (w, h) } else { (h, w) };
        let mid = len as f32 / 2.0;
        let min = (MIN_LEN * len as f32) as u32;
        let steps = (MAX_SLOPE / SLOPE_STEP) as i32;
        for k in -steps..=steps {
            let slope = k as f32 * SLOPE_STEP;
            for c in 0..across {
                let on = |t: u32| {
                    let u = (c as f32 + slope * (t as f32 - mid)).round() as i64;
                    if horizontal { edges.along(t as i64, u, true) } else { edges.along(u, t as i64, false) }
                };
                let mut t = 0;
                while t < len {
                    if !on(t) {
                        t += 1;
                        continue;
                    }
                    let (start, mut last, mut hits) = (t, t, 0);
                    while t < len && t - last <= MAX_BREAK {
                        if on(t) {
                            last = t;
                            hits += 1;
                        }
                        t += 1;
                    }
                    // (A frame line is continuous; a row of lettering is broken up.)
                    if last + 1 - start >= min && hits as f32 >= DENSE * (last + 1 - start) as f32 {
                        found.push(Line { horizontal, slope, c: c as f32, lo: start, hi: last + 1, hits });
                    }
                }
            }
        }
    }
    // Keep the strongest of lines lying within a few pixels of each other over their overlap. A
    // line is only compared with kept lines of its direction whose offset at the middle of the
    // page is near (bucketed by it).
    found.sort_by(|a, b| b.hits.cmp(&a.hits));
    let bucket = |l: &Line| (l.horizontal, (l.c / 8.0) as i64);
    let mut kept: Vec<Line> = Vec::new();
    let mut index: std::collections::HashMap<(bool, i64), Vec<usize>> = Default::default();
    for l in found {
        let m = if l.horizontal { w as f32 / 2.0 } else { h as f32 / 2.0 };
        let (dir, b) = bucket(&l);
        // (A slanted line's offset differs from a level one's by up to the slant over half the
        // page: neighbouring buckets cover that.)
        let reach = ((MAX_SLOPE * m) / 8.0).ceil() as i64 + 1;
        let dup = (b - reach..=b + reach).flat_map(|k| index.get(&(dir, k)).into_iter().flatten()).any(|&i| {
            let k = &kept[i];
            let (lo, hi) = (k.lo.max(l.lo), k.hi.min(l.hi));
            hi > lo
                && [lo, (lo + hi) / 2, hi - 1].iter().all(|&t| {
                    let (a, b) = (k.at(t as f32, m), l.at(t as f32, m));
                    (if l.horizontal { a.1 - b.1 } else { a.0 - b.0 }).abs() <= 6.0
                })
        });
        if !dup {
            index.entry((dir, b)).or_default().push(kept.len());
            kept.push(l);
        }
    }
    kept
}

/// Share of points along `l` with gutter 2 to 8 pixels off it on side `dir`.
fn gutter_side(gutter: &[bool], l: &Line, dir: f32, w: u32, h: u32) -> f32 {
    let mid = if l.horizontal { w as f32 / 2.0 } else { h as f32 / 2.0 };
    let at = |x: f32, y: f32| x >= 0.0 && y >= 0.0 && (x as u32) < w && (y as u32) < h && gutter[y as usize * w as usize + x as usize];
    let ts: Vec<u32> = (l.lo..l.hi).step_by(3).collect();
    let n = ts
        .iter()
        .filter(|&&t| {
            let (x, y) = l.at(t as f32, mid);
            (2..=8).any(|d| {
                let d = d as f32 * dir;
                if l.horizontal { at(x, y + d) } else { at(x + d, y) }
            })
        })
        .count();
    n as f32 / ts.len().max(1) as f32
}

/// Frame lines among `lines`: gutter along one side, or ending on frame lines at both ends.
fn keep_frames(gray: &GrayImage, bg: u8, gutter: &[bool], lines: &[Line], w: u32, h: u32) -> Vec<Line> {
    const GUTTER_SIDE: f32 = 0.6;
    /// Evidence a line needs (each of the four tests below scores one).
    const EVIDENCE: usize = 2;
    let tol = (w.min(h) / 40).max(4) as f32;
    let mid = |l: &Line| if l.horizontal { w as f32 / 2.0 } else { h as f32 / 2.0 };
    let gutter_beside: Vec<bool> =
        lines.iter().map(|l| gutter_side(gutter, l, -1.0, w, h).max(gutter_side(gutter, l, 1.0, w, h)) >= GUTTER_SIDE).collect();
    // 1. Thickness: frames on a page are drawn with one pen. The typical thickness is that of
    // the lines with gutter beside them.
    let thick: Vec<f32> = lines.iter().map(|l| thickness(gray, bg, l, w, h)).collect();
    let mut typical: Vec<f32> = thick.iter().zip(&gutter_beside).filter(|(_, g)| **g).map(|(t, _)| *t).filter(|&t| t > 0.0).collect();
    typical.sort_by(f32::total_cmp);
    let pen = typical.get(typical.len() / 2).copied();
    let same_pen = |i: usize| pen.is_some_and(|p| thick[i] > 0.0 && (thick[i] - p).abs() <= (0.5 * p).max(1.5));
    // 2. Continuity: a frame line is nearly unbroken.
    let continuous = |l: &Line| l.hits as f32 >= 0.95 * (l.hi - l.lo) as f32;
    // 3. Alignment: frames line up across the page (a gutter runs on past a panel): another line
    // of the same direction at the same offset, not overlapping it.
    let aligned = |i: usize| {
        let l = &lines[i];
        lines.iter().enumerate().any(|(j, o)| {
            j != i && o.horizontal == l.horizontal && (o.slope - l.slope).abs() <= 2.0 * SLOPE_STEP && (o.c - l.c).abs() <= 4.0 && (o.hi <= l.lo || l.hi <= o.lo)
        })
    };
    // 4. Closing: both ends land on another line or the page border (it is a side of a box).
    let ends_on = |l: &Line, end: u32, others: &[&Line]| {
        let (x, y) = l.at(end as f32, mid(l));
        if x <= tol || y <= tol || x >= w as f32 - tol || y >= h as f32 - tol {
            return true;
        }
        others.iter().any(|o| {
            if o.horizontal == l.horizontal {
                return false;
            }
            let t = if o.horizontal { x } else { y };
            if t < o.lo as f32 - tol || t > o.hi as f32 + tol {
                return false;
            }
            let (ox, oy) = o.at(t, mid(o));
            (if o.horizontal { oy - y } else { ox - x }).abs() <= tol
        })
    };
    let all: Vec<&Line> = lines.iter().collect();
    let closes: Vec<bool> = lines.iter().map(|l| ends_on(l, l.lo, &all) && ends_on(l, l.hi - 1, &all)).collect();
    let score = |i: usize| [same_pen(i), continuous(&lines[i]), aligned(i), closes[i]].iter().filter(|&&b| b).count();
    // A line with gutter beside it is a frame unless drawn with a different pen and leading
    // nowhere; one without (an inset's side, art on both sides) needs the evidence.
    lines
        .iter()
        .enumerate()
        .filter(|&(i, _)| if gutter_beside[i] { score(i) >= 1 } else { closes[i] && score(i) >= EVIDENCE + 1 })
        .map(|(_, l)| *l)
        .collect()
}

/// Typical width of the drawn stroke along `l` (dark pixels across it at sampled points), or 0.
fn thickness(gray: &GrayImage, bg: u8, l: &Line, w: u32, h: u32) -> f32 {
    let m = if l.horizontal { w as f32 / 2.0 } else { h as f32 / 2.0 };
    let ink = |x: i64, y: i64| x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h && (gray.get_pixel(x as u32, y as u32)[0] as i16 - bg as i16).abs() > 60;
    let mut widths: Vec<u32> = (l.lo..l.hi)
        .step_by(((l.hi - l.lo) / 20).max(1) as usize)
        .filter_map(|t| {
            let (x, y) = l.at(t as f32, m);
            let (x, y) = (x.round() as i64, y.round() as i64);
            let at = |d: i64| if l.horizontal { ink(x, y + d) } else { ink(x + d, y) };
            // The stroke the edge borders: on whichever side is inked.
            let start = (-2..=2).find(|&d| at(d))?;
            let lo = (0..12).take_while(|&k| at(start - k)).count() as i64;
            let hi = (0..12).take_while(|&k| at(start + k)).count() as i64;
            let wd = (lo + hi - 1) as u32;
            (wd < 12).then_some(wd)
        })
        .collect();
    if widths.len() < 5 {
        return 0.0;
    }
    widths.sort_unstable();
    widths[widths.len() / 2] as f32
}

/// Cells between the frame lines: (label per pixel, panels).
fn cells(gray: &GrayImage, bg: u8, gutter: &[bool], frames: &[bool], w: u32, h: u32, scale: f32) -> (Vec<u32>, Vec<Cell>) {
    let (wu, hu) = (w as usize, h as usize);
    // Gutters are walls: a gap in a frame cannot leak into the margin or a neighbour.
    let mut wall = gutter.to_vec();
    for (i, &f) in frames.iter().enumerate() {
        wall[i] |= f;
    }
    let mut labels = vec![0u32; wu * hu];
    let mut seen = vec![false; wu * hu];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    let page = (wu * hu) as f32;
    for start in 0..wu * hu {
        if wall[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut pix = Vec::new();
        while let Some(i) = stack.pop() {
            pix.push(i);
            let (x, y) = (i % wu, i / wu);
            for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                if nx < wu && ny < hu {
                    let j = ny * wu + nx;
                    if !wall[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        // A panel: big enough, with art in it (not a gutter or the margin).
        let art = pix.iter().filter(|&&i| (gray.as_raw()[i] as i16 - bg as i16).abs() > 40).count() as f32 / pix.len() as f32;
        // (Gutters and the margin are walls already: a cell with a little art is a panel, a
        // blank one is enclosed paper.)
        if (pix.len() as f32) < 0.012 * page || art < 0.1 {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for &i in &pix {
            let (x, y) = (i % wu, i / wu);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let bw = (x1 - x0 + 1) as f32;
        let bh = (y1 - y0 + 1) as f32;
        let hull = hull_area(&pix, wu);
        out.push(Cell {
            rect: Rect { x: (x0 as f32 * scale) as u32, y: (y0 as f32 * scale) as u32, w: (bw * scale) as u32, h: (bh * scale) as u32 },
            box_fill: pix.len() as f32 / (bw * bh),
            hull_fill: pix.len() as f32 / hull.max(1.0),
        });
        // Relabel to the cell's index (1-based).
        for &i in &pix {
            labels[i] = out.len() as u32;
        }
    }
    (labels, out)
}

/// Close off the stretches of the page border where a panel bleeds off the page: the frame lines
/// reaching a side split it into stretches, and one where art touches the border along a good
/// share of it is a panel's open side. Its blank parts (sky, white background) are then the
/// panel's, not gutter.
fn wall_bleeding_sides(gray: &GrayImage, bg: u8, frames: &[Line], walls: &mut [bool]) {
    const ART: f32 = 0.3;
    const DEPTH: usize = 4;
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    let art = |x: usize, y: usize| (gray.as_raw()[y * w + x] as i16 - bg as i16).abs() > 40;
    let reach = (w.min(h) / 40) as f32;
    // Side: (along the x axis, at the far edge).
    for (horizontal_side, far) in [(true, false), (true, true), (false, false), (false, true)] {
        let (len, edge) = if horizontal_side { (w, if far { h - 1 } else { 0 }) } else { (h, if far { w - 1 } else { 0 }) };
        // Where frame lines crossing this side meet it.
        let mut cuts: Vec<usize> = vec![0, len];
        for f in frames.iter().filter(|f| f.horizontal != horizontal_side) {
            let m = if f.horizontal { w as f32 / 2.0 } else { h as f32 / 2.0 };
            let end = if far { f.hi as f32 } else { f.lo as f32 };
            if (end - edge as f32).abs() <= reach {
                let (x, y) = f.at(edge as f32, m);
                cuts.push((if horizontal_side { x } else { y }).clamp(0.0, len as f32 - 1.0) as usize);
            }
        }
        cuts.sort_unstable();
        for pair in cuts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if b <= a + 2 {
                continue;
            }
            let at = |t: usize, d: usize| {
                let u = if far { edge - d } else { edge + d };
                if horizontal_side { (t, u) } else { (u, t) }
            };
            let inked = (a..b).filter(|&t| (0..DEPTH).any(|d| {
                let (x, y) = at(t, d);
                art(x, y)
            })).count();
            if inked as f32 >= ART * (b - a) as f32 {
                for t in a..b {
                    for d in 0..3 {
                        let (x, y) = at(t, d);
                        walls[y * w + x] = true;
                    }
                }
            }
        }
    }
}

/// The page border and the frame lines drawn as walls.
fn draw_frames(frames: &[Line], w: u32, h: u32) -> Vec<bool> {
    let (wu, hu) = (w as usize, h as usize);
    let mut wall = vec![false; wu * hu];
    // The page border, and each frame line drawn 3 pixels thick (extended a little at its ends
    // so it closes against its neighbours).
    for x in 0..wu {
        wall[x] = true;
        wall[(hu - 1) * wu + x] = true;
    }
    for y in 0..hu {
        wall[y * wu] = true;
        wall[y * wu + wu - 1] = true;
    }
    // (Extended past its ends to close the gap where a balloon or art breaks a frame corner.)
    let ext = (w.min(h) / 30) as i64;
    for l in frames {
        let m = if l.horizontal { w as f32 / 2.0 } else { h as f32 / 2.0 };
        for t in (l.lo as i64 - ext)..(l.hi as i64 + ext) {
            let (x, y) = l.at(t as f32, m);
            for d in -2..=2 {
                let (xx, yy) = if l.horizontal { (x as i64, y as i64 + d) } else { (x as i64 + d, y as i64) };
                if xx >= 0 && yy >= 0 && (xx as usize) < wu && (yy as usize) < hu {
                    wall[yy as usize * wu + xx as usize] = true;
                }
            }
        }
    }
    wall
}

/// Area of the convex hull of the pixels (by each row's extent).
fn hull_area(pix: &[usize], w: usize) -> f32 {
    let mut rows: std::collections::BTreeMap<usize, (usize, usize)> = Default::default();
    for &i in pix {
        let (x, y) = (i % w, i / w);
        let e = rows.entry(y).or_insert((x, x));
        e.0 = e.0.min(x);
        e.1 = e.1.max(x);
    }
    let mut pts: Vec<(i64, i64)> = Vec::new();
    for (&y, &(a, b)) in &rows {
        pts.extend([(a as i64, y as i64), (b as i64 + 1, y as i64), (a as i64, y as i64 + 1), (b as i64 + 1, y as i64 + 1)]);
    }
    pts.sort_unstable();
    pts.dedup();
    if pts.len() < 3 {
        return pix.len() as f32;
    }
    let cross = |o: (i64, i64), a: (i64, i64), b: (i64, i64)| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut hull: Vec<(i64, i64)> = Vec::new();
    for pass in 0..2 {
        let start = hull.len();
        let it: Box<dyn Iterator<Item = &(i64, i64)>> = if pass == 0 { Box::new(pts.iter()) } else { Box::new(pts.iter().rev()) };
        for &p in it {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    let n = hull.len();
    (0..n).map(|i| hull[i].0 * hull[(i + 1) % n].1 - hull[(i + 1) % n].0 * hull[i].1).sum::<i64>().abs() as f32 / 2.0
}

/// The page's pattern from its traced panels, or `None` when too few were traced.
pub fn pattern(t: &Trace) -> Option<Pattern> {
    let cells = &t.cells;
    if cells.is_empty() {
        return None;
    }
    let (pw, ph) = ((t.w as f32 * t.scale) as u32, (t.h as f32 * t.scale) as u32);
    let page = pw as u64 * ph as u64;
    if cells.len() == 1 {
        return (cells[0].rect.area() * 10 >= page * 6).then_some(Pattern::Splash);
    }
    // (A held cell is a panel, not a scrap: 3% of the page at least.)
    let holds = |a: &Cell, b: &Cell| b.rect.area() < a.rect.area() && b.rect.area() * 100 >= page * 3 && b.rect.overlap_area(&a.rect) * 10 >= b.rect.area() * 8;
    // A background picture with panels laid on it: a big cell reaching three or more page sides
    // with two or more panels floating on it; or long frames at several different slants.
    let margin = pw.min(ph) / 25;
    let sides = |r: &Rect| [r.x <= margin, r.y <= margin, r.right() + margin >= pw, r.bottom() + margin >= ph].iter().filter(|&&b| b).count();
    // (Floating: background art around a held panel on three sides at least. An L's inset sits
    // in a corner of the L's box.)
    let floating = |a: &Rect, b: &Rect| {
        let m = margin;
        [b.x >= a.x + m, b.y >= a.y + m, b.right() + m <= a.right(), b.bottom() + m <= a.bottom()].iter().filter(|&&f| f).count() >= 3
    };
    let background = cells.iter().any(|a| {
        a.rect.area() * 10 >= page * 4 && sides(&a.rect) >= 3 && cells.iter().filter(|b| holds(a, b) && floating(&a.rect, &b.rect)).count() >= 2
    });
    let mut slants: Vec<i32> = t
        .frames
        .iter()
        .filter(|f| (f.hi - f.lo) as f32 >= 0.2 * if f.horizontal { t.w } else { t.h } as f32 && f.slope.abs() >= 0.1)
        .map(|f| (f.slope / 0.05).round() as i32 * if f.horizontal { 1 } else { -1 })
        .collect();
    slants.sort_unstable();
    slants.dedup();
    if background || slants.len() >= 3 {
        return Some(Pattern::Collage);
    }
    // A notched cell holding another: an L around an inset.
    if cells.iter().any(|a| a.box_fill < 0.85 && cells.iter().any(|b| holds(a, b))) {
        return Some(Pattern::LShape);
    }
    // Slanted sides: long frames leaning, or cells far from filling their boxes though convex.
    // (One slightly leaning frame is scan skew.)
    if cells.iter().any(|c| c.hull_fill >= 0.9 && c.box_fill < 0.85 && c.rect.area() * 100 >= page * 3) {
        return Some(Pattern::Slanted);
    }
    // A tall cell with two or more cells stacked beside it.
    let gap = pw / 25;
    let tall = cells.iter().any(|a| {
        a.rect.h as f32 >= 0.4 * ph as f32
            && cells
                .iter()
                .filter(|c| {
                    let r = &c.rect;
                    r.y + r.h / 10 >= a.rect.y
                        && r.bottom() <= a.rect.bottom() + r.h / 10
                        && ((r.right() <= a.rect.x + gap && r.right() + gap >= a.rect.x) || (r.x + gap >= a.rect.right() && r.x <= a.rect.right() + gap))
                })
                .count()
                >= 2
    });
    Some(if tall { Pattern::TColumn } else { Pattern::Grid })
}

/// The trace drawn over the page (work resolution): frame lines red, art lines grey, cells tinted.
pub fn debug_image(gray: &GrayImage, t: &Trace) -> image::RgbImage {
    let small = image::imageops::resize(gray, t.w, t.h, FilterType::Triangle);
    let tints = [[255, 200, 200], [200, 255, 200], [200, 200, 255], [255, 255, 180], [255, 200, 255], [180, 255, 255]];
    let mut out = image::RgbImage::from_fn(t.w, t.h, |x, y| {
        let v = small.get_pixel(x, y)[0] as u32;
        let l = t.labels[(y * t.w + x) as usize];
        if l == 0 {
            if t.gutter[(y * t.w + x) as usize] { image::Rgb([(v * 3 / 4) as u8, (v * 3 / 4) as u8, v as u8]) } else { image::Rgb([v as u8; 3]) }
        } else {
            let c = tints[(l as usize - 1) % tints.len()];
            image::Rgb([(v * c[0] / 255) as u8, (v * c[1] / 255) as u8, (v * c[2] / 255) as u8])
        }
    });
    let mut draw = |l: &Line, color: [u8; 3]| {
        let m = if l.horizontal { t.w as f32 / 2.0 } else { t.h as f32 / 2.0 };
        for s in l.lo..l.hi {
            let (x, y) = l.at(s as f32, m);
            if x >= 0.0 && y >= 0.0 && (x as u32) < t.w && (y as u32) < t.h {
                out.put_pixel(x as u32, y as u32, image::Rgb(color));
            }
        }
    };
    for l in &t.lines {
        draw(l, [120, 120, 255]);
    }
    for l in &t.frames {
        draw(l, [255, 0, 0]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    /// A page (700 x 1100), its background `bg`, with framed panels of busy art (frames 3 px).
    fn page(bg: impl Fn(u32, u32) -> u8, panels: &[(u32, u32, u32, u32)]) -> GrayImage {
        GrayImage::from_fn(700, 1100, |x, y| {
            for &(x0, y0, x1, y1) in panels {
                if (x0..x1).contains(&x) && (y0..y1).contains(&y) {
                    let frame = x < x0 + 3 || x + 3 >= x1 || y < y0 + 3 || y + 3 >= y1;
                    // Art: strokes on blank paper, with white sky in the top quarter.
                    let sky = y < y0 + (y1 - y0) / 4;
                    let stroke = !sky && (x * 7 + y * 3) % 23 < 3;
                    return Luma([if frame || stroke { 0 } else { 255 }]);
                }
            }
            Luma([bg(x, y)])
        })
    }

    fn of(panels: &[(u32, u32, u32, u32)]) -> (usize, Option<Pattern>) {
        let t = trace(&page(|_, _| 255, panels));
        (t.cells.len(), pattern(&t))
    }

    #[test]
    fn traces_panels_and_their_pattern() {
        let grid = [(40, 60, 660, 400), (40, 420, 340, 1040), (360, 420, 660, 1040)];
        assert_eq!(of(&grid), (3, Some(Pattern::Grid)));
        let t = [(40, 60, 420, 540), (40, 560, 420, 1040), (440, 60, 660, 1040)];
        assert_eq!(of(&t), (3, Some(Pattern::TColumn)));
    }

    #[test]
    fn white_sky_inside_a_frame_is_no_gutter() {
        // Each panel's top quarter is blank: still one cell per panel.
        let (n, _) = of(&[(40, 60, 660, 520), (40, 540, 660, 1040)]);
        assert_eq!(n, 2);
    }

    #[test]
    fn a_panel_bleeding_off_the_page_keeps_its_blank_part() {
        // A panel running off the top and sides, sky blank: its cell reaches the top of the page.
        let t = trace(&page(|_, _| 255, &[(0, 0, 700, 600), (40, 640, 660, 1040)]));
        let top = t.cells.iter().map(|c| c.rect).min_by_key(|r| r.y).unwrap();
        assert!(top.y <= 10 && top.h >= 500, "{top:?}");
    }

    #[test]
    fn panels_on_a_background_picture_are_a_collage() {
        // Busy background art over the whole page, three framed panels floating on it.
        let bg = |x: u32, y: u32| if (x * 5 + y * 11) % 17 < 4 { 30 } else { 200 };
        let t = trace(&page(bg, &[(80, 100, 360, 400), (380, 450, 640, 760), (100, 800, 400, 1000)]));
        assert_eq!(pattern(&t), Some(Pattern::Collage), "{:?}", t.cells.iter().map(|c| c.rect).collect::<Vec<_>>());
    }
}
