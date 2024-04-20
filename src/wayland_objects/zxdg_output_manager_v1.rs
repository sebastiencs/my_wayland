use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{
        zxdg_output_manager_v1, Constants, NewId, WaylandId, ZxdgOutputManagerV1Id,
    },
    state::State,
};

#[derive(Debug)]
pub struct ZxdgOutputManagerV1Global;

impl ZxdgOutputManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for ZxdgOutputManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (ZxdgOutputManagerV1::INTERFACE_NAME, ZxdgOutputManagerV1::INTERFACE_VERSION)
    }
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let id = ZxdgOutputManagerV1Id::from(id);
        client.insert_object(state, ZxdgOutputManagerV1 { id });
    }
}

#[derive(Debug)]
pub struct ZxdgOutputManagerV1 {
    id: ZxdgOutputManagerV1Id,
}

impl ZxdgOutputManagerV1 {
    pub fn new(id: ZxdgOutputManagerV1Id) -> Self {
        Self { id }
    }
}

impl Object for ZxdgOutputManagerV1 {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, _: &mut Client, _: &mut State, req: RawRequest, _: &mut ClientDispatcher) {
        use zxdg_output_manager_v1::Request::*;

        match Self::parse(req).unwrap() {
            Destroy => todo!(),
            GetXdgOutput { id, output } => todo!(),
        }
    }

    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
