//! Panel detection.
//!
//! 1. Binarise the page against the paper colour (median of the page edge).
//! 2. Flood-fill paper inward from the page edge. Paper reached this way, plus any
//!    thin elongated paper strips, is the gutter network; everything else is a
//!    "solid" panel blob with its interior filled.
//! 3. Erode the solid mask slightly so thin bridges break, and take connected
//!    components as panel candidates.
//! 4. A candidate can still hold several panels joined by a speech balloon that
//!    crosses a gutter. Because panel interiors are filled, a line through a panel
//!    is almost fully covered while a gutter line is covered only where a balloon
//!    crosses it, so each candidate is split recursively along low-coverage lines.
//! 5. Panels are put in reading order with an XY-cut over the boxes: rows top to
//!    bottom, then left to right (right to left for manga).
//!
//! Each panel keeps its blob as a [`Shape`] so non-rectangular panels can be cut
//! out exactly instead of as a bounding box.

use image::{DynamicImage, GrayImage, imageops::FilterType};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }
    pub fn bottom(&self) -> u32 {
        self.y + self.h
    }
    pub fn right(&self) -> u32 {
        self.x + self.w
    }
    pub fn overlap_area(&self, o: &Rect) -> u64 {
        let ix = self.right().min(o.right()).saturating_sub(self.x.max(o.x));
        let iy = self.bottom().min(o.bottom()).saturating_sub(self.y.max(o.y));
        ix as u64 * iy as u64
    }
}

#[derive(Clone, Debug)]
pub struct DetectOptions {
    /// Luma difference from the background that counts as ink.
    pub threshold: u8,
    /// A line through a panel blob counts as a gutter when at most this fraction of it is covered.
    pub split_coverage: f32,
    /// Panels smaller than this fraction of the page area are dropped (page numbers, specks).
    pub min_panel_area: f32,
    /// Padding added around each panel, as a fraction of the shorter page side.
    pub padding: f32,
    /// Keep the outline of non-rectangular panels so they can be cut out exactly.
    pub exact_shapes: bool,
    pub rtl: bool,
}

/// Longest side used for analysis; detection runs on a downscaled copy.
const DETECT_MAX: u32 = 1200;
/// Erosion radius (analysis pixels) used to break thin bridges between panels.
const ERODE: usize = 3;
const MAX_DEPTH: u32 = 12;

/// A panel together with its exact outline when it isn't a plain rectangle.
#[derive(Clone, Debug)]
pub struct Panel {
    pub rect: Rect,
    pub shape: Option<Shape>,
    /// How much the detection can be trusted, 0..=1 (see [`Issue`]).
    pub confidence: f32,
    pub issues: Vec<Issue>,
}

/// Panels scoring below this are re-examined in a second round.
pub const LOW_CONFIDENCE: f32 = 0.6;

/// Why a panel's detection is doubtful. Each issue multiplies the confidence by its factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Issue {
    /// The blob has notches: often two panels joined by a balloon.
    Notched,
    /// Cut out of a bigger blob along a balloon-crossed gutter.
    Split,
    /// A blank, gutter-like line runs across it: probably several panels.
    InnerGutter,
    /// Little of its outline is a drawn border: a fragment, a title or borderless art.
    NoBorder,
    /// Tiny or sliver-shaped for a panel.
    Small,
    /// Bigger than the book's typical panel on a page with other panels; ruled out
    /// when the second round finds no gutter inside.
    Large,
    /// Far bigger than the book's typical panel on a page with other panels. Stays
    /// doubtful even without a gutter: panels can also be merged without one.
    Oversized,
    /// Produced or re-checked by the second round (split along a gutter, merged,
    /// or searched for a gutter at full resolution and none found).
    Refined,
    /// Grown by, or made of, ink outside the detected panels (a caption, a title, a
    /// borderless panel). The second round leaves it as it is.
    Joined,
}

impl Issue {
    pub fn factor(self) -> f32 {
        match self {
            Issue::Notched => 0.6,
            Issue::Split => 0.85,
            Issue::InnerGutter => 0.4,
            Issue::NoBorder => 0.75,
            Issue::Small => 0.55,
            Issue::Large => 0.55,
            Issue::Oversized => 0.55,
            Issue::Refined => 1.0,
            Issue::Joined => 1.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Issue::Notched => "notched outline",
            Issue::Split => "split at a balloon",
            Issue::InnerGutter => "gutter-like line inside",
            Issue::NoBorder => "little border",
            Issue::Small => "tiny or sliver",
            Issue::Large => "bigger than usual",
            Issue::Oversized => "much bigger than usual",
            Issue::Refined => "re-checked in second round",
            Issue::Joined => "joined with nearby text",
        }
    }
}

impl Panel {
    pub fn new(rect: Rect, shape: Option<Shape>, issues: Vec<Issue>) -> Panel {
        let confidence = issues.iter().map(|i| i.factor()).product();
        Panel { rect, shape, confidence, issues }
    }

    pub fn add_issue(&mut self, issue: Issue) {
        if !self.issues.contains(&issue) {
            self.issues.push(issue);
            self.confidence *= issue.factor();
        }
    }

    /// Drop an issue that a stronger check ruled out.
    pub fn clear_issue(&mut self, issue: Issue) {
        self.issues.retain(|&i| i != issue);
        self.confidence = self.issues.iter().map(|i| i.factor()).product();
    }

    pub fn is_low(&self) -> bool {
        self.confidence < LOW_CONFIDENCE
    }
}

/// Panel outline: a bit mask at analysis resolution placed in page coordinates.
#[derive(Clone, Debug)]
pub struct Shape {
    /// Page coordinates of the mask's top-left corner.
    ox: f32,
    oy: f32,
    /// Page pixels per mask pixel.
    sx: f32,
    sy: f32,
    w: usize,
    h: usize,
    bits: Vec<u64>,
}

impl Shape {
    pub(crate) fn from_mask(m: &Mask, ox: f32, oy: f32, sx: f32, sy: f32) -> Shape {
        let mut bits = vec![0u64; (m.w * m.h).div_ceil(64)];
        for (i, _) in m.data.iter().enumerate().filter(|(_, b)| **b) {
            bits[i / 64] |= 1 << (i % 64);
        }
        Shape { ox, oy, sx, sy, w: m.w, h: m.h, bits }
    }

    fn get(&self, x: isize, y: isize) -> bool {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return false;
        }
        let i = y as usize * self.w + x as usize;
        self.bits[i / 64] >> (i % 64) & 1 == 1
    }

    /// How much of the page pixel at (`px`, `py`) belongs to the panel, 0..=1.
    /// Bilinear over the mask, then steepened so edges stay crisp but anti-aliased.
    pub fn alpha(&self, px: f32, py: f32) -> f32 {
        let mx = (px - self.ox) / self.sx - 0.5;
        let my = (py - self.oy) / self.sy - 0.5;
        let (x0, y0) = (mx.floor(), my.floor());
        let (tx, ty) = (mx - x0, my - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let v = |x, y| self.get(x, y) as u8 as f32;
        let top = v(x0, y0) * (1.0 - tx) + v(x0 + 1, y0) * tx;
        let bot = v(x0, y0 + 1) * (1.0 - tx) + v(x0 + 1, y0 + 1) * tx;
        let a = top * (1.0 - ty) + bot * ty;
        ((a - 0.5) * 2.0 + 0.5).clamp(0.0, 1.0)
    }

    /// Mask pixels on the outline, as page-coordinate rectangles.
    fn outline(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        (0..self.h as isize).flat_map(move |y| {
            (0..self.w as isize).filter_map(move |x| {
                let edge = self.get(x, y)
                    && !(self.get(x - 1, y) && self.get(x + 1, y) && self.get(x, y - 1) && self.get(x, y + 1));
                edge.then(|| (self.ox + x as f32 * self.sx, self.oy + y as f32 * self.sy))
            })
        })
    }
}

/// A shape that fills this much of its bounding box is treated as a rectangle.
const RECT_FILL: f32 = 0.97;

#[derive(Clone)]
pub(crate) struct Mask {
    pub(crate) w: usize,
    pub(crate) h: usize,
    pub(crate) data: Vec<bool>,
}

impl Mask {
    pub(crate) fn new(w: usize, h: usize) -> Self {
        Mask { w, h, data: vec![false; w * h] }
    }

    pub(crate) fn at(&self, x: usize, y: usize) -> bool {
        self.data[y * self.w + x]
    }

    fn row_profile(&self, r: Rect) -> Vec<usize> {
        (r.y..r.bottom())
            .map(|y| {
                let row = &self.data[y as usize * self.w..];
                row[r.x as usize..r.right() as usize].iter().filter(|&&b| b).count()
            })
            .collect()
    }

    fn col_profile(&self, r: Rect) -> Vec<usize> {
        let mut cols = vec![0usize; r.w as usize];
        for y in r.y..r.bottom() {
            for (i, c) in cols.iter_mut().enumerate() {
                if self.at(r.x as usize + i, y as usize) {
                    *c += 1;
                }
            }
        }
        cols
    }

    /// Replace the mask with its convex hull. Panels are almost always convex, and the
    /// hull also closes white balloons that break through the panel border into the
    /// gutter, which would otherwise read as holes. L-shaped panels get their notch filled.
    pub(crate) fn convex_fill(&self) -> Mask {
        // Pixel-corner points of each row's extent are enough to build the hull.
        let mut pts: Vec<(i64, i64)> = Vec::new();
        for y in 0..self.h {
            let line = &self.data[y * self.w..(y + 1) * self.w];
            if let (Some(a), Some(b)) = (line.iter().position(|&v| v), line.iter().rposition(|&v| v)) {
                let (y, a, b) = (y as i64, a as i64, b as i64 + 1);
                pts.extend([(a, y), (b, y), (a, y + 1), (b, y + 1)]);
            }
        }
        let hull = convex_hull(pts);
        let mut out = Mask::new(self.w, self.h);
        if hull.len() < 3 {
            return self.clone();
        }
        // Scanline fill at pixel centres.
        for y in 0..self.h {
            let cy = y as f64 + 0.5;
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for i in 0..hull.len() {
                let (x0, y0) = (hull[i].0 as f64, hull[i].1 as f64);
                let (x1, y1) = (hull[(i + 1) % hull.len()].0 as f64, hull[(i + 1) % hull.len()].1 as f64);
                if (y0 <= cy && cy <= y1) || (y1 <= cy && cy <= y0) {
                    let x = if y1 == y0 { x0.min(x1) } else { x0 + (cy - y0) * (x1 - x0) / (y1 - y0) };
                    let x_hi = if y1 == y0 { x0.max(x1) } else { x };
                    lo = lo.min(x);
                    hi = hi.max(x_hi);
                }
            }
            if lo < hi {
                let a = (lo - 0.5).ceil().max(0.0) as usize;
                let b = ((hi - 0.5).floor() as i64).min(self.w as i64 - 1);
                if b >= a as i64 {
                    out.data[y * self.w + a..=y * self.w + b as usize].fill(true);
                }
            }
        }
        out
    }

    /// Square-kernel dilation (`erode = false`) or erosion (`erode = true`) of radius `r`.
    /// Separable, using running counts. Outside the mask counts as set when eroding.
    fn morph(&self, r: usize, erode: bool) -> Mask {
        self.morph_xy(r, r, erode)
    }

    /// `morph` with separate horizontal and vertical radii.
    fn morph_xy(&self, rx: usize, ry: usize, erode: bool) -> Mask {
        let pass = |src: &Mask, horizontal: bool| -> Mask {
            let r = if horizontal { rx } else { ry };
            let (len, lines) = if horizontal { (src.w, src.h) } else { (src.h, src.w) };
            let idx = |line: usize, i: usize| if horizontal { line * src.w + i } else { i * src.w + line };
            let mut out = Mask::new(src.w, src.h);
            let mut prefix = vec![0usize; len + 1];
            for line in 0..lines {
                for i in 0..len {
                    prefix[i + 1] = prefix[i] + src.data[idx(line, i)] as usize;
                }
                for i in 0..len {
                    let (a, b) = (i.saturating_sub(r), (i + r + 1).min(len));
                    let n = prefix[b] - prefix[a];
                    out.data[idx(line, i)] = if erode { n == b - a } else { n > 0 };
                }
            }
            out
        };
        pass(&pass(self, true), false)
    }
}

/// Share of the page's ink outside all panels above which the page is kept whole.
const UNCOVERED_INK: f32 = 0.3;

pub fn detect_panels(img: &DynamicImage, o: &DetectOptions) -> Vec<Panel> {
    let (ow, oh) = (img.width(), img.height());
    let full = Panel::new(Rect { x: 0, y: 0, w: ow, h: oh }, None, Vec::new());
    if ow < 32 || oh < 32 {
        return vec![full];
    }

    let mut gray = img.to_luma8();
    let longest = ow.max(oh);
    if longest > DETECT_MAX {
        let s = DETECT_MAX as f32 / longest as f32;
        let nw = ((ow as f32 * s).round() as u32).max(1);
        let nh = ((oh as f32 * s).round() as u32).max(1);
        gray = image::imageops::resize(&gray, nw, nh, FilterType::Triangle);
    }
    let (dw, dh) = gray.dimensions();
    let (w, h) = (dw as usize, dh as usize);
    let fx = ow as f32 / dw as f32;
    let fy = oh as f32 / dh as f32;

    let bg = background_luma(&gray);
    let thr = o.threshold as i16;
    let ink = Mask {
        w,
        h,
        data: gray.pixels().map(|p| (p[0] as i16 - bg as i16).abs() > thr).collect(),
    }
    .morph(1, false);

    // Strictly paper-coloured pixels (pale art like sky is not paper).
    let paper = Mask { w, h, data: gray.pixels().map(|p| (p[0] as i16 - bg as i16).abs() <= 20).collect() };

    let solid = solid_mask(&ink);
    let core = solid.morph(ERODE, true);
    let (labels, boxes) = label(&core);

    let mut panels = Vec::new();
    let page_area = (w * h) as f32;
    for (k, b) in boxes.iter().enumerate() {
        // Grow back what the erosion removed.
        let x0 = (b.x as usize).saturating_sub(ERODE);
        let y0 = (b.y as usize).saturating_sub(ERODE);
        let x1 = (b.right() as usize + ERODE).min(w);
        let y1 = (b.bottom() as usize + ERODE).min(h);
        if ((x1 - x0) * (y1 - y0)) as f32 <= page_area * 0.01 {
            continue;
        }
        // This component only, restored to full size and clipped to the solid mask.
        let mut own = Mask::new(x1 - x0, y1 - y0);
        for y in y0..y1 {
            for x in x0..x1 {
                own.data[(y - y0) * own.w + (x - x0)] = labels[y * w + x] == k as u32 + 1;
            }
        }
        let mut own = own.morph(ERODE, false);
        for y in y0..y1 {
            for x in x0..x1 {
                own.data[(y - y0) * own.w + (x - x0)] &= solid.at(x, y);
            }
        }
        let mut parts = Vec::new();
        let local = Rect { x: 0, y: 0, w: own.w as u32, h: own.h as u32 };
        let min_seg = ((w as f32 * 0.08) as usize, (h as f32 * 0.06) as usize);
        split(&own, local, o.split_coverage, min_seg, 0, &mut parts);
        let was_split = parts.len() > 1;
        for r in parts {
            // The piece's own pixels, grown by one pixel so the panel border line survives.
            let mut m = Mask::new(r.w as usize, r.h as usize);
            for y in 0..m.h {
                for x in 0..m.w {
                    m.data[y * m.w + x] = own.at(r.x as usize + x, r.y as usize + y);
                }
            }
            let raw = m.morph(1, false);
            let raw_count = raw.data.iter().filter(|&&b| b).count();
            let m = raw.convex_fill();
            panels.push(Cand { r: Rect { x: r.x + x0 as u32, y: r.y + y0 as u32, ..r }, m, raw_count, was_split, joined: false });
        }
    }

    let min_area = (page_area * o.min_panel_area) as u64;
    panels.retain(|c| c.r.area() >= min_area && c.r.w as f32 >= 0.03 * w as f32 && c.r.h as f32 >= 0.03 * h as f32);
    // Drop boxes that sit mostly inside a bigger one (insets, stray balloons).
    let all: Vec<Rect> = panels.iter().map(|c| c.r).collect();
    panels.retain(|c| !all.iter().any(|b| b.area() > c.r.area() && c.r.overlap_area(b) * 10 >= c.r.area() * 8));
    // Pages of unframed pictures (gag cartoons, illustrated text, ads): cutting out the
    // drawings splits figures and their captions apart. Keep the whole page instead.
    let frameless = panels.iter().filter(|c| score(c, &ink, &paper, w, h).contains(&Issue::NoBorder)).count();
    if panels.len() >= 2 && frameless * 2 >= panels.len() {
        return vec![full];
    }
    // Pages without a panel layout (ads, a logo over a collage, pin-ups): when most candidates
    // have no drawn frame, they are pieces of one picture, not panels. Keep the page whole.
    let frameless = panels.iter().filter(|c| score(c, &ink, &paper, w, h).contains(&Issue::NoBorder)).count();
    if panels.len() >= 2 && frameless * 2 > panels.len() {
        return vec![full];
    }
    // Text pages (prose stories, editorials, ads): when much of the ink lies outside every
    // panel, cutting out the panels would drop the text. Keep the whole page instead.
    let total_ink = ink.data.iter().filter(|&&b| b).count();
    let outside = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            ink.at(x, y)
                && !panels.iter().any(|c| {
                    let (x, y) = (x as u32, y as u32);
                    x >= c.r.x && y >= c.r.y && x < c.r.right() && y < c.r.bottom()
                })
        })
        .count();
    if total_ink > 0 && outside as f32 > UNCOVERED_INK * total_ink as f32 {
        return vec![full];
    }
    attach_stray_ink(&mut panels, &ink);
    // A box grown over the pieces of a borderless panel (figures found on their own) takes them in.
    merge_overlapping(&mut panels);
    let panels = reading_order(panels, |c| c.r, o.rtl);

    let pad = (ow.min(oh) as f32 * o.padding).round() as i64;
    let panels: Vec<Panel> = panels
        .iter()
        .map(|c| {
            let (r, m) = (&c.r, &c.m);
            let x0 = ((r.x as f32 * fx).floor() as i64 - pad).max(0);
            let y0 = ((r.y as f32 * fy).floor() as i64 - pad).max(0);
            let x1 = ((r.right() as f32 * fx).ceil() as i64 + pad).min(ow as i64);
            let y1 = ((r.bottom() as f32 * fy).ceil() as i64 + pad).min(oh as i64);
            let rect = Rect { x: x0 as u32, y: y0 as u32, w: (x1 - x0) as u32, h: (y1 - y0) as u32 };
            let fill = m.data.iter().filter(|&&b| b).count() as f32 / m.data.len().max(1) as f32;
            let shape = (o.exact_shapes && fill < RECT_FILL)
                .then(|| Shape::from_mask(m, r.x as f32 * fx, r.y as f32 * fy, fx, fy));
            Panel::new(rect, shape, score(c, &ink, &paper, w, h))
        })
        .collect();

    if panels.is_empty() { vec![full] } else { panels }
}

/// A panel candidate at analysis resolution.
#[derive(Clone)]
struct Cand {
    r: Rect,
    /// Convex outline, local to `r`.
    m: Mask,
    /// Pixels of the blob itself before the convex fill.
    raw_count: usize,
    was_split: bool,
    /// Grown by or made of stray ink (see [`attach_stray_ink`]).
    joined: bool,
}

/// A box grown over stray ink that now overlaps another box by over half of the smaller one
/// covers one picture with it (the figures of a borderless panel), as in Kumiko's panel merging:
/// replace them by their union. Framed panels are never merged with each other: the boxes of
/// slanted panels overlap a lot.
fn merge_overlapping(panels: &mut Vec<Cand>) {
    loop {
        let pair = (0..panels.len()).flat_map(|i| (i + 1..panels.len()).map(move |j| (i, j))).find(|&(i, j)| {
            let (a, b) = (&panels[i].r, &panels[j].r);
            (panels[i].joined || panels[j].joined) && a.overlap_area(b) * 2 > a.area().min(b.area())
        });
        let Some((i, j)) = pair else { return };
        let (a, b) = (panels[i].r, panels[j].r);
        let (x, y) = (a.x.min(b.x), a.y.min(b.y));
        let r = Rect { x, y, w: a.right().max(b.right()) - x, h: a.bottom().max(b.bottom()) - y };
        let mut m = Mask::new(r.w as usize, r.h as usize);
        m.data.fill(true);
        let was_split = panels[i].was_split || panels[j].was_split;
        panels.remove(j);
        panels[i] = Cand { r, raw_count: m.data.len(), m, was_split, joined: true };
    }
}

/// Ink outside every panel (a caption under a gag cartoon, the caption or art of a borderless
/// panel, a title) is given to a panel by partitioning the page into cells, as in Kumiko's panel
/// expansion: each panel's box grows in every direction to the middle of the gap to its nearest
/// neighbour that way, or to the page edge. Ink is grouped into blocks (letters into words, close
/// lines into paragraphs), and each block joins the panel whose cell holds its centre. A block
/// that would make that panel overlap another one (a title over two columns) becomes a panel of
/// its own. Running heads and page numbers, thin lines at the very top or bottom, are left out.
fn attach_stray_ink(panels: &mut Vec<Cand>, ink: &Mask) {
    let (w, h) = (ink.w, ink.h);
    if panels.is_empty() {
        return;
    }
    // Inside a panel's outline, not just its box: ink in the corner a slanted outline cuts off
    // (a caption beside a diagonal border) would otherwise be painted white.
    let inside = |c: &Cand, x: usize, y: usize| {
        let (xu, yu) = (x as u32, y as u32);
        xu >= c.r.x
            && yu >= c.r.y
            && xu < c.r.right()
            && yu < c.r.bottom()
            && c.m.at(x - c.r.x as usize, y - c.r.y as usize)
    };
    // The band along the image border is left out, so scan shadow there does not join up
    // with a caption near the edge.
    let band = (w.max(h) / 100).max(2);
    let mut stray = Mask::new(w, h);
    for y in band..h - band {
        for x in band..w - band {
            stray.data[y * w + x] = ink.at(x, y) && !panels.iter().any(|c| inside(c, x, y));
        }
    }
    // Letters into words and lines into blocks; each block's box is that of its own ink.
    // Vertically only close lines merge: a running head sits further from the caption below it.
    let reach_px = (w.max(h) / 80).max(3);
    let (labels, merged) = label(&stray.morph_xy(reach_px, (reach_px / 6).max(1), false));
    let mut groups: Vec<Option<Rect>> = vec![None; merged.len()];
    for y in 0..h {
        for x in 0..w {
            if !stray.at(x, y) {
                continue;
            }
            let g = &mut groups[labels[y * w + x] as usize - 1];
            let (x, y) = (x as u32, y as u32);
            *g = Some(match *g {
                None => Rect { x, y, w: 1, h: 1 },
                Some(r) => {
                    let (x0, y0) = (r.x.min(x), r.y.min(y));
                    Rect { x: x0, y: y0, w: r.right().max(x + 1) - x0, h: r.bottom().max(y + 1) - y0 }
                }
            });
        }
    }
    let detected: Vec<Rect> = panels.iter().map(|c| c.r).collect();
    let cells = cells(&detected, w as u32, h as u32);
    // Each group's ink per cell: how much of it falls in the cell, and the box of that part.
    let n = panels.len();
    let mut share: Vec<(usize, Option<Rect>)> = vec![(0, None); groups.len() * n];
    for y in 0..h {
        for x in 0..w {
            if !stray.at(x, y) {
                continue;
            }
            let k = labels[y * w + x] as usize - 1;
            let (x, y) = (x as u32, y as u32);
            for (i, c) in cells.iter().enumerate() {
                if x >= c.x && y >= c.y && x < c.right() && y < c.bottom() {
                    let (count, b) = &mut share[k * n + i];
                    *count += 1;
                    *b = Some(match *b {
                        None => Rect { x, y, w: 1, h: 1 },
                        Some(r) => {
                            let (x0, y0) = (r.x.min(x), r.y.min(y));
                            Rect { x: x0, y: y0, w: r.right().max(x + 1) - x0, h: r.bottom().max(y + 1) - y0 }
                        }
                    });
                }
            }
        }
    }
    let margin = (w.max(h) / 100) as u32;
    let union = |a: &Rect, b: &Rect| {
        let (x, y) = (a.x.min(b.x), a.y.min(b.y));
        Rect { x, y, w: a.right().max(b.right()) - x, h: a.bottom().max(b.bottom()) - y }
    };

    let mut grown: Vec<Option<Rect>> = vec![None; n];
    let mut own: Vec<Rect> = Vec::new();
    for (k, g) in groups.iter().enumerate() {
        let Some(g) = *g else { continue };
        // Specks, and thin ink along a box (its border line).
        // Stray ink touching the image border is never a caption: it is scan shadow, the stack of
        // page edges or the table beside a photographed book (art reaching the border is inside a
        // panel already).
        let at_border = g.x <= margin || g.y <= margin || g.right() + margin >= w as u32 || g.bottom() + margin >= h as u32;
        // The stack of page edges beside or below a photographed book lies a little inside the
        // image border: a tall thin column near a side, or a page-wide band near the top or bottom.
        let (fw, fh) = (w as f32, h as f32);
        let edge_column = (g.w as f32) < 0.16 * fw
            && g.h > 3 * g.w
            && ((g.x as f32) < 0.1 * fw || g.right() as f32 > 0.9 * fw);
        let edge_band = (g.w as f32) > 0.8 * fw
            && (g.h as f32) < 0.12 * fh
            && ((g.y as f32) < 0.06 * fh || g.bottom() as f32 > 0.94 * fh);
        let at_border = at_border || edge_column || edge_band;
        // (A border line is thinner than a line of lettering.)
        let hugs = (g.w <= margin || g.h <= margin)
            && detected.iter().any(|r| {
                g.x + margin >= r.x && g.y + margin >= r.y && g.right() <= r.right() + margin && g.bottom() <= r.bottom() + margin
            });
        // A running head lies above every panel or is a short line centred on the page; a page
        // number is a short line below every panel. A caption under the last row is longer.
        let thin_line = (g.h as usize) < h / 25;
        let at_end = (g.y as usize) < h * 8 / 100 || g.bottom() as usize > h - h * 8 / 100;
        // (A panel's top edge may sit a little higher than the head's baseline.)
        let above_all = detected.iter().all(|r| g.bottom() <= r.y + r.h / 20);
        let below_all = detected.iter().all(|r| g.y + r.h / 20 >= r.bottom()) && (g.w as usize) < w * 8 / 100;
        let centred = (g.x as i64 + g.w as i64 / 2 - w as i64 / 2).unsigned_abs() < w as u64 / 10
            && (g.w as usize) < w * 4 / 10;
        let head = thin_line && at_end && (above_all || below_all || centred);
        if g.area() < (w * h) as u64 / 2000 || at_border || hugs || head {
            continue;
        }
        // The part in the cell holding most of the group's ink joins that panel, when that is
        // nearly all of it. A group spread over several cells (a title over two columns) is a
        // panel of its own, and so is one whose part would make its panel overlap another.
        let ink_count = stray_count(&stray, &labels, k, &g);
        // Cells holding nearly all of it, nearest panel first (cells overlap where panels are
        // staggered); the first one it can join without its panel covering another wins.
        let gap = |r: &Rect| {
            let dx = (g.x as i64 - r.right() as i64).max(r.x as i64 - g.right() as i64).max(0) as f32;
            let dy = (g.y as i64 - r.bottom() as i64).max(r.y as i64 - g.bottom() as i64).max(0) as f32;
            dx.hypot(dy)
        };
        let mut owners: Vec<usize> = (0..n).filter(|&i| share[k * n + i].0 * 10 >= ink_count * 8).collect();
        owners.sort_by(|&a, &b| gap(&detected[a]).total_cmp(&gap(&detected[b])));
        // A caption on a panel's frame, split between two cells: the panel it touches, then the
        // cell holding most of it.
        // (A few lines of lettering at most: a taller block is a borderless panel.)
        let on_frame = owners.is_empty() && (g.h as usize) < h / 12;
        if on_frame {
            let mut touching: Vec<usize> = (0..n).filter(|&i| gap(&detected[i]) <= margin as f32).collect();
            touching.sort_by_key(|&i| std::cmp::Reverse(share[k * n + i].0));
            owners = touching;
        }
        let mut placed = false;
        for i in owners {
            let Some(part) = share[k * n + i].1 else { continue };
            let base = grown[i].unwrap_or(detected[i]);
            // On the frame, the panel only grows up or down to the caption, keeping its width.
            // (All of the caption's lines: the top one may lie in the cell above.)
            let part = if on_frame { Rect { x: base.x, y: g.y, w: base.w, h: g.h } } else { part };
            let u = union(&base, &part);
            // Only overlap the growth adds counts: staggered panels can overlap from the start.
            let covers = detected.iter().enumerate().any(|(j, r)| {
                j != i && (r.overlap_area(&u) - r.overlap_area(&detected[i])) * 10 > r.area()
            });
            if !covers {
                grown[i] = Some(u);
                placed = true;
                break;
            }
        }
        // Too small to show on its own (a scrap of art cut off by a split): it joins the nearest
        // panel, which may then overlap a neighbour a little.
        if !placed && (g.area() as f32) < 0.02 * (w * h) as f32 {
            if let Some(i) = (0..n).min_by(|&a, &b| gap(&detected[a]).total_cmp(&gap(&detected[b]))) {
                grown[i] = Some(union(&grown[i].unwrap_or(detected[i]), &g));
                placed = true;
            }
        }
        // A block of its own must stand apart from the panels (a title between rows); one that
        // overlaps them is a mix of their edges and margins.
        // Boxes mostly inside it are the pieces of a borderless panel (figures found on their
        // own) and are taken in; the rest may barely overlap it.
        let inside = |r: &Rect| r.overlap_area(&g) * 10 >= r.area() * 8;
        let overlap: u64 = detected.iter().filter(|r| !inside(r)).map(|r| r.overlap_area(&g)).sum();
        if !placed && overlap * 4 < g.area() {
            own.push(g);
        }
    }
    for (c, r) in panels.iter_mut().zip(grown) {
        if let Some(r) = r {
            // The grown box is cropped as a rectangle: its old outline no longer covers it.
            let mut m = Mask::new(r.w as usize, r.h as usize);
            m.data.fill(true);
            *c = Cand { r, raw_count: m.data.len(), m, was_split: c.was_split, joined: true };
        }
    }
    for r in own {
        let r = grow_over_ink(r, ink);
        panels.retain(|c| c.r.overlap_area(&r) * 10 < c.r.area() * 8);
        let mut m = Mask::new(r.w as usize, r.h as usize);
        m.data.fill(true);
        panels.push(Cand { r, raw_count: m.data.len(), m, was_split: false, joined: true });
    }
}

/// Ink pixels of group `k` (labels are 1-based) inside its box `g`.
fn stray_count(stray: &Mask, labels: &[u32], k: usize, g: &Rect) -> usize {
    (g.y as usize..g.bottom() as usize)
        .map(|y| {
            (g.x as usize..g.right() as usize).filter(|&x| stray.at(x, y) && labels[y * stray.w + x] as usize == k + 1).count()
        })
        .sum()
}

/// Grow a text block's box sideways while the strip just outside it still holds ink (letters
/// that a neighbour's outline reached over), until a blank gap, by at most a quarter of its width.
fn grow_over_ink(mut r: Rect, ink: &Mask) -> Rect {
    let step = (ink.w.max(ink.h) / 200).max(1) as u32;
    let limit = r.w / 4;
    let (y0, y1) = (r.y, r.bottom());
    let has_ink = |x0: u32, x1: u32| (y0..y1).any(|y| (x0..x1).any(|x| ink.at(x as usize, y as usize)));
    let mut grown = 0;
    while grown < limit && r.right() + step <= ink.w as u32 && has_ink(r.right(), r.right() + step) {
        r.w += step;
        grown += step;
    }
    let mut grown = 0;
    while grown < limit && r.x >= step && has_ink(r.x - step, r.x) {
        r.x -= step;
        r.w += step;
        grown += step;
    }
    r
}

/// Each box grown in every direction to the middle of the gap to its nearest neighbour that way
/// (one overlapping it across that direction), or to the page edge when there is none.
fn cells(boxes: &[Rect], w: u32, h: u32) -> Vec<Rect> {
    let spans = |a0: u32, a1: u32, b0: u32, b1: u32| a0.max(b0) < a1.min(b1);
    boxes
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let others = || boxes.iter().enumerate().filter(move |&(j, _)| j != i).map(|(_, o)| o);
            let left = others()
                .filter(|o| spans(r.y, r.bottom(), o.y, o.bottom()) && o.right() <= r.x + r.w / 10)
                .map(|o| (o.right().min(r.x) + r.x) / 2)
                .max()
                .unwrap_or(0);
            let right = others()
                .filter(|o| spans(r.y, r.bottom(), o.y, o.bottom()) && o.x + r.w / 10 >= r.right())
                .map(|o| (o.x.max(r.right()) + r.right()) / 2)
                .min()
                .unwrap_or(w);
            let top = others()
                .filter(|o| spans(r.x, r.right(), o.x, o.right()) && o.bottom() <= r.y + r.h / 10)
                .map(|o| (o.bottom().min(r.y) + r.y) / 2)
                .max()
                .unwrap_or(0);
            let bottom = others()
                .filter(|o| spans(r.x, r.right(), o.x, o.right()) && o.y + r.h / 10 >= r.bottom())
                .map(|o| (o.y.max(r.bottom()) + r.bottom()) / 2)
                .min()
                .unwrap_or(h);
            let (x0, y0) = (left.min(r.x), top.min(r.y));
            Rect { x: x0, y: y0, w: right.max(r.right()) - x0, h: bottom.max(r.bottom()) - y0 }
        })
        .collect()
}

/// Page-local reasons to doubt a candidate. Book-level ones ([`Issue::Oversized`]) are added later.
fn score(c: &Cand, ink: &Mask, paper: &Mask, page_w: usize, page_h: usize) -> Vec<Issue> {
    // Its box was built around the ink itself: the outline checks do not apply.
    if c.joined {
        return vec![Issue::Joined];
    }
    let mut issues = Vec::new();
    let hull = c.m.data.iter().filter(|&&b| b).count().max(1);
    if (c.raw_count as f32) < 0.88 * hull as f32 {
        issues.push(Issue::Notched);
    }
    if c.was_split {
        issues.push(Issue::Split);
    }
    if has_inner_gutter(c, ink, paper, page_h) {
        issues.push(Issue::InnerGutter);
    }
    if border_fraction(c, ink) < 0.5 {
        issues.push(Issue::NoBorder);
    }
    let (rw, rh) = (c.r.w as f32, c.r.h as f32);
    if (c.r.area() as f32) < 0.03 * (page_w * page_h) as f32 || rw.max(rh) > 6.0 * rw.min(rh) {
        issues.push(Issue::Small);
    }
    issues
}

/// A thin band of paper-coloured lines crossing the panel, bounded by border lines
/// on both sides: the signature of a gutter between two panels that were merged.
fn has_inner_gutter(c: &Cand, ink: &Mask, paper: &Mask, page_h: usize) -> bool {
    let max_thin = ((page_h as f32 * 0.02) as usize).max(2);
    // Blank fraction of each row and each column, counting only pixels inside the outline.
    let (w, h) = (c.m.w, c.m.h);
    let mut row_ink = vec![0usize; h];
    let mut row_n = vec![0usize; h];
    let mut row_paper = vec![0usize; h];
    let mut col_ink = vec![0usize; w];
    let mut col_n = vec![0usize; w];
    let mut col_paper = vec![0usize; w];
    for y in 0..h {
        for x in 0..w {
            if !c.m.at(x, y) {
                continue;
            }
            let (px, py) = (c.r.x as usize + x, c.r.y as usize + y);
            let i = ink.at(px, py) as usize;
            let p = paper.at(px, py) as usize;
            row_ink[y] += i;
            row_n[y] += 1;
            row_paper[y] += p;
            col_ink[x] += i;
            col_n[x] += 1;
            col_paper[x] += p;
        }
    }
    let blank_band = |inks: &[usize], papers: &[usize], ns: &[usize], span: usize| {
        let blank: Vec<bool> = (0..ns.len())
            .map(|j| ns[j] as f32 >= 0.6 * span as f32 && papers[j] as f32 >= 0.9 * ns[j] as f32)
            .collect();
        let len = blank.len();
        let (lo, hi) = (len / 10, len - len / 10);
        let mut i = lo;
        while i < hi {
            if !blank[i] {
                i += 1;
                continue;
            }
            let start = i;
            while i < hi && blank[i] {
                i += 1;
            }
            let run = i - start;
            // Border lines right before and after the band: mostly ink across the panel,
            // unlike the blank rows between lines of caption text.
            let inked = |j: usize| ns[j] > 0 && inks[j] as f32 > 0.55 * ns[j] as f32;
            let before = (start.saturating_sub(4)..start).any(inked);
            let after = (i..(i + 4).min(len)).any(inked);
            if run <= max_thin && before && after {
                return true;
            }
        }
        false
    };
    blank_band(&row_ink, &row_paper, &row_n, w) || blank_band(&col_ink, &col_paper, &col_n, h)
}

/// Share of the outline that has ink right inside it (a drawn panel border).
fn border_fraction(c: &Cand, ink: &Mask) -> f32 {
    let (w, h) = (c.m.w as isize, c.m.h as isize);
    let inside = |x: isize, y: isize| x >= 0 && y >= 0 && x < w && y < h && c.m.at(x as usize, y as usize);
    let (mut edge, mut inked) = (0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if !inside(x, y) || (inside(x - 1, y) && inside(x + 1, y) && inside(x, y - 1) && inside(x, y + 1)) {
                continue;
            }
            edge += 1;
            let near = (-2..=2).any(|dy| {
                (-2..=2).any(|dx| {
                    inside(x + dx, y + dy) && ink.at((c.r.x as isize + x + dx) as usize, (c.r.y as isize + y + dy) as usize)
                })
            });
            inked += near as usize;
        }
    }
    if edge == 0 { 0.0 } else { inked as f32 / edge as f32 }
}

/// Andrew's monotone chain; returns hull vertices in order.
fn convex_hull(mut pts: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    pts.sort_unstable();
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: (i64, i64), a: (i64, i64), b: (i64, i64)| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut hull: Vec<(i64, i64)> = Vec::with_capacity(pts.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &(i64, i64)>> =
            if pass == 0 { Box::new(pts.iter()) } else { Box::new(pts.iter().rev()) };
        for &p in iter {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

/// Median luma of a thin band around the page edge: the paper / gutter colour.
pub(crate) fn background_luma(g: &GrayImage) -> u8 {
    let (w, h) = g.dimensions();
    let band = ((w.min(h) as f32 * 0.01) as u32).clamp(2, w.min(h) / 2);
    let mut hist = [0u64; 256];
    for (x, y, p) in g.enumerate_pixels() {
        if x < band || y < band || x >= w - band || y >= h - band {
            hist[p[0] as usize] += 1;
        }
    }
    let total: u64 = hist.iter().sum();
    let mut acc = 0;
    for (v, &n) in hist.iter().enumerate() {
        acc += n;
        if acc * 2 >= total {
            return v as u8;
        }
    }
    255
}

/// Drop narrow strips lying outside the grid of the other panels, beside or below all of them,
/// close to the page edge and without a clean drawn border: the stack of page edges, a book
/// cover or the table in a photo of an open book. Title strips are kept: a row is dropped only
/// when it is very thin.
pub fn drop_edge_strips(panels: &mut Vec<Panel>, page_w: u32, page_h: u32) {
    // A thin strip along the image border is the stack of page edges or scan shadow, never a
    // panel: a narrow column near the left or right side, or a low band near the top or bottom.
    // A doubtful column (notched, borderless, or made of stray ink) may be wider: the stack of
    // page edges beside a photographed book.
    let thin_at_border = |p: &Panel| {
        let r = &p.rect;
        let (w, h) = (page_w as f32, page_h as f32);
        let unsure = p.confidence < 1.0 || p.issues.contains(&Issue::Joined);
        let limit = if unsure { 0.16 } else { 0.12 };
        let column = (r.w as f32) < limit * w
            && r.h > 2 * r.w
            && ((r.x as f32) < 0.1 * w || r.right() as f32 > 0.9 * w);
        // (A page-edge band spans the page; a title strip does not.)
        let band = (r.h as f32) < 0.08 * h
            && (r.w as f32) > 0.85 * w
            && ((r.y as f32) < 0.06 * h || r.bottom() as f32 > 0.94 * h);
        column || band
    };
    if panels.len() > 1 {
        panels.retain(|p| !thin_at_border(p));
    }
    if panels.len() < 3 {
        return;
    }
    let median = |f: fn(&Rect) -> u32| {
        let mut v: Vec<u32> = panels.iter().map(|p| f(&p.rect)).collect();
        v.sort_unstable();
        v[v.len() / 2]
    };
    let (mw, mh) = (median(|r| r.w), median(|r| r.h));
    let rects: Vec<Rect> = panels.iter().map(|p| p.rect).collect();
    let mut i = 0;
    panels.retain(|p| {
        let k = i;
        i += 1;
        let r = p.rect;
        if !p.issues.iter().any(|i| matches!(i, Issue::Notched | Issue::NoBorder | Issue::Small)) {
            return true;
        }
        let others = rects.iter().enumerate().filter(|&(j, _)| j != k).map(|(_, o)| o);
        // Beside every other panel: no horizontal overlap beyond a sliver of the strip.
        let tol_x = r.w / 5;
        let beside = others.clone().all(|o| r.x + tol_x >= o.right() || r.right() <= o.x + tol_x);
        let tol_y = r.h / 5;
        let below_or_above = others.clone().all(|o| r.y + tol_y >= o.bottom() || r.bottom() <= o.y + tol_y);
        let near_side = r.x < page_w / 12 || r.right() > page_w - page_w / 12;
        let near_end = r.y < page_h / 25 || r.bottom() > page_h - page_h / 25;
        let thin_column = beside && near_side && (r.w as f32) < 0.3 * mw as f32 && r.h > 2 * r.w;
        let thin_row = below_or_above
            && near_end
            && p.issues.contains(&Issue::Small)
            && (r.h as f32) < 0.15 * mh as f32
            && r.w > 4 * r.h;
        !(thin_column || thin_row)
    });
}

/// Everything that is not gutter. Gutter = paper reachable from the page edge, plus
/// enclosed paper regions shaped like a gutter (thin and long), which happen when
/// balloons or captions close off both ends of a gutter.
fn solid_mask(ink: &Mask) -> Mask {
    let (w, h) = (ink.w, ink.h);
    let mut paper = Mask { w, h, data: ink.data.iter().map(|&b| !b).collect() };
    let (labels, boxes) = label(&paper);
    let mut area = vec![0usize; boxes.len()];
    for &l in &labels {
        if l > 0 {
            area[l as usize - 1] += 1;
        }
    }
    let mut gutter = vec![false; boxes.len()];
    for (i, b) in boxes.iter().enumerate() {
        let touches_edge = b.x == 0 || b.y == 0 || b.right() as usize == w || b.bottom() as usize == h;
        let long = b.w.max(b.h) as f32;
        let thickness = area[i] as f32 / long;
        let thin_long = long >= 0.15 * w.min(h) as f32 && thickness <= 0.012 * w.min(h) as f32;
        gutter[i] = touches_edge || thin_long;
    }
    for (p, &l) in paper.data.iter_mut().zip(&labels) {
        *p = !(l > 0 && gutter[l as usize - 1]);
    }
    paper
}

/// 4-connected components. Returns per-pixel labels (0 = unset, k+1 = component k) and bounding boxes.
fn label(m: &Mask) -> (Vec<u32>, Vec<Rect>) {
    let (w, h) = (m.w, m.h);
    let mut labels = vec![0u32; w * h];
    let mut boxes = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !m.data[start] || labels[start] != 0 {
            continue;
        }
        let id = boxes.len() as u32 + 1;
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        labels[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            let mut visit = |j: usize| {
                if m.data[j] && labels[j] == 0 {
                    labels[j] = id;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        boxes.push(Rect { x: x0 as u32, y: y0 as u32, w: (x1 - x0 + 1) as u32, h: (y1 - y0 + 1) as u32 });
    }
    (labels, boxes)
}

/// Recursively cut a blob along lines covered at most `coverage` of their length.
/// `min_seg` = (min width, min height) of a piece; cuts that would create slivers are rejected.
fn split(m: &Mask, r: Rect, coverage: f32, min_seg: (usize, usize), depth: u32, out: &mut Vec<Rect>) {
    let Some(r) = trim(m, r) else { return };
    if depth < MAX_DEPTH {
        let rows = segments(&m.row_profile(r), (coverage * r.w as f32) as usize, min_seg.1);
        if rows.len() > 1 {
            for (a, b) in rows {
                let sub = Rect { x: r.x, y: r.y + a as u32, w: r.w, h: (b - a) as u32 };
                split(m, sub, coverage, min_seg, depth + 1, out);
            }
            return;
        }
        let cols = segments(&m.col_profile(r), (coverage * r.h as f32) as usize, min_seg.0);
        if cols.len() > 1 {
            for (a, b) in cols {
                let sub = Rect { x: r.x + a as u32, y: r.y, w: (b - a) as u32, h: r.h };
                split(m, sub, coverage, min_seg, depth + 1, out);
            }
            return;
        }
    }
    out.push(r);
}

/// Shrink a region to the lines that are more than 2% covered.
fn trim(m: &Mask, r: Rect) -> Option<Rect> {
    if r.w == 0 || r.h == 0 {
        return None;
    }
    let rows = m.row_profile(r);
    let rl = r.w as usize / 50;
    let top = rows.iter().position(|&n| n > rl)?;
    let bot = rows.iter().rposition(|&n| n > rl)? + 1;
    let r = Rect { x: r.x, y: r.y + top as u32, w: r.w, h: (bot - top) as u32 };
    let cols = m.col_profile(r);
    let cl = r.h as usize / 50;
    let left = cols.iter().position(|&n| n > cl)?;
    let right = cols.iter().rposition(|&n| n > cl)? + 1;
    Some(Rect { x: r.x + left as u32, y: r.y, w: (right - left) as u32, h: r.h })
}

/// Runs `[start, end)` of lines above `limit`, separated by at least 2 lines at or below it.
/// Returns a single run if any piece would be shorter than `min_len`.
fn segments(profile: &[usize], limit: usize, min_len: usize) -> Vec<(usize, usize)> {
    const MIN_GAP: usize = 2;
    let mut segs = Vec::new();
    let mut start: Option<usize> = None;
    let mut last = 0;
    for (i, &n) in profile.iter().enumerate() {
        if n <= limit {
            continue;
        }
        match start {
            Some(s) if i - last - 1 >= MIN_GAP => {
                segs.push((s, last + 1));
                start = Some(i);
            }
            None => start = Some(i),
            _ => {}
        }
        last = i;
    }
    if let Some(s) = start {
        segs.push((s, last + 1));
    }
    if segs.len() > 1 && segs.iter().any(|(a, b)| b - a < min_len) {
        return vec![(segs[0].0, segs[segs.len() - 1].1)];
    }
    segs
}

/// XY-cut over boxes: split into horizontal bands where no box straddles the cut
/// (allowing 15% overlap for slanted panels), else into columns, recursively.
pub(crate) fn reading_order<T: Clone>(boxes: Vec<T>, rect: fn(&T) -> Rect, rtl: bool) -> Vec<T> {
    if boxes.len() <= 1 {
        return boxes;
    }
    for horizontal in [true, false] {
        let span = |t: &T| {
            let b = rect(t);
            if horizontal { (b.y, b.bottom()) } else { (b.x, b.right()) }
        };
        let mut sorted = boxes.clone();
        sorted.sort_by_key(|b| {
            let (lo, hi) = span(b);
            lo + hi
        });
        let mut groups: Vec<Vec<T>> = Vec::new();
        let mut end = 0;
        for b in sorted {
            let (lo, hi) = span(&b);
            if let Some(g) = groups.last_mut() {
                let smallest = g.iter().map(|x| span(x).1 - span(x).0).min().unwrap();
                let slack = 0.15 * (hi - lo).min(smallest) as f32;
                if (lo as f32) < end as f32 - slack {
                    g.push(b);
                    end = end.max(hi);
                    continue;
                }
            }
            groups.push(vec![b]);
            end = hi;
        }
        if groups.len() > 1 {
            if !horizontal && rtl {
                groups.reverse();
            }
            return groups.into_iter().flat_map(|g| reading_order(g, rect, rtl)).collect();
        }
    }
    // No clean cut (overlapping slanted panels): rows by vertical overlap, then across.
    let mut boxes = boxes;
    boxes.sort_by_key(|t| rect(t).y + rect(t).h / 2);
    let mut rows: Vec<Vec<T>> = Vec::new();
    for t in boxes {
        let r = rect(&t);
        if let Some(row) = rows.last_mut() {
            let top = row.iter().map(|x| rect(x).y).min().unwrap();
            let bot = row.iter().map(|x| rect(x).bottom()).max().unwrap();
            let overlap = bot.min(r.bottom()).saturating_sub(top.max(r.y));
            if overlap as f32 >= 0.5 * r.h.min(bot - top) as f32 {
                row.push(t);
                continue;
            }
        }
        rows.push(vec![t]);
    }
    for row in &mut rows {
        row.sort_by_key(|t| rect(t).x + rect(t).w / 2);
        if rtl {
            row.reverse();
        }
    }
    rows.into_iter().flatten().collect()
}

/// Draw detected panels on a copy of the page: exact outlines and numbered badges in reading order.
pub fn debug_image(img: &DynamicImage, panels: &[Panel]) -> image::RgbImage {
    let mut out = img.to_rgb8();
    let (w, h) = out.dimensions();
    let t = (w.min(h) / 250).max(2);
    let colors = [[230, 57, 70], [29, 120, 214], [46, 160, 67], [240, 140, 0], [142, 68, 173]];
    let fill = |out: &mut image::RgbImage, x0: f32, y0: f32, x1: f32, y1: f32, c: [u8; 3]| {
        let (x0, y0) = (x0.max(0.0) as u32, y0.max(0.0) as u32);
        for y in y0..(y1.max(0.0) as u32).min(h) {
            for x in x0..(x1.max(0.0) as u32).min(w) {
                out.put_pixel(x, y, image::Rgb(c));
            }
        }
    };
    for (i, p) in panels.iter().enumerate() {
        let c = colors[i % colors.len()];
        let r = p.rect;
        let t = t as f32;
        match &p.shape {
            Some(s) => {
                let half = (t - s.sx.max(s.sy)).max(0.0) / 2.0;
                for (x, y) in s.outline() {
                    fill(&mut out, x - half, y - half, x + s.sx + half, y + s.sy + half, c);
                }
            }
            None => {
                let (x0, y0, x1, y1) = (r.x as f32, r.y as f32, (r.x + r.w) as f32, r.bottom() as f32);
                fill(&mut out, x0, y0, x1, y0 + t, c);
                fill(&mut out, x0, y1 - t, x1, y1, c);
                fill(&mut out, x0, y0, x0 + t, y1, c);
                fill(&mut out, x1 - t, y0, x1, y1, c);
            }
        }
        // Numbered badge in the top-left corner.
        let radius = (w.min(h) as f32 / 38.0).max(12.0);
        let (cx, cy) = (r.x as f32 + radius * 1.4, r.y as f32 + radius * 1.4);
        for y in (cy - radius - 3.0) as u32..((cy + radius + 3.0) as u32).min(h) {
            for x in (cx - radius - 3.0).max(0.0) as u32..((cx + radius + 3.0) as u32).min(w) {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                if d <= radius {
                    out.put_pixel(x, y, image::Rgb(c));
                } else if d <= radius + 3.0 {
                    out.put_pixel(x, y, image::Rgb([255, 255, 255]));
                }
            }
        }
        let label = if p.is_low() { format!("{}?", i + 1) } else { (i + 1).to_string() };
        draw_label(&mut out, &label, cx, cy, (radius / 4.5).max(2.0));
    }
    out
}

/// Draw digits and '?' centred at (`cx`, `cy`) in white with a 3x5 pixel font scaled by `k`.
fn draw_label(img: &mut image::RgbImage, text: &str, cx: f32, cy: f32, k: f32) {
    const DIGITS: [[u8; 5]; 10] = [
        [7, 5, 5, 5, 7], [2, 6, 2, 2, 7], [7, 1, 7, 4, 7], [7, 1, 7, 1, 7], [5, 5, 7, 1, 1],
        [7, 4, 7, 1, 7], [7, 4, 7, 5, 7], [7, 1, 1, 1, 1], [7, 5, 7, 5, 7], [7, 5, 7, 1, 7],
    ];
    const QUESTION: [u8; 5] = [7, 1, 2, 0, 2];
    let total_w = (text.len() as f32 * 4.0 - 1.0) * k;
    let (x0, y0) = (cx - total_w / 2.0, cy - 2.5 * k);
    let (w, h) = img.dimensions();
    for (di, ch) in text.bytes().enumerate() {
        let glyph = if ch == b'?' { QUESTION } else { DIGITS[(ch - b'0') as usize] };
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..3 {
                if bits >> (2 - col) & 1 == 0 {
                    continue;
                }
                let px = x0 + (di as f32 * 4.0 + col as f32) * k;
                let py = y0 + row as f32 * k;
                for y in py.max(0.0) as u32..((py + k) as u32).min(h) {
                    for x in px.max(0.0) as u32..((px + k) as u32).min(w) {
                        img.put_pixel(x, y, image::Rgb([255, 255, 255]));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stray_captions_join_their_cartoon_and_titles_stand_alone() {
        let (w, h) = (400usize, 600usize);
        let mut ink = Mask::new(w, h);
        let mut fill = |x0: usize, y0: usize, x1: usize, y1: usize| {
            for y in y0..y1 {
                for x in (x0..x1).step_by(3) {
                    ink.data[y * w + x] = true;
                }
            }
        };
        fill(40, 150, 360, 170); // a title line, far from the cartoons
        fill(50, 300, 180, 312); // the caption under the left cartoon
        fill(195, 585, 205, 592); // the page number
        let cand = |x, y, cw, ch| {
            let m = Mask { w: cw as usize, h: ch as usize, data: vec![true; (cw * ch) as usize] };
            Cand { r: Rect { x, y, w: cw, h: ch }, raw_count: m.data.len(), m, was_split: false, joined: false }
        };
        let mut panels = vec![cand(40, 200, 150, 90), cand(220, 200, 150, 90)];
        attach_stray_ink(&mut panels, &ink);
        assert_eq!(panels.len(), 3, "the title is a panel of its own");
        assert!(panels[0].r.bottom() >= 312, "the caption joined the left cartoon: {:?}", panels[0].r);
        assert_eq!(panels[1].r, Rect { x: 220, y: 200, w: 150, h: 90 });
        assert!(panels.iter().all(|c| c.r.bottom() < 585), "the page number is left out");
    }

    #[test]
    fn edge_strips_outside_the_grid_are_dropped() {
        let r = |x, y, w, h| Rect { x, y, w, h };
        let grid = || {
            vec![
                Panel::new(r(80, 280, 1480, 730), None, vec![]),
                Panel::new(r(80, 1020, 1480, 730), None, vec![]),
                Panel::new(r(80, 1770, 1480, 730), None, vec![]),
            ]
        };
        // The stack of page edges beside the panels, near the page side.
        let mut panels = grid();
        panels.push(Panel::new(r(1670, 110, 300, 1400), None, vec![Issue::Notched]));
        drop_edge_strips(&mut panels, 2100, 2800);
        assert_eq!(panels.len(), 3);
        // A clean narrow panel in the same place is art, and a title above the grid is kept.
        let mut panels = grid();
        panels.push(Panel::new(r(1670, 280, 300, 1400), None, vec![]));
        panels.push(Panel::new(r(300, 40, 1000, 200), None, vec![Issue::NoBorder]));
        drop_edge_strips(&mut panels, 2100, 2800);
        assert_eq!(panels.len(), 5);
    }

    use super::*;
    use image::{Luma, GrayImage};

    fn page_with(panels: &[(u32, u32, u32, u32)]) -> DynamicImage {
        let mut g = GrayImage::from_pixel(1000, 1500, Luma([255]));
        for &(x, y, w, h) in panels {
            for yy in y..y + h {
                for xx in x..x + w {
                    // Border plus mid-grey art.
                    let border = xx < x + 4 || xx >= x + w - 4 || yy < y + 4 || yy >= y + h - 4;
                    g.put_pixel(xx, yy, Luma([if border { 0 } else { 150 }]));
                }
            }
        }
        DynamicImage::ImageLuma8(g)
    }

    fn opts(rtl: bool) -> DetectOptions {
        DetectOptions {
            threshold: 48,
            split_coverage: 0.35,
            min_panel_area: 0.01,
            padding: 0.0,
            exact_shapes: true,
            rtl,
        }
    }

    #[test]
    fn convex_fill_closes_open_holes_and_keeps_slant() {
        // A slanted panel (wider at the bottom) with a balloon-shaped gap open at the top.
        let rows = [
            "0011100000",
            "0011100110",
            "0111100111",
            "0111111111",
            "1111111111",
        ];
        let m = Mask {
            w: 10,
            h: 5,
            data: rows.iter().flat_map(|r| r.bytes().map(|b| b == b'1')).collect(),
        };
        let f = m.convex_fill();
        assert!(f.at(6, 1) && f.at(5, 0), "open gap inside the outline is filled");
        assert!(!f.at(0, 0) && !f.at(0, 1), "area outside the slanted edge stays out");
    }

    #[test]
    fn grid_in_reading_order() {
        // Row 1: two panels. Row 2: one wide panel. Row 3: tall left + two stacked right.
        let img = page_with(&[
            (50, 50, 430, 400),
            (520, 50, 430, 400),
            (50, 500, 900, 400),
            (50, 950, 430, 500),
            (520, 950, 430, 230),
            (520, 1220, 430, 230),
        ]);
        let expect = |p: Vec<Panel>, want: &[(u32, u32)]| {
            assert_eq!(p.len(), want.len(), "{p:?}");
            assert!(p.iter().all(|p| p.shape.is_none()), "rectangles need no shape mask");
            for (r, &(x, y)) in p.iter().map(|p| p.rect).zip(want) {
                assert!(r.x.abs_diff(x) <= 4 && r.y.abs_diff(y) <= 4, "{r:?} vs {:?}", (x, y));
            }
        };
        expect(
            detect_panels(&img, &opts(false)),
            &[(50, 50), (520, 50), (50, 500), (50, 950), (520, 950), (520, 1220)],
        );
        expect(
            detect_panels(&img, &opts(true)),
            &[(520, 50), (50, 50), (50, 500), (520, 950), (520, 1220), (50, 950)],
        );
    }

    /// Fill text-like strokes (short dark bars, like letters) into `g` over a box.
    fn letters(g: &mut GrayImage, x0: u32, y0: u32, w: u32, h: u32) {
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                if (y - y0) % 30 < 14 && (x - x0) % 22 < 12 {
                    g.put_pixel(x, y, Luma([0]));
                }
            }
        }
    }

    fn with_panels(panels: &[(u32, u32, u32, u32)]) -> GrayImage {
        page_with(panels).to_luma8()
    }

    // Regression (All-Negro p. 2, 28): a prose page with one small illustration is kept whole,
    // not cut down to the picture.
    #[test]
    fn text_page_is_kept_whole() {
        let mut g = with_panels(&[(350, 100, 300, 300)]);
        letters(&mut g, 80, 480, 840, 900);
        let p = detect_panels(&DynamicImage::ImageLuma8(g), &opts(false));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].rect, Rect { x: 0, y: 0, w: 1000, h: 1500 });
    }

    // Regression (All-Negro p. 5, 8, 22, 33): a running head above the grid is not a panel and
    // does not make the first panel grow up to it.
    #[test]
    fn running_head_is_left_out() {
        let mut g = with_panels(&[(50, 150, 430, 600), (520, 150, 430, 600), (50, 800, 900, 600)]);
        letters(&mut g, 380, 70, 240, 16);
        let p = detect_panels(&DynamicImage::ImageLuma8(g), &opts(false));
        assert_eq!(p.len(), 3, "{p:?}");
        assert!(p.iter().all(|p| p.rect.y >= 140), "{p:?}");
    }

    // Regression (All-Negro p. 41): a caption under a gag cartoon stays with that cartoon, and the
    // cartoon beside it does not take it.
    #[test]
    fn caption_under_a_cartoon_joins_it() {
        let mut g = with_panels(&[(50, 100, 400, 500), (550, 100, 400, 500), (50, 800, 900, 600)]);
        letters(&mut g, 60, 630, 380, 44);
        let p = detect_panels(&DynamicImage::ImageLuma8(g), &opts(false));
        assert_eq!(p.len(), 3, "{p:?}");
        assert!(p[0].rect.bottom() >= 670, "caption joined the left cartoon: {:?}", p[0].rect);
        assert!(p[1].rect.bottom() <= 610, "right cartoon unchanged: {:?}", p[1].rect);
    }

    // Regression (All-Negro p. 36, 50): caption lines between two rows are all kept, each whole
    // inside one panel, so no text is lost or cut through.
    #[test]
    fn caption_between_rows_is_kept() {
        let mut g = with_panels(&[(50, 50, 900, 500), (50, 900, 900, 550)]);
        letters(&mut g, 60, 650, 880, 150);
        let p = detect_panels(&DynamicImage::ImageLuma8(g), &opts(false));
        for line in (650..800).step_by(30) {
            let (top, bottom) = (line, line + 14);
            assert!(
                p.iter().any(|p| p.rect.y <= top && p.rect.bottom() >= bottom && p.rect.x <= 60 && p.rect.right() >= 940),
                "caption line at {line} not whole in a panel: {p:?}"
            );
        }
    }

    // Regression (ブラック悪魔): the stack of page edges along the side of a photographed book
    // is dropped, before and after the second round.
    #[test]
    fn page_edge_column_at_the_side_is_dropped() {
        let r = |x, y, w, h| Rect { x, y, w, h };
        let mut panels = vec![
            Panel::new(r(320, 280, 1480, 730), None, vec![]),
            Panel::new(r(320, 1020, 1480, 730), None, vec![]),
            Panel::new(r(1730, 250, 150, 2560), None, vec![Issue::Joined]),
        ];
        drop_edge_strips(&mut panels, 1880, 2820);
        assert_eq!(panels.len(), 2);
        // A real tall panel in the middle of the page stays.
        let mut panels = vec![Panel::new(r(700, 100, 200, 2000), None, vec![]), Panel::new(r(100, 100, 500, 900), None, vec![])];
        drop_edge_strips(&mut panels, 1880, 2820);
        assert_eq!(panels.len(), 2);
    }

    // Each panel's cell reaches halfway to its neighbours and out to the page edge.
    #[test]
    fn cells_partition_the_page() {
        let r = |x, y, w, h| Rect { x, y, w, h };
        let c = cells(&[r(100, 100, 300, 300), r(500, 100, 300, 300)], 900, 600);
        assert_eq!(c[0], r(0, 0, 450, 600));
        assert_eq!(c[1], r(450, 0, 450, 600));
    }

    // Regression (All-Negro p. 31): a caption block whose lines run on under a neighbour's
    // slanted outline is widened to take in the rest of the lettering.
    #[test]
    fn caption_box_grows_over_its_lettering() {
        let mut ink = Mask::new(400, 300);
        for y in 50..120 {
            for x in (20..260).filter(|x| x % 3 != 0) {
                ink.data[y * 400 + x] = true;
            }
        }
        let r = grow_over_ink(Rect { x: 20, y: 50, w: 200, h: 70 }, &ink);
        assert!(r.right() >= 258 && r.right() <= 262, "{r:?}");
        assert_eq!(r.x, 20);
    }
}
