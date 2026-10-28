//! Second round for panels the first pass doubts (see [`Panel::is_low`]).
//!
//! The first pass works on a downscaled page and cuts only straight along rows
//! and columns, so it misses narrow or slanted gutters. Here each doubtful panel
//! is searched at full resolution for a straight gutter at any angle within ±12°
//! of horizontal or vertical: a line that is almost all paper with a dark border
//! line close by on *both* sides. Requiring both borders keeps it from cutting
//! along a caption box or a horizon, which are single lines. Small doubtful
//! fragments (title lettering) are merged into the nearest panel, and slivers
//! along the page edge (scan debris) are dropped.

use crate::detect::{self, Issue, Mask, Panel, Rect, Shape};
use image::DynamicImage;

/// Longest page side used for the search.
const WORK_MAX: u32 = 2400;
/// Steepest gutter tried, as a slope (tan 12°), and the angle step (≈0.5°).
const MAX_SLOPE: f32 = 0.2126;
const SLOPE_STEP: f32 = 0.0087;
const MAX_DEPTH: u32 = 3;
/// A gutter crossed by art: at least this share paper, with frame lines at least
/// `CROSSED_BORDER` dark on both sides.
const CROSSED_PAPER: f32 = 0.72;
/// A black frame line between two panels: share of dark pixels along it.
/// Both set above 1 (off): on test pages they cut through a caption bar and a wide panel.
/// Only clean white gutters split; was 0.7 and 0.85.
const CROSSED_BORDER: f32 = 2.0;
const BLACK_LINE: f32 = 2.0;

#[derive(Default, Debug, Clone, Copy)]
pub struct Stats {
    /// Doubtful panels split along a gutter, and how many panels they became.
    pub split: usize,
    pub pieces: usize,
    /// Fragments merged into a neighbour, and slivers of scan debris dropped.
    pub merged: usize,
    pub dropped: usize,
}

/// Re-examine the doubtful panels of one page. Returns the page's panels in reading order.
pub fn refine_page(img: &DynamicImage, panels: Vec<Panel>, rtl: bool, padding: f32) -> (Vec<Panel>, Stats) {
    let mut stats = Stats::default();
    if !panels.iter().any(Panel::is_low) {
        let mut panels = panels;
        merge_unframed_overlaps(&mut panels, img, &mut stats);
        if stats.merged > 0 {
            panels = detect::reading_order(panels, |p| p.rect, rtl);
        }
        return (panels, stats);
    }
    let (pw, ph) = (img.width(), img.height());
    let mut gray = img.to_luma8();
    let ws = (WORK_MAX as f32 / pw.max(ph) as f32).min(1.0);
    if ws < 1.0 {
        let (w, h) = ((pw as f32 * ws) as u32, (ph as f32 * ws) as u32);
        gray = image::imageops::resize(&gray, w, h, image::imageops::FilterType::Triangle);
    }
    let bg = detect::background_luma(&gray) as i32;
    let page = Page {
        w: gray.width() as usize,
        h: gray.height() as usize,
        // Paper is the page's own colour, light or dark; a border is dark ink or a strong edge
        // (a frame on a dark page is light).
        paper: gray.pixels().map(|p| ((p[0] as i32 - bg).abs() <= 45) as u8).collect(),
        dark: {
            let edges = crate::edges::Edges::new(&gray);
            let w = gray.width() as usize;
            gray.pixels()
                .enumerate()
                .map(|(i, p)| {
                    let (x, y) = ((i % w) as i64, (i / w) as i64);
                    ((p[0] as i32) <= (bg * 45 / 100).min(110) || (bg < 128 && (edges.along(x, y, true) || edges.along(x, y, false)))) as u8
                })
                .collect()
        },
        ws,
    };
    let pad = (pw.min(ph) as f32 * padding).round() as i64;

    let mut out: Vec<Panel> = Vec::new();
    let mut fragments: Vec<Panel> = Vec::new();
    for p in panels {
        // A box grown around text keeps its shape, unless it is bigger than usual or a gutter
        // runs across it: then it may hold several panels. Panels kept together for a balloon
        // they share are not cut apart again.
        let oversized = p.issues.iter().any(|i| matches!(i, Issue::Oversized | Issue::Large | Issue::InnerGutter));
        if !p.is_low() || (p.issues.contains(&Issue::Joined) && !oversized) {
            out.push(p);
            continue;
        }
        if p.issues.contains(&Issue::Small) {
            // A sliver along the page edge is scan debris, not content.
            let r = p.rect;
            let edge = (pw.min(ph) as f32 * 0.01) as u32;
            let at_edge = r.x <= edge || r.y <= edge || r.right() + edge >= pw || r.bottom() + edge >= ph;
            if at_edge && r.w.max(r.h) > 6 * r.w.min(r.h) {
                stats.dropped += 1;
            } else {
                fragments.push(p);
            }
            continue;
        }
        if p.issues.contains(&Issue::KeptWhole) {
            out.push(p);
            continue;
        }
        let region = page.region(&p);
        let mut pieces = Vec::new();
        split(&page, region, 0, &mut pieces);
        if pieces.len() > 1 {
            stats.split += 1;
            stats.pieces += pieces.len();
            out.extend(pieces.into_iter().map(|r| r.into_panel(&page, pw, ph, pad)));
        } else {
            // The full-resolution search is the stronger test of the gutter hypothesis.
            let mut p = p;
            for refuted in [Issue::InnerGutter, Issue::Large] {
                if p.issues.contains(&refuted) {
                    p.clear_issue(refuted);
                    p.add_issue(Issue::Refined);
                }
            }
            out.push(p);
        }
    }

    // A fragment next to a panel belongs to it (a title over a splash, a sliver of a frame).
    let reach = 0.03 * pw.max(ph) as f32;
    for f in fragments {
        // Merging must not make a panel swallow its neighbours: the box of a title merged into
        // a narrow panel can span a whole row.
        let swallows = |i: usize| {
            let u = union(&out[i].rect, &f.rect);
            out.iter().enumerate().any(|(j, o)| j != i && o.rect.overlap_area(&u) * 5 > o.rect.area())
        };
        let nearest = out
            .iter()
            .enumerate()
            .map(|(i, p)| (i, gap(&f.rect, &p.rect)))
            .filter(|&(i, d)| d <= reach && !swallows(i))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        match nearest {
            Some((i, _)) => {
                let t = &mut out[i];
                t.rect = union(&t.rect, &f.rect);
                // The union is no longer the old outline: crop it as a rectangle.
                t.shape = None;
                t.add_issue(Issue::Refined);
                stats.merged += 1;
            }
            None => out.push(f),
        }
    }
    merge_unframed_overlaps(&mut out, img, &mut stats);
    (detect::reading_order(out, |p| p.rect, rtl), stats)
}

/// Two panels whose boxes overlap a good deal, one of them without a frame of its own (a box
/// grown around ink, a borderless candidate), are pieces of one borderless picture: figures on
/// blank rain with no gutter between them. Framed panels are left alone: the boxes of slanted
/// panels overlap. So is a pair with a frame line across the overlap: a framed panel whose box
/// grew past its frame.
fn merge_unframed_overlaps(panels: &mut Vec<Panel>, img: &DynamicImage, stats: &mut Stats) {
    let unframed = |p: &Panel| p.issues.iter().any(|i| matches!(i, Issue::Joined | Issue::NoBorder));
    let gray = img.to_luma8();
    let bg = detect::background_luma(&gray) as i32;
    let dark = |x: u32, y: u32| (gray.get_pixel(x, y)[0] as i32) < bg - 100;
    // A line along the overlap's long side dark nearly all along it (a figure crossing the overlap
    // spans its short side). Only a long thin overlap is a strip along a frame: in a squarer one, a
    // figure in black is dark all along too.
    let frame_across = |o: Rect| {
        if o.w.max(o.h) < 3 * o.w.min(o.h) {
            false
        } else if o.w >= o.h {
            (o.y..o.bottom()).any(|y| (o.x..o.right()).filter(|&x| dark(x, y)).count() * 10 >= o.w as usize * 8)
        } else {
            (o.x..o.right()).any(|x| (o.y..o.bottom()).filter(|&y| dark(x, y)).count() * 10 >= o.h as usize * 8)
        }
    };
    let overlap = |a: &Rect, b: &Rect| {
        let (x0, y0) = (a.x.max(b.x), a.y.max(b.y));
        let (x1, y1) = (a.right().min(b.right()).min(gray.width()), a.bottom().min(b.bottom()).min(gray.height()));
        (x1 > x0 && y1 > y0).then(|| Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    };
    loop {
        let pair = (0..panels.len()).flat_map(|i| (i + 1..panels.len()).map(move |j| (i, j))).find(|&(i, j)| {
            let (a, b) = (&panels[i], &panels[j]);
            (unframed(a) || unframed(b))
                && a.rect.overlap_area(&b.rect) * 100 >= 15 * a.rect.area().min(b.rect.area())
                && overlap(&a.rect, &b.rect).is_some_and(|o| !frame_across(o))
        });
        let Some((i, j)) = pair else { return };
        let b = panels.remove(j);
        let a = &mut panels[i];
        a.rect = union(&a.rect, &b.rect);
        a.shape = None;
        for issue in b.issues {
            a.add_issue(issue);
        }
        a.add_issue(Issue::Refined);
        stats.merged += 1;
    }
}

/// Paper and dark-ink maps of the page at search resolution.
struct Page {
    w: usize,
    h: usize,
    paper: Vec<u8>,
    dark: Vec<u8>,
    /// Search pixels per page pixel.
    ws: f32,
}

/// Part of the page being searched: a box plus which of its pixels belong to the panel.
struct Region {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    m: Vec<bool>,
}

impl Page {
    fn region(&self, p: &Panel) -> Region {
        let x = ((p.rect.x as f32 * self.ws) as usize).min(self.w - 1);
        let y = ((p.rect.y as f32 * self.ws) as usize).min(self.h - 1);
        let w = ((p.rect.w as f32 * self.ws) as usize).clamp(1, self.w - x);
        let h = ((p.rect.h as f32 * self.ws) as usize).clamp(1, self.h - y);
        let mut m = vec![true; w * h];
        if let Some(s) = &p.shape {
            for yy in 0..h {
                for xx in 0..w {
                    let (px, py) = (((x + xx) as f32 + 0.5) / self.ws, ((y + yy) as f32 + 0.5) / self.ws);
                    m[yy * w + xx] = s.alpha(px, py) > 0.5;
                }
            }
        }
        Region { x, y, w, h, m }
    }
}

impl Region {
    fn area(&self) -> usize {
        self.m.iter().filter(|&&b| b).count()
    }

    /// Smallest region holding the pixels where `keep` is true.
    fn sub(&self, keep: impl Fn(usize, usize) -> bool) -> Option<Region> {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.m[y * self.w + x] && keep(x, y) {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let mut m = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (x0 + x, y0 + y);
                m[y * w + x] = self.m[sy * self.w + sx] && keep(sx, sy);
            }
        }
        Some(Region { x: self.x + x0, y: self.y + y0, w, h, m })
    }

    fn into_panel(self, page: &Page, pw: u32, ph: u32, pad: i64) -> Panel {
        let s = 1.0 / page.ws;
        let x0 = ((self.x as f32 * s) as i64 - pad).max(0);
        let y0 = ((self.y as f32 * s) as i64 - pad).max(0);
        let x1 = (((self.x + self.w) as f32 * s).ceil() as i64 + pad).min(pw as i64);
        let y1 = (((self.y + self.h) as f32 * s).ceil() as i64 + pad).min(ph as i64);
        let rect = Rect { x: x0 as u32, y: y0 as u32, w: (x1 - x0) as u32, h: (y1 - y0) as u32 };
        let fill = self.area() as f32 / (self.w * self.h) as f32;
        let shape = (fill < 0.97)
            .then(|| {
                let mask = Mask { w: self.w, h: self.h, data: self.m };
                Shape::from_mask(&mask, self.x as f32 * s, self.y as f32 * s, s, s)
            })
            .filter(|sh| !sh.cuts_bleed(&rect, pw, ph));
        Panel::new(rect, shape, vec![Issue::Refined])
    }
}

/// A straight gutter: `horizontal` cuts run left to right. Along the cut axis the
/// line sits at `offset + slope * (t - centre)`.
#[derive(Clone, Copy, Debug)]
struct Cut {
    horizontal: bool,
    slope: f32,
    offset: f32,
    score: f32,
}

fn split(page: &Page, r: Region, depth: u32, out: &mut Vec<Region>) {
    let total = r.area();
    let page_min = (0.03 * (page.w * page.h) as f32) as usize;
    if depth < MAX_DEPTH
        && let Some(cut) = [true, false]
            .into_iter()
            .filter_map(|h| find_cut(page, &r, h))
            .max_by(|a, b| a.score.total_cmp(&b.score))
    {
        let (cx, cy) = (r.w as f32 / 2.0, r.h as f32 / 2.0);
        let before = |x: usize, y: usize| {
            if cut.horizontal {
                (y as f32) < cut.offset + cut.slope * (x as f32 - cx)
            } else {
                (x as f32) < cut.offset + cut.slope * (y as f32 - cy)
            }
        };
        let a = r.sub(before);
        let b = r.sub(|x, y| !before(x, y));
        if let (Some(a), Some(b)) = (a, b) {
            let ok = |p: &Region| p.area() as f32 >= 0.12 * total as f32 && p.area() >= page_min;
            if ok(&a) && ok(&b) {
                split(page, a, depth + 1, out);
                split(page, b, depth + 1, out);
                return;
            }
        }
    }
    out.push(r);
}

/// Best gutter across the region in one orientation, if any.
fn find_cut(page: &Page, r: &Region, horizontal: bool) -> Option<Cut> {
    // Work in (t, u): t runs along the cut, u across it.
    let (len_t, len_u) = if horizontal { (r.w, r.h) } else { (r.h, r.w) };
    let at = |t: usize, u: usize| if horizontal { (t, u) } else { (u, t) };
    let max_gap = ((page.h.max(page.w) as f32 * 0.012) as usize).max(3);
    let margin = (len_u as f32 * 0.12) as usize;
    let centre = len_t as f32 / 2.0;

    let mut best: Option<Cut> = None;
    let steps = (MAX_SLOPE / SLOPE_STEP) as i32;
    let (mut n, mut paper, mut dark) = (vec![0u32; len_u], vec![0u32; len_u], vec![0u32; len_u]);
    for k in -steps..=steps {
        let slope = k as f32 * SLOPE_STEP;
        let shift: Vec<i64> = (0..len_t).map(|t| (slope * (t as f32 - centre)).round() as i64).collect();
        for c in 0..len_u {
            let (mut nn, mut pp, mut dd) = (0, 0, 0);
            for (t, &s) in shift.iter().enumerate() {
                let u = c as i64 + s;
                if u < 0 || u >= len_u as i64 {
                    continue;
                }
                let (x, y) = at(t, u as usize);
                if !r.m[y * r.w + x] {
                    continue;
                }
                let i = (r.y + y) * page.w + r.x + x;
                nn += 1;
                pp += page.paper[i] as u32;
                dd += page.dark[i] as u32;
            }
            n[c] = nn;
            paper[c] = pp;
            dark[c] = dd;
        }
        let full = |c: usize| n[c] as f32 >= 0.6 * len_t as f32;
        let dark_frac = |c: usize| if full(c) { dark[c] as f32 / n[c] as f32 } else { 0.0 };
        let paper_frac = |c: usize| if full(c) { paper[c] as f32 / n[c] as f32 } else { 1.0 };
        for c in margin.max(max_gap)..len_u.saturating_sub(margin.max(max_gap)) {
            if !full(c) {
                continue;
            }
            let p = paper[c] as f32 / n[c] as f32;
            if p < CROSSED_PAPER {
                // Or a single black frame line between the panels, with no white gap.
                if let Some(score) = black_line(c, len_u, &dark_frac, &paper_frac, max_gap) {
                    if best.is_none_or(|b| score > b.score) {
                        best = Some(Cut { horizontal, slope, offset: c as f32, score });
                    }
                }
                continue;
            }
            let above = (1..=max_gap).map(|d| dark_frac(c - d)).fold(0.0, f32::max);
            let below = (1..=max_gap).map(|d| dark_frac(c + d)).fold(0.0, f32::max);
            // A clean gutter needs some border on both sides; one crossed by art (a rifle, a
            // balloon) needs the two long, parallel frame lines that make a gutter a gutter.
            let clean = p >= 0.9 && above >= 0.45 && below >= 0.45;
            let crossed = above >= CROSSED_BORDER && below >= CROSSED_BORDER;
            if !clean && !crossed {
                continue;
            }
            let score = p + 0.5 * (above + below) / 2.0;
            if best.is_none_or(|b| score > b.score)
                && lined_ends(page, r, horizontal, &shift, c, max_gap)
                && !enclosed_cut(page, r, horizontal, &shift, c, max_gap)
                && !crosses_balloon(page, r, horizontal, &shift, c, max_gap)
            {
                best = Some(Cut { horizontal, slope, offset: c as f32, score });
            }
        }
    }
    best
}

/// Whether the line at `c` (shifted by `shift` along it) is closed off at both ends by a frame
/// line running straight across it: it lies inside one picture, whose side frames a real gutter
/// would break.
fn enclosed_cut(page: &Page, r: &Region, horizontal: bool, shift: &[i64], c: usize, max_gap: usize) -> bool {
    const NEAR: usize = 12;
    let (len_t, len_u) = if horizontal { (r.w, r.h) } else { (r.h, r.w) };
    let at = |t: usize, d: i64| {
        let u = c as i64 + shift[t] + d;
        (u >= 0 && u < len_u as i64).then(|| {
            let (x, y) = if horizontal { (t, u as usize) } else { (u as usize, t) };
            (r.m[y * r.w + x], page.dark[(r.y + y) * page.w + r.x + x] == 1)
        })
    };
    let inside = |t: usize| at(t, 0).is_some_and(|(m, _)| m);
    let g = max_gap as i64;
    // Dark all across the gutter's width (and a little past it) at `t`.
    let crossed = |t: usize| (-g..=g).all(|d| at(t, d).is_some_and(|(_, dark)| dark));
    let (Some(first), Some(last)) = ((0..len_t).find(|&t| inside(t)), (0..len_t).rfind(|&t| inside(t))) else {
        return false;
    };
    (first..(first + NEAR).min(len_t)).any(crossed) && (last.saturating_sub(NEAR)..=last).any(crossed)
}

/// Whether the gutter at line `c` (shifted by `shift` along it) has a frame line beside it at both
/// of its ends (on one side at least: a panel bleeding off the page has no frame there). A line
/// carried on from a gutter into a panel across its end (a tall panel beside two stacked ones)
/// has none there: the cut belongs along the tall panel's side first.
fn lined_ends(page: &Page, r: &Region, horizontal: bool, shift: &[i64], c: usize, max_gap: usize) -> bool {
    const END: f32 = 0.25;
    const LINED: f32 = 0.4;
    let (len_t, len_u) = if horizontal { (r.w, r.h) } else { (r.h, r.w) };
    let dark = |t: usize, d: i64| {
        let u = c as i64 + shift[t] + d;
        if u < 0 || u >= len_u as i64 {
            return false;
        }
        let (x, y) = if horizontal { (t, u as usize) } else { (u as usize, t) };
        r.m[y * r.w + x] && page.dark[(r.y + y) * page.w + r.x + x] == 1
    };
    let g = max_gap as i64;
    let inside = |t: usize| {
        let u = c as i64 + shift[t];
        u >= 0 && u < len_u as i64 && {
            let (x, y) = if horizontal { (t, u as usize) } else { (u as usize, t) };
            r.m[y * r.w + x]
        }
    };
    let ts: Vec<usize> = (0..len_t).filter(|&t| inside(t)).collect();
    let n = (ts.len() as f32 * END) as usize;
    if n < 4 {
        return true;
    }
    let lined = |part: &[usize]| {
        part.iter().filter(|&&t| (1..=g).any(|d| dark(t, -d)) || (1..=g).any(|d| dark(t, d))).count() as f32
            >= LINED * part.len() as f32
    };
    lined(&ts[..n]) && lined(&ts[ts.len() - n..])
}

/// Whether a balloon shared by the panels on both sides crosses the gutter at line `c` (shifted
/// by `shift` along it). Paper flooding out from the line stays in a gutter lined by borders; where
/// a balloon crosses, it spills into the balloon on both sides (around its lettering). Such a
/// stretch counts when the gutter runs on with borders beyond its ends, or beyond one of them when
/// it reaches the line's other end: near the end of a gutter, the page margin and pale art can lie
/// on both sides.
fn crosses_balloon(page: &Page, r: &Region, horizontal: bool, shift: &[i64], c: usize, max_gap: usize) -> bool {
    let (len_t, len_u) = if horizontal { (r.w, r.h) } else { (r.h, r.w) };
    let reach = 4 * max_gap as i64;
    let far = 2 * max_gap as i64;
    let span = (2 * reach + 1) as usize;
    // Paper at offset `d` across the line at `t`, inside the region.
    let paper = |t: usize, d: i64| {
        let u = c as i64 + shift[t] + d;
        if u < 0 || u >= len_u as i64 {
            return false;
        }
        let (x, y) = if horizontal { (t, u as usize) } else { (u as usize, t) };
        r.m[y * r.w + x] && page.paper[(r.y + y) * page.w + r.x + x] == 1
    };
    let mut seen = vec![false; len_t * span];
    let mut stack: Vec<(usize, i64)> = (0..len_t).filter(|&t| paper(t, 0)).map(|t| (t, 0)).collect();
    for &(t, d) in &stack {
        seen[t * span + (d + reach) as usize] = true;
    }
    let (mut before, mut after) = (vec![false; len_t], vec![false; len_t]);
    while let Some((t, d)) = stack.pop() {
        if d < -far {
            before[t] = true;
        } else if d > far {
            after[t] = true;
        }
        for (nt, nd) in [(t as i64 - 1, d), (t as i64 + 1, d), (t as i64, d - 1), (t as i64, d + 1)] {
            if nt < 0 || nt >= len_t as i64 || nd.abs() > reach {
                continue;
            }
            let (nt, k) = (nt as usize, nt as usize * span + (nd + reach) as usize);
            if !seen[k] && paper(nt, nd) {
                seen[k] = true;
                stack.push((nt, nd));
            }
        }
    }
    let mut open: Vec<bool> = (0..len_t).map(|t| before[t] && after[t]).collect();
    // Lettering touching the line breaks the stretch for a few pixels.
    let mut last = None;
    for t in 0..len_t {
        if open[t] {
            if let Some(l) = last
                && t - l <= 6
            {
                open[l..t].fill(true);
            }
            last = Some(t);
        }
    }
    let min_run = (0.03 * page.w.max(page.h) as f32) as usize;
    // Lined by borders on most of `range` (the page margin may lie past a gutter's end), for at
    // least `min_run` or up to the line's end.
    let closed = |range: std::ops::Range<usize>| {
        // (Open page margin at the line's very end is left out.)
        let mut range = range;
        if range.start == 0 {
            range.start = (0..range.end).find(|&t| !open[t]).unwrap_or(range.end);
        }
        if range.end == len_t {
            range.end = (range.start..len_t).rfind(|&t| !open[t]).map_or(range.start, |t| t + 1);
        }
        let ends = range.start <= min_run / 2 || range.end + min_run / 2 >= len_t;
        (range.len() >= min_run || (!range.is_empty() && ends))
            && range.clone().filter(|&t| !open[t]).count() * 10 >= range.len() * 8
    };
    let mut t = 0;
    while t < len_t {
        if !open[t] {
            t += 1;
            continue;
        }
        let start = t;
        while t < len_t && open[t] {
            t += 1;
        }
        // (A balloon may also hang over the gutter's end: the stretch then runs to the line's end.)
        let (lead, tail) = (closed(start.saturating_sub(min_run)..start), closed(t..(t + min_run).min(len_t)));
        let (at_start, at_end) = (start <= 3, t + 3 >= len_t);
        if t - start >= min_run && ((lead && (tail || at_end)) || (at_start && tail)) {
            return true;
        }
    }
    false
}

/// A gutter drawn as one thick black line: line `c` is the middle of a band of at most
/// `max_gap` lines that are each at least `BLACK_LINE` dark, with clearly lighter lines on both
/// sides. Neither side may be nearly all paper: that is the border of a caption box, with the
/// caption's blank interior on one side. Returns the cut's score.
fn black_line(
    c: usize,
    len: usize,
    dark: &impl Fn(usize) -> f32,
    paper: &impl Fn(usize) -> f32,
    max_gap: usize,
) -> Option<f32> {
    // Every line looked at (the band and max_gap lines past it) must exist.
    if c < 2 * max_gap + 2 || c + 2 * max_gap + 2 >= len {
        return None;
    }
    let d = dark(c);
    if d < BLACK_LINE {
        return None;
    }
    let (mut lo, mut hi) = (c, c);
    while lo > 0 && c - lo <= max_gap && dark(lo - 1) >= BLACK_LINE {
        lo -= 1;
    }
    while c.abs_diff(hi) <= max_gap && dark(hi + 1) >= BLACK_LINE {
        hi += 1;
    }
    // Only the band's middle line is a candidate, and the band must be thin.
    if hi - lo > max_gap || c != (lo + hi) / 2 || lo < max_gap {
        return None;
    }
    let side = |line: usize| paper(line) >= 0.8;
    if (2..=max_gap).any(|off| side(lo - off) || side(hi + off)) {
        return None;
    }
    let outside = |off: usize| dark(lo - off).max(dark(hi + off));
    let lighter = (2..=max_gap).map(outside).fold(1.0, f32::min);
    (lighter < 0.6).then_some(d + 0.5 * (1.0 - lighter))
}

fn gap(a: &Rect, b: &Rect) -> f32 {
    let dx = (b.x as i64 - a.right() as i64).max(a.x as i64 - b.right() as i64).max(0);
    let dy = (b.y as i64 - a.bottom() as i64).max(a.y as i64 - b.bottom() as i64).max(0);
    ((dx * dx + dy * dy) as f32).sqrt()
}

fn union(a: &Rect, b: &Rect) -> Rect {
    let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
    let (x1, y1) = (a.right().max(b.right()), a.bottom().max(b.bottom()));
    Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    /// Two framed panels separated by a slanted 6 px gutter, detected as one doubtful panel.
    #[test]
    fn splits_along_slanted_gutter() {
        let (w, h) = (600u32, 800u32);
        let mut g = GrayImage::from_pixel(w, h, Luma([240]));
        let gutter = |x: u32| 400.0 + 0.1 * (x as f32 - 300.0);
        for y in 40..h - 40 {
            for x in 40..w - 40 {
                let d = y as f32 - gutter(x);
                let v = if d.abs() < 3.0 {
                    240 // gutter paper
                } else if d.abs() < 7.0 || x < 44 || x >= w - 44 || y < 44 || y >= h - 44 {
                    20 // borders
                } else {
                    130 // art
                };
                g.put_pixel(x, y, Luma([v]));
            }
        }
        let img = DynamicImage::ImageLuma8(g);
        let merged = Panel::new(Rect { x: 38, y: 38, w: w - 76, h: h - 76 }, None, vec![Issue::InnerGutter]);
        let (panels, stats) = refine_page(&img, vec![merged], false, 0.0);
        assert_eq!(stats.split, 1, "{panels:?}");
        assert_eq!(panels.len(), 2);
        assert!(panels[0].rect.y < panels[1].rect.y, "top piece first");
        assert!(panels[0].shape.is_some(), "slanted cut keeps an exact outline");
    }

    #[test]
    fn keeps_single_panel_with_caption_line() {
        // A caption box's bottom border is a single line: no gutter, no cut.
        let (w, h) = (600u32, 800u32);
        let mut g = GrayImage::from_pixel(w, h, Luma([240]));
        for y in 40..h - 40 {
            for x in 40..w - 40 {
                let border = x < 44 || x >= w - 44 || y < 44 || y >= h - 44 || (200..204).contains(&y);
                g.put_pixel(x, y, Luma([if border { 20 } else if y < 200 { 235 } else { 130 }]));
            }
        }
        let img = DynamicImage::ImageLuma8(g);
        let p = Panel::new(Rect { x: 38, y: 38, w: w - 76, h: h - 76 }, None, vec![Issue::InnerGutter]);
        let (panels, stats) = refine_page(&img, vec![p], false, 0.0);
        assert_eq!((panels.len(), stats.split), (1, 0));
    }

    // Regression: panels separated by one thick black line and no white
    // gutter are split along it.
    #[test]
    #[ignore = "black-line splits are off (BLACK_LINE > 1)"]
    fn splits_along_black_frame_line() {
        let (w, h) = (600u32, 800u32);
        let mut g = GrayImage::from_pixel(w, h, Luma([240]));
        for y in 40..h - 40 {
            for x in 40..w - 40 {
                let line = (396..410).contains(&y);
                let border = x < 46 || x >= w - 46 || y < 46 || y >= h - 46;
                // Art: busy mid tones, never mostly paper.
                let art = 90 + ((x * 7 + y * 3) % 60) as u8;
                g.put_pixel(x, y, Luma([if line || border { 10 } else { art }]));
            }
        }
        let img = DynamicImage::ImageLuma8(g);
        let merged = Panel::new(Rect { x: 38, y: 38, w: w - 76, h: h - 76 }, None, vec![Issue::Large]);
        let (panels, stats) = refine_page(&img, vec![merged], false, 0.0);
        assert_eq!((panels.len(), stats.split), (2, 1), "{panels:?}");
        assert!(panels[0].rect.bottom() <= 420 && panels[1].rect.y >= 390, "{panels:?}");
    }
}
