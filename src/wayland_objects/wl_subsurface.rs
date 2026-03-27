use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    drm::{schedule_active_output, schedule_outputs_for_rect},
    object::{Object, Parser},
    protocol_generated::{wl_subsurface, WaylandId, WlSubsurfaceId, WlSurfaceId},
    state::{ObjectId, State},
    wayland_objects::wl_surface::WlSurface,
};

#[derive(Debug)]
pub struct WlSubsurface {
    id: WlSubsurfaceId,
    surface_object_id: ObjectId,
    parent_surface_object_id: ObjectId,
}

impl WlSubsurface {
    pub fn new(
        id: WlSubsurfaceId,
        surface_object_id: ObjectId,
        parent_surface_object_id: ObjectId,
    ) -> Self {
        Self { id, surface_object_id, parent_surface_object_id }
    }

    fn sibling_target(
        &self,
        client: &Client,
        state: &State,
        sibling: WlSurfaceId,
    ) -> Result<Option<ObjectId>, ()> {
        let Some(sibling_object_id) = client.try_object_id(sibling, state) else {
            return Err(());
        };
        let Some(sibling_surface) = state.objects.try_ref_object::<WlSurface>(sibling_object_id)
        else {
            return Err(());
        };
        if sibling_object_id == self.surface_object_id {
            return Err(());
        }
        if sibling_object_id == self.parent_surface_object_id {
            return Ok(None);
        }
        if sibling_surface.parent_surface_object_id() != Some(self.parent_surface_object_id) {
            return Err(());
        }
        Ok(Some(sibling_object_id))
    }

    fn bad_surface(&self, client: &Client, dispatcher: &mut ClientDispatcher, message: &str) {
        client.send_protocol_error(
            dispatcher,
            self.id.into(),
            wl_subsurface::Error::BadSurface as u32,
            message,
        );
    }
}

impl Object for WlSubsurface {
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
        use wl_subsurface::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                if let Some(mut surface) = state
                    .objects
                    .try_grab_object::<WlSurface>(self.surface_object_id)
                {
                    surface.unmap_due_to_parent_loss(client, state, dispatcher);
                    surface.destroy_subsurface_role();
                }
                if let Some(mut parent_surface) = state
                    .objects
                    .try_grab_object::<WlSurface>(self.parent_surface_object_id)
                {
                    parent_surface.remove_child_from_tree(self.surface_object_id);
                }
                state.register_root_surface(self.surface_object_id);
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
                if let Some(bounds) = state
                    .objects
                    .try_ref_object::<WlSurface>(self.surface_object_id)
                    .and_then(|surface| surface.global_logical_bounds())
                {
                    schedule_outputs_for_rect(state, bounds, dispatcher);
                } else {
                    schedule_active_output(state, dispatcher);
                }
            }
            SetPosition { x, y } => {
                let Some(mut surface) = state
                    .objects
                    .try_grab_object::<WlSurface>(self.surface_object_id)
                else {
                    return;
                };
                surface.set_subsurface_position(x, y);
            }
            PlaceAbove { sibling } => {
                let Ok(sibling_object_id) = self.sibling_target(client, state, sibling) else {
                    self.bad_surface(
                        client,
                        dispatcher,
                        "place_above sibling must be the parent or a sibling surface",
                    );
                    return;
                };
                let Some(mut parent_surface) = state
                    .objects
                    .try_grab_object::<WlSurface>(self.parent_surface_object_id)
                else {
                    return;
                };
                if !parent_surface.restack_child_relative(
                    self.surface_object_id,
                    sibling_object_id,
                    true,
                ) {
                    self.bad_surface(
                        client,
                        dispatcher,
                        "place_above sibling must be in the same parent stack",
                    );
                }
            }
            PlaceBelow { sibling } => {
                let Ok(sibling_object_id) = self.sibling_target(client, state, sibling) else {
                    self.bad_surface(
                        client,
                        dispatcher,
                        "place_below sibling must be the parent or a sibling surface",
                    );
                    return;
                };
                let Some(mut parent_surface) = state
                    .objects
                    .try_grab_object::<WlSurface>(self.parent_surface_object_id)
                else {
                    return;
                };
                if !parent_surface.restack_child_relative(
                    self.surface_object_id,
                    sibling_object_id,
                    false,
                ) {
                    self.bad_surface(
                        client,
                        dispatcher,
                        "place_below sibling must be in the same parent stack",
                    );
                }
            }
            SetSync => {
                let Some(mut surface) = state
                    .objects
                    .try_grab_object::<WlSurface>(self.surface_object_id)
                else {
                    return;
                };
                surface.set_subsurface_sync_mode(true);
            }
            SetDesync => {
                let Some(mut surface) = state
                    .objects
                    .try_grab_object::<WlSurface>(self.surface_object_id)
                else {
                    return;
                };
                surface.set_subsurface_sync_mode(false);
                surface.apply_cached_if_parent_desynchronized(client, state, dispatcher);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::WlSubsurface,
        crate::{
            clients::{Client, ClientId, RawEvent},
            egl::EGL,
            globals::Globals,
            object::Object,
            protocol_generated::{WlSubsurfaceId, WlSurfaceId},
            runner::Dispatcher,
            state::State,
            wayland_objects::wl_surface::{StackEntry, WlSurface},
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
            backend_syncobj_globals: Default::default(),
            globals,
            activation_tokens: Default::default(),
            egl: EGL::new(),
            seat: Default::default(),
            keyboard,
            xkb,
        }
    }

    fn make_client() -> (Client, Dispatcher) {
        (Client::new_for_tests(ClientId::from_raw(5)), Dispatcher::default())
    }

    fn place_above_request(
        object_id: WlSubsurfaceId,
        sibling: WlSurfaceId,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(object_id.into(), 2, 1);
        event.id(sibling);
        (&*event.into_bytes()).try_into().unwrap()
    }

    #[test]
    fn place_above_rejects_non_sibling_surface() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (_, parent_object_id) = client
            .insert_object(&mut state, WlSurface::new(WlSurfaceId::from(10), client.client_id(), 6))
            .0;
        state.register_root_surface(parent_object_id);
        let (_, child_object_id) = client
            .insert_object(&mut state, WlSurface::new(WlSurfaceId::from(11), client.client_id(), 6))
            .0;
        state.register_root_surface(child_object_id);
        let (_, stranger_object_id) = client
            .insert_object(&mut state, WlSurface::new(WlSurfaceId::from(12), client.client_id(), 6))
            .0;
        state.register_root_surface(stranger_object_id);
        let ((_, subsurface_object_id), _) = client.insert_object(
            &mut state,
            WlSubsurface::new(WlSubsurfaceId::from(30), child_object_id, parent_object_id),
        );

        {
            let parent = state
                .objects
                .get_mut(parent_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            parent.add_pending_child(child_object_id);
        }
        {
            let child = state
                .objects
                .get_mut(child_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child.assign_subsurface(WlSubsurfaceId::from(30), parent_object_id);
        }

        let mut subsurface = state
            .objects
            .grab_object::<WlSubsurface>(subsurface_object_id);
        subsurface.on(
            &mut client,
            &mut state,
            place_above_request(WlSubsurfaceId::from(30), WlSurfaceId::from(12)),
            &mut client_dispatcher,
        );
        drop(subsurface);

        let mut parent = state.objects.grab_object::<WlSurface>(parent_object_id);
        assert!(parent.apply_pending_tree_state(&mut state));
        assert_eq!(
            parent.current_stack(),
            &[StackEntry::Child(child_object_id), StackEntry::Parent]
        );
    }
}
