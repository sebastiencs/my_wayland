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

    pub fn geometry(&self) -> Option<RectI> {
        let (width, height) = self.size?;
        let anchor_rect = self.anchor_rect?;
        if width <= 0 || height <= 0 {
            return None;
        }

        let (anchor_x, anchor_y) = anchor_point(anchor_rect, self.anchor.clone());
        let x = match self.gravity {
            xdg_positioner::Gravity::Left
            | xdg_positioner::Gravity::TopLeft
            | xdg_positioner::Gravity::BottomLeft => anchor_x - width,
            xdg_positioner::Gravity::Right
            | xdg_positioner::Gravity::TopRight
            | xdg_positioner::Gravity::BottomRight => anchor_x,
            _ => anchor_x - width / 2,
        } + self.offset.0;
        let y = match self.gravity {
            xdg_positioner::Gravity::Top
            | xdg_positioner::Gravity::TopLeft
            | xdg_positioner::Gravity::TopRight => anchor_y - height,
            xdg_positioner::Gravity::Bottom
            | xdg_positioner::Gravity::BottomLeft
            | xdg_positioner::Gravity::BottomRight => anchor_y,
            _ => anchor_y - height / 2,
        } + self.offset.1;
        RectI::new(x, y, width, height)
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
