use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{xdg_activation_v1, Constants, NewId, WaylandId, XdgActivationV1Id},
    state::State,
};

#[derive(Debug)]
pub struct XdgActivationV1Global;

impl XdgActivationV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for XdgActivationV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (XdgActivationV1::INTERFACE_NAME, XdgActivationV1::INTERFACE_VERSION)
    }
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let id = XdgActivationV1Id::from(id);
        client.insert_object(state, XdgActivationV1 { id });
    }
}

#[derive(Debug)]
pub struct XdgActivationV1 {
    id: XdgActivationV1Id,
}

impl XdgActivationV1 {
    pub fn new(id: XdgActivationV1Id) -> Self {
        Self { id }
    }
}

impl Object for XdgActivationV1 {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, _: &mut Client, _: &mut State, req: RawRequest, _: &mut ClientDispatcher) {
        use xdg_activation_v1::Request::*;

        match Self::parse(req).unwrap() {
            Destroy => todo!(),
            GetActivationToken { id } => todo!(),
            Activate { token, surface } => todo!(),
        }
    }
}
