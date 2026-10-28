//! Synthetic pages that reproduce detection failures found on real pages, for regression tests.
//!
//! Real pages can't go into the repository (copyright), so each failure is redrawn here with
//! plain shapes: frames, flat grey art, sparse strokes, balloons with lettering. Each builder
//! names the book page it stands for. `scripts/synthetic_pages.sh` writes them all out as PNGs to
//! look at.

use crate::detect::{self, DetectOptions, Panel, Rect};
use image::{DynamicImage, GrayImage, Luma};

/// Manga page size (as scanned for a phone or e-reader).
pub const W: u32 = 700;
pub const H: u32 = 1100;
/// Flat mid-grey art: ink, but no frame line.
pub const ART: u8 = 130;

/// A white page to draw on.
pub struct Page(pub GrayImage);

impl Page {
    pub fn new() -> Page {
        Page::sized(W, H)
    }

    pub fn sized(w: u32, h: u32) -> Page {
        Page(GrayImage::from_pixel(w, h, Luma([255])))
    }

    pub fn fill(&mut self, x0: u32, y0: u32, x1: u32, y1: u32, v: u8) -> &mut Self {
        let (w, h) = self.0.dimensions();
        for y in y0.min(h)..y1.min(h) {
            for x in x0.min(w)..x1.min(w) {
                self.0.put_pixel(x, y, Luma([v]));
            }
        }
        self
    }

    /// A closed panel frame, 3 pixels thick, its inside left as it is.
    pub fn frame(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) -> &mut Self {
        self.fill(x0, y0, x1, y0 + 3, 0).fill(x0, y1 - 3, x1, y1, 0).fill(x0, y0, x0 + 3, y1, 0).fill(x1 - 3, y0, x1, y1, 0)
    }

    /// A framed panel filled with flat art.
    pub fn panel(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) -> &mut Self {
        self.fill(x0, y0, x1, y1, ART).frame(x0, y0, x1, y1)
    }

    /// A panel at `x`, `y` sized `w` x `h` with a 4 pixel frame and light grey art (the pages of
    /// the first regression tests).
    pub fn boxed(&mut self, x: u32, y: u32, w: u32, h: u32) -> &mut Self {
        self.fill(x, y, x + w, y + h, 0).fill(x + 4, y + 4, x + w - 4, y + h - 4, 150)
    }

    /// One frame line: horizontal at row `at` over `a..b`, or vertical at column `at`.
    pub fn line(&mut self, horizontal: bool, at: u32, a: u32, b: u32) -> &mut Self {
        if horizontal { self.fill(a, at, b, at + 3, 0) } else { self.fill(at, a, at + 3, b, 0) }
    }

    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, v: u8) -> &mut Self {
        self.each_in_ellipse(cx, cy, rx, ry, |d| (d <= 1.0).then_some(v))
    }

    /// A speech balloon: white inside, a dark outline, and lines of lettering.
    pub fn balloon(&mut self, cx: f32, cy: f32, rx: f32, ry: f32) -> &mut Self {
        let edge = 3.0 / rx.min(ry);
        self.each_in_ellipse(cx, cy, rx, ry, |d| (d <= 1.0).then_some(if d > 1.0 - edge { 0 } else { 255 }));
        // Lettering: short bars on lines 14 pixels apart, inside the middle of the balloon.
        let (x0, x1) = ((cx - rx * 0.6) as u32, (cx + rx * 0.6) as u32);
        let (y0, y1) = ((cy - ry * 0.6) as u32, (cy + ry * 0.6) as u32);
        for y in (y0..y1).filter(|y| (y - y0) % 14 < 6) {
            for x in (x0..x1).filter(|x| (x - x0) % 9 < 5) {
                self.0.put_pixel(x, y, Luma([0]));
            }
        }
        self
    }

    /// Two thick strokes crossing corner to corner (a figure's limbs, a sword): drawn art whose
    /// outline runs over paper nearly all the way round.
    pub fn cross(&mut self, x0: u32, y0: u32, x1: u32, y1: u32) -> &mut Self {
        self.cross_with(x0, y0, x1, y1, 40, ART)
    }

    pub fn cross_with(&mut self, x0: u32, y0: u32, x1: u32, y1: u32, thick: u32, v: u8) -> &mut Self {
        let (w, h) = ((x1 - x0 - thick) as f32, (y1 - y0) as f32);
        for y in y0..y1 {
            let t = (y - y0) as f32 / h;
            for c in [x0 as f32 + w * t, (x1 - thick) as f32 - w * t] {
                self.fill(c as u32, y, c as u32 + thick, y + 1, v);
            }
        }
        self
    }

    /// Sparse diagonal strokes: drawn art with a lot of paper between the lines (sky, rain,
    /// speed lines).
    pub fn strokes(&mut self, x0: u32, y0: u32, x1: u32, y1: u32, step: u32) -> &mut Self {
        let (w, h) = self.0.dimensions();
        for y in y0..y1.min(h) {
            for x in x0..x1.min(w) {
                if (x + y) % step < 2 {
                    self.0.put_pixel(x, y, Luma([40]));
                }
            }
        }
        self
    }

    fn each_in_ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, f: impl Fn(f32) -> Option<u8>) -> &mut Self {
        let (w, h) = self.0.dimensions();
        let (y0, y1) = ((cy - ry).max(0.0) as u32, ((cy + ry) as u32 + 1).min(h));
        let (x0, x1) = ((cx - rx).max(0.0) as u32, ((cx + rx) as u32 + 1).min(w));
        for y in y0..y1 {
            for x in x0..x1 {
                let (dx, dy) = ((x as f32 - cx) / rx, (y as f32 - cy) / ry);
                if let Some(v) = f((dx * dx + dy * dy).sqrt()) {
                    self.0.put_pixel(x, y, Luma([v]));
                }
            }
        }
        self
    }

    pub fn image(&self) -> DynamicImage {
        DynamicImage::ImageLuma8(self.0.clone())
    }
}

/// Panels stacked on the left share their right frame
/// line with a tall panel that bleeds off the top, bottom and right of the page, its sky blank.
/// The gutters between the stacked panels end at that line, so the page read as one panel.
pub fn shared_frame_line() -> Page {
    let mut p = Page::new();
    // Figures in flat grey, with paper around them inside the frames.
    for (y0, y1) in [(100, 390), (410, 700), (720, 1020)] {
        p.frame(40, y0, 363, y1).ellipse(200.0, (y0 + y1) as f32 / 2.0, 110.0, 110.0, ART);
    }
    // The tall panel: buildings (up against the line) under a blank sky with clouds, off the
    // page on three sides.
    p.fill(363, 330, 700, 1100, ART).ellipse(470.0, 180.0, 70.0, 40.0, ART).ellipse(600.0, 250.0, 60.0, 35.0, ART);
    p.strokes(385, 0, 700, 330, 37);
    p.line(false, 360, 0, 1100);
    p
}

/// A wide panel with a tall brush stroke of a sound effect and speed lines
/// beside it. The stroke is no frame line, and the blank between speed lines no gutter.
pub fn sound_effect_stroke() -> Page {
    let mut p = Page::new();
    p.panel(40, 60, 660, 380);
    p.panel(40, 400, 660, 620);
    for y in (410..610).step_by(9) {
        p.fill(560, y, 650, y + 2, 0);
    }
    p.fill(545, 403, 555, 617, 0);
    p.panel(40, 640, 660, 1040);
    p
}

/// A panel bleeding off the top of the page, with only its left and bottom
/// frame drawn. Blank paper from the top edge cuts off the boots at its top, and a balloon of
/// the panel below crosses the gutter and breaks the bottom frame edge.
pub fn bleed_with_broken_edge() -> Page {
    let mut p = Page::new();
    p.line(false, 30, 0, 703).line(true, 700, 30, 700);
    p.fill(45, 0, 160, 180, ART); // the boots, off the top of the page
    p.strokes(33, 0, 700, 240, 41); // rubble lines on the blank sheet
    p.fill(60, 260, 690, 697, ART); // the figures
    p.panel(30, 730, 670, 1030);
    p.balloon(540.0, 745.0, 70.0, 60.0);
    p
}

/// A panel with no frame at all in the bottom left corner, open to the page
/// edges: a balloon, a lamp post and a figure on blank rain. Its only side is the right column's
/// frame. Each figure read as a panel of its own.
pub fn borderless_corner_panel() -> Page {
    let mut p = Page::new();
    p.panel(30, 30, 670, 300);
    p.panel(360, 330, 670, 600).panel(360, 620, 670, 1070);
    p.strokes(0, 320, 345, 1100, 53);
    p.balloon(170.0, 470.0, 55.0, 100.0);
    p.fill(245, 420, 262, 990, ART).fill(225, 440, 285, 520, ART); // lamp post and sign
    p.ellipse(165.0, 810.0, 38.0, 160.0, ART); // a figure
    p
}

/// Framed panels in the middle, with panels bleeding off the top left and
/// the bottom of the page, whose outlines there have no frame (the page edge cuts them). Half
/// the candidates looked borderless, so the page was kept whole.
pub fn bleeding_panels_not_frameless() -> Page {
    let mut p = Page::new();
    // Top left, off the top and left edges: only its bottom frame drawn.
    p.line(true, 300, 0, 303).cross(0, 0, 300, 300);
    p.panel(330, 20, 670, 303);
    p.panel(40, 330, 670, 820);
    // Bottom, off the bottom edge: only its top frame drawn.
    p.line(true, 850, 40, 670).cross(40, 853, 670, 1100);
    p
}

/// A balloon crossing the gutter between two panels, its text reaching
/// into both. Cutting along the gutter cut the text.
pub fn shared_balloon() -> Page {
    let mut p = Page::new();
    p.panel(40, 100, 660, 390).panel(40, 410, 660, 700);
    p.balloon(520.0, 400.0, 60.0, 95.0);
    p.panel(40, 730, 660, 1040);
    p
}

/// A balloon outside every frame, beside a panel that bleeds off the bottom.
/// It read as a panel of its own. A small framed panel holding only a short line of text
/// (p. 26) stays a panel.
pub fn lone_balloon() -> Page {
    let mut p = Page::new();
    p.panel(40, 60, 660, 400);
    p.panel(40, 420, 330, 560).balloon(185.0, 490.0, 40.0, 40.0).frame(40, 420, 330, 560);
    p.panel(350, 420, 660, 700);
    p.balloon(100.0, 870.0, 55.0, 110.0);
    p.line(true, 730, 190, 660).line(false, 190, 730, 1100).fill(193, 733, 660, 1100, ART);
    p
}

/// Speed lines running from a panel past its frame off the top of the
/// page, apart from the frame. They were left out of the panel.
pub fn art_off_the_page_edge() -> Page {
    let mut p = Page::new();
    p.panel(30, 70, 670, 580);
    for i in 0..6u32 {
        let x = 470 + i * 30;
        p.fill(x, 0, x + 8, 60, 0);
    }
    p.panel(30, 610, 670, 1040);
    p
}

/// A panel bleeding off the top left corner of the page, a blank wall in the
/// corner. Its outline cut the corner off diagonally.
pub fn bleed_corner() -> Page {
    let mut p = Page::new();
    p.line(false, 670, 0, 500).line(true, 497, 0, 673);
    p.strokes(0, 0, 200, 260, 31); // the wall: a few lines on blank paper
    p.fill(0, 260, 667, 494, ART).fill(200, 0, 667, 260, ART);
    p.panel(40, 520, 670, 1060);
    p
}

/// A tall panel beside two stacked ones, with a gutter too thin
/// to survive downscaling. The gutter between the stacked panels runs into the tall panel;
/// cutting along it cut the tall panel in two.
pub fn thin_gutter_t_junction() -> Page {
    let mut p = Page::sized(1000, 1500);
    p.boxed(50, 100, 550, 500).boxed(50, 624, 550, 776).boxed(603, 100, 277, 1300);
    p
}

/// A panel bleeding off the bottom of the page and into the margin on the right has
/// a frame on two sides only. Paper flowing in through the open sides broke its art into figures
/// that read as borderless panels (and so the page was kept whole).
pub fn open_at_the_page_edge() -> Page {
    let mut p = Page::sized(1000, 1500);
    p.boxed(50, 50, 900, 600);
    p.fill(50, 700, 950, 704, 0).fill(50, 700, 54, 1500, 0); // top and left frame lines
    // Figures, apart from each other and the frame; the last runs off the page at the bottom and
    // stops at the margin on the right.
    for (cx, cy, rx, ry) in [(260.0, 1055.0, 160.0, 295.0), (610.0, 950.0, 90.0, 150.0), (800.0, 1400.0, 220.0, 250.0)] {
        p.ellipse(cx, cy, rx, ry, 110);
    }
    p.fill(950, 700, 1000, 1500, 255);
    p
}

/// One framed panel beside a big borderless candidate (panels merged, their outline
/// opened up by a bleed) is not a page of unframed pictures.
pub fn big_borderless_candidate() -> Page {
    let mut p = Page::sized(1000, 1500);
    p.boxed(50, 50, 900, 400).cross_with(50, 500, 950, 1450, 60, 110);
    p
}

/// Two panels in a row with a gutter too thin to survive downscaling merge into one
/// candidate under a third of the page. It still has to be searched at full resolution.
pub fn thin_gutter_row() -> Page {
    let mut p = Page::sized(1000, 1500);
    p.boxed(50, 50, 448, 500).boxed(501, 50, 449, 500).boxed(50, 600, 900, 850);
    p
}

/// A tall panel beside two stacked ones, separated from them by a hairline
/// between two frame lines. The gutter between the stacked ones, carried on across the tall panel,
/// runs through a balloon there: blank, so it read as a gutter and the tall panel was cut.
pub fn balloon_at_a_t_junction() -> Page {
    let mut p = Page::new();
    p.panel(40, 60, 660, 520);
    p.panel(40, 540, 204, 1040).balloon(122.0, 700.0, 74.0, 75.0);
    p.panel(206, 540, 660, 680).panel(206, 700, 660, 1040);
    // The hairline is grey from scanning, no clean gutter.
    for y in (540..1040).step_by(2) {
        p.fill(204, y, 206, y + 1, 180);
    }
    p
}

/// A borderless panel (figures on blank rain, no frame) found as two pieces
/// whose boxes overlap, with no gutter between them.
pub fn overlapping_borderless_pieces() -> Page {
    let mut p = Page::new();
    p.panel(30, 60, 340, 500).panel(30, 520, 340, 1040);
    p.strokes(360, 60, 700, 1100, 53);
    p.balloon(450.0, 250.0, 50.0, 110.0).ellipse(600.0, 520.0, 60.0, 250.0, ART).ellipse(520.0, 930.0, 90.0, 160.0, ART);
    p
}

/// A pencil sketch on a black page. It is the page's one picture: the page
/// stays whole instead of being cropped to the sketch.
pub fn sketch_on_black() -> Page {
    let mut p = Page::new();
    p.fill(0, 0, W, H, 0);
    p.cross_with(180, 640, 440, 1030, 30, 220); // the sketch: bold pale strokes
    for i in 0..8u32 {
        p.fill(150 + i * 40, 600 + i * 6, 153 + i * 40, 640 + i * 6, 220); // and a few thin ones around
    }
    p
}

/// An L-shaped panel bleeding off the left of the page: a blank wall down its
/// left side (paper reaches in from the page edge), a framed inset in its corner. The gutter between
/// the two panels stacked on the right carries on across the blank wall and read as a gutter
/// through the L panel.
pub fn l_panel_with_blank_wall() -> Page {
    let mut p = Page::new();
    p.panel(40, 60, 660, 360);
    p.line(true, 380, 0, 406).line(false, 403, 380, 1100); // the L panel's top and right frame
    p.strokes(0, 383, 230, 1100, 61); // the blank wall, a crack or two
    p.fill(230, 600, 403, 1100, ART).fill(60, 860, 230, 1100, ART); // figures at its foot, up behind the inset
    p.panel(240, 390, 395, 690).fill(250, 400, 385, 680, 200); // the inset
    p.line(false, 409, 380, 1100); // hairline beside the right frame: the right column's own frame
    p.panel(409, 380, 660, 650).panel(409, 668, 660, 1060);
    p
}

/// Panels side by side with a hairline gutter, two frame
/// lines with a grey line a pixel or two wide between them. The thickened ink closes it, and the
/// panels read as one. The book's style pass finds hairlines on many pages and turns on cutting
/// along them.
pub fn hairline_gutters() -> Page {
    let mut p = Page::new();
    p.panel(40, 60, 660, 400);
    // One pixel of light grey between the frames.
    p.panel(40, 420, 300, 1040).panel(301, 420, 660, 700).panel(301, 720, 660, 1040);
    p.fill(300, 420, 301, 1040, 205);
    p
}

/// A ledge drawn as a double line across part of a tall panel looks like a
/// hairline gutter. With hairline cutting on, it must not cut the panel.
pub fn double_line_in_art() -> Page {
    let mut p = Page::new();
    p.panel(40, 60, 660, 400);
    p.panel(40, 420, 660, 1040);
    p.fill(150, 700, 500, 703, 0).fill(150, 703, 500, 704, 220).fill(150, 704, 500, 707, 0);
    p
}

/// A framed panel above a borderless one (figures on blank rain), its box
/// grown down past its bottom frame around a caption, so the two boxes overlap in a strip along
/// that frame.
pub fn rain_under_a_panel() -> Page {
    let mut p = Page::new();
    p.panel(30, 30, 670, 300);
    p.strokes(0, 304, 700, 1100, 53);
    p.balloon(170.0, 470.0, 55.0, 100.0).ellipse(450.0, 700.0, 60.0, 250.0, ART);
    p
}

/// A colour page with panels of pale yellow sky, one of them drawn without side frames. Pale
/// yellow is light enough to read as paper: paper floods in from the sides and only the figure
/// is left of the panel. In a colour book the colour is ink.
pub fn pale_sky_panel() -> image::RgbImage {
    let mut p = image::RgbImage::from_pixel(W, H, image::Rgb([255, 255, 255]));
    let mut fill = |x0: u32, y0: u32, x1: u32, y1: u32, c: [u8; 3]| {
        for y in y0..y1 {
            for x in x0..x1 {
                p.put_pixel(x, y, image::Rgb(c));
            }
        }
    };
    for (x0, y0, x1, y1) in [(40, 60, 660, 400), (40, 420, 660, 700), (40, 720, 660, 1040)] {
        fill(x0, y0, x1, y1, [0, 0, 0]);
        fill(x0 + 3, y0 + 3, x1 - 3, y1 - 3, [250, 235, 120]); // pale yellow sky
    }
    // A figure in the middle panel, whose sides are open (no frame drawn there).
    fill(300, 470, 400, 690, [150, 60, 40]);
    fill(40, 423, 43, 697, [250, 235, 120]);
    fill(657, 423, 660, 697, [250, 235, 120]);
    p
}

/// A colour cover printed to the edge: dark red all over, a title box, a figure.
pub fn colour_splash() -> image::RgbImage {
    let mut p = image::RgbImage::from_pixel(W, H, image::Rgb([120, 20, 20]));
    for y in 80..300 {
        for x in 60..400 {
            p.put_pixel(x, y, image::Rgb([10, 10, 10]));
        }
    }
    for y in 500..1000 {
        for x in 250..550 {
            p.put_pixel(x, y, image::Rgb([200, 160, 60]));
        }
    }
    p
}

/// A tall panel beside three stacked ones, with a rail drawn across it level with a gutter of the
/// stacked ones: two straight lines with a blank band between, like a gutter, but closed off at
/// both ends by the tall panel's side frames. The tall panel was cut along it.
pub fn rail_in_a_tall_panel() -> Page {
    let mut p = Page::new();
    p.panel(40, 60, 430, 380).panel(40, 400, 430, 720).panel(40, 740, 430, 1040);
    p.panel(450, 60, 660, 1040);
    p.fill(453, 380, 657, 384, 0).fill(453, 384, 657, 396, 255).fill(453, 396, 657, 400, 0);
    p
}

/// Two columns of panels (the left one two stacked panels, whose gutter ends at the frame line
/// between the columns) above a panel the width of the page; the frame line between the columns
/// stops at their foot, and nothing frames the rows apart (the panels are open there), only a
/// pole crossing the gap joins them. The line was cut along all the way down, through the bottom
/// panel.
pub fn frame_line_stopping_short() -> Page {
    let mut p = Page::new();
    p.fill(40, 60, 660, 700, ART).fill(40, 1040, 660, 1043, 0);
    p.fill(40, 60, 660, 63, 0).fill(40, 60, 43, 700, 0).fill(657, 60, 660, 700, 0);
    p.fill(347, 60, 353, 700, 0); // the frame between the upper two
    p.fill(43, 380, 347, 400, 255).fill(43, 377, 347, 380, 0).fill(43, 400, 347, 403, 0); // the left one is two
    p.fill(40, 740, 660, 1040, ART).fill(40, 740, 43, 1043, 0).fill(657, 740, 660, 1043, 0);
    p.fill(150, 690, 180, 750, 0); // the pole across the gap
    p
}

/// Every synthetic page, by name.
pub const ALL: &[(&str, fn() -> Page)] = &[
    ("shared_frame_line", shared_frame_line),
    ("sound_effect_stroke", sound_effect_stroke),
    ("bleed_with_broken_edge", bleed_with_broken_edge),
    ("borderless_corner_panel", borderless_corner_panel),
    ("bleeding_panels_not_frameless", bleeding_panels_not_frameless),
    ("shared_balloon", shared_balloon),
    ("lone_balloon", lone_balloon),
    ("art_off_the_page_edge", art_off_the_page_edge),
    ("bleed_corner", bleed_corner),
    ("thin_gutter_t_junction", thin_gutter_t_junction),
    ("open_at_the_page_edge", open_at_the_page_edge),
    ("big_borderless_candidate", big_borderless_candidate),
    ("thin_gutter_row", thin_gutter_row),
    ("balloon_at_a_t_junction", balloon_at_a_t_junction),
    ("overlapping_borderless_pieces", overlapping_borderless_pieces),
    ("sketch_on_black", sketch_on_black),
    ("l_panel_with_blank_wall", l_panel_with_blank_wall),
    ("hairline_gutters", hairline_gutters),
    ("double_line_in_art", double_line_in_art),
    ("rain_under_a_panel", rain_under_a_panel),
    ("rail_in_a_tall_panel", rail_in_a_tall_panel),
    ("frame_line_stopping_short", frame_line_stopping_short),
];

/// Detection as `main` runs it on one page: first pass, then the second round.
pub fn detect(img: &DynamicImage) -> Vec<Panel> {
    detect_styled(img, false)
}

/// `detect` in a book of the given style (`hairlines`: hairline gutters, see [`crate::style`]).
pub fn detect_styled(img: &DynamicImage, hairlines: bool) -> Vec<Panel> {
    let o = DetectOptions { threshold: 48, split_coverage: 0.35, min_panel_area: 0.01, padding: 0.0, exact_shapes: true, rtl: false, tuning: crate::style::Tuning { hairlines, ..Default::default() } };
    let mut panels = detect::detect_panels(img, &o);
    detect::drop_edge_strips(&mut panels, img.width(), img.height());
    let (mut panels, _) = crate::refine::refine_page(img, panels, false, 0.0);
    detect::drop_edge_strips(&mut panels, img.width(), img.height());
    panels
}

/// Whether one panel holds all of `x0..x1` x `y0..y1` (within a few pixels).
pub fn held(panels: &[Panel], x0: u32, y0: u32, x1: u32, y1: u32) -> bool {
    panels.iter().any(|p| {
        let r: Rect = p.rect;
        r.x <= x0 + 6 && r.y <= y0 + 6 && r.right() + 6 >= x1 && r.bottom() + 6 >= y1
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(page: Page) -> Vec<Panel> {
        detect(&page.image())
    }

    #[test]
    fn panels_sharing_a_frame_line_are_cut_along_it() {
        let p = run(shared_frame_line());
        assert_eq!(p.len(), 4, "{p:?}");
        for (y0, y1) in [(100, 390), (410, 700), (720, 1020)] {
            assert!(held(&p, 40, y0, 363, y1), "stacked panel {y0}..{y1}: {p:?}");
        }
        assert!(held(&p, 366, 330, 700, 1100), "tall panel cut: {p:?}");
        assert!(!held(&p, 40, 410, 700, 700), "stacked panel merged with the tall one: {p:?}");
    }

    #[test]
    fn a_sound_effect_stroke_is_no_frame_line() {
        let p = run(sound_effect_stroke());
        assert_eq!(p.len(), 3, "{p:?}");
        assert!(held(&p, 40, 400, 660, 620), "{p:?}");
    }

    #[test]
    fn a_panel_bleeding_off_the_top_is_kept_whole() {
        let p = run(bleed_with_broken_edge());
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(held(&p, 30, 0, 690, 700), "boots cut off: {p:?}");
    }

    #[test]
    fn a_borderless_corner_panel_is_one_panel() {
        let p = run(borderless_corner_panel());
        assert_eq!(p.len(), 4, "{p:?}");
        assert!(held(&p, 115, 370, 285, 990), "{p:?}");
    }

    #[test]
    fn panels_bleeding_off_the_page_are_not_frameless() {
        let p = run(bleeding_panels_not_frameless());
        assert!(p.len() >= 3, "page kept whole: {p:?}");
        assert!(held(&p, 40, 330, 670, 820) && !held(&p, 0, 0, 700, 1100), "{p:?}");
    }

    #[test]
    fn a_balloon_shared_across_a_gutter_is_not_cut() {
        let p = run(shared_balloon());
        assert!(held(&p, 460, 305, 580, 495), "balloon cut: {p:?}");
        assert!(held(&p, 40, 730, 660, 1040), "{p:?}");
    }

    #[test]
    fn the_second_round_does_not_cut_a_shared_balloon() {
        // The first pass took the two panels as one; the second round looks for the gutter.
        let img = shared_balloon().image();
        let merged = Panel::new(Rect { x: 38, y: 98, w: 624, h: 604 }, None, vec![detect::Issue::Large]);
        let (p, _) = crate::refine::refine_page(&img, vec![merged], false, 0.0);
        assert!(held(&p, 460, 305, 580, 495), "balloon cut: {p:?}");
    }

    #[test]
    fn a_lone_balloon_joins_its_panel() {
        let p = run(lone_balloon());
        assert!(held(&p, 45, 760, 660, 1100), "balloon apart: {p:?}");
        assert!(held(&p, 40, 420, 330, 560) && !held(&p, 40, 420, 660, 700), "small framed panel merged: {p:?}");
    }

    #[test]
    fn art_off_the_page_edge_joins_its_panel() {
        let p = run(art_off_the_page_edge());
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(held(&p, 30, 0, 670, 580), "speed lines left out: {p:?}");
    }

    #[test]
    fn a_bleeding_corner_is_kept() {
        let p = run(bleed_corner());
        let top = p.iter().find(|p| p.rect.y < 10).expect("top panel");
        assert!(top.rect.x <= 2, "{p:?}");
        assert!(top.shape.as_ref().is_none_or(|s| s.alpha(20.0, 20.0) > 0.5), "corner cut off: {:?}", top.rect);
    }

    fn first_pass(page: Page) -> Vec<Panel> {
        let o = DetectOptions { threshold: 48, split_coverage: 0.35, min_panel_area: 0.01, padding: 0.0, exact_shapes: true, rtl: false, tuning: Default::default() };
        detect::detect_panels(&page.image(), &o)
    }

    #[test]
    fn a_tall_panel_beside_a_thin_gutter_is_not_cut() {
        assert!(held(&first_pass(thin_gutter_t_junction()), 603, 100, 880, 1400), "cut in the first pass");
        let p = run(thin_gutter_t_junction());
        assert!(held(&p, 603, 100, 880, 1400), "tall panel cut: {p:?}");
        assert_eq!(p.len(), 3, "{p:?}");
    }

    #[test]
    fn a_panel_open_at_the_page_edge_is_kept_whole() {
        for p in [first_pass(open_at_the_page_edge()), run(open_at_the_page_edge())] {
            assert_eq!(p.len(), 2, "{p:?}");
            assert!(held(&p, 50, 50, 950, 650), "{p:?}");
            assert!(held(&p, 54, 700, 950, 1500), "bleeding panel in pieces: {p:?}");
        }
    }

    #[test]
    fn a_big_borderless_candidate_does_not_keep_the_page_whole() {
        let p = first_pass(big_borderless_candidate());
        assert!(p.len() >= 2, "page kept whole: {p:?}");
        assert!(held(&p, 50, 50, 950, 450), "{p:?}");
        assert!(p.iter().all(|p| p.rect.area() < 900 * 1400), "{p:?}");
    }

    #[test]
    fn a_row_joined_by_a_thin_gutter_is_split_in_the_second_round() {
        let first = first_pass(thin_gutter_row());
        assert!(first.iter().any(|p| p.rect.w > 800 && p.rect.h < 600 && p.issues.contains(&detect::Issue::Large)), "{first:?}");
        let p = run(thin_gutter_row());
        assert_eq!(p.len(), 3, "{p:?}");
        assert!(held(&p, 50, 50, 498, 550) && held(&p, 501, 50, 950, 550), "{p:?}");
    }

    #[test]
    fn a_tall_panel_is_not_cut_through_its_balloon() {
        let p = run(balloon_at_a_t_junction());
        assert!(held(&p, 40, 540, 204, 1040), "tall panel cut: {p:?}");
    }

    #[test]
    fn overlapping_borderless_pieces_are_one_panel() {
        let p = run(overlapping_borderless_pieces());
        assert!(held(&p, 400, 140, 690, 1090), "borderless panel in pieces: {p:?}");
        assert!(!held(&p, 30, 60, 690, 500), "{p:?}");
    }

    #[test]
    fn a_sketch_on_a_black_page_keeps_the_page() {
        let p = run(sketch_on_black());
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(held(&p, 0, 0, W, H), "cropped to the sketch: {p:?}");
    }

    #[test]
    fn an_l_panel_with_a_blank_wall_is_not_cut() {
        let p = run(l_panel_with_blank_wall());
        assert!(held(&p, 0, 380, 403, 1100), "L panel cut: {p:?}");
        assert!(held(&p, 409, 380, 660, 650) && !held(&p, 0, 380, 660, 650), "right column merged: {p:?}");
    }

    #[test]
    fn hairline_gutters_are_cut_in_a_hairline_book() {
        let img = hairline_gutters().image();
        assert!(crate::style::measure_page(&img).gutter_px.iter().any(|&w| w <= 3), "style pass misses the hairline");
        let plain = detect_styled(&img, false);
        assert!(held(&plain, 40, 420, 660, 1040), "without hairlines the row is one panel: {plain:?}");
        let p = detect_styled(&img, true);
        assert_eq!(p.len(), 4, "{p:?}");
        assert!(held(&p, 40, 420, 300, 1040) && !held(&p, 40, 420, 660, 700), "{p:?}");
        assert_eq!(crate::style::Tuning::for_book(&crate::style::Traits::from_pages(&[crate::style::measure_page(&img)]), None, &[], false).hairlines, true);
    }

    #[test]
    fn a_double_line_in_the_art_is_no_hairline_gutter() {
        let p = detect_styled(&double_line_in_art().image(), true);
        assert!(held(&p, 40, 420, 660, 1040), "panel cut along a ledge: {p:?}");
    }

    #[test]
    fn a_framed_panel_stays_apart_from_rain_below() {
        use detect::Issue;
        let img = rain_under_a_panel().image();
        let framed = Panel::new(Rect { x: 28, y: 28, w: 644, h: 330 }, None, vec![Issue::Joined]);
        let rain = Panel::new(Rect { x: 0, y: 290, w: 700, h: 810 }, None, vec![Issue::NoBorder, Issue::Large]);
        let (p, _) = crate::refine::refine_page(&img, vec![framed.clone(), rain.clone()], false, 0.0);
        assert!(held(&p, 30, 30, 670, 300) && !held(&p, 30, 30, 670, 600), "framed panel merged with the rain: {p:?}");
        // Pieces of the rain overlapping each other, with no frame between, are one panel.
        let a = Panel::new(Rect { x: 0, y: 300, w: 400, h: 800 }, None, vec![Issue::NoBorder]);
        let b = Panel::new(Rect { x: 300, y: 300, w: 400, h: 800 }, None, vec![Issue::Joined]);
        let (p, _) = crate::refine::refine_page(&img, vec![framed, a, b], false, 0.0);
        assert!(held(&p, 0, 300, 700, 1100), "rain in pieces: {p:?}");
    }

    fn preset(preset: crate::style::Preset, img: &DynamicImage) -> Vec<Panel> {
        let o = DetectOptions { threshold: 48, split_coverage: 0.35, min_panel_area: 0.01, padding: 0.0, exact_shapes: true, rtl: false, tuning: preset.tuning() };
        let mut panels = detect::detect_panels(img, &o);
        detect::drop_edge_strips(&mut panels, img.width(), img.height());
        crate::refine::refine_page(img, panels, false, 0.0).0
    }

    #[test]
    fn pale_colour_is_ink_in_a_colour_book() {
        use crate::style::Preset;
        let img = DynamicImage::ImageRgb8(pale_sky_panel());
        let grey = preset(Preset::Manga, &img);
        assert!(!held(&grey, 40, 420, 660, 700), "without colour ink the panel holds together: {grey:?}");
        let p = preset(Preset::BandeDessinee, &img);
        assert_eq!(p.len(), 3, "{p:?}");
        assert!(held(&p, 40, 420, 660, 700), "pale panel fell apart: {p:?}");
    }

    #[test]
    fn colour_ink_leaves_a_cover_alone() {
        // Printed to the edge: the border colour is the paper, and colour ink stays off.
        use crate::style::Preset;
        let img = DynamicImage::ImageRgb8(colour_splash());
        let rects = |p: Vec<Panel>| p.iter().map(|p| p.rect).collect::<Vec<_>>();
        assert_eq!(rects(preset(Preset::WesternModern, &img)), rects(preset(Preset::Manga, &img)));
    }

    #[test]
    fn a_rail_in_a_tall_panel_is_no_gutter() {
        let p = run(rail_in_a_tall_panel());
        assert!(held(&p, 450, 60, 660, 1040), "tall panel cut along its rail: {p:?}");
        assert_eq!(p.len(), 4, "{p:?}");
    }

    #[test]
    fn a_frame_line_is_not_cut_past_its_end() {
        let p = run(frame_line_stopping_short());
        assert!(held(&p, 40, 740, 660, 1040), "bottom panel cut: {p:?}");
        assert!(held(&first_pass(frame_line_stopping_short()), 40, 740, 660, 1040), "cut in the first pass");
    }

    /// The second round on one doubtful panel covering `r`.
    fn refine_one(page: Page, r: Rect) -> Vec<Panel> {
        let img = page.image();
        crate::refine::refine_page(&img, vec![Panel::new(r, None, vec![detect::Issue::Large])], false, 0.0).0
    }

    #[test]
    fn the_second_round_cuts_beside_a_tall_panel_first() {
        // The gutter of the stacked panels carried on across the tall panel's blank interior is
        // paper all along, but has no frame beside it there.
        let mut page = Page::new();
        page.panel(40, 60, 430, 380).panel(40, 400, 430, 720).frame(450, 60, 660, 720);
        page.fill(500, 120, 600, 300, ART).fill(500, 500, 600, 680, ART);
        let p = refine_one(page, Rect { x: 38, y: 58, w: 624, h: 664 });
        assert!(held(&p, 450, 60, 660, 720), "tall panel cut: {p:?}");
        assert!(held(&p, 40, 60, 430, 380) && !held(&p, 40, 60, 430, 720), "{p:?}");
    }

    #[test]
    fn the_second_round_keeps_a_picture_whole() {
        // One framed picture with a blank band across it between two drawn lines; the side
        // frames run straight across the band.
        let mut page = Page::new();
        page.panel(40, 60, 660, 1040);
        page.fill(43, 490, 657, 494, 0).fill(43, 494, 657, 506, 255).fill(43, 506, 657, 510, 0);
        let p = refine_one(page, Rect { x: 38, y: 58, w: 624, h: 984 });
        assert_eq!(p.len(), 1, "picture cut: {p:?}");
    }

    /// Writes every synthetic page to `SYNTH_OUT` (default `target/synthetic`), with the detected
    /// panels drawn on a copy: `scripts/synthetic_pages.sh`.
    #[test]
    #[ignore = "writes files; run through scripts/synthetic_pages.sh"]
    fn write_synthetic_pages() {
        let dir = std::path::PathBuf::from(std::env::var("SYNTH_OUT").unwrap_or_else(|_| "target/synthetic".into()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, build) in ALL {
            let img = build().image();
            img.save(dir.join(format!("{name}.png"))).unwrap();
            detect::debug_image(&img, &detect(&img)).save(dir.join(format!("{name}_detected.jpg"))).unwrap();
        }
        println!("wrote {} page(s) to {}", ALL.len(), dir.display());
    }
}
