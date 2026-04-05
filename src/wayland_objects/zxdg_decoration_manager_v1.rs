use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{
        wl_display, zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1, Constants, NewId,
        WaylandId, XdgToplevelId, ZxdgDecorationManagerV1Id, ZxdgToplevelDecorationV1Id,
    },
    state::State,
    wayland_objects::{
        wl_display::WlDisplay, wl_surface::WlSurface, xdg_surface::xdg_toplevel::XdgToplevel,
        zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    },
};

#[derive(Debug)]
pub struct ZxdgDecorationManagerV1Global;

impl ZxdgDecorationManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ZxdgDecorationManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (ZxdgDecorationManagerV1::INTERFACE_NAME, ZxdgDecorationManagerV1::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = ZxdgDecorationManagerV1Id::from(id);
        let version = version.min(ZxdgDecorationManagerV1::INTERFACE_VERSION);
        client.insert_object(state, ZxdgDecorationManagerV1 { id, version });
    }
}

#[derive(Debug)]
pub struct ZxdgDecorationManagerV1 {
    id: ZxdgDecorationManagerV1Id,
    version: u32,
}

impl Object for ZxdgDecorationManagerV1 {
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
        use zxdg_decoration_manager_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetToplevelDecoration { id, toplevel } => {
                let id = ZxdgToplevelDecorationV1Id::from(id);
                let toplevel = client.resolve_or_disconnect(toplevel, state, dispatcher)?;
                let toplevel_ref = toplevel.rref(state);
                if toplevel_ref.has_decoration_object() {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        zxdg_toplevel_decoration_v1::Error::AlreadyConstructed as u32,
                        "xdg_toplevel already has a zxdg_toplevel_decoration_v1 object",
                    );
                    return Ok(());
                }
                let Some(xdg_surface_ref) = toplevel_ref.xdg_surface().try_ref(state) else {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidObject as u32,
                        "get_toplevel_decoration requires a live xdg_surface",
                    );
                    return Ok(());
                };
                let wl_surface_object = xdg_surface_ref.wl_surface();
                if wl_surface_object
                    .try_ref(state)
                    .is_some_and(WlSurface::has_attached_or_committed_buffer)
                {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        zxdg_toplevel_decoration_v1::Error::UnconfiguredBuffer as u32,
                        "xdg_toplevel_decoration must not have a buffer at creation",
                    );
                    return Ok(());
                }

                let decoration = ZxdgToplevelDecorationV1::new(id, toplevel);
                let (decoration_resolved, _) = client.insert_object(state, decoration);
                {
                    let mut toplevel = toplevel.grab(state);
                    assert!(toplevel.set_decoration_object(decoration_resolved));
                }
                let mut decoration = client.grab_object(id, state);
                decoration.initialize_negotiation(client, state, dispatcher);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            clients::{Client, ClientId, RawEvent},
            drm::ARGB8888,
            egl::EGL,
            globals::Globals,
            io_uring::model::IoUringAction,
            protocol_generated::{WlBufferId, WlSurfaceId, XdgSurfaceId, XdgWmBaseId},
            runner::Dispatcher,
            state::{ObjectId, State},
            wayland_objects::{
                wl_buffer::{NewWlBufferParams, WlBuffer},
                wl_shm_pool::shared_memory_from_bytes,
                wl_surface::Role,
                xdg_surface::XdgSurface,
            },
            xkb::Xkb,
        },
    };

    fn make_state() -> State {
        let xkb = Xkb::new();
        let keyboard = xkb.create_keyboard_state();
        let mut globals = Globals::new();
        globals.initialize();
        State {
            objects: Default::default(),
            root_surfaces: Default::default(),
            layer_surface_objects: Default::default(),
            inputs: Default::default(),
            drm: Default::default(),
            outputs: Default::default(),
            active_output: None,
            workspace_groups: Default::default(),
            workspaces: Default::default(),
            output_workspace_groups: Default::default(),
            root_surface_workspaces: Default::default(),
            managed_root_surfaces: Default::default(),
            xdg_toplevel_roles: Default::default(),
            ctm_control_owner: None,
            data_offers_by_source: Default::default(),
            next_workspace_serial: 1,
            backend_syncobj_globals: Default::default(),
            globals,
            activation_tokens: Default::default(),
            egl: EGL::new(),
            config: Default::default(),
            seat: Default::default(),
            keyboard,
            xkb,
        }
    }

    fn insert_test_buffer(state: &mut State, client_id: ClientId) -> ObjectId {
        let bytes = [0u8; 16];
        state
            .insert_object(
                WlBuffer::new(NewWlBufferParams {
                    id: WlBufferId::from(40),
                    owner_client_id: client_id,
                    offset: 0,
                    width: 2,
                    height: 2,
                    stride: 8,
                    format: &ARGB8888,
                    mem: shared_memory_from_bytes(&bytes),
                })
                .unwrap(),
            )
            .1
    }

    fn get_toplevel_decoration_request(
        manager_id: ZxdgDecorationManagerV1Id,
        decoration_id: ZxdgToplevelDecorationV1Id,
        toplevel_id: XdgToplevelId,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(manager_id.into(), 1, 2);
        event.id(decoration_id);
        event.id(toplevel_id);
        (&*event.into_bytes()).try_into().unwrap()
    }

    #[test]
    fn get_toplevel_decoration_rejects_surfaces_with_existing_buffer() {
        let mut state = make_state();
        let mut client = Client::new_for_tests(ClientId::from_raw(8));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let manager_id = ZxdgDecorationManagerV1Id::from(1u32);
        let wl_surface_id = WlSurfaceId::from(2u32);
        let xdg_surface_id = XdgSurfaceId::from(3u32);
        let toplevel_id = XdgToplevelId::from(4u32);
        let decoration_id = ZxdgToplevelDecorationV1Id::from(5u32);

        client.insert_object(&mut state, ZxdgDecorationManagerV1 { id: manager_id, version: 1 });
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
                Resolved::new(client.client_id(), XdgWmBaseId::from(6), ObjectId::from(6usize)),
                Resolved::new(client.client_id(), wl_surface_id, surface_object_id),
                7,
            ),
        );
        let xdg_surface_object_id = xdg_surface_resolved.object_id();
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            surface.set_xdg_surface_impl(Resolved::new(
                client.client_id(),
                xdg_surface_id,
                xdg_surface_object_id,
            ));
            let buffer_object_id = insert_test_buffer(&mut state, client.client_id());
            surface.test_map(buffer_object_id, 0, 0, 2, 2);
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

        {
            let mut manager = client.grab_object(manager_id, &mut state);
            manager.on(
                &mut client,
                &mut state,
                get_toplevel_decoration_request(manager_id, decoration_id, toplevel_id),
                &mut client_dispatcher,
            );
        }
        drop(client_dispatcher);

        assert!(!client.contains_object(decoration_id, &state));
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));
    }
}
