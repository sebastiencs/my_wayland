use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{
        wp_fractional_scale_manager_v1, Constants, NewId, WaylandId, WlSurfaceId,
        WpFractionalScaleManagerV1Id, WpFractionalScaleV1Id,
    },
    state::State,
    wayland_objects::{
        wl_display::WlDisplay, wl_surface::WlSurface, wp_fractional_scale_v1::WpFractionalScaleV1,
    },
};

#[derive(Debug)]
pub struct WpFractionalScaleManagerV1Global;

impl WpFractionalScaleManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for WpFractionalScaleManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (WpFractionalScaleManagerV1::INTERFACE_NAME, WpFractionalScaleManagerV1::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        client.insert_object(state, WpFractionalScaleManagerV1 { id: id.into() });
    }
}

#[derive(Debug)]
pub struct WpFractionalScaleManagerV1 {
    id: WpFractionalScaleManagerV1Id,
}

impl Object for WpFractionalScaleManagerV1 {
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
        use wp_fractional_scale_manager_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetFractionalScale { id, surface } => {
                let id = WpFractionalScaleV1Id::from(id);
                let surface = WlSurfaceId::from(surface);
                if !client.object_is(surface, state) {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        crate::protocol_generated::wl_display::Error::InvalidObject as u32,
                        "invalid wl_surface for wp_fractional_scale_manager_v1.get_fractional_scale",
                    );
                    return;
                }
                let surface_oid = client.object_id(surface);
                {
                    let surface = state.objects.try_ref_object::<WlSurface>(surface_oid);
                    if surface.is_some_and(|s| s.has_fractional_scale_object()) {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            wp_fractional_scale_manager_v1::Error::FractionalScaleExists as u32,
                            "wl_surface already has a wp_fractional_scale_v1 object",
                        );
                        return;
                    }
                }

                let fractional = WpFractionalScaleV1::new(id, surface_oid);
                let ((_, object_id), _) = client.insert_object(state, fractional);
                {
                    let mut surface = state.objects.grab_object::<WlSurface>(surface_oid);
                    let _ = surface.set_fractional_scale_object(object_id);
                }
                let mut surface = state.objects.grab_object::<WlSurface>(surface_oid);
                surface.send_preferred_fractional_scale(state, dispatcher);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        clients::{Client, ClientsAction, EventWriter},
        egl::EGL,
        globals::Globals,
        io_uring::model::IoUringAction,
        protocol_generated::{WlRegionId, WpFractionalScaleManagerV1Id},
        runner::Dispatcher,
        state::State,
        wayland_objects::wl_region::WlRegion,
        xkb::Xkb,
    };

    use super::*;

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

    fn get_fractional_scale_request(
        manager_id: WpFractionalScaleManagerV1Id,
        id: WpFractionalScaleV1Id,
        surface: u32,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = crate::clients::RawEvent::new(manager_id.into(), 1, 2);
        event.id(id);
        event.uint(surface);
        (&*event.into_bytes()).try_into().unwrap()
    }

    #[test]
    fn get_fractional_scale_rejects_non_surface_objects() {
        let mut state = make_state();
        let mut client = Client::new_for_tests(crate::clients::ClientId::from_raw(7));
        let manager_id = WpFractionalScaleManagerV1Id::from(3u32);
        let fractional_id = WpFractionalScaleV1Id::from(4u32);
        let region_id = WlRegionId::from(5u32);
        client.insert_object(&mut state, WpFractionalScaleManagerV1 { id: manager_id });
        client.insert_object(&mut state, WlRegion::new(region_id));

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        {
            let mut manager = client.grab_object(manager_id, &mut state);
            manager.on(
                &mut client,
                &mut state,
                get_fractional_scale_request(manager_id, fractional_id, region_id.into()),
                &mut client_dispatcher,
            );
        }
        drop(client_dispatcher);

        assert!(!client.contains_object(fractional_id, &state));
        assert!(dispatcher.test_has_queued_action::<ClientsAction>(|action| {
            matches!(action, ClientsAction::Close { id } if *id == client.client_id())
        }));
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));
    }
}
