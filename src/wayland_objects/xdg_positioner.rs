use std::cmp::min;

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wl_display, xdg_positioner, WaylandId, XdgPositionerId},
    region::RectI,
    state::State,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PositionerState {
    pub size: Option<(i32, i32)>,
    pub anchor_rect: Option<RectI>,
    pub anchor: xdg_positioner::Anchor,
    pub gravity: xdg_positioner::Gravity,
    pub constraint_adjustment: u32,
    pub offset: (i32, i32),
    pub reactive: bool,
    pub parent_size: Option<(i32, i32)>,
    pub parent_configure: Option<u32>,
}

impl Default for PositionerState {
    fn default() -> Self {
        Self {
            size: None,
            anchor_rect: None,
            anchor: xdg_positioner::Anchor::None,
            gravity: xdg_positioner::Gravity::None,
            constraint_adjustment: xdg_positioner::ConstraintAdjustment::None as u32,
            offset: (0, 0),
            reactive: false,
            parent_size: None,
            parent_configure: None,
        }
    }
}

impl PositionerState {
    pub fn is_complete(&self) -> bool {
        let Some((width, height)) = self.size else {
            return false;
        };
        let Some(anchor_rect) = self.anchor_rect else {
            return false;
        };
        width > 0 && height > 0 && anchor_rect.width > 0 && anchor_rect.height > 0
    }

    fn gravity_has_edge(&self, edge: xdg_positioner::Gravity) -> bool {
        match edge {
            xdg_positioner::Gravity::Top => {
                matches!(
                    self.gravity,
                    xdg_positioner::Gravity::Top
                        | xdg_positioner::Gravity::TopLeft
                        | xdg_positioner::Gravity::TopRight
                )
            }
            xdg_positioner::Gravity::Bottom => {
                matches!(
                    self.gravity,
                    xdg_positioner::Gravity::Bottom
                        | xdg_positioner::Gravity::BottomLeft
                        | xdg_positioner::Gravity::BottomRight
                )
            }
            xdg_positioner::Gravity::Left => {
                matches!(
                    self.gravity,
                    xdg_positioner::Gravity::Left
                        | xdg_positioner::Gravity::TopLeft
                        | xdg_positioner::Gravity::BottomLeft
                )
            }
            xdg_positioner::Gravity::Right => {
                matches!(
                    self.gravity,
                    xdg_positioner::Gravity::Right
                        | xdg_positioner::Gravity::TopRight
                        | xdg_positioner::Gravity::BottomRight
                )
            }
            xdg_positioner::Gravity::None => self.gravity == xdg_positioner::Gravity::None,
            _ => false,
        }
    }

    fn base_geometry(&self) -> Option<RectI> {
        let (width, height) = self.size?;
        let anchor_rect = self.anchor_rect?;
        if width <= 0 || height <= 0 {
            return None;
        }

        let (anchor_x, anchor_y) = anchor_point(anchor_rect, self.anchor.clone());
        let x = if self.gravity_has_edge(xdg_positioner::Gravity::Left) {
            anchor_x - width
        } else if self.gravity_has_edge(xdg_positioner::Gravity::Right) {
            anchor_x
        } else {
            anchor_x - width / 2
        } + self.offset.0;
        let y = if self.gravity_has_edge(xdg_positioner::Gravity::Top) {
            anchor_y - height
        } else if self.gravity_has_edge(xdg_positioner::Gravity::Bottom) {
            anchor_y
        } else {
            anchor_y - height / 2
        } + self.offset.1;
        RectI::new(x, y, width, height)
    }

    pub fn geometry(&self) -> Option<RectI> {
        let geometry = self.base_geometry()?;
        let target = self
            .parent_size
            .and_then(|(width, height)| RectI::new(0, 0, width, height));
        Some(self.apply_constraint_adjustment(geometry, target))
    }

    pub fn geometry_in_bounds(&self, bounds: RectI) -> Option<RectI> {
        self.base_geometry()
            .map(|geometry| self.apply_constraint_adjustment(geometry, Some(bounds)))
    }

    fn apply_constraint_adjustment(&self, mut geometry: RectI, target: Option<RectI>) -> RectI {
        let Some(target) = target else {
            return geometry;
        };
        let mut positioner = self.clone();
        let (mut off_left, mut off_right, mut off_top, mut off_bottom) =
            compute_offsets(target, geometry);

        if (off_left > 0 || off_right > 0)
            && self.constraint_adjustment & xdg_positioner::ConstraintAdjustment::FlipX as u32 != 0
        {
            let mut flipped = positioner.clone();
            flipped.anchor = invert_anchor_x(flipped.anchor);
            flipped.gravity = invert_gravity_x(flipped.gravity);
            if let Some(new_geometry) = flipped.base_geometry() {
                let (new_off_left, new_off_right, _, _) = compute_offsets(target, new_geometry);
                if new_off_left <= 0 && new_off_right <= 0 {
                    positioner = flipped;
                    geometry = new_geometry;
                    off_left = 0;
                    off_right = 0;
                }
            }
        }

        if (off_top > 0 || off_bottom > 0)
            && self.constraint_adjustment & xdg_positioner::ConstraintAdjustment::FlipY as u32 != 0
        {
            let mut flipped = positioner.clone();
            flipped.anchor = invert_anchor_y(flipped.anchor);
            flipped.gravity = invert_gravity_y(flipped.gravity);
            if let Some(new_geometry) = flipped.base_geometry() {
                let (_, _, new_off_top, new_off_bottom) = compute_offsets(target, new_geometry);
                if new_off_top <= 0 && new_off_bottom <= 0 {
                    geometry = new_geometry;
                    off_top = 0;
                    off_bottom = 0;
                }
            }
        }

        if (off_left > 0 || off_right > 0)
            && self.constraint_adjustment & xdg_positioner::ConstraintAdjustment::SlideX as u32 != 0
        {
            if off_left > 0 {
                geometry.x += off_left;
            } else if off_right > 0 {
                geometry.x -= min(off_right, -off_left);
            }
            (off_left, off_right, _, _) = compute_offsets(target, geometry);
        }

        if (off_top > 0 || off_bottom > 0)
            && self.constraint_adjustment & xdg_positioner::ConstraintAdjustment::SlideY as u32 != 0
        {
            if off_top > 0 {
                geometry.y += off_top;
            } else if off_bottom > 0 {
                geometry.y -= min(off_bottom, -off_top);
            }
            (_, _, off_top, off_bottom) = compute_offsets(target, geometry);
        }

        if self.constraint_adjustment & xdg_positioner::ConstraintAdjustment::ResizeX as u32 != 0 {
            if off_left > 0 && off_left < geometry.width {
                geometry.x += off_left;
                geometry.width -= off_left;
            }
            if off_right > 0 && off_right < geometry.width {
                geometry.width -= off_right;
            }
        }

        if self.constraint_adjustment & xdg_positioner::ConstraintAdjustment::ResizeY as u32 != 0 {
            if off_top > 0 && off_top < geometry.height {
                geometry.y += off_top;
                geometry.height -= off_top;
            }
            if off_bottom > 0 && off_bottom < geometry.height {
                geometry.height -= off_bottom;
            }
        }

        geometry
    }
}

fn anchor_point(rect: RectI, anchor: xdg_positioner::Anchor) -> (i32, i32) {
    let x = match anchor {
        xdg_positioner::Anchor::Left
        | xdg_positioner::Anchor::TopLeft
        | xdg_positioner::Anchor::BottomLeft => rect.x,
        xdg_positioner::Anchor::Right
        | xdg_positioner::Anchor::TopRight
        | xdg_positioner::Anchor::BottomRight => rect.x2(),
        _ => rect.x + rect.width / 2,
    };
    let y = match anchor {
        xdg_positioner::Anchor::Top
        | xdg_positioner::Anchor::TopLeft
        | xdg_positioner::Anchor::TopRight => rect.y,
        xdg_positioner::Anchor::Bottom
        | xdg_positioner::Anchor::BottomLeft
        | xdg_positioner::Anchor::BottomRight => rect.y2(),
        _ => rect.y + rect.height / 2,
    };
    (x, y)
}

fn compute_offsets(target: RectI, popup: RectI) -> (i32, i32, i32, i32) {
    let off_left = target.x - popup.x;
    let off_right = popup.x2() - target.x2();
    let off_top = target.y - popup.y;
    let off_bottom = popup.y2() - target.y2();
    (off_left, off_right, off_top, off_bottom)
}

fn invert_anchor_x(anchor: xdg_positioner::Anchor) -> xdg_positioner::Anchor {
    match anchor {
        xdg_positioner::Anchor::Left => xdg_positioner::Anchor::Right,
        xdg_positioner::Anchor::Right => xdg_positioner::Anchor::Left,
        xdg_positioner::Anchor::TopLeft => xdg_positioner::Anchor::TopRight,
        xdg_positioner::Anchor::TopRight => xdg_positioner::Anchor::TopLeft,
        xdg_positioner::Anchor::BottomLeft => xdg_positioner::Anchor::BottomRight,
        xdg_positioner::Anchor::BottomRight => xdg_positioner::Anchor::BottomLeft,
        anchor => anchor,
    }
}

fn invert_anchor_y(anchor: xdg_positioner::Anchor) -> xdg_positioner::Anchor {
    match anchor {
        xdg_positioner::Anchor::Top => xdg_positioner::Anchor::Bottom,
        xdg_positioner::Anchor::Bottom => xdg_positioner::Anchor::Top,
        xdg_positioner::Anchor::TopLeft => xdg_positioner::Anchor::BottomLeft,
        xdg_positioner::Anchor::TopRight => xdg_positioner::Anchor::BottomRight,
        xdg_positioner::Anchor::BottomLeft => xdg_positioner::Anchor::TopLeft,
        xdg_positioner::Anchor::BottomRight => xdg_positioner::Anchor::TopRight,
        anchor => anchor,
    }
}

fn invert_gravity_x(gravity: xdg_positioner::Gravity) -> xdg_positioner::Gravity {
    match gravity {
        xdg_positioner::Gravity::Left => xdg_positioner::Gravity::Right,
        xdg_positioner::Gravity::Right => xdg_positioner::Gravity::Left,
        xdg_positioner::Gravity::TopLeft => xdg_positioner::Gravity::TopRight,
        xdg_positioner::Gravity::TopRight => xdg_positioner::Gravity::TopLeft,
        xdg_positioner::Gravity::BottomLeft => xdg_positioner::Gravity::BottomRight,
        xdg_positioner::Gravity::BottomRight => xdg_positioner::Gravity::BottomLeft,
        gravity => gravity,
    }
}

fn invert_gravity_y(gravity: xdg_positioner::Gravity) -> xdg_positioner::Gravity {
    match gravity {
        xdg_positioner::Gravity::Top => xdg_positioner::Gravity::Bottom,
        xdg_positioner::Gravity::Bottom => xdg_positioner::Gravity::Top,
        xdg_positioner::Gravity::TopLeft => xdg_positioner::Gravity::BottomLeft,
        xdg_positioner::Gravity::TopRight => xdg_positioner::Gravity::BottomRight,
        xdg_positioner::Gravity::BottomLeft => xdg_positioner::Gravity::TopLeft,
        xdg_positioner::Gravity::BottomRight => xdg_positioner::Gravity::TopRight,
        gravity => gravity,
    }
}

#[derive(Debug)]
pub struct XdgPositioner {
    id: XdgPositionerId,
    state: PositionerState,
}

impl XdgPositioner {
    pub fn new(id: XdgPositionerId) -> Self {
        Self { id, state: PositionerState::default() }
    }

    pub fn snapshot(&self) -> PositionerState {
        self.state.clone()
    }

    fn delete_self(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        use crate::wayland_objects::wl_display::WlDisplay;
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }

    fn send_error(
        &self,
        client: &Client,
        dispatcher: &mut ClientDispatcher,
        code: xdg_positioner::Error,
        message: impl AsRef<str>,
    ) {
        client.send_protocol_error(dispatcher, self.id.into(), code as u32, message);
    }
}

fn decode_anchor(raw: u32) -> Option<xdg_positioner::Anchor> {
    Some(match raw {
        0 => xdg_positioner::Anchor::None,
        1 => xdg_positioner::Anchor::Top,
        2 => xdg_positioner::Anchor::Bottom,
        3 => xdg_positioner::Anchor::Left,
        4 => xdg_positioner::Anchor::Right,
        5 => xdg_positioner::Anchor::TopLeft,
        6 => xdg_positioner::Anchor::BottomLeft,
        7 => xdg_positioner::Anchor::TopRight,
        8 => xdg_positioner::Anchor::BottomRight,
        _ => return None,
    })
}

fn decode_gravity(raw: u32) -> Option<xdg_positioner::Gravity> {
    Some(match raw {
        0 => xdg_positioner::Gravity::None,
        1 => xdg_positioner::Gravity::Top,
        2 => xdg_positioner::Gravity::Bottom,
        3 => xdg_positioner::Gravity::Left,
        4 => xdg_positioner::Gravity::Right,
        5 => xdg_positioner::Gravity::TopLeft,
        6 => xdg_positioner::Gravity::BottomLeft,
        7 => xdg_positioner::Gravity::TopRight,
        8 => xdg_positioner::Gravity::BottomRight,
        _ => return None,
    })
}

fn valid_constraint_adjustment(mask: u32) -> bool {
    const VALID_BITS: u32 = xdg_positioner::ConstraintAdjustment::SlideX as u32
        | xdg_positioner::ConstraintAdjustment::SlideY as u32
        | xdg_positioner::ConstraintAdjustment::FlipX as u32
        | xdg_positioner::ConstraintAdjustment::FlipY as u32
        | xdg_positioner::ConstraintAdjustment::ResizeX as u32
        | xdg_positioner::ConstraintAdjustment::ResizeY as u32;
    mask & !VALID_BITS == 0
}

impl Object for XdgPositioner {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) {
        use xdg_positioner::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => self.delete_self(client, state, dispatcher),
            SetSize { width, height } => {
                if width <= 0 || height <= 0 {
                    self.send_error(
                        client,
                        dispatcher,
                        xdg_positioner::Error::InvalidInput,
                        "xdg_positioner.set_size requires positive dimensions",
                    );
                    return;
                }
                self.state.size = Some((width, height));
            }
            SetAnchorRect { x, y, width, height } => {
                let Some(anchor_rect) = RectI::new(x, y, width, height) else {
                    self.send_error(
                        client,
                        dispatcher,
                        xdg_positioner::Error::InvalidInput,
                        "xdg_positioner.set_anchor_rect requires non-negative dimensions",
                    );
                    return;
                };
                self.state.anchor_rect = Some(anchor_rect);
            }
            SetAnchor { anchor } => {
                let Some(anchor) = decode_anchor(anchor) else {
                    self.send_error(
                        client,
                        dispatcher,
                        xdg_positioner::Error::InvalidInput,
                        "xdg_positioner.set_anchor received an unknown enum value",
                    );
                    return;
                };
                self.state.anchor = anchor;
            }
            SetGravity { gravity } => {
                let Some(gravity) = decode_gravity(gravity) else {
                    self.send_error(
                        client,
                        dispatcher,
                        xdg_positioner::Error::InvalidInput,
                        "xdg_positioner.set_gravity received an unknown enum value",
                    );
                    return;
                };
                self.state.gravity = gravity;
            }
            SetConstraintAdjustment { constraint_adjustment } => {
                if !valid_constraint_adjustment(constraint_adjustment) {
                    self.send_error(
                        client,
                        dispatcher,
                        xdg_positioner::Error::InvalidInput,
                        "xdg_positioner.set_constraint_adjustment received unknown bits",
                    );
                    return;
                }
                self.state.constraint_adjustment = constraint_adjustment;
            }
            SetOffset { x, y } => {
                self.state.offset = (x, y);
            }
            SetReactive => {
                self.state.reactive = true;
            }
            SetParentSize { parent_width, parent_height } => {
                if parent_width < 0 || parent_height < 0 {
                    self.send_error(
                        client,
                        dispatcher,
                        xdg_positioner::Error::InvalidInput,
                        "xdg_positioner.set_parent_size requires non-negative dimensions",
                    );
                    return;
                }
                self.state.parent_size = Some((parent_width, parent_height));
            }
            SetParentConfigure { serial } => {
                self.state.parent_configure = Some(serial);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PositionerState;
    use crate::{
        protocol_generated::xdg_positioner,
        region::RectI,
    };

    #[test]
    fn geometry_applies_constraint_adjustment_with_parent_size() {
        let state = PositionerState {
            size: Some((50, 40)),
            anchor_rect: Some(RectI::new(90, 10, 10, 10).unwrap()),
            anchor: xdg_positioner::Anchor::TopLeft,
            gravity: xdg_positioner::Gravity::BottomRight,
            constraint_adjustment: xdg_positioner::ConstraintAdjustment::FlipX as u32,
            offset: (0, 0),
            reactive: false,
            parent_size: Some((100, 100)),
            parent_configure: None,
        };

        assert_eq!(state.geometry(), Some(RectI::new(50, 10, 50, 40).unwrap()));
    }

    #[test]
    fn geometry_in_bounds_slides_inside_target() {
        let state = PositionerState {
            size: Some((50, 30)),
            anchor_rect: Some(RectI::new(45, 45, 10, 10).unwrap()),
            anchor: xdg_positioner::Anchor::BottomRight,
            gravity: xdg_positioner::Gravity::BottomRight,
            constraint_adjustment: xdg_positioner::ConstraintAdjustment::SlideX as u32
                | xdg_positioner::ConstraintAdjustment::SlideY as u32,
            offset: (0, 0),
            reactive: true,
            parent_size: None,
            parent_configure: Some(12),
        };

        assert_eq!(
            state.geometry_in_bounds(RectI::new(0, 0, 80, 80).unwrap()),
            Some(RectI::new(30, 50, 50, 30).unwrap())
        );
    }
}
