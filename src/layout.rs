//! Pack detected panels onto Kindle pages.
//!
//! Readability rule: a panel must be shown at least `readable_scale` times its
//! printed size. With the screen's ppi known, that gives a minimum number of
//! screen pixels per source pixel. Panels are kept at that size (or as big as
//! the screen allows) and stacked until the page is full, then each page is
//! scaled up uniformly to use the leftover space.
//!
//! With `overview`, each source page is first shown whole on its own page,
//! followed by its panels. With `cover`, the first source page is shown whole and
//! not split, so it works as the book's cover.

use crate::detect::Rect;

#[derive(Clone, Debug)]
pub struct SourcePage {
    pub width: u32,
    pub height: u32,
    /// Show the panels at the screen's width instead of their printed size (webtoon pieces,
    /// which have no printed page to measure against).
    pub fit_width: bool,
    pub panels: Vec<Rect>,
}

#[derive(Clone, Debug)]
pub struct LayoutOptions {
    pub screen_w: u32,
    pub screen_h: u32,
    pub ppi: u32,
    pub margin: u32,
    pub gap: u32,
    /// Minimum on-screen size relative to print (1.0 = same size as the paper comic).
    pub readable_scale: f32,
    /// Physical height of the printed source page in inches.
    pub print_height_in: f32,
    pub max_upscale: f32,
    pub rotate: bool,
    pub page_break: bool,
    /// Show each source page whole before its panels.
    pub overview: bool,
    /// Show the first source page whole, without its panels.
    pub cover: bool,
    pub rtl: bool,
}

#[derive(Clone, Debug)]
pub struct Placement {
    pub page: usize,
    /// Index of the panel within its source page; `None` for the whole page (overview).
    pub panel: Option<usize>,
    pub src: Rect,
    pub x: u32,
    pub y: u32,
    /// Final on-canvas size (after rotation).
    pub w: u32,
    pub h: u32,
    pub rotated: bool,
}

#[derive(Clone, Debug, Default)]
pub struct OutPage {
    pub items: Vec<Placement>,
}

struct Unit {
    page: usize,
    /// Panel index of `panels[0]`; a unit always holds consecutive panels.
    first: usize,
    panels: Vec<Rect>,
    scale: f32,
    rotated: bool,
    /// The whole source page (overview), always alone on its e-reader page.
    whole: bool,
}

impl Unit {
    fn size(&self, gap: f32) -> (f32, f32) {
        if self.rotated {
            let p = self.panels[0];
            return (p.h as f32 * self.scale, p.w as f32 * self.scale);
        }
        let w: f32 = self.panels.iter().map(|p| p.w as f32 * self.scale).sum::<f32>()
            + gap * (self.panels.len() - 1) as f32;
        let h = self.panels.iter().map(|p| p.h as f32 * self.scale).fold(0.0, f32::max);
        (w, h)
    }
}

pub fn layout(pages: &[SourcePage], o: &LayoutOptions) -> Vec<OutPage> {
    let uw = o.screen_w.saturating_sub(2 * o.margin).max(1) as f32;
    let uh = o.screen_h.saturating_sub(2 * o.margin).max(1) as f32;
    let gap = o.gap as f32;

    let mut units = Vec::new();
    for (pi, sp) in pages.iter().enumerate() {
        // Screen pixels per source pixel needed for readability.
        let target = if sp.fit_width {
            uw / sp.width as f32
        } else {
            o.readable_scale * o.print_height_in * o.ppi as f32 / sp.height as f32
        };
        let page = Rect { x: 0, y: 0, w: sp.width, h: sp.height };
        let cover = o.cover && pi == 0;
        if o.overview || cover {
            let (w, h) = (sp.width as f32, sp.height as f32);
            let s = (uw / w).min(uh / h);
            let sr = (uw / h).min(uh / w);
            let rotated = o.rotate && w > h && sr > s * 1.2;
            let scale = if rotated { sr } else { s };
            units.push(Unit { page: pi, first: 0, panels: vec![page], scale, rotated, whole: true });
            if cover {
                continue;
            }
            // A single panel covering most of the page would only repeat the overview.
            if let [p] = sp.panels[..] {
                if p.area() as f32 > 0.9 * page.area() as f32 {
                    continue;
                }
            }
        }
        let mut first = 0;
        for row in group_rows(&sp.panels) {
            let row_first = first;
            first += row.len();
            if row.len() > 1 {
                let sum_w: f32 = row.iter().map(|p| p.w as f32).sum();
                let max_h = row.iter().map(|p| p.h as f32).fold(0.0, f32::max);
                let s = target
                    .min((uw - gap * (row.len() - 1) as f32) / sum_w)
                    .min(uh / max_h);
                // Keep panels side by side when that costs at most 10% of the target size.
                if s >= target * 0.9 {
                    units.push(Unit { page: pi, first: row_first, panels: row, scale: s, rotated: false, whole: false });
                    continue;
                }
            }
            for (j, p) in row.into_iter().enumerate() {
                let first = row_first + j;
                let (w, h) = (p.w as f32, p.h as f32);
                let s = target.min(uw / w).min(uh / h);
                if o.rotate && s < target * 0.95 && w > h {
                    let sr = target.min(uw / h).min(uh / w);
                    if sr > s * 1.2 {
                        units.push(Unit { page: pi, first, panels: vec![p], scale: sr, rotated: true, whole: false });
                        continue;
                    }
                }
                units.push(Unit { page: pi, first, panels: vec![p], scale: s, rotated: false, whole: false });
            }
        }
    }

    let mut out = Vec::new();
    let mut cur: Vec<Unit> = Vec::new();
    let mut cur_h = 0.0;
    for u in units {
        let (_, h) = u.size(gap);
        if let Some(last) = cur.last() {
            let brk = u.rotated
                || last.rotated
                || u.whole
                || last.whole
                || (o.page_break && last.page != u.page)
                || cur_h + gap + h > uh;
            if brk {
                out.push(place(std::mem::take(&mut cur), o, uw, uh));
                cur_h = 0.0;
            }
        }
        cur_h = if cur.is_empty() { h } else { cur_h + gap + h };
        cur.push(u);
    }
    if !cur.is_empty() {
        out.push(place(cur, o, uw, uh));
    }
    out
}

/// Group consecutive panels that share a horizontal band (same row on the page).
fn group_rows(panels: &[Rect]) -> Vec<Vec<Rect>> {
    let mut rows: Vec<Vec<Rect>> = Vec::new();
    for &p in panels {
        if let Some(row) = rows.last_mut() {
            let top = row.iter().map(|r| r.y).min().unwrap();
            let bot = row.iter().map(|r| r.bottom()).max().unwrap();
            let overlap = bot.min(p.bottom()).saturating_sub(top.max(p.y));
            if overlap as f32 >= 0.6 * p.h.min(bot - top) as f32 {
                row.push(p);
                continue;
            }
        }
        rows.push(vec![p]);
    }
    rows
}

fn place(units: Vec<Unit>, o: &LayoutOptions, uw: f32, uh: f32) -> OutPage {
    let gap = o.gap as f32;
    let sizes: Vec<(f32, f32)> = units.iter().map(|u| u.size(gap)).collect();
    let n = units.len() as f32;
    let max_w = sizes.iter().map(|s| s.0).fold(0.0, f32::max);
    let sum_h: f32 = sizes.iter().map(|s| s.1).sum();
    // Gaps between panels inside a row don't scale, so solve for width conservatively.
    let k = (uw / max_w)
        .min((uh - gap * (n - 1.0)) / sum_h)
        .min(o.max_upscale)
        .max(0.01);

    let total_h = sum_h * k + gap * (n - 1.0);
    let mut y = o.margin as f32 + ((uh - total_h) / 2.0).max(0.0);
    let mut items = Vec::new();
    for (u, &(_, uh_unit)) in units.iter().zip(&sizes) {
        let s = u.scale * k;
        let row_h = uh_unit * k;
        let dims: Vec<(f32, f32)> = u
            .panels
            .iter()
            .map(|p| {
                if u.rotated {
                    (p.h as f32 * s, p.w as f32 * s)
                } else {
                    (p.w as f32 * s, p.h as f32 * s)
                }
            })
            .collect();
        let row_w: f32 = dims.iter().map(|d| d.0).sum::<f32>() + gap * (dims.len() - 1) as f32;
        let mut x = o.margin as f32 + ((uw - row_w) / 2.0).max(0.0);
        let mut order: Vec<usize> = (0..u.panels.len()).collect();
        if o.rtl {
            order.reverse();
        }
        for i in order {
            let (w, h) = dims[i];
            let (w, h) = (w.round().max(1.0) as u32, h.round().max(1.0) as u32);
            items.push(Placement {
                page: u.page,
                panel: (!u.whole).then_some(u.first + i),
                src: u.panels[i],
                x: x.round() as u32,
                y: (y + (row_h - h as f32) / 2.0).round() as u32,
                w: w.min(o.screen_w),
                h: h.min(o.screen_h),
                rotated: u.rotated,
            });
            x += dims[i].0 + gap;
        }
        y += row_h + gap;
    }
    // Keep source reading order in the item list regardless of on-screen position.
    if o.rtl {
        items.sort_by_key(|p| (p.y, std::cmp::Reverse(p.x)));
    }
    OutPage { items }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(overview: bool) -> LayoutOptions {
        LayoutOptions {
            screen_w: 1000,
            screen_h: 1400,
            ppi: 300,
            margin: 0,
            gap: 10,
            readable_scale: 0.8,
            print_height_in: 10.0,
            max_upscale: 2.0,
            rotate: false,
            page_break: false,
            overview,
            cover: false,
            rtl: false,
        }
    }

    fn page(panels: Vec<Rect>) -> SourcePage {
        SourcePage { width: 2000, height: 3000, fit_width: false, panels }
    }

    #[test]
    fn overview_puts_each_page_before_its_panels() {
        let half = |y| Rect { x: 0, y, w: 2000, h: 1400 };
        let pages = [page(vec![half(0), half(1500)]), page(vec![half(0), half(1500)])];
        let out = layout(&pages, &opts(true));
        let seq: Vec<Vec<(usize, Option<usize>)>> =
            out.iter().map(|p| p.items.iter().map(|it| (it.page, it.panel)).collect()).collect();
        // Each overview is alone on its e-reader page.
        assert!(seq.iter().all(|p| p.len() == 1 || p.iter().all(|it| it.1.is_some())));
        let order: Vec<(usize, Option<usize>)> = seq.concat();
        let pos = |x| order.iter().position(|&o| o == x).unwrap();
        assert!(pos((0, None)) < pos((0, Some(0))) && pos((0, Some(1))) < pos((1, None)));
        assert!(pos((1, None)) < pos((1, Some(0))));
    }

    #[test]
    fn cover_is_whole_and_not_split() {
        let half = |y| Rect { x: 0, y, w: 2000, h: 1400 };
        let pages = [page(vec![half(0), half(1500)]), page(vec![half(0), half(1500)])];
        let out = layout(&pages, &LayoutOptions { cover: true, ..opts(false) });
        let items: Vec<(usize, Option<usize>)> = out.iter().flat_map(|p| p.items.iter().map(|it| (it.page, it.panel))).collect();
        assert_eq!(items[0], (0, None));
        assert_eq!(out[0].items.len(), 1, "the cover is alone on its page");
        assert!(!items.contains(&(0, Some(0))), "the cover's panels are not repeated");
        assert!(items.contains(&(1, Some(0))) && items.contains(&(1, Some(1))));
    }

    #[test]
    fn overview_skips_panels_that_repeat_the_page() {
        let pages = [page(vec![Rect { x: 10, y: 10, w: 1980, h: 2980 }])];
        let out = layout(&pages, &opts(true));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].items[0].panel, None);
        assert!(layout(&pages, &opts(false))[0].items.iter().all(|it| it.panel == Some(0)));
    }
}
