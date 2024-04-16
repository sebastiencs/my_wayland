use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{
        wp_cursor_shape_manager_v1, Constants, NewId, WaylandId, WpCursorShapeManagerV1Id,
    },
    state::State,
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
        _version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let id = WpCursorShapeManagerV1Id::from(id);
        client.insert_object(state, WpCursorShapeManagerV1 { id });
    }
}

#[derive(Debug)]
pub struct WpCursorShapeManagerV1 {
    pub id: WpCursorShapeManagerV1Id,
}

impl WpCursorShapeManagerV1 {
    pub fn new(id: WpCursorShapeManagerV1Id) -> Self {
        Self { id }
    }
}

impl Object for WpCursorShapeManagerV1 {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, _: &mut Client, _: &mut State, req: RawRequest, _: &mut ClientDispatcher) {
        use wp_cursor_shape_manager_v1::Request::*;

        match Self::parse(req).unwrap() {
            Destroy => todo!(),
            GetPointer { cursor_shape_device, pointer } => todo!(),
            GetTabletToolV2 { cursor_shape_device, tablet_tool } => todo!(),
        }
    }
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
