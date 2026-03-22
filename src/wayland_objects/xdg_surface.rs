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
    is_initialized: bool,
    serial: u32,
    acked_serial: Option<u32>,
}

impl XdgSurface {
    pub fn new(id: XdgSurfaceId, wl_surface: WlSurfaceId) -> Self {
        Self {
            id,
            wl_surface,
            role: Role::None,
            impl_id: None,
            is_initialized: false,
            serial: 1,
            acked_serial: None,
        }
    }

    pub fn set_role(
        &mut self,
        new_role: Role,
        dispatcher: &mut ClientDispatcher,
    ) -> Result<(), xdg_wm_base::Error> {
        if self.role != Role::None && self.role != new_role {
            self.send_error(
                dispatcher,
                xdg_wm_base::Error::Role as u32,
                "invalid xdg_surface role",
            );
            return Err(xdg_wm_base::Error::Role);
        }
        self.role = new_role;
        Ok(())
    }

    pub fn set_impl(&mut self, id: XdgToplevelId) {
        self.impl_id = Some(id);
    }

    pub fn send_configure(&self, serial: u32, dispatcher: &mut ClientDispatcher) {
        self.send_event(xdg_surface::Event::Configure { serial }, dispatcher);
    }

    pub fn on_commit(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if !self.is_initialized {
            let Some(xdg_toplevel_id) = self.impl_id else {
                log::debug!(
                    "xdg_surface {:?}: commit before role object; sending xdg_surface.not_constructed",
                    self.id
                );
                client.protocol_violation_disconnect(
                    dispatcher,
                    self.id.into(),
                    xdg_surface::Error::NotConstructed as u32,
                    "xdg_surface committed before get_toplevel or get_popup",
                );
                return;
            };
            let mut xdg_toplevel = client.grab_object(xdg_toplevel_id, state);
            xdg_toplevel.on_first_commit(client, state, dispatcher);
            self.send_configure(self.serial, dispatcher);
            self.is_initialized = true;
        }
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

        match client.parse::<Self>(req).unwrap() {
            Destroy => todo!(),
            GetToplevel { id } => {
                if self.set_role(Role::Toplevel, dispatcher).is_err() {
                    return;
                }
                self.set_impl(id);

                let xdg_toplevel = xdg_toplevel::XdgToplevel::new(id, self.id);
                xdg_toplevel.send_capabilities(dispatcher);

                client.insert_object(state, xdg_toplevel);
            }
            GetPopup { id, parent, positioner } => todo!(),
            SetWindowGeometry { x, y, width, height } => todo!(),
            AckConfigure { serial: acked } => {
                if acked == self.serial {
                    self.acked_serial = Some(acked);
                }
            }
        }
    }
}

pub mod xdg_toplevel {
    use std::collections::BTreeSet;

    use {
        crate::protocol_generated::{xdg_toplevel as proto_xdg_toplevel, Array, XdgToplevelId},
        proto_xdg_toplevel::State as SurfaceState,
    };

    use super::*;

    #[derive(Debug)]
    pub struct XdgToplevel {
        id: XdgToplevelId,
        xdg_surface_id: XdgSurfaceId,

        title: String,
        app_id: String,
        states: BTreeSet<SurfaceState>,
    }

    impl XdgToplevel {
        pub fn new(id: XdgToplevelId, xdg_surface_id: XdgSurfaceId) -> Self {
            let mut states = BTreeSet::default();
            states.insert(SurfaceState::Fullscreen);
            Self {
                id,
                xdg_surface_id,
                title: String::new(),
                app_id: String::new(),
                states,
            }
        }

        pub fn send_capabilities(&self, dispatcher: &mut ClientDispatcher) {
            use proto_xdg_toplevel::WmCapabilities;
            let capabilities_list =
                [WmCapabilities::Fullscreen as u32, WmCapabilities::Maximize as u32];
            let capabilities = proto_xdg_toplevel::Event::WmCapabilities {
                capabilities: Array(capabilities_list.to_vec()),
            };
            self.send_event(capabilities, dispatcher);
        }

        fn send_configure(&self, dispatcher: &mut ClientDispatcher) {
            let states: Vec<_> = self.states.iter().cloned().map(|s| s as u32).collect();
            self.send_event(
                proto_xdg_toplevel::Event::Configure { width: 0, height: 0, states: Array(states) },
                dispatcher,
            );
        }

        pub fn on_first_commit(
            &mut self,
            client: &mut Client,
            state: &mut State,
            dispatcher: &mut ClientDispatcher,
        ) {
            self.send_configure(dispatcher);
        }
    }

    impl Object for XdgToplevel {
        fn id(&self) -> WaylandId {
            self.id.into()
        }

        fn on(
            &mut self,
            client: &mut Client,
            _: &mut State,
            req: RawRequest,
            _: &mut ClientDispatcher,
        ) {
            use proto_xdg_toplevel::Request::*;

            match client.parse::<Self>(req).unwrap() {
                Destroy => todo!(),
                SetParent { parent } => todo!(),
                SetTitle { title } => {
                    self.title = title;
                }
                SetAppId { app_id } => {
                    self.app_id = app_id;
                }
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
