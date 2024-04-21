use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser},
    protocol_generated::{
        xdg_surface, xdg_wm_base, WaylandId, WlSurfaceId, XdgSurfaceId, XdgToplevelId,
    },
    state::State,
};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Role {
    None,
    Toplevel,
}

#[derive(Debug)]
pub struct XdgSurface {
    id: XdgSurfaceId,
    wl_surface: WlSurfaceId,
    role: Role,
    impl_id: Option<XdgToplevelId>,
}

impl XdgSurface {
    pub fn new(id: XdgSurfaceId, wl_surface: WlSurfaceId) -> Self {
        Self { id, wl_surface, role: Role::None, impl_id: None }
    }

    pub fn set_role(&mut self, new_role: Role) -> Result<(), xdg_wm_base::Error> {
        if self.role != Role::None && self.role != new_role {
            return Err(xdg_wm_base::Error::Role);
        }
        self.role = new_role;
        Ok(())
    }

    pub fn set_impl(&mut self, id: XdgToplevelId) {
        self.impl_id = Some(id);
    }

    pub fn on_commit(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let xdg_toplevel_id = self.impl_id.unwrap();
        let mut xdg_toplevel = client.grab_object(xdg_toplevel_id, state);
        xdg_toplevel.on_commit(client, state, dispatcher);
    }
}

impl Object for XdgSurface {
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
        use xdg_surface::Request::*;

        match Self::parse(req).unwrap() {
            Destroy => todo!(),
            GetToplevel { id } => {
                self.set_role(Role::Toplevel).unwrap();
                self.set_impl(id);

                let xdg_toplevel = xdg_toplevel::XdgToplevel::new(id, self.id);
                xdg_toplevel.send_capabilities(dispatcher);

                client.insert_object(state, xdg_toplevel);
            }
            GetPopup { id, parent, positioner } => todo!(),
            SetWindowGeometry { x, y, width, height } => todo!(),
            AckConfigure { serial } => todo!(),
        }
    }
}

pub mod xdg_toplevel {
    use crate::protocol_generated::{xdg_toplevel as proto_xdg_toplevel, Array, XdgToplevelId};

    use super::*;

    #[derive(Debug)]
    pub struct XdgToplevel {
        id: XdgToplevelId,
        xdg_surface_id: XdgSurfaceId,

        title: String,
    }

    impl XdgToplevel {
        pub fn new(id: XdgToplevelId, xdg_surface_id: XdgSurfaceId) -> Self {
            Self { id, xdg_surface_id, title: String::new() }
        }

        pub fn send_capabilities(&self, dispatcher: &mut ClientDispatcher) {
            use proto_xdg_toplevel::WmCapabilities;
            let capabilities = proto_xdg_toplevel::Event::WmCapabilities {
                capabilities: Array(vec![WmCapabilities::Fullscreen as u32]),
            };
            self.send_event(capabilities, dispatcher);
        }

        pub fn on_commit(
            &mut self,
            client: &mut Client,
            state: &mut State,
            dispatcher: &mut ClientDispatcher,
        ) {
            todo!()
        }
    }

    impl Object for XdgToplevel {
        fn id(&self) -> WaylandId {
            self.id.into()
        }

        fn on(&mut self, _: &mut Client, _: &mut State, req: RawRequest, _: &mut ClientDispatcher) {
            use proto_xdg_toplevel::Request::*;

            match Self::parse(req).unwrap() {
                Destroy => todo!(),
                SetParent { parent } => todo!(),
                SetTitle { title } => {
                    self.title = title;
                }
                SetAppId { app_id } => todo!(),
                ShowWindowMenu { seat, serial, x, y } => todo!(),
                Move { seat, serial } => todo!(),
                Resize { seat, serial, edges } => todo!(),
                SetMaxSize { width, height } => todo!(),
                SetMinSize { width, height } => todo!(),
                SetMaximized => todo!(),
                UnsetMaximized => todo!(),
                SetFullscreen { output } => todo!(),
                UnsetFullscreen => todo!(),
                SetMinimized => todo!(),
            }
        }
    }
}
