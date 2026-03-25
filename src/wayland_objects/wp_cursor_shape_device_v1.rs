use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    drm::schedule_active_output,
    object::{Object, Parser},
    protocol_generated::{
        wp_cursor_shape_device_v1, WaylandId, WlPointerId, WpCursorShapeDeviceV1Id,
    },
    state::State,
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Clone, Copy, Debug)]
enum DeviceKind {
    Pointer { pointer_id: WlPointerId },
    Inert,
}

#[derive(Debug)]
pub struct WpCursorShapeDeviceV1 {
    id: WpCursorShapeDeviceV1Id,
    kind: DeviceKind,
    version: u32,
}

impl WpCursorShapeDeviceV1 {
    pub fn new_pointer(id: WpCursorShapeDeviceV1Id, pointer_id: WlPointerId, version: u32) -> Self {
        Self { id, kind: DeviceKind::Pointer { pointer_id }, version }
    }

    pub fn new_inert(id: WpCursorShapeDeviceV1Id, version: u32) -> Self {
        Self { id, kind: DeviceKind::Inert, version }
    }
}

impl Object for WpCursorShapeDeviceV1 {
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
        use wp_cursor_shape_device_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            SetShape { serial, shape } => {
                let DeviceKind::Pointer { pointer_id } = self.kind else {
                    return;
                };
                if !client.object_is(pointer_id, state) {
                    return;
                }
                if !state
                    .seat
                    .pointer_cursor_request_allowed(client.client_id(), serial)
                {
                    return;
                }
                let Some(icon_names) = cursor_icon_names(shape, self.version) else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wp_cursor_shape_device_v1::Error::InvalidShape as u32,
                        "the specified cursor shape is invalid",
                    );
                    return;
                };
                if state.seat.set_named_cursor(client.client_id(), icon_names) {
                    schedule_active_output(state, dispatcher);
                }
            }
        }
    }
}

pub fn cursor_icon_names(shape: u32, version: u32) -> Option<&'static [&'static str]> {
    use wp_cursor_shape_device_v1::Shape;

    Some(match shape {
        value if value == Shape::Default as u32 => &["default", "left_ptr"],
        value if value == Shape::ContextMenu as u32 => &["context-menu", "context_menu"],
        value if value == Shape::Help as u32 => &["help", "question_arrow", "left_ptr_help"],
        value if value == Shape::Pointer as u32 => &["pointer", "hand2", "hand1"],
        value if value == Shape::Progress as u32 => &["progress", "left_ptr_watch", "watch"],
        value if value == Shape::Wait as u32 => &["wait", "watch"],
        value if value == Shape::Cell as u32 => &["cell", "plus"],
        value if value == Shape::Crosshair as u32 => &["crosshair", "cross"],
        value if value == Shape::Text as u32 => &["text", "xterm"],
        value if value == Shape::VerticalText as u32 => {
            &["vertical-text", "vertical-text", "xterm"]
        }
        value if value == Shape::Alias as u32 => &["alias", "link", "dnd-link"],
        value if value == Shape::Copy as u32 => &["copy", "dnd-copy", "plus"],
        value if value == Shape::Move as u32 => &["move", "dnd-move", "fleur"],
        value if value == Shape::NoDrop as u32 => &["no-drop", "dnd-none", "crossed_circle"],
        value if value == Shape::NotAllowed as u32 => {
            &["not-allowed", "forbidden", "crossed_circle"]
        }
        value if value == Shape::Grab as u32 => &["grab", "openhand", "hand1"],
        value if value == Shape::Grabbing as u32 => &["grabbing", "closedhand", "hand2"],
        value if value == Shape::EResize as u32 => &["e-resize", "right_side"],
        value if value == Shape::NResize as u32 => &["n-resize", "top_side"],
        value if value == Shape::NeResize as u32 => &["ne-resize", "top_right_corner"],
        value if value == Shape::NwResize as u32 => &["nw-resize", "top_left_corner"],
        value if value == Shape::SResize as u32 => &["s-resize", "bottom_side"],
        value if value == Shape::SeResize as u32 => &["se-resize", "bottom_right_corner"],
        value if value == Shape::SwResize as u32 => &["sw-resize", "bottom_left_corner"],
        value if value == Shape::WResize as u32 => &["w-resize", "left_side"],
        value if value == Shape::EwResize as u32 => {
            &["ew-resize", "sb_h_double_arrow", "h_double_arrow"]
        }
        value if value == Shape::NsResize as u32 => {
            &["ns-resize", "sb_v_double_arrow", "v_double_arrow"]
        }
        value if value == Shape::NeswResize as u32 => &["nesw-resize", "fd_double_arrow"],
        value if value == Shape::NwseResize as u32 => &["nwse-resize", "bd_double_arrow"],
        value if value == Shape::ColResize as u32 => {
            &["col-resize", "sb_h_double_arrow", "h_double_arrow"]
        }
        value if value == Shape::RowResize as u32 => {
            &["row-resize", "sb_v_double_arrow", "v_double_arrow"]
        }
        value if value == Shape::AllScroll as u32 => &["all-scroll", "fleur", "size_all"],
        value if value == Shape::ZoomIn as u32 => &["zoom-in"],
        value if value == Shape::ZoomOut as u32 => &["zoom-out"],
        value if value == Shape::DndAsk as u32 && version >= 2 => &["dnd-ask", "question_arrow"],
        value if value == Shape::AllResize as u32 && version >= 2 => {
            &["all-resize", "fleur", "size_all"]
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use {super::cursor_icon_names, crate::protocol_generated::wp_cursor_shape_device_v1::Shape};

    #[test]
    fn version_one_rejects_v2_only_shapes() {
        assert!(cursor_icon_names(Shape::DndAsk as u32, 1).is_none());
        assert!(cursor_icon_names(Shape::AllResize as u32, 1).is_none());
    }

    #[test]
    fn version_two_accepts_v2_only_shapes() {
        assert!(cursor_icon_names(Shape::DndAsk as u32, 2).is_some());
        assert!(cursor_icon_names(Shape::AllResize as u32, 2).is_some());
    }
}
