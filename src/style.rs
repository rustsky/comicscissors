//! The drawing style of a book, measured in a quick pass over all its pages before detection, so
//! the detection can be set up for it.
//!
//! The pass measures a few [`Traits`] (gutter widths, how many pages bleed off the edge, colour,
//! paper), picks the [`Preset`] they fit and adjusts its [`Tuning`] to the measured numbers:
//!
//! - **Hairline gutters.** Some books (often manga printed small) leave almost no gutter
//!   between panels: two frame lines with a light line one or two pixels wide between them.
//!   Detection thickens the ink by a pixel to close gaps in drawn lines, which closes these gutters
//!   too. In a book that has them, the hairlines are found and a frame line with one along it is
//!   cut along. In other books the search is skipped: hatching and speed lines can look like
//!   hairlines on a single page.
//! - **Analysis resolution.** Detection works on a downscaled page; a book with narrow gutters is
//!   analysed finer so they survive.
//! - **Bleeds and shared frames.** Pages where panels run off the edge, and panels that share a
//!   frame line with no gutter, need the rules that merge a bleeding panel's pieces and cut along a
//!   shared line. Books with a clean grid of framed panels never need them, and are safer without.
//! - **Colour.** In a colour book, pale coloured areas (a yellow sky) are art, not paper.

use crate::detect::Mask;
use image::GrayImage;

/// A hairline is lighter by this much than the frame lines on both sides of it.
const CONTRAST: i16 = 50;
/// Frame lines are this much darker than the paper.
const DARK: i16 = 100;
/// The farthest a frame line lies from the hairline's middle, in pixels.
const REACH: usize = 3;
/// A hairline gutter runs straight for this share of the page side at least.
const MIN_RUN: f32 = 0.1;

/// Share of a book's gutters that are hairlines above which the book uses them.
const THIN_GUTTERS: f32 = 0.2;
/// Share of pages bleeding off the edge above which a book is laid out with bleeds.
const BLEED_PAGES: f32 = 0.25;
/// Share of colour pages above which a book is in colour.
const COLOR_PAGES: f32 = 0.5;
/// Pages at least this wide for their height are albums (bande dessinée), narrower ones
/// American comic books.
const ALBUM_ASPECT: f32 = 0.72;
/// Share of strip-layout pages (four rows or more) above which a book is a strip collection.
const STRIP_PAGES: f32 = 0.75;
/// Paper darker than this is yellowed (or the scan is grey).
const AGED_PAPER: u8 = 225;
/// Share of two-page images above which a book is scanned as spreads.
const TWO_PAGE: f32 = 0.5;
/// A photographed book's page border is this clean at most (the book edge and table show).
const PHOTO_CLEAN: f32 = 0.9;
/// Gutters narrower than this (share of the longest side) make a dense layout.
const DENSE_GUTTER: f32 = 0.01;
/// A gutter needs this many analysis pixels to survive the ink thickening.
const GUTTER_PX: f32 = 4.0;
/// Range of the analysis resolution (longest side).
const ANALYSIS_MIN: u32 = 1200;
const ANALYSIS_MAX: u32 = 2000;

/// The book styles detection is set up for: the first of three layers of settings, refined by
/// the book's [`Substyle`]s and then by each page's pattern (see [`crate::pattern`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Preset {
    /// Read right to left or black and white: panels bleeding off the page, shared frame lines.
    Manga,
    /// Colour, panels bleeding off the page, splashes (American comics today).
    WesternModern,
    /// Colour, a grid of framed panels with white gutters, on narrow pages (American comic books).
    WesternClassic,
    /// A dense grid of framed panels on wide album pages (Franco-Belgian albums).
    BandeDessinee,
    /// Newspaper strips collected on a page: four or more rows of panels, often unframed.
    NewspaperStrip,
}

impl Preset {
    pub fn name(self) -> &'static str {
        match self {
            Preset::Manga => "manga",
            Preset::WesternModern => "western-modern",
            Preset::WesternClassic => "western-classic",
            Preset::BandeDessinee => "bande-dessinée",
            Preset::NewspaperStrip => "newspaper-strip",
        }
    }

    /// The preset a book's traits fit (`rtl`: read right to left, as manga is).
    pub fn for_traits(t: &Traits, rtl: bool) -> Preset {
        let color = t.color_pages >= COLOR_PAGES;
        let bleeds = t.bleed_pages >= BLEED_PAGES;
        if t.strip_pages >= STRIP_PAGES {
            Preset::NewspaperStrip
        } else if rtl || (!color && bleeds && t.aspect < ALBUM_ASPECT) {
            Preset::Manga
        } else if color && bleeds && !Substyle::Aged.found(t) {
            Preset::WesternModern
        } else if color && t.aspect >= ALBUM_ASPECT && !Substyle::Aged.found(t) {
            Preset::BandeDessinee
        } else {
            Preset::WesternClassic
        }
    }

    /// The preset's settings, before the book's substyles adjust them.
    pub fn tuning(self) -> Tuning {
        let base = Tuning { preset: Some(self), ..Tuning::default() };
        match self {
            Preset::Manga => base,
            Preset::WesternModern => Tuning { color_ink: true, ..base },
            // A clean grid never needs bleed merging or shared-frame cuts, and they only misfire.
            Preset::WesternClassic | Preset::BandeDessinee => Tuning { bleeds: false, frame_lines: false, color_ink: true, ..base },
            // Rows of strips: no bleeds, and no shared frames to cut along.
            Preset::NewspaperStrip => Tuning { bleeds: false, frame_lines: false, ..base },
        }
    }
}

/// A trait of a whole book that adjusts its preset's settings; a book can have several.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Substyle {
    /// In colour (colour manga): pale colours are art, not paper.
    Color,
    /// Panels apart by hairline gutters (two frame lines, a light line between): cut along them.
    Hairline,
    /// Most pages run off the edge: merge the pieces of bleeding panels, cut along shared frames.
    Bleed,
    /// Narrow gutters: analysed finer.
    Dense,
    /// Printed long ago and scanned: yellowed or patchy paper, uneven frames. No cuts along shared
    /// frames: worn frames and printing marks look like them.
    Aged,
    /// Two pages per image (a scan of the open book): split into pages before detection.
    TwoPage,
    /// Photographed open book: the book's edges and the table around it are no bleeds.
    Photographed,
}

impl Substyle {
    pub const ALL: [Substyle; 7] = [
        Substyle::Color,
        Substyle::Hairline,
        Substyle::Bleed,
        Substyle::Dense,
        Substyle::Aged,
        Substyle::TwoPage,
        Substyle::Photographed,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Substyle::Color => "colour",
            Substyle::Hairline => "hairline",
            Substyle::Bleed => "bleed",
            Substyle::Dense => "dense",
            Substyle::Aged => "aged",
            Substyle::TwoPage => "two-page",
            Substyle::Photographed => "photographed",
        }
    }

    /// Whether a book with these traits has the substyle.
    pub fn found(self, t: &Traits) -> bool {
        match self {
            Substyle::Color => t.color_pages >= COLOR_PAGES,
            Substyle::Hairline => t.thin_gutters >= THIN_GUTTERS,
            Substyle::Bleed => t.bleed_pages >= BLEED_PAGES,
            Substyle::Dense => t.gutter > 0.0 && t.gutter < DENSE_GUTTER,
            Substyle::Aged => t.paper < AGED_PAPER || (t.clean < 0.5 && t.bleed_pages < BLEED_PAGES),
            Substyle::TwoPage => t.spread_pages >= TWO_PAGE,
            Substyle::Photographed => t.spread_pages >= TWO_PAGE && t.paper < AGED_PAPER && t.clean < PHOTO_CLEAN,
        }
    }

    fn adjust(self, t: &mut Tuning, traits: &Traits) {
        match self {
            Substyle::Color => t.color_ink = true,
            Substyle::Hairline => t.hairlines = true,
            Substyle::Bleed => {
                t.bleeds = true;
                t.frame_lines = true;
            }
            Substyle::Aged => t.frame_lines = false,
            // (Spreads are split before detection: nothing to set.)
            Substyle::TwoPage => {}
            Substyle::Photographed => t.bleeds = false,
            // Analyse finely enough that a narrow (not hairline) gutter keeps `GUTTER_PX` pixels.
            Substyle::Dense => {
                if traits.narrow_gutter > 0.0 {
                    t.analysis_max = ((GUTTER_PX / traits.narrow_gutter).ceil() as u32).clamp(ANALYSIS_MIN, ANALYSIS_MAX);
                }
            }
        }
    }
}

/// The substyles of a book, as a set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Substyles(u8);

impl Substyles {
    pub fn contains(self, s: Substyle) -> bool {
        self.0 & (1 << s as u8) != 0
    }

    pub fn insert(&mut self, s: Substyle) {
        self.0 |= 1 << s as u8;
    }

    pub fn iter(self) -> impl Iterator<Item = Substyle> {
        Substyle::ALL.into_iter().filter(move |&s| self.contains(s))
    }
}

/// Detection settings, layered: the book's style preset, adjusted by its substyles, then by
/// each page's pattern.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    /// The preset these settings come from (`None`: the defaults, no style pass), and the
    /// substyles that adjusted them.
    pub preset: Option<Preset>,
    pub substyles: Substyles,
    /// Longest side of the page as analysed.
    pub analysis_max: u32,
    /// Cut along hairline gutters.
    pub hairlines: bool,
    /// Merge the pieces of panels bleeding off the page, and treat the page edge as an open side.
    pub bleeds: bool,
    /// Cut along a frame line shared by panels with no gutter between them.
    pub frame_lines: bool,
    /// Count clearly coloured pixels as ink even when they are pale.
    pub color_ink: bool,
}

impl Default for Tuning {
    fn default() -> Tuning {
        Tuning {
            preset: None,
            substyles: Substyles::default(),
            analysis_max: ANALYSIS_MIN,
            hairlines: false,
            bleeds: true,
            frame_lines: true,
            color_ink: false,
        }
    }
}

impl Tuning {
    /// The settings for a book: its preset (`forced`, or the one its traits fit), adjusted by its
    /// substyles (`forced_subs` when given, else the ones its traits show).
    pub fn for_book(t: &Traits, forced: Option<Preset>, forced_subs: &[Substyle], rtl: bool) -> Tuning {
        let preset = forced.unwrap_or_else(|| Preset::for_traits(t, rtl));
        let mut tuning = preset.tuning();
        for s in Substyle::ALL {
            if if forced_subs.is_empty() { s.found(t) } else { forced_subs.contains(&s) } {
                tuning.substyles.insert(s);
                s.adjust(&mut tuning, t);
            }
        }
        tuning
    }

    /// One line describing the settings, for the console.
    pub fn describe(&self, t: &Traits) -> String {
        let mut parts = vec![format!("gutters ~{:.1}% of the page", t.gutter * 100.0)];
        parts.push(format!("{:.0}% of pages bleed", t.bleed_pages * 100.0));
        if self.analysis_max > ANALYSIS_MIN {
            parts.push(format!("analysed at {}px", self.analysis_max));
        }
        let name = self.preset.map_or("default", Preset::name);
        let subs: Vec<&str> = self.substyles.iter().map(Substyle::name).collect();
        if subs.is_empty() {
            format!("{name} ({})", parts.join(", "))
        } else {
            format!("{name} · {} ({})", subs.join("+"), parts.join(", "))
        }
    }
}

/// What the quick pass finds on one page.
#[derive(Clone, Debug, Default)]
pub struct PageStyle {
    /// Share of the page's pixels in clear colour (not grey).
    pub color: f32,
    /// Widths of the gutters found (straight blank channels between two dark lines), as a share
    /// of the page's longest side, and in pixels at the measuring resolution.
    pub gutters: Vec<f32>,
    pub gutter_px: Vec<usize>,
    /// Sides of the page with art running off them (out of 4).
    pub bleeds: u8,
    /// Paper luma, and the share of the page border that is clean paper (low on a photographed or
    /// yellowed scan).
    pub paper: u8,
    pub clean: f32,
    /// Width over height of the page.
    pub aspect: f32,
    /// Rows of panels: blank bands running the page's whole width, plus one.
    pub tiers: u8,
}

/// Longest side the gutters are measured at: fine enough for thin gutters.
const MEASURE_MAX: u32 = 1600;
/// A gutter is a blank channel at most this wide (share of the longest side).
const MAX_GUTTER: f32 = 0.05;

/// The quick look at one page, on a copy fine enough for thin gutters.
pub fn measure_page(img: &image::DynamicImage) -> PageStyle {
    let mut style = PageStyle { aspect: img.width() as f32 / img.height().max(1) as f32, ..Default::default() };
    let fine = {
        let g = img.to_luma8();
        let (w, h) = g.dimensions();
        let s = (MEASURE_MAX as f32 / w.max(h) as f32).min(1.0);
        if s < 1.0 {
            image::imageops::resize(&g, ((w as f32 * s) as u32).max(1), ((h as f32 * s) as u32).max(1), image::imageops::FilterType::Triangle)
        } else {
            g
        }
    };
    let bg = crate::detect::background_luma(&fine);
    let longest = fine.width().max(fine.height()) as f32;
    style.gutter_px = gutter_widths(&fine, bg);
    style.gutters = style.gutter_px.iter().map(|&w| w as f32 / longest).collect();
    style.bleeds = bleed_sides(&fine, bg);
    style.paper = bg;
    style.clean = clean_border(&fine, bg);
    style.color = color_share(img);
    style.tiers = tiers(&fine, bg);
    style
}

/// Rows of the page's layout: bands of blank rows running across the middle nine tenths of the
/// page, at least 0.4% of its height, between inked rows; plus one.
fn tiers(gray: &GrayImage, bg: u8) -> u8 {
    let (w, h) = gray.dimensions();
    let (x0, x1) = (w / 20, w - w / 20);
    let blank = |y: u32| (x0..x1).filter(|&x| gray.get_pixel(x, y)[0] as i16 >= bg as i16 - 40).count() * 100 >= (x1 - x0) as usize * 97;
    let min_band = (h / 250).max(2);
    let rows: Vec<bool> = (0..h).map(blank).collect();
    let (Some(first), Some(last)) = (rows.iter().position(|&b| !b), rows.iter().rposition(|&b| !b)) else { return 0 };
    let mut bands = 0;
    let mut y = first;
    while y <= last {
        if rows[y] {
            let start = y;
            while y <= last && rows[y] {
                y += 1;
            }
            bands += (y - start >= min_band as usize) as u8;
        } else {
            y += 1;
        }
    }
    bands.saturating_add(1)
}

/// Share of pixels (sampled) whose channels differ clearly: coloured, not grey.
fn color_share(img: &image::DynamicImage) -> f32 {
    let Some(rgb) = img.as_rgb8().map(std::borrow::Cow::Borrowed).or_else(|| (img.color().channel_count() >= 3).then(|| std::borrow::Cow::Owned(img.to_rgb8())))
    else {
        return 0.0;
    };
    // Spread of the channels, against that of the paper: yellowed paper and the brown ink printed
    // on it are tinted alike, and are not colour.
    let spread = |p: &image::Rgb<u8>| {
        let [r, g, b] = p.0;
        r.max(g).max(b) as i16 - r.min(g).min(b) as i16
    };
    let (w, h) = rgb.dimensions();
    let band = (w.min(h) / 50).max(1);
    let mut border: Vec<i16> =
        rgb.enumerate_pixels().filter(|&(x, y, _)| x < band || y < band || x + band >= w || y + band >= h).step_by(5).map(|(_, _, p)| spread(p)).collect();
    border.sort_unstable();
    let paper = border.get(border.len() / 2).copied().unwrap_or(0);
    let (mut n, mut c) = (0usize, 0usize);
    for p in rgb.pixels().step_by(7) {
        n += 1;
        c += (spread(p) > paper + 25) as usize;
    }
    c as f32 / n.max(1) as f32
}

/// Widths of straight blank channels with a dark line on both sides (gutters between framed
/// panels), each followed along every fourth line for a tenth of the page side or more.
fn gutter_widths(gray: &GrayImage, bg: u8) -> Vec<usize> {
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    let longest = w.max(h);
    let max_w = ((MAX_GUTTER * longest as f32) as usize).max(3);
    let bg = bg as i16;
    let g = |x: usize, y: usize| gray.get_pixel(x as u32, y as u32)[0] as i16;
    let mut out = Vec::new();
    for vertical in [true, false] {
        let (len, lines) = if vertical { (w, h) } else { (h, w) };
        let at = |t: usize, u: usize| if vertical { g(t, u) } else { g(u, t) };
        // Channels on each sampled line: light runs with dark ends.
        let runs = |u: usize| {
            let mut found = Vec::new();
            let mut t = 1;
            while t < len {
                if at(t, u) >= bg - 60 && at(t - 1, u) <= bg - DARK {
                    let a = t;
                    while t < len && at(t, u) >= bg - 60 {
                        t += 1;
                    }
                    if t < len && t - a <= max_w && at(t, u) <= bg - DARK {
                        found.push((a, t));
                    }
                } else {
                    t += 1;
                }
            }
            found
        };
        // Follow each channel from line to line; (start, end, lines followed).
        let need = (MIN_RUN * lines as f32 / 4.0) as usize;
        let mut open: Vec<(usize, usize, usize, Vec<usize>)> = Vec::new();
        for u in (0..lines).step_by(4) {
            let found = runs(u);
            let mut next = Vec::new();
            for (a, b) in found {
                let cont = open.iter().position(|o| o.0.abs_diff(a) <= 2 && o.1.abs_diff(b) <= 2);
                match cont {
                    Some(i) => {
                        let (_, _, n, mut widths) = open.swap_remove(i);
                        widths.push(b - a);
                        next.push((a, b, n + 1, widths));
                    }
                    None => next.push((a, b, 1, vec![b - a])),
                }
            }
            for (_, _, n, mut widths) in open {
                if n >= need {
                    widths.sort_unstable();
                    out.push(widths[widths.len() / 2]);
                }
            }
            open = next;
        }
        for (_, _, n, mut widths) in open {
            if n >= need {
                widths.sort_unstable();
                out.push(widths[widths.len() / 2]);
            }
        }
    }
    out
}

/// Sides of the page that art runs off: the outermost band holds ink along much of the side.
fn bleed_sides(gray: &GrayImage, bg: u8) -> u8 {
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    let band = (w.min(h) / 60).max(2);
    let ink = |x: usize, y: usize| (gray.get_pixel(x as u32, y as u32)[0] as i16) < bg as i16 - 60;
    let side = |vertical: bool, far: bool| {
        let (len, depth) = if vertical { (h, w) } else { (w, h) };
        let inked = (0..len)
            .filter(|&t| {
                (0..band).any(|d| {
                    let d = if far { depth - 1 - d } else { d };
                    if vertical { ink(d, t) } else { ink(t, d) }
                })
            })
            .count();
        inked * 10 >= len * 3
    };
    [side(true, false), side(true, true), side(false, false), side(false, true)].iter().filter(|&&b| b).count() as u8
}

/// Share of the page border within a little of the paper colour.
fn clean_border(gray: &GrayImage, bg: u8) -> f32 {
    let (w, h) = gray.dimensions();
    let band = (w.min(h) / 100).max(2);
    let (mut n, mut clean) = (0usize, 0usize);
    for (x, y, p) in gray.enumerate_pixels() {
        if x < band || y < band || x >= w - band || y >= h - band {
            n += 1;
            clean += ((p[0] as i16 - bg as i16).abs() <= 12) as usize;
        }
    }
    clean as f32 / n.max(1) as f32
}

/// The traits of a whole book, from its pages.
#[derive(Clone, Copy, Debug, Default)]
pub struct Traits {
    /// Share of pages in colour.
    pub color_pages: f32,
    /// Share of the gutters found that are hairlines (3 pixels or less).
    pub thin_gutters: f32,
    /// Typical gutter, and a narrow one (the 20th percentile), as a share of the longest side.
    pub gutter: f32,
    pub narrow_gutter: f32,
    /// Share of pages with art running off at least one side.
    pub bleed_pages: f32,
    /// Typical paper luma, and how much of the page border is clean paper.
    pub paper: u8,
    pub clean: f32,
    /// Typical width over height of the portrait pages.
    pub aspect: f32,
    /// Share of images holding two pages side by side (landscape scans).
    pub spread_pages: f32,
    /// Share of pages laid out as stacked strips (strip collections).
    pub strip_pages: f32,
}

impl Traits {
    pub fn from_pages(pages: &[PageStyle]) -> Traits {
        let n = pages.len().max(1) as f32;
        // Hairlines are counted apart (`thin_gutters`).
        let mut gutters: Vec<f32> = pages
            .iter()
            .flat_map(|p| p.gutters.iter().zip(&p.gutter_px).filter(|&(_, &px)| px > 3).map(|(&g, _)| g))
            .collect();
        gutters.sort_by(f32::total_cmp);
        let pct = |v: &[f32], q: f32| if v.is_empty() { 0.0 } else { v[((v.len() - 1) as f32 * q) as usize] };
        let median_u8 = |mut v: Vec<u8>| {
            v.sort_unstable();
            v.get(v.len() / 2).copied().unwrap_or(255)
        };
        let mut clean: Vec<f32> = pages.iter().map(|p| p.clean).collect();
        clean.sort_by(f32::total_cmp);
        Traits {
            color_pages: pages.iter().filter(|p| p.color >= 0.05).count() as f32 / n,
            thin_gutters: {
                let all: Vec<usize> = pages.iter().flat_map(|p| p.gutter_px.iter().copied()).collect();
                all.iter().filter(|&&w| w <= 3).count() as f32 / all.len().max(1) as f32
            },
            gutter: pct(&gutters, 0.5),
            narrow_gutter: pct(&gutters, 0.2),
            bleed_pages: pages.iter().filter(|p| p.bleeds > 0).count() as f32 / n,
            paper: median_u8(pages.iter().map(|p| p.paper).collect()),
            clean: pct(&clean, 0.5),
            spread_pages: pages.iter().filter(|p| p.aspect > 1.1).count() as f32 / n,
            // Strips stacked five or more deep on a page, or two or more on a landscape page (a
            // comic-book page has up to four tiers).
            strip_pages: pages.iter().filter(|p| (p.aspect <= 0.85 && p.tiers >= 5) || ((0.85..=1.1).contains(&p.aspect) && p.tiers >= 2)).count() as f32 / n,
            aspect: {
                let mut a: Vec<f32> = pages.iter().map(|p| p.aspect).filter(|&a| a > 0.0 && a < 1.0).collect();
                a.sort_by(f32::total_cmp);
                pct(&a, 0.5)
            },
        }
    }
}

/// Light pixels on one line of pixels with a clearly dark pixel within `REACH` on both sides.
fn ridges(line: &[i16], bg: i16) -> Vec<bool> {
    let len = line.len();
    (0..len)
        .map(|t| {
            if t < 2 || t + 2 >= len {
                return false;
            }
            let v = line[t];
            let dark = |s: usize| line[s] <= v - CONTRAST && line[s] <= bg - DARK;
            (t.saturating_sub(REACH)..t).any(dark) && (t + 1..(t + REACH + 1).min(len)).any(dark)
        })
        .collect()
}

/// Hairline gutters on a page (luma at analysis resolution, paper `bg`): light lines between two
/// frame lines, running straight (drifting a pixel at a time) for a tenth of the page side or more.
pub fn hairlines(gray: &GrayImage, bg: u8) -> Mask {
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    let mut out = Mask::new(w, h);
    let g = |x: usize, y: usize| gray.get_pixel(x as u32, y as u32)[0] as i16;
    for vertical in [true, false] {
        let (len, lines) = if vertical { (w, h) } else { (h, w) };
        let at = |t: usize, u: usize| if vertical { g(t, u) } else { g(u, t) };
        let ridge: Vec<Vec<bool>> = (0..lines).map(|u| ridges(&(0..len).map(|t| at(t, u)).collect::<Vec<_>>(), bg as i16)).collect();
        let min_run = (MIN_RUN * lines as f32) as usize;
        // Follow each ridge along the lines, a pixel of drift per line; keep the long ones.
        let mut seen = vec![false; len * lines];
        for u0 in 0..lines {
            for t0 in 0..len {
                if !ridge[u0][t0] || seen[u0 * len + t0] {
                    continue;
                }
                let mut path = vec![(t0, u0)];
                let (mut t, mut u) = (t0, u0);
                while u + 1 < lines {
                    let Some(next) = [t, t.saturating_sub(1), (t + 1).min(len - 1)].into_iter().find(|&s| ridge[u + 1][s]) else {
                        break;
                    };
                    t = next;
                    u += 1;
                    path.push((t, u));
                }
                for &(t, u) in &path {
                    seen[u * len + t] = true;
                }
                if path.len() >= min_run {
                    for (t, u) in path {
                        let (x, y) = if vertical { (t, u) } else { (u, t) };
                        out.data[y * w + x] = true;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Luma};

    /// Two panels side by side with a hairline gutter between them.
    fn hairline_page() -> GrayImage {
        let mut g = GrayImage::from_pixel(400, 600, Luma([255]));
        for y in 50..550 {
            for x in 50..350 {
                let v = if (196..199).contains(&x) || (200..203).contains(&x) { 10 } else if x == 199 { 220 } else { 140 };
                g.put_pixel(x, y, Luma([v]));
            }
        }
        g
    }

    #[test]
    fn finds_a_hairline_gutter() {
        let g = hairline_page();
        assert!(measure_page(&DynamicImage::ImageLuma8(g.clone())).gutter_px.contains(&1));
        let m = hairlines(&g, 255);
        assert!((60..540).all(|y| m.at(199, y)), "hairline not found along its length");
        assert!(!m.at(100, 300) && !m.at(197, 300), "only the hairline itself");
    }

    #[test]
    fn a_plain_page_has_none() {
        let mut g = GrayImage::from_pixel(400, 600, Luma([255]));
        for y in 50..550 {
            for x in 50..350 {
                g.put_pixel(x, y, Luma([if x < 53 || x >= 347 || y < 53 || y >= 547 { 10 } else { 140 }]));
            }
        }
        assert!(measure_page(&DynamicImage::ImageLuma8(g.clone())).gutter_px.is_empty());
        assert!(!hairlines(&g, 255).data.iter().any(|&b| b));
    }

    fn traits(thin: f32, bleed: f32, color: f32, paper: u8, clean: f32) -> Traits {
        Traits { thin_gutters: thin, bleed_pages: bleed, color_pages: color, paper, clean, gutter: 0.01, narrow_gutter: 0.008, aspect: 0.7, spread_pages: 0.0, strip_pages: 0.0 }
    }

    // Traits measured on real books of each kind pick their presets and substyles.
    #[test]
    fn books_get_their_presets() {
        let with_aspect = |mut t: Traits, a: f32| {
            t.aspect = a;
            t
        };
        use Substyle::*;
        let cases = [
            ("manga with hairline gutters", with_aspect(traits(0.43, 0.70, 0.0, 255, 0.82), 0.64), Preset::Manga, vec![Hairline, Bleed]),
            ("manga", with_aspect(traits(0.09, 0.78, 0.03, 255, 0.87), 0.69), Preset::Manga, vec![Bleed]),
            ("bande dessinée", with_aspect(traits(0.0, 0.01, 0.99, 255, 1.0), 0.76), Preset::BandeDessinee, vec![Color]),
            ("bande dessinée, some bleeds", with_aspect(traits(0.0, 0.06, 0.92, 255, 1.0), 0.74), Preset::BandeDessinee, vec![Color]),
            ("nine-panel grid", with_aspect(traits(0.006, 0.08, 0.81, 255, 1.0), 0.65), Preset::WesternClassic, vec![Color]),
            ("modern colour comic", with_aspect(traits(0.0, 0.51, 0.97, 255, 0.61), 0.65), Preset::WesternModern, vec![Color, Bleed]),
            ("golden-age scan", traits(0.0, 0.1, 0.9, 210, 0.4), Preset::WesternClassic, vec![Color, Aged]),
            ("black and white strips, every page bleeding", with_aspect(traits(0.0, 1.0, 0.03, 255, 0.84), 0.74), Preset::WesternClassic, vec![Bleed]),
        ];
        for (book, t, preset, subs) in cases.clone() {
            let tuning = Tuning::for_book(&t, None, &[], false);
            assert_eq!(tuning.preset, Some(preset), "{book}");
            assert_eq!(tuning.substyles.iter().collect::<Vec<_>>(), subs, "{book}");
        }
        assert_eq!(Tuning::for_book(&cases[2].1, Some(Preset::Manga), &[], false).preset, Some(Preset::Manga), "forced");
        let forced = Tuning::for_book(&cases[2].1, None, &[Bleed], false);
        assert!(forced.bleeds, "forced substyle applied");
        assert_eq!(forced.substyles.iter().collect::<Vec<_>>(), vec![Bleed], "forced substyles replace the found ones");
    }

    #[test]
    fn strip_collections_need_most_pages_stacked() {
        let mut t = with_strips(0.82);
        assert_eq!(Preset::for_traits(&t, false), Preset::NewspaperStrip);
        t.strip_pages = 0.6; // a kids' comic with some four-tier pages
        assert_ne!(Preset::for_traits(&t, false), Preset::NewspaperStrip);
    }

    fn with_strips(s: f32) -> Traits {
        Traits { strip_pages: s, ..traits(0.0, 0.1, 0.9, 255, 1.0) }
    }

    #[test]
    fn colour_manga_gets_colour_ink() {
        let t = Tuning::for_book(&traits(0.0, 0.7, 0.9, 255, 0.8), None, &[], true);
        assert_eq!(t.preset, Some(Preset::Manga));
        assert!(t.substyles.contains(Substyle::Color) && t.color_ink);
    }

    #[test]
    fn narrow_gutters_raise_the_resolution() {
        let mut t = traits(0.0, 0.0, 1.0, 255, 1.0);
        t.gutter = 0.006;
        t.narrow_gutter = 0.004; // 4.8 px at 1200
        assert_eq!(Tuning::for_book(&t, None, &[], false).analysis_max, 1200);
        t.narrow_gutter = 0.0025; // 3 px at 1200: analyse at 1600
        assert_eq!(Tuning::for_book(&t, None, &[], false).analysis_max, 1600);
    }
}
