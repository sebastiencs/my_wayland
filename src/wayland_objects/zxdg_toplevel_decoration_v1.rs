use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zxdg_toplevel_decoration_v1, Constants, WaylandId, WlSurfaceId, XdgToplevelId,
        ZxdgToplevelDecorationV1Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::{
        wl_display::WlDisplay,
        xdg_surface::{
            xdg_toplevel::{ToplevelDecorationMode, XdgToplevel},
            XdgSurface,
        },
    },
};

#[derive(Debug)]
pub struct ZxdgToplevelDecorationV1 {
    id: ZxdgToplevelDecorationV1Id,
    toplevel: Resolved<XdgToplevelId>,
    requested_mode: Option<ToplevelDecorationMode>,
}

impl ZxdgToplevelDecorationV1 {
    pub fn new(id: ZxdgToplevelDecorationV1Id, toplevel: Resolved<XdgToplevelId>) -> Self {
        Self { id, toplevel, requested_mode: None }
    }

    pub fn requested_mode(&self) -> Option<ToplevelDecorationMode> {
        self.requested_mode
    }

    pub(crate) fn send_configure_mode(
        &self,
        mode: ToplevelDecorationMode,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            zxdg_toplevel_decoration_v1::Event::Configure { mode: encode_mode(mode) as u32 },
            dispatcher,
        );
    }

    fn root_surface(&self, _client: &Client, state: &State) -> Option<Resolved<WlSurfaceId>> {
        let toplevel = self.toplevel.try_ref(state)?;
        let xdg_surface = toplevel.xdg_surface().try_ref(state)?;
        Some(xdg_surface.wl_surface())
    }

    fn update_effective_mode(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        force_xdg_configure: bool,
        send_xdg_configure: bool,
    ) {
        let root_surface = self.root_surface(client, state);
        let effective_mode =
            state.xdg_decoration_mode_for_surface(root_surface, true, self.requested_mode);
        let Some(toplevel) = self.toplevel.try_ref_mut(state) else {
            return;
        };
        toplevel.schedule_decoration_mode(effective_mode);
        let xdg_surface = toplevel.xdg_surface();
        if !send_xdg_configure {
            return;
        }
        let configure_bounds = {
            let xdg_surface = xdg_surface.rref(state);
            XdgToplevel::configure_bounds_for_xdg_surface(client, state, xdg_surface)
        };
        let mut xdg_surface = xdg_surface.grab(state);
        if !xdg_surface.initial_commit_complete() {
            return;
        }
        let mut toplevel = self.toplevel.grab(state);
        if !force_xdg_configure
            && toplevel.decoration_mode_for_configure() == toplevel.decoration_mode()
        {
            return;
        }
        toplevel.schedule_configure_with_known_decoration(
            self,
            state,
            configure_bounds,
            &mut xdg_surface,
            dispatcher,
        );
    }

    pub fn initialize_negotiation(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.update_effective_mode(client, state, dispatcher, true, true);
    }

    fn revert_to_unnegotiated_mode(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let target_mode =
            state.xdg_decoration_mode_for_surface(self.root_surface(client, state), false, None);
        let Some(toplevel) = self.toplevel.try_ref_mut(state) else {
            return;
        };
        let changed = toplevel.schedule_decoration_mode(target_mode);
        let xdg_surface = toplevel.xdg_surface();
        if !changed {
            return;
        }
        let configure_bounds = {
            let xdg_surface = xdg_surface.rref(state);
            XdgToplevel::configure_bounds_for_xdg_surface(client, state, xdg_surface)
        };
        let mut xdg_surface = xdg_surface.grab(state);
        if !xdg_surface.initial_commit_complete() {
            return;
        }
        let mut toplevel = self.toplevel.grab(state);
        toplevel.schedule_configure(client, state, configure_bounds, &mut xdg_surface, dispatcher);
    }
}

fn decode_mode(raw_mode: u32) -> Option<ToplevelDecorationMode> {
    match raw_mode {
        value if value == zxdg_toplevel_decoration_v1::Mode::ClientSide as u32 => {
            Some(ToplevelDecorationMode::ClientSide)
        }
        value if value == zxdg_toplevel_decoration_v1::Mode::ServerSide as u32 => {
            Some(ToplevelDecorationMode::ServerSide)
        }
        _ => None,
    }
}

fn encode_mode(mode: ToplevelDecorationMode) -> zxdg_toplevel_decoration_v1::Mode {
    match mode {
        ToplevelDecorationMode::ClientSide => zxdg_toplevel_decoration_v1::Mode::ClientSide,
        ToplevelDecorationMode::ServerSide => zxdg_toplevel_decoration_v1::Mode::ServerSide,
    }
}

impl Object for ZxdgToplevelDecorationV1 {
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
        use zxdg_toplevel_decoration_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                if let Some(mut toplevel) = self.toplevel.try_grab(state) {
                    let this = client.resolve(self.id, state).unwrap();
                    toplevel.clear_decoration_object(this);
                }
                self.revert_to_unnegotiated_mode(client, state, dispatcher);
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            SetMode { mode } => {
                let Some(mode) = decode_mode(mode) else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        zxdg_toplevel_decoration_v1::Error::InvalidMode as u32,
                        "invalid zxdg_toplevel_decoration_v1 mode",
                    );
                    return Ok(());
                };
                self.requested_mode = Some(mode);
                self.update_effective_mode(client, state, dispatcher, true, true);
            }
            UnsetMode => {
                self.requested_mode = None;
                self.update_effective_mode(client, state, dispatcher, true, true);
            }
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        client: &mut Client,
        state: &mut State,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let Some(decoration) = client.resolve(self.id, state) else {
            return;
        };
        if let Some(mut toplevel) = self.toplevel.try_grab(state) {
            toplevel.clear_decoration_object(decoration);
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            clients::{Client, ClientId, RawEvent},
            protocol_generated::{WlSurfaceId, XdgSurfaceId, XdgToplevelId, XdgWmBaseId},
            runner::Dispatcher,
            state::{GrabbedObject, ObjectId, State},
            test_support::state_for_tests,
            wayland_objects::wl_surface::{Role, WlSurface},
        },
    };

    fn set_mode_request(
        decoration_id: ZxdgToplevelDecorationV1Id,
        mode: zxdg_toplevel_decoration_v1::Mode,
    ) -> RawRequest<'static> {
        let mut event = RawEvent::new(decoration_id.into(), 1, 1);
        event.uint(mode as u32);
        (&*event.into_bytes()).try_into().unwrap()
    }

    #[test]
    fn set_mode_after_initial_configure_queues_xdg_configure_when_policy_is_unchanged() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(90));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let wl_surface_id = WlSurfaceId::from(10u32);
        let xdg_surface_id = XdgSurfaceId::from(11u32);
        let toplevel_id = XdgToplevelId::from(12u32);
        let decoration_id = ZxdgToplevelDecorationV1Id::from(13u32);

        let (surface_resolved, _) =
            client.insert_object(&mut state, WlSurface::new(wl_surface_id, client.client_id(), 6));
        let surface_object_id = surface_resolved.object_id();
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            assert!(surface
                .set_role(Role::XdgSurface, &mut client_dispatcher)
                .is_ok());
        }
        let (xdg_surface_resolved, _) = client.insert_object(
            &mut state,
            XdgSurface::new(
                xdg_surface_id,
                crate::clients::Resolved::new(
                    client.client_id(),
                    XdgWmBaseId::from(1),
                    ObjectId::from(1usize),
                ),
                crate::clients::Resolved::new(client.client_id(), wl_surface_id, surface_object_id),
                7,
            ),
        );
        let xdg_surface_object_id = xdg_surface_resolved.object_id();
        state
            .objects
            .get_mut(surface_object_id)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_xdg_surface_impl(crate::clients::Resolved::new(
                client.client_id(),
                xdg_surface_id,
                xdg_surface_object_id,
            ));
        {
            let xdg_surface = state
                .objects
                .get_mut(xdg_surface_object_id)
                .downcast_mut::<XdgSurface>()
                .unwrap();
            xdg_surface.test_set_initial_commit_complete(true);
        }
        let (toplevel_resolved, _) = client.insert_object(
            &mut state,
            XdgToplevel::new(
                toplevel_id,
                Resolved::new(client.client_id(), xdg_surface_id, xdg_surface_object_id),
                7,
            ),
        );
        let toplevel_object_id = toplevel_resolved.object_id();
        state.register_xdg_toplevel_role(xdg_surface_resolved, toplevel_resolved);
        let (decoration_resolved, _) = client.insert_object(
            &mut state,
            ZxdgToplevelDecorationV1::new(decoration_id, toplevel_resolved),
        );
        let decoration_object_id = decoration_resolved.object_id();
        {
            let toplevel = state
                .objects
                .get_mut(toplevel_object_id)
                .downcast_mut::<XdgToplevel>()
                .unwrap();
            assert!(toplevel.set_decoration_object(Resolved::new(
                client.client_id(),
                decoration_id,
                decoration_object_id,
            )));
        }

        {
            let mut decoration = client.grab_object(decoration_id, &mut state);
            decoration.on(
                &mut client,
                &mut state,
                set_mode_request(decoration_id, zxdg_toplevel_decoration_v1::Mode::ClientSide),
                &mut client_dispatcher,
            );
        }

        let xdg_surface = state
            .objects
            .try_ref_object::<XdgSurface>(xdg_surface_object_id)
            .unwrap();
        assert_eq!(xdg_surface.test_pending_configure_count(), 1);
    }

    #[test]
    fn unset_mode_after_initial_configure_queues_xdg_configure_when_policy_is_unchanged() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(91));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let wl_surface_id = WlSurfaceId::from(20u32);
        let xdg_surface_id = XdgSurfaceId::from(21u32);
        let toplevel_id = XdgToplevelId::from(22u32);
        let decoration_id = ZxdgToplevelDecorationV1Id::from(23u32);

        let (surface_resolved, _) =
            client.insert_object(&mut state, WlSurface::new(wl_surface_id, client.client_id(), 6));
        let surface_object_id = surface_resolved.object_id();
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            assert!(surface
                .set_role(Role::XdgSurface, &mut client_dispatcher)
                .is_ok());
        }
        let (xdg_surface_resolved, _) = client.insert_object(
            &mut state,
            XdgSurface::new(
                xdg_surface_id,
                crate::clients::Resolved::new(
                    client.client_id(),
                    XdgWmBaseId::from(1),
                    ObjectId::from(1usize),
                ),
                crate::clients::Resolved::new(client.client_id(), wl_surface_id, surface_object_id),
                7,
            ),
        );
        let xdg_surface_object_id = xdg_surface_resolved.object_id();
        state
            .objects
            .get_mut(surface_object_id)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_xdg_surface_impl(crate::clients::Resolved::new(
                client.client_id(),
                xdg_surface_id,
                xdg_surface_object_id,
            ));
        {
            let xdg_surface = state
                .objects
                .get_mut(xdg_surface_object_id)
                .downcast_mut::<XdgSurface>()
                .unwrap();
            xdg_surface.test_set_initial_commit_complete(true);
        }
        let (toplevel_resolved, _) = client.insert_object(
            &mut state,
            XdgToplevel::new(
                toplevel_id,
                Resolved::new(client.client_id(), xdg_surface_id, xdg_surface_object_id),
                7,
            ),
        );
        let toplevel_object_id = toplevel_resolved.object_id();
        state.register_xdg_toplevel_role(xdg_surface_resolved, toplevel_resolved);
        let (decoration_resolved, _) = client.insert_object(
            &mut state,
            ZxdgToplevelDecorationV1::new(decoration_id, toplevel_resolved),
        );
        let decoration_object_id = decoration_resolved.object_id();
        {
            let toplevel = state
                .objects
                .get_mut(toplevel_object_id)
                .downcast_mut::<XdgToplevel>()
                .unwrap();
            assert!(toplevel.set_decoration_object(Resolved::new(
                client.client_id(),
                decoration_id,
                decoration_object_id,
            )));
        }

        {
            let mut decoration = client.grab_object(decoration_id, &mut state);
            decoration.on(
                &mut client,
                &mut state,
                (&*RawEvent::new(decoration_id.into(), 2, 0).into_bytes())
                    .try_into()
                    .unwrap(),
                &mut client_dispatcher,
            );
        }

        let xdg_surface = state
            .objects
            .try_ref_object::<XdgSurface>(xdg_surface_object_id)
            .unwrap();
        assert_eq!(xdg_surface.test_pending_configure_count(), 1);
    }
}
