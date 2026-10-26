//! The layout pattern of a page, the third layer of settings after the book's style and
//! substyles (see [`crate::style`]).
//!
//! Every pair of neighbouring panels must have something between them: a frame line (one, or two
//! with a hairline between) or a white gutter. A seam with neither runs through drawn art: the
//! two are one panel cut in two (along a band that only looked like a gutter), and are merged.
//! A merged panel whose box takes in another panel wraps around it (an L-shaped panel around an
//! inset): its outline leaves the inset out.
//!
//! The page is then named by its [`Pattern`].

use crate::detect::{Issue, Mask, Panel, Rect, Shape};
use crate::edges::Edges;
use crate::style::Tuning;
use image::{DynamicImage, GrayImage};

/// Ways panels sit on a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// One picture over the page.
    Splash,
    /// Rows and columns of panels.
    Grid,
    /// A tall panel beside panels stacked on each other (gutters meeting it in a T).
    TColumn,
    /// A panel wrapped around another (an L-shaped panel and the inset in its corner).
    LShape,
    /// A small panel laid over a bigger one.
    Inset,
    /// Four or more rows of panels of one height (newspaper strips collected on a page).
    Strip,
    /// Columns of four or more stacked panels of one size (four-panel strips): read down each
    /// column, not across the rows.
    VerticalStrips,
    /// A page of text (prose, an ad), kept whole.
    Text,
    /// Panels with slanted sides (trapezoids, diagonal gutters).
    Slanted,
    /// Panels laid over a background picture or over each other, at angles: no grid.
    Collage,
}

impl Pattern {
    /// The pattern's preset: how it adjusts the book's settings for this page (`panels`: the
    /// page's panels, `w` x `h`).
    pub fn adjust(self, t: &mut Tuning, panels: &[Panel], w: u32, h: u32) {
        match self {
            // A tall panel beside stacked ones: the gutters meet its frame in a T, and the cut
            // along that frame line is needed even in a book of clean grids.
            Pattern::TColumn => t.frame_lines = true,
            // A grid kept inside the page: nothing bleeds, and merging bleeding pieces only risks
            // joining panels.
            Pattern::Grid => {
                let margin = (w.min(h) / 100).max(2);
                let inside = panels.iter().all(|p| {
                    p.rect.x > margin && p.rect.y > margin && p.rect.right() + margin < w && p.rect.bottom() + margin < h
                });
                if inside {
                    t.bleeds = false;
                }
            }
            // Rows of strips: nothing bleeds between them, and there are no shared frames.
            Pattern::Strip => {
                t.bleeds = false;
                t.frame_lines = false;
            }
            Pattern::VerticalStrips => {
                t.bleeds = false;
                t.frame_lines = false;
            }
            // (Turning off cuts along shared frames there merged whole pages: the panels laid
            // over a background still have frames between them.)
            Pattern::Collage | Pattern::Splash | Pattern::LShape | Pattern::Inset | Pattern::Text | Pattern::Slanted => {}
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Pattern::Splash => "splash",
            Pattern::Grid => "grid",
            Pattern::TColumn => "T column",
            Pattern::LShape => "L shape",
            Pattern::Inset => "inset",
            Pattern::Strip => "strip",
            Pattern::VerticalStrips => "vertical strips",
            Pattern::Text => "text",
            Pattern::Slanted => "slanted",
            Pattern::Collage => "collage",
        }
    }
}

/// A seam has a divider when a straight frame line runs along this share of it (a drawn frame is
/// a few pixels thick: one row of it is dark nearly all along), or a straight blank band: along
/// `BLANK_ALONE` of it, or along `BLANK` with a frame line beside it (the blank rows inside a
/// balloon on the seam have no frame beside them).
const FRAME: f32 = 0.75;
const BLANK: f32 = 0.6;
const BLANK_ALONE: f32 = 0.9;
/// Steepest lean of a frame line tried (as a slope).
const LEAN: f32 = 0.03;
/// How far from a blank band its frame line lies.
const FRAME_BESIDE: i64 = 6;
/// Two boxes are neighbours across a seam when they lie at most this far apart (share of the
/// shorter page side), side by side along at least half of the shorter one.
const NEAR: f32 = 0.02;

/// The seam between two neighbouring boxes: `horizontal` (one above the other) at `at` (page
/// row or column, the middle of their gap or overlap), along `lo..hi`, with the gap's half width.
struct Seam {
    horizontal: bool,
    at: u32,
    lo: u32,
    hi: u32,
    half: u32,
}

fn seam(a: &Rect, b: &Rect, near: u32) -> Option<Seam> {
    let span = |a0: u32, a1: u32, b0: u32, b1: u32| (a0.max(b0), a1.min(b1));
    let gap = |a1: u32, b0: u32| b0 as i64 - a1 as i64;
    let (top, bottom) = if a.y <= b.y { (a, b) } else { (b, a) };
    let (left, right) = if a.x <= b.x { (a, b) } else { (b, a) };
    let (x0, x1) = span(a.x, a.right(), b.x, b.right());
    let (y0, y1) = span(a.y, a.bottom(), b.y, b.bottom());
    let g = gap(top.bottom(), bottom.y);
    if x1 > x0 && (x1 - x0) * 2 >= a.w.min(b.w) && g.abs() <= near as i64 && y1 <= y0 + near.max(g.unsigned_abs() as u32) {
        let at = ((top.bottom() as i64 + bottom.y as i64) / 2) as u32;
        return Some(Seam { horizontal: true, at, lo: x0, hi: x1, half: (g.unsigned_abs() as u32 / 2).max(2) });
    }
    let g = gap(left.right(), right.x);
    if y1 > y0 && (y1 - y0) * 2 >= a.h.min(b.h) && g.abs() <= near as i64 && x1 <= x0 + near.max(g.unsigned_abs() as u32) {
        let at = ((left.right() as i64 + right.x as i64) / 2) as u32;
        return Some(Seam { horizontal: false, at, lo: y0, hi: y1, half: (g.unsigned_abs() as u32 / 2).max(2) });
    }
    None
}

/// Whether a straight frame line or blank band runs along the seam, a little either way of it.
fn divided(gray: &GrayImage, edges: &Edges, bg: u8, s: &Seam) -> bool {
    let (w, h) = gray.dimensions();
    // (Boxes grown around ink overlap past the gutter: look a little way either side.)
    let reach = s.half + (w.min(h) as f32 * 0.03) as u32;
    let px = |t: u32, u: i64| {
        let (x, y) = if s.horizontal { (t as i64, u) } else { (u, t as i64) };
        (x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h).then(|| gray.get_pixel(x as u32, y as u32)[0] as i16)
    };
    let len = s.hi.saturating_sub(s.lo);
    if len < 8 {
        return true;
    }
    let bg = bg as i16;
    let share = |test: &dyn Fn(i16) -> bool, u: i64| {
        (s.lo..s.hi).filter(|&t| px(t, u).is_some_and(test)).count() as f32 / len as f32
    };
    let blank = |v: i16| v >= bg - 30;
    let at = s.at as i64;
    // A frame line is a straight edge (any colour on any paper); it may lean a little (a scan is
    // rarely square): tried at slopes up to `LEAN`.
    let mid = (s.lo + s.hi) as f32 / 2.0;
    let slopes: Vec<f32> = (-6..=6).map(|k| k as f32 * LEAN / 6.0).collect();
    let edge = |t: u32, u: i64| {
        let (x, y) = if s.horizontal { (t as i64, u) } else { (u, t as i64) };
        (-1..=1).any(|e| if s.horizontal { edges.along(x, y + e, true) } else { edges.along(x + e, y, false) })
    };
    let line = |u: i64| {
        slopes.iter().any(|&m| {
            (s.lo..s.hi).filter(|&t| edge(t, u + (m * (t as f32 - mid)).round() as i64)).count() as f32 >= FRAME * len as f32
        })
    };
    let window = || at - reach as i64..=at + reach as i64;
    // (The frame of a wide gutter lies at its edge, anywhere in the seam's window.)
    let framed = window().clone().any(line);
    let wider = || at - reach as i64 - FRAME_BESIDE..=at + reach as i64 + FRAME_BESIDE;
    framed
        || window().any(|u| {
            let b = share(&blank, u);
            b >= BLANK_ALONE || (b >= BLANK && wider().any(line))
        })
}

/// Merge the panels with no divider between them, give a merged panel wrapped around another an
/// outline leaving it out, and name the page's pattern. `img` is the page.
pub fn resolve(img: &DynamicImage, panels: &mut Vec<Panel>) -> Pattern {
    let (w, h) = (img.width(), img.height());
    let gray = img.to_luma8();
    let bg = crate::detect::background_luma(&gray);
    let near = (NEAR * w.min(h) as f32) as u32;
    drop_undrawn_outlines(panels, &gray, bg);
    let edges = Edges::new(&gray);
    let mut merged_any = merge_pieces(panels, &edges, w, h);
    loop {
        let pair = (0..panels.len()).flat_map(|i| (i + 1..panels.len()).map(move |j| (i, j))).find(|&(i, j)| {
            seam(&panels[i].rect, &panels[j].rect, near).is_some_and(|s| !divided(&gray, &edges, bg, &s))
        });
        let Some((i, j)) = pair else { break };
        let b = panels.remove(j);
        let a = &mut panels[i];
        let (x, y) = (a.rect.x.min(b.rect.x), a.rect.y.min(b.rect.y));
        a.rect = Rect { x, y, w: a.rect.right().max(b.rect.right()) - x, h: a.rect.bottom().max(b.rect.bottom()) - y };
        a.shape = None;
        for &issue in &b.issues {
            a.add_issue(issue);
        }
        a.add_issue(Issue::Seamless);
        merged_any = true;
    }
    // A merged panel wrapping around another: its outline leaves the other out.
    let mut l_shape = false;
    if merged_any {
        let rects: Vec<Rect> = panels.iter().map(|p| p.rect).collect();
        for (i, p) in panels.iter_mut().enumerate() {
            if !p.issues.contains(&Issue::Seamless) {
                continue;
            }
            let inside: Vec<Rect> = rects
                .iter()
                .enumerate()
                .filter(|&(j, r)| j != i && r.overlap_area(&p.rect) * 10 >= r.area() * 8)
                .map(|(_, r)| *r)
                .collect();
            if inside.is_empty() {
                continue;
            }
            p.shape = Some(outline_without(&p.rect, &inside));
            l_shape = true;
        }
    }
    classify(panels, w, h, l_shape)
}

/// A cut-out outline is kept only along a drawn frame, with the gutter outside it: where art
/// carries on past the outline (the open side of a panel bleeding off the page, a guessed slant)
/// it would cut the art away, and the panel is cropped as its rectangle instead.
fn drop_undrawn_outlines(panels: &mut [Panel], gray: &GrayImage, bg: u8) {
    /// Outline points with art (not paper) just outside them, at most.
    const ART_OUTSIDE: f32 = 0.25;
    let (w, h) = gray.dimensions();
    // (Paper is the page's own colour, light or dark.)
    let paper = |x: i64, y: i64| x < 0 || y < 0 || x as u32 >= w || y as u32 >= h || (gray.get_pixel(x as u32, y as u32)[0] as i16 - bg as i16).abs() <= 40;
    for p in panels.iter_mut() {
        let Some(shape) = &p.shape else { continue };
        let pts: Vec<(f32, f32)> = shape.outline().step_by(3).collect();
        // Points where the cut-out leaves the box (not the box's own sides, which are framed or
        // the page edge), with what lies 8 to 14 pixels outside them.
        let (mut n, mut art) = (0, 0);
        for &(x, y) in &pts {
            let (xi, yi) = (x as i64, y as i64);
            let r = p.rect;
            // (Within a couple of mask pixels of the box's sides.)
            const SIDE: i64 = 10;
            let on_box = xi <= r.x as i64 + SIDE || yi <= r.y as i64 + SIDE || xi + SIDE >= r.right() as i64 || yi + SIDE >= r.bottom() as i64;
            if on_box {
                continue;
            }
            // Outward: the direction (of eight) whose point is outside the outline.
            let Some((dx, dy)) = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)]
                .into_iter()
                .find(|&(dx, dy)| shape.alpha(x + 6.0 * dx as f32, y + 6.0 * dy as f32) < 0.5)
            else {
                continue;
            };
            n += 1;
            art += (8..=14).any(|d| !paper(xi + d * dx, yi + d * dy)) as usize;
        }
        if n > 0 && art as f32 > ART_OUTSIDE * n as f32 {
            p.shape = None;
        }
    }
}

/// Reading order fix for L-shaped panels: one wrapping around an inset starts above it, so it
/// comes before the panels it holds.
pub fn l_first(panels: &mut [Panel]) {
    let n = panels.len();
    for i in 0..n {
        if panels[i].shape.is_none() || !panels[i].issues.contains(&Issue::Seamless) {
            continue;
        }
        let l = panels[i].rect;
        let first_inside = (0..i).find(|&j| {
            let r = panels[j].rect;
            r.overlap_area(&l) * 10 >= r.area() * 8 && r.y > l.y
        });
        if let Some(j) = first_inside {
            panels[j..=i].rotate_right(1);
        }
    }
}

/// Share of a panel's own area (its outline where it has one) a point sample says is inside.
fn inside(p: &Panel, x: u32, y: u32) -> bool {
    x >= p.rect.x
        && y >= p.rect.y
        && x < p.rect.right()
        && y < p.rect.bottom()
        && p.shape.as_ref().is_none_or(|s| s.alpha(x as f32 + 0.5, y as f32 + 0.5) > 0.5)
}

/// Area two panels share, over the smaller one's area (by their outlines, sampled).
fn shared(a: &Panel, b: &Panel) -> f32 {
    let (x0, y0) = (a.rect.x.max(b.rect.x), a.rect.y.max(b.rect.y));
    let (x1, y1) = (a.rect.right().min(b.rect.right()), a.rect.bottom().min(b.rect.bottom()));
    if x1 <= x0 || y1 <= y0 {
        return 0.0;
    }
    const STEP: u32 = 4;
    let own = |p: &Panel| {
        let mut n = 0usize;
        for y in (p.rect.y..p.rect.bottom()).step_by(STEP as usize) {
            for x in (p.rect.x..p.rect.right()).step_by(STEP as usize) {
                n += inside(p, x, y) as usize;
            }
        }
        n.max(1)
    };
    let mut both = 0usize;
    for y in (y0..y1).step_by(STEP as usize) {
        for x in (x0..x1).step_by(STEP as usize) {
            both += (inside(a, x, y) && inside(b, x, y)) as usize;
        }
    }
    both as f32 / own(a).min(own(b)) as f32
}

/// Sides of `r` drawn as a straight frame line: an edge along three quarters of the frame's own
/// length, somewhere from a little outside the box to 3% of the page inside it (a box is padded,
/// or grown around a caption past its frame).
fn framed_sides(edges: &Edges, w: u32, h: u32, r: &Rect) -> usize {
    let depth = (w.min(h) as f32 * 0.03) as i64;
    // `inward`: the direction into the box; the line runs over the middle 80% of the side (its
    // corners may be cut by a neighbour's box).
    let line = |horizontal: bool, at: i64, inward: i64, lo: u32, hi: u32| {
        let (lo, hi) = (lo + (hi - lo) / 10, hi - (hi - lo) / 10);
        (-4..=depth).any(|d| {
            let u = at + inward * d;
            // (A scanned line drifts a pixel or two along its length.)
            edges.line(horizontal, u, lo, hi, 2) >= 0.75
        })
    };
    [
        line(true, r.y as i64, 1, r.x, r.right()),
        line(true, r.bottom() as i64 - 1, -1, r.x, r.right()),
        line(false, r.x as i64, 1, r.y, r.bottom()),
        line(false, r.right() as i64 - 1, -1, r.y, r.bottom()),
    ]
    .iter()
    .filter(|&&b| b)
    .count()
}

/// Merge pieces of one panel: outlines overlapping by half the smaller; unframed pieces touching
/// each other; a small unframed scrap touching a panel. Returns whether any were merged.
fn merge_pieces(panels: &mut Vec<Panel>, edges: &Edges, w: u32, h: u32) -> bool {
    const OVERLAP: f32 = 0.5;
    const SCRAP: f32 = 0.05;
    let touch = (w.min(h) / 60).max(4) as i64;
    let gap = |a: &Rect, b: &Rect| {
        let dx = (b.x as i64 - a.right() as i64).max(a.x as i64 - b.right() as i64);
        let dy = (b.y as i64 - a.bottom() as i64).max(a.y as i64 - b.bottom() as i64);
        dx.max(dy)
    };
    let page = w as f32 * h as f32;
    let mut any = false;
    loop {
        let sides: Vec<usize> = panels.iter().map(|p| framed_sides(edges, w, h, &p.rect)).collect();
        // Pieces built around loose ink (figures on blank paper) with at most one frame side.
        let loose: Vec<bool> = panels.iter().zip(&sides).map(|(p, &n)| n <= 1 && p.issues.contains(&Issue::Joined)).collect();
        // A small piece with no frame at all.
        let scrap: Vec<bool> = panels.iter().zip(&sides).map(|(p, &n)| n == 0 && (p.rect.area() as f32) < SCRAP * page).collect();
        let pair = (0..panels.len()).flat_map(|i| (i + 1..panels.len()).map(move |j| (i, j))).find(|&(i, j)| {
            let (a, b) = (&panels[i], &panels[j]);
            let touching = gap(&a.rect, &b.rect) <= touch;
            // (The merged box must not swallow a third panel: a sliver across two panels joins neither.)
            let (x, y) = (a.rect.x.min(b.rect.x), a.rect.y.min(b.rect.y));
            let u = Rect { x, y, w: a.rect.right().max(b.rect.right()) - x, h: a.rect.bottom().max(b.rect.bottom()) - y };
            let swallows = panels.iter().enumerate().any(|(k, o)| k != i && k != j && o.rect.overlap_area(&u) * 5 > o.rect.area());
            !swallows && (shared(a, b) >= OVERLAP || (touching && ((loose[i] && loose[j]) || scrap[i] || scrap[j])))
        });
        let Some((i, j)) = pair else { return any };
        let b = panels.remove(j);
        let a = &mut panels[i];
        let (x, y) = (a.rect.x.min(b.rect.x), a.rect.y.min(b.rect.y));
        a.rect = Rect { x, y, w: a.rect.right().max(b.rect.right()) - x, h: a.rect.bottom().max(b.rect.bottom()) - y };
        a.shape = None;
        for &issue in &b.issues {
            a.add_issue(issue);
        }
        a.add_issue(Issue::Seamless);
        any = true;
    }
}

/// The outline of `r` less the boxes `holes`, at a quarter of the page resolution.
pub(crate) fn outline_without(r: &Rect, holes: &[Rect]) -> Shape {
    const SCALE: u32 = 4;
    let (mw, mh) = ((r.w / SCALE).max(1) as usize, (r.h / SCALE).max(1) as usize);
    let mut m = Mask::new(mw, mh);
    for y in 0..mh {
        for x in 0..mw {
            let (px, py) = (r.x + x as u32 * SCALE + SCALE / 2, r.y + y as u32 * SCALE + SCALE / 2);
            m.data[y * mw + x] = !holes.iter().any(|h| px >= h.x && px < h.right() && py >= h.y && py < h.bottom());
        }
    }
    let s = SCALE as f32;
    Shape::from_mask(&m, r.x as f32, r.y as f32, s, s)
}

fn classify(panels: &[Panel], w: u32, h: u32, l_shape: bool) -> Pattern {
    let page = w as u64 * h as u64;
    if panels.len() == 1 && panels[0].issues.contains(&Issue::Text) {
        return Pattern::Text;
    }
    if panels.len() == 1 || panels.iter().any(|p| p.rect.area() * 10 >= page * 8) {
        return Pattern::Splash;
    }
    if l_shape {
        return Pattern::LShape;
    }
    let inside = |a: &Rect, b: &Rect| a.overlap_area(b) * 10 >= a.area() * 8 && b.area() > a.area();
    if panels.iter().any(|a| panels.iter().any(|b| inside(&a.rect, &b.rect))) {
        return Pattern::Inset;
    }
    if let Some(p) = strips(panels) {
        return p;
    }
    // Panels laid over each other: two or more pairs of boxes overlapping by a fifth of the
    // smaller one.
    let overlapping = (0..panels.len())
        .flat_map(|i| (i + 1..panels.len()).map(move |j| (i, j)))
        .filter(|&(i, j)| {
            let (a, b) = (&panels[i].rect, &panels[j].rect);
            a.overlap_area(b) * 5 >= a.area().min(b.area())
        })
        .count();
    if overlapping >= 2 {
        return Pattern::Collage;
    }
    // A panel beside two or more stacked ones that it spans.
    let tall = panels.iter().any(|t| {
        let beside: Vec<&Rect> = panels
            .iter()
            .map(|p| &p.rect)
            .filter(|r| {
                let rows = r.y.max(t.rect.y) < r.bottom().min(t.rect.bottom());
                let apart = r.right() <= t.rect.x + t.rect.w / 10 || r.x + t.rect.w / 10 >= t.rect.right();
                rows && apart && r.y + r.h / 10 >= t.rect.y && r.bottom() <= t.rect.bottom() + r.h / 10
            })
            .collect();
        let stacked = beside.iter().any(|a| beside.iter().any(|b| b.y >= a.bottom().saturating_sub(a.h / 10) && a.x < b.right() && b.x < a.right()));
        stacked
    });
    if tall { Pattern::TColumn } else { Pattern::Grid }
}

/// Panels grouped into bands along one axis (`rows`: by their vertical extent), each band a list
/// of panel indices: panels overlapping a band by half the smaller extent join it.
fn bands(panels: &[Panel], rows: bool) -> Vec<Vec<usize>> {
    let span = |r: &Rect| if rows { (r.y, r.bottom()) } else { (r.x, r.right()) };
    let mut idx: Vec<usize> = (0..panels.len()).collect();
    idx.sort_by_key(|&i| {
        let (a, b) = span(&panels[i].rect);
        a + b
    });
    let mut out: Vec<(u32, u32, Vec<usize>)> = Vec::new();
    for i in idx {
        let (a, b) = span(&panels[i].rect);
        match out.last_mut() {
            Some((lo, hi, members)) if b.min(*hi).saturating_sub(a.max(*lo)) * 2 >= (b - a).min(*hi - *lo) => {
                *lo = (*lo).min(a);
                *hi = (*hi).max(b);
                members.push(i);
            }
            _ => out.push((a, b, vec![i])),
        }
    }
    out.into_iter().map(|(_, _, m)| m).collect()
}

/// Whether all `panels` are about one size along an axis (`heights`): within `tol` of the median.
fn even(panels: &[&Panel], heights: bool, tol: f32) -> bool {
    let mut v: Vec<u32> = panels.iter().map(|p| if heights { p.rect.h } else { p.rect.w }).collect();
    v.sort_unstable();
    let m = v[v.len() / 2] as f32;
    v.iter().all(|&x| (x as f32 - m).abs() <= tol * m)
}

fn median(mut v: Vec<u32>) -> u32 {
    v.sort_unstable();
    v[v.len() / 2]
}

/// Strips: four or more rows of panels of one height, or columns each of four or more stacked
/// panels of one size.
fn strips(panels: &[Panel]) -> Option<Pattern> {
    let all: Vec<&Panel> = panels.iter().collect();
    let cols = bands(panels, false);
    if cols.len() >= 2 && cols.iter().all(|c| c.len() >= 4) {
        let ps: Vec<&Panel> = cols.iter().flatten().map(|&i| &panels[i]).collect();
        // (The columns stand well apart: a grid's rows and columns are spaced alike.)
        let gaps = |members: &[usize], vertical: bool| -> Vec<i64> {
            let mut m: Vec<&Rect> = members.iter().map(|&i| &panels[i].rect).collect();
            m.sort_by_key(|r| if vertical { r.y } else { r.x });
            m.windows(2).map(|w| if vertical { w[1].y as i64 - w[0].bottom() as i64 } else { w[1].x as i64 - w[0].right() as i64 }).collect()
        };
        let row_gap = median(cols.iter().flat_map(|c| gaps(c, true)).map(|g| g.max(0) as u32).collect()).max(1) as i64;
        // (By the median edges: a box grown around a caption may stick out.)
        let edges: Vec<(u32, u32)> = cols
            .iter()
            .map(|c| (median(c.iter().map(|&i| panels[i].rect.x).collect()), median(c.iter().map(|&i| panels[i].rect.right()).collect())))
            .collect();
        let mut edges = edges;
        edges.sort_unstable();
        let col_gap = edges.windows(2).map(|w| w[1].0 as i64 - w[0].1 as i64).min().unwrap_or(0);
        // (The first panel of a strip may be taller: it holds the strip's title.)
        if even(&ps, true, 0.3) && even(&ps, false, 0.2) && col_gap >= 2 * row_gap {
            return Some(Pattern::VerticalStrips);
        }
    }
    let rows = bands(panels, true);
    (rows.len() >= 4 && even(&all, true, 0.2)).then_some(Pattern::Strip)
}

/// Reading order for vertical strips: down each column, the columns left to right (right to left
/// in manga).
pub fn column_order(panels: Vec<Panel>, rtl: bool) -> Vec<Panel> {
    let mut cols = bands(&panels, false);
    for c in &mut cols {
        c.sort_by_key(|&i| panels[i].rect.y);
    }
    if rtl {
        cols.reverse();
    }
    let order: Vec<usize> = cols.into_iter().flatten().collect();
    let mut slots: Vec<Option<Panel>> = panels.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots[i].take()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    fn page(f: impl Fn(u32, u32) -> u8) -> DynamicImage {
        DynamicImage::ImageLuma8(GrayImage::from_fn(700, 1100, |x, y| Luma([f(x, y)])))
    }

    fn framed(x0: u32, y0: u32, x1: u32, y1: u32) -> impl Fn(u32, u32) -> Option<u8> {
        move |x, y| {
            if x < x0 || x >= x1 || y < y0 || y >= y1 {
                None
            } else if x < x0 + 3 || x + 3 >= x1 || y < y0 + 3 || y + 3 >= y1 {
                Some(0)
            } else {
                Some(140)
            }
        }
    }

    /// Drawn art: dark strokes on a third of the pixels, scattered.
    fn busy(x: u32, y: u32) -> u8 {
        let n = x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263);
        if n.wrapping_mul(2_654_435_761) >> 29 < 3 { 20 } else { 200 }
    }

    fn panel(x: u32, y: u32, w: u32, h: u32) -> Panel {
        Panel::new(Rect { x, y, w, h }, None, vec![Issue::Split])
    }

    #[test]
    fn keeps_panels_apart_across_a_gutter_or_frame() {
        // Two panels with a white gutter, and two sharing one frame line.
        let (a, b) = (framed(40, 60, 660, 500), framed(40, 520, 660, 1040));
        let img = page(|x, y| a(x, y).or(b(x, y)).unwrap_or(255));
        let mut p = vec![panel(40, 60, 620, 440), panel(40, 520, 620, 520)];
        assert_eq!(resolve(&img, &mut p), Pattern::Grid);
        assert_eq!(p.len(), 2);
        let (a, b) = (framed(40, 60, 660, 503), framed(40, 500, 660, 1040));
        let img = page(|x, y| a(x, y).or(b(x, y)).unwrap_or(255));
        let mut p = vec![panel(40, 60, 620, 443), panel(40, 500, 620, 540)];
        resolve(&img, &mut p);
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn merges_a_panel_cut_through_its_art() {
        // One framed panel with busy art, found as two pieces cut at y = 500.
        let a = framed(40, 60, 660, 1040);
        let img = page(|x, y| a(x, y).map(|v| if v == 140 { busy(x, y) } else { v }).unwrap_or(255));
        let mut p = vec![panel(40, 60, 620, 440), panel(40, 500, 620, 540)];
        resolve(&img, &mut p);
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!(p[0].rect, Rect { x: 40, y: 60, w: 620, h: 980 });
    }

    #[test]
    fn an_l_shaped_panel_leaves_its_inset_out() {
        // An L-shaped panel (left column and bottom row, one picture) around an inset at top
        // right, found as three pieces: the left column, the bottom row and the inset.
        let inset = framed(350, 60, 660, 500);
        let img = page(|x, y| {
            if let Some(v) = inset(x, y) {
                return v;
            }
            let in_l = (40..660).contains(&x) && (60..1040).contains(&y);
            let frame = x < 43 || x >= 657 || y < 63 || y >= 1037;
            if !in_l { 255 } else if frame { 0 } else { busy(x, y) }
        });
        let mut p = vec![panel(40, 60, 300, 450), panel(40, 510, 620, 530), panel(350, 60, 310, 440)];
        assert_eq!(resolve(&img, &mut p), Pattern::LShape);
        assert_eq!(p.len(), 2, "{p:?}");
        let l = p.iter().find(|p| p.rect.h > 900).expect("the L panel");
        let s = l.shape.as_ref().expect("an outline");
        assert!(s.alpha(100.0, 300.0) > 0.5 && s.alpha(100.0, 800.0) > 0.5, "the L itself");
        assert!(s.alpha(500.0, 300.0) < 0.5, "the inset is left out");
    }

    #[test]
    fn patterns_adjust_the_book_settings() {
        let grid_book = Tuning { bleeds: false, frame_lines: false, ..Tuning::default() };
        let mut t = grid_book;
        Pattern::TColumn.adjust(&mut t, &[], 700, 1100);
        assert!(t.frame_lines, "a T column needs the cut along the tall panel's frame");
        let inside = [panel(40, 60, 620, 440), panel(40, 520, 620, 520)];
        let bleeding = [panel(0, 0, 700, 500), panel(40, 520, 620, 520)];
        let (mut a, mut b) = (Tuning::default(), Tuning::default());
        Pattern::Grid.adjust(&mut a, &inside, 700, 1100);
        Pattern::Grid.adjust(&mut b, &bleeding, 700, 1100);
        assert!(!a.bleeds && b.bleeds, "bleed merging off only on a page that does not bleed");
    }

    #[test]
    fn merges_pieces_of_one_panel() {
        // A framed panel, and a piece cut out of it by a slanted second-round cut.
        let a = framed(40, 60, 660, 500);
        let img = page(|x, y| a(x, y).map(|v| if v == 140 { busy(x, y) } else { v }).unwrap_or(255));
        let mut p = vec![panel(40, 60, 620, 440), panel(300, 200, 200, 200)];
        resolve(&img, &mut p);
        assert_eq!(p.len(), 1, "piece inside a panel: {p:?}");
        // Figures on blank paper found as two touching pieces, each built around loose ink.
        let fig = |x: u32, y: u32| (((x as i64 - 350).pow(2) / 9 + (y as i64 - 600).pow(2) / 64) < 900).then_some(60u8);
        let img = page(|x, y| fig(x, y).unwrap_or(255));
        let loose = |x, y, w, h| Panel::new(Rect { x, y, w, h }, None, vec![Issue::Joined]);
        let mut p = vec![loose(260, 360, 180, 240), loose(260, 602, 180, 240)];
        resolve(&img, &mut p);
        assert_eq!(p.len(), 1, "borderless figure in pieces: {p:?}");
        // Two framed panels touching stay apart.
        let (a, b) = (framed(40, 60, 660, 500), framed(40, 504, 660, 1040));
        let img = page(|x, y| a(x, y).or(b(x, y)).unwrap_or(255));
        let mut p = vec![panel(40, 60, 620, 440), panel(40, 504, 620, 536)];
        resolve(&img, &mut p);
        assert_eq!(p.len(), 2, "{p:?}");
    }

    #[test]
    fn drops_an_outline_cutting_through_art() {
        // Busy art over the whole panel, with a slanted outline cutting its top left corner.
        let a = framed(40, 60, 660, 1040);
        let img = page(|x, y| a(x, y).map(|v| if v == 140 { busy(x, y) } else { v }).unwrap_or(255));
        let r = Rect { x: 40, y: 60, w: 620, h: 980 };
        let mut m = Mask::new(155, 245);
        for y in 0..245 {
            for x in 0..155 {
                m.data[y * 155 + x] = x + y >= 60;
            }
        }
        let mut p = vec![Panel::new(r, Some(Shape::from_mask(&m, 40.0, 60.0, 4.0, 4.0)), vec![Issue::Split])];
        resolve(&img, &mut p);
        assert!(p[0].shape.is_none(), "outline through art kept");
    }

    #[test]
    fn names_a_collage() {
        // Panels laid over each other.
        let p = [panel(40, 60, 400, 400), panel(240, 250, 400, 400), panel(60, 600, 400, 400), panel(300, 700, 360, 360)];
        assert_eq!(classify(&p, 700, 1100, false), Pattern::Collage);
    }

    #[test]
    fn names_strips_and_text() {
        let strips: Vec<Panel> = (0..5).flat_map(|r| (0..3).map(move |c| panel(40 + c * 210, 60 + r * 200, 200, 180))).collect();
        assert_eq!(classify(&strips, 700, 1100, false), Pattern::Strip);
        // Four rows of different heights: an ordinary page.
        let uneven = [panel(40, 60, 620, 150), panel(40, 230, 620, 400), panel(40, 650, 620, 120), panel(40, 790, 620, 250)];
        assert_eq!(classify(&uneven, 700, 1100, false), Pattern::Grid);
        // Two columns of four equal panels, read down each column, the right one first.
        let koma: Vec<Panel> = (0..2).flat_map(|c| (0..4).map(move |r| panel(60 + c * 320, 60 + r * 245, 260, 235))).collect();
        assert_eq!(classify(&koma, 700, 1100, false), Pattern::VerticalStrips);
        let order = column_order(koma, true);
        assert_eq!(order.iter().map(|p| (p.rect.x, p.rect.y)).collect::<Vec<_>>()[..5], [(380, 60), (380, 305), (380, 550), (380, 795), (60, 60)]);
        let text = [Panel::new(Rect { x: 0, y: 0, w: 700, h: 1100 }, None, vec![Issue::Text])];
        assert_eq!(classify(&text, 700, 1100, false), Pattern::Text);
    }

    #[test]
    fn names_a_tall_column() {
        let rects = [panel(40, 60, 380, 480), panel(40, 560, 380, 480), panel(440, 60, 220, 980)];
        assert_eq!(classify(&rects, 700, 1100, false), Pattern::TColumn);
        let grid = [panel(40, 60, 300, 480), panel(360, 60, 300, 480), panel(40, 560, 620, 480)];
        assert_eq!(classify(&grid, 700, 1100, false), Pattern::Grid);
    }
}
