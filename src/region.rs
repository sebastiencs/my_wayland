use crate::transform::RectF;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl RectI {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Option<Self> {
        if width < 0 || height < 0 {
            return None;
        }
        Some(Self { x, y, width, height })
    }

    pub fn from_extents(x1: i32, y1: i32, x2: i32, y2: i32) -> Option<Self> {
        let width = x2.checked_sub(x1)?;
        let height = y2.checked_sub(y1)?;
        Self::new(x1, y1, width, height)
    }

    pub fn from_f32_bounds(rect: RectF) -> Option<Self> {
        let x1 = rect.x.floor() as i32;
        let y1 = rect.y.floor() as i32;
        let x2 = (rect.x + rect.width).ceil() as i32;
        let y2 = (rect.y + rect.height).ceil() as i32;
        Self::from_extents(x1, y1, x2, y2)
    }

    pub fn is_empty(self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    pub fn x2(self) -> i32 {
        self.x.saturating_add(self.width)
    }

    pub fn y2(self) -> i32 {
        self.y.saturating_add(self.height)
    }

    pub fn contains(self, x: i32, y: i32) -> bool {
        if self.is_empty() {
            return false;
        }
        let x2 = self.x2();
        let y2 = self.y2();
        x >= self.x && y >= self.y && x < x2 && y < y2
    }

    pub fn intersects(self, other: Self) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.x < other.x2()
            && self.x2() > other.x
            && self.y < other.y2()
            && self.y2() > other.y
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        if !self.intersects(other) {
            return None;
        }
        Self::from_extents(
            self.x.max(other.x),
            self.y.max(other.y),
            self.x2().min(other.x2()),
            self.y2().min(other.y2()),
        )
    }

    pub fn translate(self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DamageRegion {
    rects: Vec<RectI>,
}

impl DamageRegion {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_rect(rect: RectI) -> Self {
        let mut region = Self::new();
        region.add_rect(rect);
        region
    }

    pub fn rects(&self) -> &[RectI] {
        &self.rects
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub fn clear(&mut self) {
        self.rects.clear();
    }

    pub fn extents(&self) -> Option<RectI> {
        let mut iter = self.rects.iter().copied();
        let first = iter.next()?;
        let mut x1 = first.x;
        let mut y1 = first.y;
        let mut x2 = first.x2();
        let mut y2 = first.y2();
        for rect in iter {
            x1 = x1.min(rect.x);
            y1 = y1.min(rect.y);
            x2 = x2.max(rect.x2());
            y2 = y2.max(rect.y2());
        }
        RectI::from_extents(x1, y1, x2, y2)
    }

    pub fn add_rect(&mut self, rect: RectI) {
        if rect.is_empty() {
            return;
        }
        let mut pending = vec![rect];
        for existing in &self.rects {
            let mut next = Vec::new();
            for piece in pending {
                next.extend(subtract_rect(piece, *existing));
            }
            pending = next;
            if pending.is_empty() {
                return;
            }
        }
        self.rects.extend(pending);
    }

    pub fn add_region(&mut self, other: &Self) {
        for rect in &other.rects {
            self.add_rect(*rect);
        }
    }

    pub fn subtract_rect(&mut self, rect: RectI) {
        if rect.is_empty() || self.rects.is_empty() {
            return;
        }
        let old = std::mem::take(&mut self.rects);
        for candidate in old {
            self.rects.extend(subtract_rect(candidate, rect));
        }
    }

    pub fn subtract_region(&mut self, other: &Self) {
        for rect in &other.rects {
            self.subtract_rect(*rect);
            if self.rects.is_empty() {
                return;
            }
        }
    }

    pub fn intersect_rect(&mut self, rect: RectI) {
        if rect.is_empty() {
            self.rects.clear();
            return;
        }
        self.rects = self
            .rects
            .iter()
            .copied()
            .filter_map(|candidate| candidate.intersection(rect))
            .collect();
    }

    pub fn intersect_region(&mut self, other: &Self) {
        if self.is_empty() || other.is_empty() {
            self.clear();
            return;
        }
        let mut intersection = Self::new();
        for rect in &self.rects {
            for other_rect in &other.rects {
                if let Some(overlap) = rect.intersection(*other_rect) {
                    intersection.add_rect(overlap);
                }
            }
        }
        *self = intersection;
    }

    pub fn intersects_rect(&self, rect: RectI) -> bool {
        self.rects.iter().copied().any(|candidate| candidate.intersects(rect))
    }

    pub fn translate(&mut self, dx: i32, dy: i32) {
        for rect in &mut self.rects {
            *rect = rect.translate(dx, dy);
        }
    }

    pub fn translated(&self, dx: i32, dy: i32) -> Self {
        let mut region = self.clone();
        region.translate(dx, dy);
        region
    }
}

fn subtract_rect(rect: RectI, cut: RectI) -> Vec<RectI> {
    let Some(overlap) = rect.intersection(cut) else {
        return vec![rect];
    };

    let mut pieces = Vec::with_capacity(4);

    if rect.y < overlap.y {
        pieces.push(RectI {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: overlap.y - rect.y,
        });
    }

    if overlap.y2() < rect.y2() {
        pieces.push(RectI {
            x: rect.x,
            y: overlap.y2(),
            width: rect.width,
            height: rect.y2() - overlap.y2(),
        });
    }

    if rect.x < overlap.x {
        pieces.push(RectI {
            x: rect.x,
            y: overlap.y,
            width: overlap.x - rect.x,
            height: overlap.height,
        });
    }

    if overlap.x2() < rect.x2() {
        pieces.push(RectI {
            x: overlap.x2(),
            y: overlap.y,
            width: rect.x2() - overlap.x2(),
            height: overlap.height,
        });
    }

    pieces.retain(|piece| !piece.is_empty());
    pieces
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionOp {
    Add(RectI),
    Subtract(RectI),
}

impl RegionOp {
    fn translate(self, dx: i32, dy: i32) -> Self {
        match self {
            Self::Add(rect) => Self::Add(rect.translate(dx, dy)),
            Self::Subtract(rect) => Self::Subtract(rect.translate(dx, dy)),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegionSpec {
    ops: Vec<RegionOp>,
}

impl RegionSpec {
    pub fn add_rect(&mut self, rect: RectI) {
        self.ops.push(RegionOp::Add(rect));
    }

    pub fn subtract_rect(&mut self, rect: RectI) {
        self.ops.push(RegionOp::Subtract(rect));
    }

    pub fn ops(&self) -> &[RegionOp] {
        &self.ops
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        let mut contains = false;
        for op in &self.ops {
            match op {
                RegionOp::Add(rect) if rect.contains(x, y) => contains = true,
                RegionOp::Subtract(rect) if rect.contains(x, y) => contains = false,
                _ => {}
            }
        }
        contains
    }

    pub fn translate(&self, dx: i32, dy: i32) -> Self {
        Self {
            ops: self
                .ops
                .iter()
                .copied()
                .map(|op| op.translate(dx, dy))
                .collect(),
        }
    }

    fn has_any_adds(&self) -> bool {
        self.ops.iter().any(|op| matches!(op, RegionOp::Add(_)))
    }

    pub fn to_region(&self) -> DamageRegion {
        let mut region = DamageRegion::new();
        for op in &self.ops {
            match op {
                RegionOp::Add(rect) => region.add_rect(*rect),
                RegionOp::Subtract(rect) => region.subtract_rect(*rect),
            }
        }
        region
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegionValue {
    Empty,
    Infinite,
    Spec(RegionSpec),
}

impl RegionValue {
    pub const fn empty() -> Self {
        Self::Empty
    }

    pub const fn infinite() -> Self {
        Self::Infinite
    }

    pub fn from_spec(spec: RegionSpec) -> Self {
        if spec.ops.is_empty() || !spec.has_any_adds() {
            Self::Empty
        } else {
            Self::Spec(spec)
        }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        match self {
            Self::Empty => false,
            Self::Infinite => true,
            Self::Spec(spec) => spec.contains(x, y),
        }
    }

    pub fn translate(&self, dx: i32, dy: i32) -> Self {
        match self {
            Self::Empty => Self::Empty,
            Self::Infinite => Self::Infinite,
            Self::Spec(spec) => Self::Spec(spec.translate(dx, dy)),
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    pub fn to_region_clipped(&self, clip: RectI) -> DamageRegion {
        if clip.is_empty() {
            return DamageRegion::new();
        }
        match self {
            Self::Empty => DamageRegion::new(),
            Self::Infinite => DamageRegion::from_rect(clip),
            Self::Spec(spec) => {
                let mut region = spec.to_region();
                region.intersect_rect(clip);
                region
            }
        }
    }
}

impl Default for RegionValue {
    fn default() -> Self {
        Self::Empty
    }
}

#[cfg(test)]
mod tests {
    use super::{DamageRegion, RectI, RegionSpec, RegionValue};
    use crate::transform::RectF;

    #[test]
    fn region_contains_replays_add_then_subtract_order() {
        let mut region = RegionSpec::default();
        region.add_rect(RectI::new(0, 0, 20, 20).unwrap());
        region.subtract_rect(RectI::new(5, 5, 5, 5).unwrap());
        assert!(region.contains(4, 4));
        assert!(!region.contains(7, 7));
    }

    #[test]
    fn region_contains_replays_subtract_then_add_order() {
        let mut region = RegionSpec::default();
        region.subtract_rect(RectI::new(5, 5, 5, 5).unwrap());
        region.add_rect(RectI::new(0, 0, 20, 20).unwrap());
        assert!(region.contains(7, 7));
    }

    #[test]
    fn region_translate_shifts_rectangles() {
        let mut spec = RegionSpec::default();
        spec.add_rect(RectI::new(1, 2, 3, 4).unwrap());
        let translated = RegionValue::from_spec(spec).translate(10, -2);
        assert!(translated.contains(11, 0));
        assert!(!translated.contains(1, 2));
    }

    #[test]
    fn region_from_subtract_only_spec_is_empty() {
        let mut spec = RegionSpec::default();
        spec.subtract_rect(RectI::new(0, 0, 5, 5).unwrap());
        assert!(RegionValue::from_spec(spec).is_empty());
    }

    #[test]
    fn damage_region_add_merges_without_overlap_duplicates() {
        let mut region = DamageRegion::new();
        region.add_rect(RectI::new(0, 0, 10, 10).unwrap());
        region.add_rect(RectI::new(5, 0, 10, 10).unwrap());

        assert_eq!(region.extents(), RectI::from_extents(0, 0, 15, 10));
        assert_eq!(region.rects().iter().map(|rect| rect.width * rect.height).sum::<i32>(), 150);
    }

    #[test]
    fn damage_region_subtract_splits_rect() {
        let mut region = DamageRegion::from_rect(RectI::new(0, 0, 10, 10).unwrap());
        region.subtract_rect(RectI::new(3, 3, 4, 4).unwrap());

        assert_eq!(region.rects().len(), 4);
        assert!(!region.intersects_rect(RectI::new(4, 4, 1, 1).unwrap()));
    }

    #[test]
    fn damage_region_intersection_clips_to_rect() {
        let mut region = DamageRegion::new();
        region.add_rect(RectI::new(0, 0, 10, 10).unwrap());
        region.add_rect(RectI::new(20, 20, 10, 10).unwrap());
        region.intersect_rect(RectI::new(5, 5, 20, 20).unwrap());

        assert_eq!(
            region.rects(),
            &[
                RectI::new(5, 5, 5, 5).unwrap(),
                RectI::new(20, 20, 5, 5).unwrap()
            ]
        );
    }

    #[test]
    fn region_value_to_region_clipped_replays_protocol_ops() {
        let mut spec = RegionSpec::default();
        spec.add_rect(RectI::new(0, 0, 10, 10).unwrap());
        spec.subtract_rect(RectI::new(2, 2, 4, 4).unwrap());
        let region = RegionValue::from_spec(spec).to_region_clipped(RectI::new(0, 0, 20, 20).unwrap());

        assert!(region.intersects_rect(RectI::new(0, 0, 1, 1).unwrap()));
        assert!(!region.intersects_rect(RectI::new(3, 3, 1, 1).unwrap()));
    }

    #[test]
    fn rect_from_f32_bounds_rounds_outward() {
        let rect = RectI::from_f32_bounds(RectF { x: 1.25, y: 2.75, width: 3.1, height: 4.2 }).unwrap();
        assert_eq!(rect, RectI::new(1, 2, 4, 5).unwrap());
    }
}
