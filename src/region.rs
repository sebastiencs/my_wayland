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

    pub fn contains(self, x: i32, y: i32) -> bool {
        if self.width <= 0 || self.height <= 0 {
            return false;
        }
        let x2 = self.x.saturating_add(self.width);
        let y2 = self.y.saturating_add(self.height);
        x >= self.x && y >= self.y && x < x2 && y < y2
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
}

impl Default for RegionValue {
    fn default() -> Self {
        Self::Empty
    }
}

#[cfg(test)]
mod tests {
    use super::{RectI, RegionSpec, RegionValue};

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
}
