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
    ) -> crate::object::RequestResult {
        use wl_subcompositor::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => self.delete_self(client, state, dispatcher),
            GetSubsurface { id, surface, parent } => {
                let Some(surface) = client.resolve(surface, state) else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wl_subcompositor::Error::BadSurface as u32,
                        "get_subsurface surface must be a live wl_surface",
                    );
                    return Ok(());
                };
                let Some(parent) = client.resolve(parent, state) else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wl_subcompositor::Error::BadParent as u32,
                        "get_subsurface parent must be a live wl_surface",
                    );
                    return Ok(());
                };

                if surface.object_id() == parent.object_id() {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wl_subcompositor::Error::BadParent as u32,
                        "get_subsurface parent must be different from the child surface",
                    );
                    return Ok(());
                }

                {
                    let surface_ref = surface.rref(state);
                    if surface_ref.has_role() || surface_ref.subsurface_object().is_some() {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            wl_subcompositor::Error::BadSurface as u32,
                            "get_subsurface surface already has a role or subsurface object",
                        );
                        return Ok(());
                    }
                }

                {
                    let Some(surface_ref) = surface.try_ref(state) else {
                        let message = if state.objects.contains(surface.object_id()) {
                            "get_subsurface surface is not a wl_surface"
                        } else {
                            "get_subsurface surface disappeared during validation"
                        };
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            wl_subcompositor::Error::BadSurface as u32,
                            message,
                        );
                        return Ok(());
                    };
                    if surface_ref.is_ancestor_of(surface.object_id(), state, parent.object_id()) {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            wl_subcompositor::Error::BadParent as u32,
                            "get_subsurface parent must not be a descendant of the child surface",
                        );
                        return Ok(());
                    }
                }

                client.insert_object(
                    state,
                    WlSubsurface::new(id, surface.with_client(client), parent.with_client(client)),
                );
                state.unregister_root_surface(surface.object_id());

                {
                    let mut surface_ref = surface
                        .try_grab(state)
                        .expect("resolved wl_surface vanished");
                    surface_ref.assign_subsurface(id, parent.object_id());
                }
                {
                    let mut parent_ref = parent
                        .try_grab(state)
                        .expect("resolved wl_surface vanished");
                    parent_ref.add_pending_child(surface.object_id());
                }
            }
        }
        Ok(())
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
