//! Page layouts learned from the book.
//!
//! A book reuses a handful of page layouts: two rows of two, a tall column beside stacked panels,
//! a wide panel over three small ones. The pages detected with confidence teach those layouts, as
//! [`Template`]s of panel boxes relative to the page. A page whose panels fit a template, except
//! that two or more of them fill one of its cells together, has a panel cut in two (along art
//! that looked like a gutter): they are merged back. Templates only ever merge, never cut, and
//! only pieces that came from a doubtful cut.

use crate::detect::{Issue, Panel, Rect};

/// A box relative to the page: x0, y0, x1, y1 in 0..=1.
type Cell = [f32; 4];

/// Panels per page a template may have.
const MIN_CELLS: usize = 2;
const MAX_CELLS: usize = 9;
/// Two layouts are the same when every box of one matches a box of the other this closely.
const SAME: f32 = 0.8;
/// A page fits a template when each panel lies this much in one cell, and each cell's panels
/// fill it this well together.
const IN_CELL: f32 = 0.7;
const FIT: f32 = 0.8;
/// A template seen on one page only must fit this well.
const FIT_ONCE: f32 = 0.9;

#[derive(Clone, Debug)]
pub struct Template {
    cells: Vec<Cell>,
    /// Pages the layout was seen on.
    seen: usize,
}

fn cell(r: &Rect, w: u32, h: u32) -> Cell {
    let (w, h) = (w as f32, h as f32);
    [r.x as f32 / w, r.y as f32 / h, r.right() as f32 / w, r.bottom() as f32 / h]
}

fn area(c: &Cell) -> f32 {
    (c[2] - c[0]).max(0.0) * (c[3] - c[1]).max(0.0)
}

fn overlap(a: &Cell, b: &Cell) -> f32 {
    area(&[a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])])
}

fn iou(a: &Cell, b: &Cell) -> f32 {
    let i = overlap(a, b);
    i / (area(a) + area(b) - i).max(1e-6)
}

fn union(a: &Cell, b: &Cell) -> Cell {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

impl Template {
    fn same(&self, cells: &[Cell]) -> bool {
        self.cells.len() == cells.len() && cells.iter().all(|c| self.cells.iter().any(|t| iou(c, t) >= SAME))
    }
}

/// A page detected with confidence: every panel sure, and none merged from pieces of ink.
fn confident(panels: &[Panel]) -> bool {
    (MIN_CELLS..=MAX_CELLS).contains(&panels.len())
        && panels.iter().all(|p| !p.is_low() && !p.issues.iter().any(|i| matches!(i, Issue::Joined | Issue::KeptWhole)))
}

/// The layouts of the confidently detected pages of a book (`pages`: width, height, panels).
pub fn learn(pages: &[(u32, u32, &[Panel])]) -> Vec<Template> {
    let mut lib: Vec<Template> = Vec::new();
    for &(w, h, panels) in pages {
        if !confident(panels) {
            continue;
        }
        let cells: Vec<Cell> = panels.iter().map(|p| cell(&p.rect, w, h)).collect();
        match lib.iter_mut().find(|t| t.same(&cells)) {
            Some(t) => t.seen += 1,
            None => lib.push(Template { cells, seen: 1 }),
        }
    }
    lib
}

/// How `panels` fit `t`: each panel's cell and the mean fill of the cells, or `None`.
fn fit(t: &Template, cells: &[Cell]) -> Option<(Vec<usize>, f32)> {
    let mut assign = Vec::with_capacity(cells.len());
    for c in cells {
        let (k, share) = t
            .cells
            .iter()
            .enumerate()
            .map(|(k, tc)| (k, overlap(c, tc) / area(c).max(1e-6)))
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        if share < IN_CELL {
            return None;
        }
        assign.push(k);
    }
    let mut total = 0.0;
    for (k, tc) in t.cells.iter().enumerate() {
        let group = cells.iter().zip(&assign).filter(|&(_, &a)| a == k).map(|(c, _)| *c);
        let u = group.reduce(|a, b| union(&a, &b))?;
        total += iou(&u, tc);
    }
    Some((assign, total / t.cells.len() as f32))
}

/// Merge the panels of a page (`w` x `h`) that together fill one cell of the template it fits
/// best. Returns how many panels were merged away.
pub fn apply(panels: &mut Vec<Panel>, w: u32, h: u32, lib: &[Template]) -> usize {
    if panels.len() < 2 {
        return 0;
    }
    let cells: Vec<Cell> = panels.iter().map(|p| cell(&p.rect, w, h)).collect();
    // The best fit among the templates with fewer cells than the page has panels (a panel cut in
    // two adds one), or as many (the page fits already: nothing to do).
    let best = lib
        .iter()
        .filter(|t| t.cells.len() <= panels.len())
        .filter_map(|t| fit(t, &cells).map(|(assign, score)| (t, assign, score)))
        .filter(|(t, _, score)| *score >= if t.seen > 1 { FIT } else { FIT_ONCE })
        .max_by(|a, b| a.2.total_cmp(&b.2));
    let Some((t, assign, _)) = best else { return 0 };
    if t.cells.len() == panels.len() {
        return 0;
    }
    // Only pieces the second round cut are merged: its cuts are the doubtful ones, along lines
    // that only look like gutters. (The first pass cuts along white gutters; boxes cannot tell its
    // panels from pieces, since panel boxes overlap across a narrow gutter too.)
    let doubtful = |p: &Panel| p.issues.contains(&Issue::Refined);
    let mut merged = 0;
    let mut out: Vec<Panel> = Vec::new();
    let mut taken = vec![false; panels.len()];
    for k in 0..t.cells.len() {
        let group: Vec<usize> = (0..panels.len()).filter(|&i| assign[i] == k).collect();
        if group.len() >= 2 && group.iter().all(|&i| doubtful(&panels[i])) {
            let mut rect = panels[group[0]].rect;
            for &i in &group[1..] {
                let r = panels[i].rect;
                let (x, y) = (rect.x.min(r.x), rect.y.min(r.y));
                rect = Rect { x, y, w: rect.right().max(r.right()) - x, h: rect.bottom().max(r.bottom()) - y };
            }
            let mut p = Panel::new(rect, None, vec![Issue::Template]);
            for &i in &group {
                taken[i] = true;
                for &issue in &panels[i].issues {
                    if matches!(issue, Issue::KeptWhole) {
                        p.add_issue(issue);
                    }
                }
            }
            merged += group.len() - 1;
            out.push(p);
        }
    }
    if merged == 0 {
        return 0;
    }
    out.extend(panels.iter().enumerate().filter(|&(i, _)| !taken[i]).map(|(_, p)| p.clone()));
    *panels = out;
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(x: u32, y: u32, w: u32, h: u32, issues: Vec<Issue>) -> Panel {
        Panel::new(Rect { x, y, w, h }, None, issues)
    }

    /// A tall right column beside two stacked panels, on a 700 x 1100 page.
    fn tall_column(issues: Vec<Issue>) -> Vec<Panel> {
        vec![panel(40, 60, 380, 480, vec![]), panel(40, 560, 380, 480, vec![]), panel(440, 60, 220, 980, issues)]
    }

    #[test]
    fn learns_repeated_layouts_from_confident_pages() {
        let a = tall_column(vec![]);
        let b = tall_column(vec![]);
        let doubtful = vec![panel(40, 60, 620, 980, vec![Issue::Large, Issue::Notched])];
        let lib = learn(&[(700, 1100, &a), (700, 1100, &b), (700, 1100, &doubtful)]);
        assert_eq!(lib.len(), 1);
        assert_eq!(lib[0].seen, 2);
    }

    #[test]
    fn merges_a_panel_cut_in_two_back_to_the_layout() {
        let lib = learn(&[(700, 1100, &tall_column(vec![])), (700, 1100, &tall_column(vec![]))]);
        // The tall panel cut level with the gutter of the stacked ones.
        let mut page = vec![
            panel(40, 60, 380, 480, vec![]),
            panel(40, 560, 380, 480, vec![]),
            panel(440, 60, 220, 485, vec![Issue::Refined]),
            panel(440, 545, 220, 495, vec![Issue::Refined]),
        ];
        assert_eq!(apply(&mut page, 700, 1100, &lib), 1);
        assert_eq!(page.len(), 3);
        assert!(page.iter().any(|p| p.rect == Rect { x: 440, y: 60, w: 220, h: 980 }), "{page:?}");
    }

    #[test]
    fn leaves_a_sure_cut_and_other_layouts_alone() {
        let lib = learn(&[(700, 1100, &tall_column(vec![])), (700, 1100, &tall_column(vec![]))]);
        // Cut along a clean gutter: no doubt, no merge.
        let mut sure = vec![
            panel(40, 60, 380, 480, vec![]),
            panel(40, 560, 380, 480, vec![]),
            panel(440, 60, 220, 485, vec![]),
            panel(440, 545, 220, 495, vec![]),
        ];
        assert_eq!(apply(&mut sure, 700, 1100, &lib), 0);
        // A different layout (three rows) fits no template.
        let mut rows = vec![
            panel(40, 60, 620, 300, vec![Issue::Split]),
            panel(40, 380, 620, 300, vec![Issue::Split]),
            panel(40, 700, 620, 340, vec![Issue::Split]),
        ];
        assert_eq!(apply(&mut rows, 700, 1100, &lib), 0);
    }
}
