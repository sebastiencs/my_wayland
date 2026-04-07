use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wp_fractional_scale_manager_v1, Constants, NewId, WaylandId, WlSurfaceId,
        WpFractionalScaleManagerV1Id, WpFractionalScaleV1Id,
    },
    state::{GrabbedObject, State},
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
        mut self: GrabbedObject<Self>,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        use wp_fractional_scale_manager_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetFractionalScale { id, surface } => {
                let id = WpFractionalScaleV1Id::from(id);
                let surface_resolved =
                    client.resolve_or_disconnect(WlSurfaceId::from(surface), state, dispatcher)?;
                {
                    let surface = surface_resolved.try_ref(state);
                    if surface.is_some_and(|s| s.has_fractional_scale_object()) {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            wp_fractional_scale_manager_v1::Error::FractionalScaleExists as u32,
                            "wl_surface already has a wp_fractional_scale_v1 object",
                        );
                        return Ok(());
                    }
                }

                let fractional = WpFractionalScaleV1::new(id, surface_resolved);
                client.insert_object(state, fractional);
                let object = client.resolve(id, state).unwrap();
                {
                    let mut surface = surface_resolved.grab(state);
                    let _ = surface.set_fractional_scale_object(object);
                }
                let mut surface = surface_resolved.grab(state);
                surface.send_preferred_fractional_scale_for_object(
                    state,
                    surface_resolved,
                    dispatcher,
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        clients::{Client, ClientManagerAction, EventWriter},
        io_uring::model::IoUringAction,
        protocol_generated::{WlRegionId, WpFractionalScaleManagerV1Id},
        runner::Dispatcher,
        state::State,
        test_support::state_for_tests,
        wayland_objects::wl_region::WlRegion,
    };

    use super::*;

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
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(crate::clients::ClientId::from_raw(7));
        let manager_id = WpFractionalScaleManagerV1Id::from(3u32);
        let fractional_id = WpFractionalScaleV1Id::from(4u32);
        let region_id = WlRegionId::from(5u32);
        client.insert_object(&mut state, WpFractionalScaleManagerV1 { id: manager_id });
        client.insert_object(&mut state, WlRegion::new(region_id));

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client_dispatcher.set_requester_object_id_for_tests(manager_id.into());

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
        assert!(dispatcher.test_has_queued_action::<ClientManagerAction>(|action| {
            matches!(action, ClientManagerAction::Close { id } if *id == client.client_id())
        }));
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));
    }

    #[test]
    fn get_fractional_scale_does_not_iterate_objects_while_surface_is_grabbed() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(crate::clients::ClientId::from_raw(8));
        let manager_id = WpFractionalScaleManagerV1Id::from(3u32);
        let fractional_id = WpFractionalScaleV1Id::from(4u32);
        let surface_id = WlSurfaceId::from(5u32);
        client.insert_object(&mut state, WpFractionalScaleManagerV1 { id: manager_id });
        client.insert_object(&mut state, WlSurface::new(surface_id, client.client_id(), 6));

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        {
            let mut manager = client.grab_object(manager_id, &mut state);
            manager.on(
                &mut client,
                &mut state,
                get_fractional_scale_request(manager_id, fractional_id, surface_id.into()),
                &mut client_dispatcher,
            );
        }
        drop(client_dispatcher);

        assert!(client.contains_object(fractional_id, &state));
    }
}
