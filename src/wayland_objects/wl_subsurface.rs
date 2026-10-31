use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    drm::{request_active_output, request_outputs_for_rect},
    object::{Object, Parser, RequestResult},
    protocol_generated::{wl_subsurface, WaylandId, WlSubsurfaceId, WlSurfaceId},
    state::{GrabbedObject, ObjectId, State},
    wayland_objects::wl_surface::WlSurface,
};

#[derive(Debug)]
pub struct WlSubsurface {
    id: WlSubsurfaceId,
    surface: Resolved<WlSurfaceId>,
    parent: Resolved<WlSurfaceId>,
}

impl WlSubsurface {
    pub fn new(
        id: WlSubsurfaceId,
        surface: Resolved<WlSurfaceId>,
        parent: Resolved<WlSurfaceId>,
    ) -> Self {
        Self { id, surface, parent }
    }

    fn sibling_target(
        &self,
        state: &State,
        sibling_surface: Resolved<WlSurfaceId>,
    ) -> Result<Option<Resolved<WlSurfaceId>>, ()> {
        let sibling_surface_ref = sibling_surface.rref(state);
        if sibling_surface == self.surface {
            return Err(());
        }
        if sibling_surface == self.parent {
            return Ok(None);
        }
        if sibling_surface_ref
            .parent_surface()
            .map(|surface| surface.object_id())
            != Some(self.parent.object_id())
        {
            return Err(());
        }
        Ok(Some(sibling_surface))
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
        mut self: GrabbedObject<Self>,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        use wl_subsurface::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                let old_root_surface = state.root_surface_for_surface(self.parent);
                if let Some(mut surface) = self.surface.try_grab(state) {
                    surface.unmap_due_to_parent_loss(client, state, dispatcher);
                    surface.destroy_subsurface_role();
                }
                if let Some(mut parent_surface) = self.parent.try_grab(state) {
                    parent_surface.remove_child_from_tree(self.surface);
                }
                state.register_root_surface(self.surface);
                if let Some(root_surface) = old_root_surface {
                    state.mark_root_geometry_dirty_for_surface(root_surface);
                }
                let child_root_surface = self.surface.clone();
                state.mark_root_geometry_dirty_for_surface(child_root_surface);
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
                if let Some(bounds) = state
                    .root_surface_for_surface(self.surface)
                    .and_then(|root_surface| state.root_surface_surface_bounds_global(root_surface))
                {
                    request_outputs_for_rect(state, bounds, dispatcher);
                } else {
                    request_active_output(state, dispatcher);
                }
            }
            SetPosition { x, y } => {
                let Some(mut surface) = self.surface.try_grab(state) else {
                    return Ok(());
                };
                surface.set_subsurface_position(x, y);
                drop(surface);
                state.mark_root_geometry_dirty_for_surface(self.parent);
            }
            PlaceAbove { sibling } => {
                let sibling = client.resolve_or_disconnect(sibling, state, dispatcher)?;
                let Ok(sibling_surface) = self.sibling_target(state, sibling) else {
                    self.bad_surface(
                        client,
                        dispatcher,
                        "place_above sibling must be the parent or a sibling surface",
                    );
                    return Ok(());
                };
                let Some(mut parent_surface) = self.parent.try_grab(state) else {
                    return Ok(());
                };
                if !parent_surface.restack_child_relative(self.surface, sibling_surface, true) {
                    self.bad_surface(
                        client,
                        dispatcher,
                        "place_above sibling must be in the same parent stack",
                    );
                }
            }
            PlaceBelow { sibling } => {
                let sibling = client.resolve_or_disconnect(sibling, state, dispatcher)?;
                let Ok(sibling_surface) = self.sibling_target(state, sibling) else {
                    self.bad_surface(
                        client,
                        dispatcher,
                        "place_below sibling must be the parent or a sibling surface",
                    );
                    return Ok(());
                };
                let Some(mut parent_surface) = self.parent.try_grab(state) else {
                    return Ok(());
                };
                if !parent_surface.restack_child_relative(self.surface, sibling_surface, false) {
                    self.bad_surface(
                        client,
                        dispatcher,
                        "place_below sibling must be in the same parent stack",
                    );
                }
            }
            SetSync => {
                let Some(mut surface) = self.surface.try_grab(state) else {
                    return Ok(());
                };
                surface.set_subsurface_sync_mode(true);
            }
            SetDesync => {
                let Some(mut surface) = self.surface.try_grab(state) else {
                    return Ok(());
                };
                surface.set_subsurface_sync_mode(false);
                surface.apply_cached_if_parent_desynchronized(client, state, dispatcher);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {
        super::WlSubsurface,
        crate::{
            clients::{Client, ClientId, RawEvent, Resolved},
            object::{Object, RequestResult},
            protocol_generated::{WlSubsurfaceId, WlSurfaceId},
            runner::Dispatcher,
            state::{GrabbedObject, State},
            test_support::{state_for_tests, test_client_and_dispatcher},
            wayland_objects::wl_surface::{StackEntry, WlSurface},
        },
    };

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
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(5);
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (parent_resolved, _) = client.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(10), client.client_id(), 6),
        );
        let parent_object_id = parent_resolved.object_id();
        state.register_root_surface(parent_resolved);
        let (child_resolved, _) = client.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(11), client.client_id(), 6),
        );
        let child_object_id = child_resolved.object_id();
        state.register_root_surface(child_resolved);
        let (stranger_resolved, _) = client.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(12), client.client_id(), 6),
        );
        let stranger_object_id = stranger_resolved.object_id();
        state.register_root_surface(stranger_resolved);
        let (subsurface_resolved, _) = client.insert_object(
            &mut state,
            WlSubsurface::new(
                WlSubsurfaceId::from(30),
                Resolved::new(client.client_id(), WlSurfaceId::from(11), child_object_id),
                Resolved::new(client.client_id(), WlSurfaceId::from(10), parent_object_id),
            ),
        );
        let subsurface_object_id = subsurface_resolved.object_id();
        let subsurface =
            Resolved::new(client.client_id(), WlSubsurfaceId::from(30), subsurface_object_id);
        let child_resolved =
            Resolved::new(client.client_id(), WlSurfaceId::from(11), child_object_id);

        {
            let parent = state
                .objects
                .get_mut(parent_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            parent.add_pending_child(child_resolved);
        }
        {
            let child = state
                .objects
                .get_mut(child_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child.assign_subsurface(
                subsurface,
                Resolved::new(client.client_id(), WlSurfaceId::from(10), parent_object_id),
            );
        }

        let subsurface = state
            .objects
            .grab_object::<WlSubsurface>(subsurface_object_id);
        subsurface.on(
            &mut client,
            &mut state,
            place_above_request(WlSubsurfaceId::from(30), WlSurfaceId::from(12)),
            &mut client_dispatcher,
        );

        let mut parent = state.objects.grab_object::<WlSurface>(parent_object_id);
        assert!(parent.apply_pending_tree_state(&mut state));
        assert_eq!(
            parent.current_stack(),
            &[StackEntry::Child(child_resolved), StackEntry::Parent]
        );
    }
}
