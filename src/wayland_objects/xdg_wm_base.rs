use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    drm::{FORMATS, XRGB8888},
    globals::{Global, GlobalName},
    object::{Object, Parser},
    protocol_generated::{xdg_wm_base, Constants, NewId, WaylandId, XdgWmBaseId},
    state::State,
};

#[derive(Debug)]
pub struct XdgWmBaseGlobal;

impl XdgWmBaseGlobal {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for XdgWmBaseGlobal {
    fn interface(&self) -> (&'static str, u32) {
        (XdgWmBase::INTERFACE_NAME, XdgWmBase::INTERFACE_VERSION)
    }
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let id = XdgWmBaseId::from(id);
        client.insert_object(state, XdgWmBase { id });
    }
}

#[derive(Debug)]
pub struct XdgWmBase {
    pub id: XdgWmBaseId,
}

impl XdgWmBase {
    pub fn new(id: XdgWmBaseId) -> Self {
        Self { id }
    }
}

impl Object for XdgWmBase {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, _: &mut Client, _: &mut State, req: RawRequest, _: &mut ClientDispatcher) {
        use xdg_wm_base::Request::*;

        match Self::parse(req).unwrap() {
            Destroy => todo!(),
            CreatePositioner { id } => todo!(),
            GetXdgSurface { id, surface } => todo!(),
            Pong { serial } => todo!(),
        }
    }
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
