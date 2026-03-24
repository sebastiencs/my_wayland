use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{wl_subcompositor, Constants, NewId, WaylandId, WlSubcompositorId},
    state::State,
    wayland_objects::{wl_subsurface::WlSubsurface, wl_surface::WlSurface},
};

#[derive(Debug)]
pub struct WlSubcompositorGlobal;

impl WlSubcompositorGlobal {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for WlSubcompositorGlobal {
    fn interface(&self) -> (&'static str, u32) {
        (WlSubcompositor::INTERFACE_NAME, WlSubcompositor::INTERFACE_VERSION)
    }
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = WlSubcompositorId::from(id);
        client.insert_object(state, WlSubcompositor { id });
    }
}

#[derive(Debug)]
pub struct WlSubcompositor {
    id: WlSubcompositorId,
}

impl WlSubcompositor {
    pub fn new(id: WlSubcompositorId) -> Self {
        Self { id }
    }

    fn delete_self(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        use crate::wayland_objects::wl_display::WlDisplay;
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }
}

impl Object for WlSubcompositor {
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
        use wl_subcompositor::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => self.delete_self(client, state, dispatcher),
            GetSubsurface { id, surface, parent } => {
                if !client.object_is(surface, state) {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wl_subcompositor::Error::BadSurface as u32,
                        "get_subsurface surface must be a live wl_surface",
                    );
                    return;
                }
                if !client.object_is(parent, state) {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wl_subcompositor::Error::BadParent as u32,
                        "get_subsurface parent must be a live wl_surface",
                    );
                    return;
                }

                let surface_object_id = client.object_id(surface);
                let parent_surface_object_id = client.object_id(parent);
                if surface_object_id == parent_surface_object_id {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wl_subcompositor::Error::BadParent as u32,
                        "get_subsurface parent must be different from the child surface",
                    );
                    return;
                }

                {
                    let surface_ref = client.get_object(surface, state);
                    if surface_ref.has_role() || surface_ref.subsurface_object().is_some() {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            wl_subcompositor::Error::BadSurface as u32,
                            "get_subsurface surface already has a role or subsurface object",
                        );
                        return;
                    }
                }

                {
                    let Some(surface_object) = state.objects.try_get(surface_object_id) else {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            wl_subcompositor::Error::BadSurface as u32,
                            "get_subsurface surface disappeared during validation",
                        );
                        return;
                    };
                    let Some(surface_ref) = surface_object.downcast_ref::<WlSurface>() else {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            wl_subcompositor::Error::BadSurface as u32,
                            "get_subsurface surface is not a wl_surface",
                        );
                        return;
                    };
                    if surface_ref.is_ancestor_of(
                        surface_object_id,
                        state,
                        parent_surface_object_id,
                    ) {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            wl_subcompositor::Error::BadParent as u32,
                            "get_subsurface parent must not be a descendant of the child surface",
                        );
                        return;
                    }
                }

                client.insert_object(
                    state,
                    WlSubsurface::new(id, surface_object_id, parent_surface_object_id),
                );
                state.unregister_root_surface(surface_object_id);

                {
                    let mut surface_ref = client.grab_object(surface, state);
                    surface_ref.assign_subsurface(id, parent_surface_object_id);
                }
                {
                    let mut parent_ref = client.grab_object(parent, state);
                    parent_ref.add_pending_child(surface_object_id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::WlSubcompositor,
        crate::{
            clients::{Client, ClientId, RawEvent},
            egl::EGL,
            globals::Globals,
            object::Object,
            protocol_generated::{WlSubcompositorId, WlSubsurfaceId, WlSurfaceId},
            runner::Dispatcher,
            state::State,
            wayland_objects::wl_surface::WlSurface,
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
            inputs: Default::default(),
            drm: Default::default(),
            globals,
            egl: EGL::new(),
            seat: Default::default(),
            keyboard,
            xkb,
        }
    }

    fn make_client() -> (Client, Dispatcher) {
        (Client::new_for_tests(ClientId::from_raw(4)), Dispatcher::default())
    }

    fn get_subsurface_request(
        object_id: WlSubcompositorId,
        id: WlSubsurfaceId,
        surface: WlSurfaceId,
        parent: WlSurfaceId,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(object_id.into(), 1, 3);
        event.id(id);
        event.id(surface);
        event.id(parent);
        (&*event.into_bytes()).try_into().unwrap()
    }

    #[test]
    fn get_subsurface_rejects_parent_cycle() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let mut subcompositor = WlSubcompositor::new(WlSubcompositorId::from(1));

        let (_, parent_object_id) = client
            .insert_object(&mut state, WlSurface::new(WlSurfaceId::from(10), client.client_id(), 6))
            .0;
        state.register_root_surface(parent_object_id);
        let (_, child_object_id) = client
            .insert_object(&mut state, WlSurface::new(WlSurfaceId::from(11), client.client_id(), 6))
            .0;
        state.register_root_surface(child_object_id);

        subcompositor.on(
            &mut client,
            &mut state,
            get_subsurface_request(
                WlSubcompositorId::from(1),
                WlSubsurfaceId::from(21),
                WlSurfaceId::from(11),
                WlSurfaceId::from(10),
            ),
            &mut client_dispatcher,
        );
        subcompositor.on(
            &mut client,
            &mut state,
            get_subsurface_request(
                WlSubcompositorId::from(1),
                WlSubsurfaceId::from(22),
                WlSurfaceId::from(10),
                WlSurfaceId::from(11),
            ),
            &mut client_dispatcher,
        );

        let parent = state
            .objects
            .get(parent_object_id)
            .downcast_ref::<WlSurface>()
            .unwrap();
        assert!(parent.is_root());
        assert!(parent.subsurface_object().is_none());
        assert!(!client.contains_object(WlSubsurfaceId::from(22), &state));
    }
}
