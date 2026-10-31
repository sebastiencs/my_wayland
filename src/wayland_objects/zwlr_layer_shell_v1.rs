use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_display, zwlr_layer_shell_v1, Constants, NewId, WaylandId, WlOutputId,
        ZwlrLayerShellV1Id,
    },
    state::{GrabbedObject, OutputId, State},
    wayland_objects::{
        wl_display::WlDisplay,
        wl_output::WlOutput,
        wl_surface::{Role, WlSurface},
        zwlr_layer_surface_v1::{
            relayout_client_layer_surfaces, LayerShellLayer, ZwlrLayerSurfaceV1,
        },
    },
};

#[derive(Debug)]
pub struct ZwlrLayerShellV1Global;

impl ZwlrLayerShellV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ZwlrLayerShellV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (ZwlrLayerShellV1::INTERFACE_NAME, ZwlrLayerShellV1::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = ZwlrLayerShellV1Id::from(id);
        client.insert_object(
            state,
            ZwlrLayerShellV1 {
                id,
                version: version.min(ZwlrLayerShellV1::INTERFACE_VERSION),
                surfaces: Vec::new(),
            },
        );
    }
}

#[derive(Debug)]
pub struct ZwlrLayerShellV1 {
    id: ZwlrLayerShellV1Id,
    version: u32,
    surfaces: Vec<crate::protocol_generated::ZwlrLayerSurfaceV1Id>,
}

impl ZwlrLayerShellV1 {
    pub fn unregister_surface(&mut self, id: crate::protocol_generated::ZwlrLayerSurfaceV1Id) {
        self.surfaces.retain(|candidate| *candidate != id);
    }

    fn resolve_output_id(
        &self,
        client: &Client,
        state: &State,
        output: WlOutputId,
    ) -> Result<Option<OutputId>, ()> {
        let Some(output) = output.to_opt() else {
            return Ok(None);
        };
        let Some(output) = client.resolve(output, state) else {
            return Err(());
        };
        let wl_output = output.rref(state);
        Ok(Some(wl_output.output_id().ok_or(())?))
    }
}

impl Object for ZwlrLayerShellV1 {
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
        use zwlr_layer_shell_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            GetLayerSurface { id, surface, output, layer, namespace } => {
                let surface = client.resolve_or_disconnect(surface, state, dispatcher)?;

                let Some(layer) = LayerShellLayer::from_raw(layer) else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        zwlr_layer_shell_v1::Error::InvalidLayer as u32,
                        "zwlr_layer_shell_v1.get_layer_surface used an invalid layer",
                    );
                    return Ok(());
                };

                let requested_output_id = match self.resolve_output_id(client, state, output) {
                    Ok(output_id) => output_id,
                    Err(()) => {
                        client.protocol_violation_disconnect(
                            dispatcher,
                            self.id.into(),
                            wl_display::Error::InvalidObject as u32,
                            "zwlr_layer_shell_v1.get_layer_surface requires a live wl_output when output is non-null",
                        );
                        return Ok(());
                    }
                };

                {
                    let wl_surface = surface.ref_mut(state);
                    if wl_surface.has_attached_or_committed_buffer() {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            zwlr_layer_shell_v1::Error::AlreadyConstructed as u32,
                            "wl_surface must not have an attached or committed buffer before get_layer_surface",
                        );
                        return Ok(());
                    }
                    if wl_surface.role() != Role::None {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            zwlr_layer_shell_v1::Error::Role as u32,
                            "wl_surface already has a role",
                        );
                        return Ok(());
                    }
                    if wl_surface.set_role(Role::LayerSurface, dispatcher).is_err() {
                        return Ok(());
                    }
                }

                let this = client.resolve(self.id, state).unwrap();
                let layer_surface = ZwlrLayerSurfaceV1::new(
                    id,
                    this,
                    self.version,
                    surface,
                    requested_output_id,
                    layer,
                    namespace,
                );
                let (layer_surface, _) = client.insert_object(state, layer_surface);
                surface.ref_mut(state).set_layer_surface_impl(layer_surface);
                state.register_layer_surface_object(layer_surface);
                self.surfaces.push(id);
                relayout_client_layer_surfaces(client, state, dispatcher);
            }
            Destroy => {
                if self.version < zwlr_layer_shell_v1::DESTROY_SINCE {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidMethod as u32,
                        "zwlr_layer_shell_v1.destroy requires zwlr_layer_shell_v1 version >= 3",
                    );
                    return Ok(());
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        clients::{Client, ClientId, ClientManagerAction, RawEvent},
        io_uring::model::IoUringAction,
        runner::Dispatcher,
        test_support::state_for_tests,
    };

    use super::*;

    fn destroy_request(id: ZwlrLayerShellV1Id) -> crate::clients::RawRequest<'static> {
        let event = RawEvent::new(id.into(), 1, 0);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn malformed_request(id: ZwlrLayerShellV1Id) -> crate::clients::RawRequest<'static> {
        let event = RawEvent::new(id.into(), 99, 0);
        (&*event.into_bytes()).try_into().unwrap()
    }

    #[test]
    fn destroy_requires_version_three() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(7));
        let manager_id = ZwlrLayerShellV1Id::from(3u32);
        client.insert_object(
            &mut state,
            ZwlrLayerShellV1 { id: manager_id, version: 2, surfaces: Vec::new() },
        );
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        {
            let mut manager = client.grab_object(manager_id, &mut state);
            manager.on(
                &mut client,
                &mut state,
                destroy_request(manager_id),
                &mut client_dispatcher,
            );
        }
        drop(client_dispatcher);

        assert!(client.object_is(manager_id, &state));
        assert!(dispatcher.test_has_queued_action::<ClientManagerAction>(|action| {
            matches!(action, ClientManagerAction::Close { id } if *id == client.client_id())
        }));
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));
    }

    #[test]
    fn malformed_requests_disconnect_the_client() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(7));
        let manager_id = ZwlrLayerShellV1Id::from(3u32);
        client.insert_object(
            &mut state,
            ZwlrLayerShellV1 { id: manager_id, version: 5, surfaces: Vec::new() },
        );
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        {
            let mut manager = client.grab_object(manager_id, &mut state);
            manager.on(
                &mut client,
                &mut state,
                malformed_request(manager_id),
                &mut client_dispatcher,
            );
        }
        drop(client_dispatcher);

        assert!(dispatcher.test_has_queued_action::<ClientManagerAction>(|action| {
            matches!(action, ClientManagerAction::Close { id } if *id == client.client_id())
        }));
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));
    }
}
