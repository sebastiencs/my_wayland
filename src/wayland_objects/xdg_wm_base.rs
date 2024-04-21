use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{xdg_wm_base, Constants, NewId, WaylandId, XdgWmBaseId},
    state::State,
    wayland_objects::{
        wl_surface::{Role, WlSurface},
        xdg_surface::XdgSurface,
    },
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
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = XdgWmBaseId::from(id);
        client.insert_object(state, XdgWmBase { id });
    }
}

#[derive(Debug)]
pub struct XdgWmBase {
    id: XdgWmBaseId,
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

    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        _: &mut ClientDispatcher,
    ) {
        use xdg_wm_base::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => todo!(),
            CreatePositioner { id } => todo!(),
            GetXdgSurface { id, surface } => {
                let wl_surface = client.get_object(surface, state);
                wl_surface.set_role(Role::XdgSurface).unwrap();
                wl_surface.set_impl(id);

                let xdg_surface = XdgSurface::new(id, surface);
                client.insert_object(state, xdg_surface);
            }
            Pong { serial } => todo!(),
        }
    }
}
