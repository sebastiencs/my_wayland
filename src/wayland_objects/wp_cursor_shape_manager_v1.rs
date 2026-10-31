use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_display, wp_cursor_shape_manager_v1, Constants, NewId, WaylandId, WlPointerId,
        WpCursorShapeDeviceV1Id, WpCursorShapeManagerV1Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::{wl_display::WlDisplay, wp_cursor_shape_device_v1::WpCursorShapeDeviceV1},
};

#[derive(Debug)]
pub struct WpCursorShapeManagerV1Global;

impl WpCursorShapeManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for WpCursorShapeManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (WpCursorShapeManagerV1::INTERFACE_NAME, WpCursorShapeManagerV1::INTERFACE_VERSION)
    }
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let id = WpCursorShapeManagerV1Id::from(id);
        client.insert_object(state, WpCursorShapeManagerV1 { id, version });
    }
}

#[derive(Debug)]
pub struct WpCursorShapeManagerV1 {
    id: WpCursorShapeManagerV1Id,
    version: u32,
}

impl WpCursorShapeManagerV1 {
    pub fn new(id: WpCursorShapeManagerV1Id, version: u32) -> Self {
        Self { id, version }
    }
}

impl Object for WpCursorShapeManagerV1 {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        mut self: GrabbedObject<Self>,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        use wp_cursor_shape_manager_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetPointer { cursor_shape_device, pointer } => {
                let cursor_shape_device = WpCursorShapeDeviceV1Id::from(cursor_shape_device);
                let pointer =
                    client.resolve_or_disconnect(WlPointerId::from(pointer), state, dispatcher)?;
                client.insert_object(
                    state,
                    WpCursorShapeDeviceV1::new_pointer(cursor_shape_device, pointer, self.version),
                );
            }
            GetTabletToolV2 { cursor_shape_device, tablet_tool: _ } => {
                let cursor_shape_device = WpCursorShapeDeviceV1Id::from(cursor_shape_device);
                client.insert_object(
                    state,
                    WpCursorShapeDeviceV1::new_inert(cursor_shape_device, self.version),
                );
            }
        }
        Ok(())
    }
}
