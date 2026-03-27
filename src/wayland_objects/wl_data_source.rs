use crate::{
    clients::{Client, ClientAction, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wl_data_device_manager, wl_data_source, WaylandId, WlDataSourceId},
    seat,
    state::State,
    wayland_objects::{wl_data_offer::WlDataOffer, wl_display::WlDisplay},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceUse {
    Unused,
    Selection,
    Drag,
}

#[derive(Debug)]
pub struct WlDataSource {
    id: WlDataSourceId,
    client_id: crate::clients::ClientId,
    version: u32,
    mime_types: Vec<String>,
    dnd_actions: u32,
    used: SourceUse,
}

impl WlDataSource {
    pub fn new(id: WlDataSourceId, client_id: crate::clients::ClientId, version: u32) -> Self {
        Self {
            id,
            client_id,
            version,
            mime_types: Vec::new(),
            dnd_actions: 0,
            used: SourceUse::Unused,
        }
    }

    pub fn wl_data_source_id(&self) -> WlDataSourceId {
        self.id
    }

    pub fn owner_client_id(&self) -> crate::clients::ClientId {
        self.client_id
    }

    pub fn mime_types(&self) -> &[String] {
        &self.mime_types
    }

    pub fn dnd_actions(&self) -> u32 {
        self.dnd_actions
    }

    pub fn effective_dnd_actions(&self) -> u32 {
        if self.version < wl_data_source::ACTION_SINCE {
            wl_data_device_manager::DndAction::Copy as u32
        } else {
            self.dnd_actions
        }
    }

    pub fn used(&self) -> SourceUse {
        self.used
    }

    pub fn set_used(&mut self, used: SourceUse) {
        // The protocol models wl_data_source as single-use, but some real-world
        // clients recycle one source object across multiple clipboard or DnD
        // operations. We keep tracking the latest role for local bookkeeping
        // while remaining permissive for compatibility.
        self.used = used;
    }

    pub fn send_target(&self, mime_type: Option<String>, dispatcher: &mut ClientDispatcher) {
        self.send_event(
            wl_data_source::Event::Target { mime_type: mime_type.unwrap_or_default() },
            dispatcher,
        );
    }

    pub fn send_send(&self, mime_type: String, fd: i32, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_data_source::Event::Send { mime_type, fd }, dispatcher);
    }

    pub fn send_cancelled(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_data_source::Event::Cancelled, dispatcher);
    }

    pub fn send_dnd_drop_performed(&self, dispatcher: &mut ClientDispatcher) {
        if self.version < wl_data_source::DND_DROP_PERFORMED_SINCE {
            return;
        }
        self.send_event(wl_data_source::Event::DndDropPerformed, dispatcher);
    }

    pub fn send_dnd_finished(&self, dispatcher: &mut ClientDispatcher) {
        if self.version < wl_data_source::DND_FINISHED_SINCE {
            return;
        }
        self.send_event(wl_data_source::Event::DndFinished, dispatcher);
    }

    pub fn send_action(&self, dnd_action: u32, dispatcher: &mut ClientDispatcher) {
        if self.version < wl_data_source::ACTION_SINCE {
            return;
        }
        self.send_event(wl_data_source::Event::Action { dnd_action }, dispatcher);
    }
}

impl Object for WlDataSource {
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
        use wl_data_source::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Offer { mime_type } => {
                if !self.mime_types.contains(&mime_type) {
                    self.mime_types.push(mime_type);
                }
            }
            Destroy => {
                destroy_source(client, state, dispatcher, self.id, self.client_id);
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            SetActions { dnd_actions } => {
                const VALID_MASK: u32 = wl_data_device_manager::DndAction::Copy as u32
                    | wl_data_device_manager::DndAction::Move as u32
                    | wl_data_device_manager::DndAction::Ask as u32;
                if dnd_actions & !VALID_MASK != 0 {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_data_source::Error::InvalidActionMask as u32,
                        "invalid drag-and-drop action mask",
                    );
                    return;
                }
                if self.used != SourceUse::Unused || self.dnd_actions != 0 {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_data_source::Error::InvalidSource as u32,
                        "wl_data_source.set_actions must happen exactly once before start_drag",
                    );
                    return;
                }
                self.dnd_actions = dnd_actions;
            }
        }
    }

    fn on_client_disconnect(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        destroy_source(client, state, dispatcher, self.id, self.client_id);
    }
}

fn destroy_source(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    source_id: WlDataSourceId,
    owner_client_id: crate::clients::ClientId,
) {
    let source_object_id = client.object_id(source_id);

    let offer_ids = state
        .objects
        .iter()
        .filter_map(|(object_id, object)| {
            let offer = object.downcast_ref::<WlDataOffer>()?;
            (offer.source_object_id() == Some(source_object_id)).then_some(object_id)
        })
        .collect::<Vec<_>>();
    for offer_object_id in offer_ids {
        if let Some(mut offer) = state
            .objects
            .try_grab_object::<WlDataOffer>(offer_object_id)
        {
            offer.clear_source();
        }
    }

    if state
        .seat
        .selection_source()
        .is_some_and(|selection| selection.source_object_id == source_object_id)
    {
        state
            .seat
            .replace_selection_source(None, state.seat.selection_serial());
        if let Some(focus) = state.seat.keyboard_focus() {
            dispatcher.dispatch_to(focus.client_id, ClientAction::RefreshDataDeviceSelection);
        }
    }

    if state
        .seat
        .active_drag()
        .is_some_and(|drag| drag.source_object_id == Some(source_object_id))
    {
        seat::cancel_active_drag(state, dispatcher, Some((owner_client_id, source_id)));
    }
}

#[cfg(test)]
mod tests {
    use {
        super::{destroy_source, WlDataSource},
        crate::{
            clients::{Client, ClientAction, ClientId, DataSourceClientEvent},
            egl::EGL,
            globals::Globals,
            protocol_generated::WlDataSourceId,
            runner::Dispatcher,
            seat::{set_surface_focus, SurfaceFocus},
            state::{DragPhase, DragState, DragTargetState, SelectionSourceState, State},
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

    fn insert_surface(state: &mut State, client_id: ClientId, surface_id: u32) {
        let _ = state.insert_object(WlSurface::new(surface_id.into(), client_id, 6));
    }

    #[test]
    fn source_disconnect_clears_selection_and_refreshes_focused_client() {
        let mut state = make_state();
        let mut owner = Client::new_for_tests(ClientId::from_raw(1));
        let focused_client_id = ClientId::from_raw(2);
        let owner_client_id = owner.client_id();
        let source_id = WlDataSourceId::from(10);
        let ((_, source_object_id), _) =
            owner.insert_object(&mut state, WlDataSource::new(source_id, owner_client_id, 3));
        insert_surface(&mut state, focused_client_id, 55);

        {
            let mut setup_dispatcher = Dispatcher::default();
            let mut setup_client_dispatcher = owner.test_dispatcher(&mut setup_dispatcher);
            set_surface_focus(
                &mut state,
                &mut setup_client_dispatcher,
                Some(SurfaceFocus { client_id: focused_client_id, surface_id: 55u32.into() }),
            );
        }
        state.seat.replace_selection_source(
            Some(SelectionSourceState { source_object_id, source_id, owner_client_id }),
            1,
        );

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = owner.test_dispatcher(&mut dispatcher);
        destroy_source(&mut owner, &mut state, &mut client_dispatcher, source_id, owner_client_id);

        assert!(state.seat.selection_source().is_none());
        assert!(dispatcher
            .test_has_queued_action_for::<ClientAction, ClientId>(focused_client_id, |action| {
                matches!(action, ClientAction::RefreshDataDeviceSelection)
            }));
    }

    #[test]
    fn source_disconnect_cancels_active_drag_and_notifies_target() {
        let mut state = make_state();
        let mut owner = Client::new_for_tests(ClientId::from_raw(1));
        let target_client_id = ClientId::from_raw(2);
        let owner_client_id = owner.client_id();
        let source_id = WlDataSourceId::from(10);
        let ((_, source_object_id), _) =
            owner.insert_object(&mut state, WlDataSource::new(source_id, owner_client_id, 3));

        state.seat.set_active_drag(DragState {
            source_object_id: Some(source_object_id),
            source_id: Some(source_id),
            source_client_id: owner.client_id(),
            origin_surface_id: 20u32.into(),
            icon_surface_object_id: None,
            icon_surface_id: None,
            target: Some(DragTargetState { client_id: target_client_id, surface_id: 21u32.into() }),
            negotiation: Default::default(),
            phase: DragPhase::Dragging,
        });

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = owner.test_dispatcher(&mut dispatcher);
        destroy_source(&mut owner, &mut state, &mut client_dispatcher, source_id, owner_client_id);

        assert!(state.seat.active_drag().is_none());
        assert!(dispatcher
            .test_has_queued_action_for::<ClientAction, ClientId>(target_client_id, |action| {
                matches!(action, ClientAction::DataDeviceDragLeave)
            }));
        assert!(dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            owner_client_id,
            |action| {
                matches!(
                    action,
                    ClientAction::DataSourceEvent {
                        source_id: queued_source_id,
                        event: DataSourceClientEvent::Cancelled,
                    } if *queued_source_id == source_id
                )
            }
        ));
    }
}
