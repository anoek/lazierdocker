//! Binary split tree for pane arrangement (technique from herdr's layout).

use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(pub u64);

/// `Horizontal` lays children out side by side (the divider is vertical).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir4 {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Pane(PaneId),
    Split {
        dir: SplitDir,
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SplitBorder {
    /// Column (Horizontal) or row (Vertical) of the shared divider.
    pub pos: u16,
    pub dir: SplitDir,
    /// The area occupied by the split node.
    pub area: Rect,
    /// Route from the root: false = first child, true = second child.
    pub path: Vec<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub root: Node,
    pub focus: PaneId,
    pub zoomed: bool,
}

pub const MIN_RATIO: f32 = 0.1;
pub const MAX_RATIO: f32 = 0.9;

pub fn clamp_ratio(r: f32) -> f32 {
    if r.is_nan() {
        0.5
    } else {
        r.clamp(MIN_RATIO, MAX_RATIO)
    }
}

fn split_rect(area: Rect, dir: SplitDir, ratio: f32) -> (Rect, Rect) {
    match dir {
        SplitDir::Horizontal => {
            let first_w = ((area.width as f32) * ratio).round() as u16;
            let first_w = first_w.min(area.width);
            let second_w = area.width - first_w;
            (
                Rect::new(area.x, area.y, first_w, area.height),
                Rect::new(area.x + first_w, area.y, second_w, area.height),
            )
        }
        SplitDir::Vertical => {
            let first_h = ((area.height as f32) * ratio).round() as u16;
            let first_h = first_h.min(area.height);
            let second_h = area.height - first_h;
            (
                Rect::new(area.x, area.y, area.width, first_h),
                Rect::new(area.x, area.y + first_h, area.width, second_h),
            )
        }
    }
}

impl Node {
    fn contains(&self, id: PaneId) -> bool {
        match self {
            Node::Pane(p) => *p == id,
            Node::Split { first, second, .. } => first.contains(id) || second.contains(id),
        }
    }

    fn collect_panes(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Pane(p) => out.push(*p),
            Node::Split { first, second, .. } => {
                first.collect_panes(out);
                second.collect_panes(out);
            }
        }
    }

    fn collect_rects(&self, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Node::Pane(p) => out.push((*p, area)),
            Node::Split {
                dir,
                ratio,
                first,
                second,
            } => {
                let (a, b) = split_rect(area, *dir, *ratio);
                first.collect_rects(a, out);
                second.collect_rects(b, out);
            }
        }
    }

    fn collect_splits(&self, area: Rect, path: &mut Vec<bool>, out: &mut Vec<SplitBorder>) {
        if let Node::Split {
            dir,
            ratio,
            first,
            second,
        } = self
        {
            let (a, b) = split_rect(area, *dir, *ratio);
            let pos = match dir {
                SplitDir::Horizontal => b.x,
                SplitDir::Vertical => b.y,
            };
            out.push(SplitBorder {
                pos,
                dir: *dir,
                area,
                path: path.clone(),
            });
            path.push(false);
            first.collect_splits(a, path, out);
            path.pop();
            path.push(true);
            second.collect_splits(b, path, out);
            path.pop();
        }
    }

    fn at_path_mut(&mut self, path: &[bool]) -> Option<&mut Node> {
        match path.split_first() {
            None => Some(self),
            Some((go_second, rest)) => match self {
                Node::Split { first, second, .. } => {
                    if *go_second {
                        second.at_path_mut(rest)
                    } else {
                        first.at_path_mut(rest)
                    }
                }
                Node::Pane(_) => None,
            },
        }
    }

    fn replace_leaf(&mut self, target: PaneId, f: &mut dyn FnMut(PaneId) -> Node) -> bool {
        match self {
            Node::Pane(p) if *p == target => {
                *self = f(target);
                true
            }
            Node::Pane(_) => false,
            Node::Split { first, second, .. } => {
                first.replace_leaf(target, f) || second.replace_leaf(target, f)
            }
        }
    }

    /// Remove `id`, collapsing its parent into the sibling. Returns true if removed.
    fn remove(&mut self, id: PaneId) -> bool {
        let Node::Split { first, second, .. } = self else {
            return false;
        };
        if matches!(**first, Node::Pane(p) if p == id) {
            let sibling = std::mem::replace(second.as_mut(), Node::Pane(id));
            *self = sibling;
            return true;
        }
        if matches!(**second, Node::Pane(p) if p == id) {
            let sibling = std::mem::replace(first.as_mut(), Node::Pane(id));
            *self = sibling;
            return true;
        }
        first.remove(id) || second.remove(id)
    }

    fn swap_ids(&mut self, a: PaneId, b: PaneId) {
        match self {
            Node::Pane(p) => {
                if *p == a {
                    *p = b;
                } else if *p == b {
                    *p = a;
                }
            }
            Node::Split { first, second, .. } => {
                first.swap_ids(a, b);
                second.swap_ids(a, b);
            }
        }
    }
}

impl Layout {
    pub fn new(id: PaneId) -> Self {
        Self {
            root: Node::Pane(id),
            focus: id,
            zoomed: false,
        }
    }

    pub fn contains(&self, id: PaneId) -> bool {
        self.root.contains(id)
    }

    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.root.collect_panes(&mut out);
        out
    }

    /// Split `target`, placing `new` after it (right of / below). Focuses `new`.
    pub fn split(&mut self, target: PaneId, dir: SplitDir, new: PaneId) -> bool {
        let ok = self.root.replace_leaf(target, &mut |t| Node::Split {
            dir,
            ratio: 0.5,
            first: Box::new(Node::Pane(t)),
            second: Box::new(Node::Pane(new)),
        });
        if ok {
            self.focus = new;
            self.zoomed = false;
        }
        ok
    }

    /// Remove a pane. The last pane cannot be removed.
    pub fn remove(&mut self, id: PaneId) -> bool {
        if matches!(self.root, Node::Pane(_)) {
            return false;
        }
        let removed = self.root.remove(id);
        if removed && self.focus == id {
            self.focus = self.panes()[0];
        }
        if removed && self.panes().len() == 1 {
            self.zoomed = false;
        }
        removed
    }

    /// Pane rects tiling `area`. When zoomed, only the focused pane, covering `area`.
    pub fn rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        if self.zoomed {
            return vec![(self.focus, area)];
        }
        let mut out = Vec::new();
        self.root.collect_rects(area, &mut out);
        out
    }

    pub fn splits(&self, area: Rect) -> Vec<SplitBorder> {
        if self.zoomed {
            return Vec::new();
        }
        let mut out = Vec::new();
        self.root.collect_splits(area, &mut Vec::new(), &mut out);
        out
    }

    pub fn set_ratio(&mut self, path: &[bool], ratio: f32) {
        if let Some(Node::Split { ratio: r, .. }) = self.root.at_path_mut(path) {
            *r = clamp_ratio(ratio);
        }
    }

    pub fn swap(&mut self, a: PaneId, b: PaneId) {
        self.root.swap_ids(a, b);
    }

    pub fn toggle_zoom(&mut self) {
        if self.panes().len() > 1 {
            self.zoomed = !self.zoomed;
        } else {
            self.zoomed = false;
        }
    }

    /// Nearest pane strictly on side `dir` of `from` that overlaps it on the
    /// perpendicular axis (herdr's directional focus rule).
    pub fn neighbour(&self, area: Rect, from: PaneId, dir: Dir4) -> Option<PaneId> {
        let mut all = Vec::new();
        self.root.collect_rects(area, &mut all);
        let (_, cur) = *all.iter().find(|(id, _)| *id == from)?;
        let mut best: Option<((i32, i32, i32, usize), PaneId)> = None;
        for (idx, (id, r)) in all.iter().enumerate() {
            if *id == from {
                continue;
            }
            let (edge_dist, overlap, centre) = match dir {
                Dir4::Left | Dir4::Right => {
                    let side_ok = if dir == Dir4::Left {
                        r.right() <= cur.x
                    } else {
                        r.x >= cur.right()
                    };
                    if !side_ok {
                        continue;
                    }
                    let overlap = overlap(cur.y, cur.bottom(), r.y, r.bottom());
                    let edge = if dir == Dir4::Left {
                        cur.x as i32 - r.right() as i32
                    } else {
                        r.x as i32 - cur.right() as i32
                    };
                    let centre = ((r.y as i32 * 2 + r.height as i32)
                        - (cur.y as i32 * 2 + cur.height as i32))
                        .abs();
                    (edge, overlap, centre)
                }
                Dir4::Up | Dir4::Down => {
                    let side_ok = if dir == Dir4::Up {
                        r.bottom() <= cur.y
                    } else {
                        r.y >= cur.bottom()
                    };
                    if !side_ok {
                        continue;
                    }
                    let overlap = overlap(cur.x, cur.right(), r.x, r.right());
                    let edge = if dir == Dir4::Up {
                        cur.y as i32 - r.bottom() as i32
                    } else {
                        r.y as i32 - cur.bottom() as i32
                    };
                    let centre = ((r.x as i32 * 2 + r.width as i32)
                        - (cur.x as i32 * 2 + cur.width as i32))
                        .abs();
                    (edge, overlap, centre)
                }
            };
            if overlap <= 0 {
                continue;
            }
            let key = (edge_dist, -overlap, centre, idx);
            if best.as_ref().is_none_or(|(k, _)| key < *k) {
                best = Some((key, *id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Move the divider nearest the focused pane's `dir` edge by `delta` (ratio units).
    pub fn resize_focused(&mut self, area: Rect, dir: Dir4, delta: f32) {
        let rects = self.rects(area);
        let Some((_, cur)) = rects.iter().find(|(id, _)| *id == self.focus).copied() else {
            return;
        };
        let want = match dir {
            Dir4::Left | Dir4::Right => SplitDir::Horizontal,
            Dir4::Up | Dir4::Down => SplitDir::Vertical,
        };
        // Prefer the divider on the requested edge; fall back to the opposite edge.
        let (edge_a, edge_b) = match dir {
            Dir4::Right => (cur.right(), cur.x),
            Dir4::Left => (cur.x, cur.right()),
            Dir4::Down => (cur.bottom(), cur.y),
            Dir4::Up => (cur.y, cur.bottom()),
        };
        let splits = self.splits(area);
        let touches = |s: &SplitBorder, edge: u16| {
            s.dir == want
                && s.pos.abs_diff(edge) <= 1
                && match want {
                    SplitDir::Horizontal => {
                        overlap(cur.y, cur.bottom(), s.area.y, s.area.bottom()) > 0
                    }
                    SplitDir::Vertical => overlap(cur.x, cur.right(), s.area.x, s.area.right()) > 0,
                }
        };
        let target = splits
            .iter()
            .find(|s| touches(s, edge_a))
            .or_else(|| splits.iter().find(|s| touches(s, edge_b)));
        if let Some(s) = target.cloned() {
            let sign = match dir {
                Dir4::Right | Dir4::Down => 1.0,
                Dir4::Left | Dir4::Up => -1.0,
            };
            if let Some(Node::Split { ratio, .. }) = self.root.at_path_mut(&s.path) {
                *ratio = clamp_ratio(*ratio + sign * delta);
            }
        }
    }
}

fn overlap(a0: u16, a1: u16, b0: u16, b1: u16) -> i32 {
    a1.min(b1) as i32 - a0.max(b0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: PaneId = PaneId(1);
    const B: PaneId = PaneId(2);
    const C: PaneId = PaneId(3);
    const D: PaneId = PaneId(4);

    fn area() -> Rect {
        Rect::new(0, 0, 100, 40)
    }

    #[test]
    fn split_and_rects_cover_area() {
        let mut l = Layout::new(A);
        assert!(l.split(A, SplitDir::Horizontal, B));
        assert!(l.split(B, SplitDir::Vertical, C));
        let rects = l.rects(area());
        assert_eq!(rects.len(), 3);
        let total: u32 = rects
            .iter()
            .map(|(_, r)| r.width as u32 * r.height as u32)
            .sum();
        assert_eq!(total, 100 * 40);
        assert_eq!(rects[0], (A, Rect::new(0, 0, 50, 40)));
        assert_eq!(rects[1], (B, Rect::new(50, 0, 50, 20)));
        assert_eq!(rects[2], (C, Rect::new(50, 20, 50, 20)));
        assert_eq!(l.focus, C);
    }

    #[test]
    fn remove_collapses_parent() {
        let mut l = Layout::new(A);
        l.split(A, SplitDir::Horizontal, B);
        l.split(B, SplitDir::Vertical, C);
        assert!(l.remove(B));
        assert_eq!(l.panes(), vec![A, C]);
        assert_eq!(l.rects(area())[1], (C, Rect::new(50, 0, 50, 40)));
        assert!(l.remove(C));
        assert!(!l.remove(A), "last pane cannot be removed");
        assert_eq!(l.focus, A);
    }

    #[test]
    fn ratio_clamped() {
        let mut l = Layout::new(A);
        l.split(A, SplitDir::Horizontal, B);
        l.set_ratio(&[], 0.01);
        assert_eq!(l.splits(area())[0].pos, 10);
        l.set_ratio(&[], 5.0);
        assert_eq!(l.splits(area())[0].pos, 90);
        l.set_ratio(&[], f32::NAN);
        assert_eq!(l.splits(area())[0].pos, 50);
    }

    #[test]
    fn splits_report_paths() {
        let mut l = Layout::new(A);
        l.split(A, SplitDir::Horizontal, B);
        l.split(B, SplitDir::Vertical, C);
        let s = l.splits(area());
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].pos, s[0].path.clone()), (50, vec![]));
        assert_eq!(
            (s[1].pos, s[1].path.clone(), s[1].dir),
            (20, vec![true], SplitDir::Vertical)
        );
    }

    #[test]
    fn neighbour_prefers_overlap() {
        // A | (B over C); from C going left is A; from A going right picks B (tie on
        // overlap/edge broken by index) ; from B going down is C.
        let mut l = Layout::new(A);
        l.split(A, SplitDir::Horizontal, B);
        l.split(B, SplitDir::Vertical, C);
        assert_eq!(l.neighbour(area(), C, Dir4::Left), Some(A));
        assert_eq!(l.neighbour(area(), B, Dir4::Down), Some(C));
        assert_eq!(l.neighbour(area(), A, Dir4::Right), Some(B));
        assert_eq!(l.neighbour(area(), A, Dir4::Left), None);
        l.set_ratio(&[true], 0.3);
        // C now has the larger overlap with A's right edge.
        assert_eq!(l.neighbour(area(), A, Dir4::Right), Some(C));
    }

    #[test]
    fn resize_focused_moves_nearest_divider() {
        let mut l = Layout::new(A);
        l.split(A, SplitDir::Horizontal, B);
        l.focus = A;
        l.resize_focused(area(), Dir4::Right, 0.1);
        assert_eq!(l.splits(area())[0].pos, 60);
        l.focus = B;
        l.resize_focused(area(), Dir4::Left, 0.1);
        assert_eq!(l.splits(area())[0].pos, 50);
    }

    #[test]
    fn zoom_shows_only_focus() {
        let mut l = Layout::new(A);
        l.toggle_zoom();
        assert!(!l.zoomed, "single pane cannot zoom");
        l.split(A, SplitDir::Horizontal, B);
        l.toggle_zoom();
        assert_eq!(l.rects(area()), vec![(B, area())]);
        assert!(l.splits(area()).is_empty());
    }

    #[test]
    fn swap_exchanges_positions() {
        let mut l = Layout::new(A);
        l.split(A, SplitDir::Horizontal, B);
        l.swap(A, B);
        assert_eq!(l.panes(), vec![B, A]);
    }

    #[test]
    fn layout_tiny_area_no_panic() {
        let mut l = Layout::new(A);
        l.split(A, SplitDir::Horizontal, B);
        l.split(B, SplitDir::Vertical, C);
        l.split(C, SplitDir::Horizontal, D);
        for (w, h) in [(0, 0), (1, 1), (3, 2), (5, 0)] {
            let a = Rect::new(0, 0, w, h);
            let _ = l.rects(a);
            let _ = l.splits(a);
            for dir in [Dir4::Left, Dir4::Right, Dir4::Up, Dir4::Down] {
                let _ = l.neighbour(a, C, dir);
                l.resize_focused(a, dir, 0.05);
            }
        }
    }
}
