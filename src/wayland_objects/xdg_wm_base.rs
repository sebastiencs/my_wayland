use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{
        wl_display, xdg_wm_base, Constants, NewId, WaylandId, XdgPositionerId, XdgSurfaceId,
        XdgWmBaseId,
    },
    state::State,
    wayland_objects::{
        wl_surface::{Role, WlSurface},
        xdg_positioner::XdgPositioner,
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
        version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = XdgWmBaseId::from(id);
        let version = version.min(XdgWmBase::INTERFACE_VERSION);
        client.insert_object(state, XdgWmBase { id, version, surfaces: Vec::new() });
    }
}

#[derive(Debug)]
pub struct XdgWmBase {
    id: XdgWmBaseId,
    /// Client's negotiated `xdg_wm_base` version (caps which xdg_toplevel events we may send).
    version: u32,
    surfaces: Vec<Resolved<XdgSurfaceId>>,
}

impl XdgWmBase {
    pub fn new(id: XdgWmBaseId) -> Self {
        Self {
            id,
            version: XdgWmBase::INTERFACE_VERSION,
            surfaces: Vec::new(),
        }
    }

    pub fn unregister_surface(&mut self, surface: Resolved<XdgSurfaceId>) {
        self.surfaces.retain(|candidate| *candidate != surface);
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
        dispatcher: &mut ClientDispatcher,
    ) -> crate::object::RequestResult {
        use xdg_wm_base::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                if !self.surfaces.is_empty() {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        xdg_wm_base::Error::DefunctSurfaces as u32,
                        "xdg_wm_base destroyed while created xdg_surfaces are still alive",
                    );
                    return Ok(());
                }
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            CreatePositioner { id } => {
                client.insert_object(state, XdgPositioner::new(id));
            }
            GetXdgSurface { id, surface } => {
                let surface = client.resolve_or_disconnect(surface, state, dispatcher)?;
                {
                    let wl_surface = surface.ref_mut(state);
                    if wl_surface.has_attached_or_committed_buffer() {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            xdg_wm_base::Error::InvalidSurfaceState as u32,
                            "wl_surface must not have an attached or committed buffer before get_xdg_surface",
                        );
                        return Ok(());
                    }
                    if wl_surface.set_role(Role::XdgSurface, dispatcher).is_err() {
                        return Ok(());
                    }
                }

                let wm_base = client.resolve(self.id, state).unwrap();
                let xdg_surface = XdgSurface::new(id, wm_base, surface, self.version);
                client.insert_object(state, xdg_surface);
                let xdg_surface_resolved = client.resolve(id, state).unwrap();
                surface
                    .ref_mut(state)
                    .set_xdg_surface_impl(xdg_surface_resolved);
                self.surfaces.push(xdg_surface_resolved);
            }
            Pong { serial: _ } => {}
        }
        Ok(())
    }
}
