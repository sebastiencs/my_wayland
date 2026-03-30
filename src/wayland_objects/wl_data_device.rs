use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser},
    protocol_generated::{
        wl_data_device, WaylandId, WlDataDeviceId, WlDataOfferId, WlDataSourceId, WlSeatId,
        WlSurfaceId,
    },
    seat,
    state::State,
    wayland_objects::{wl_data_source::SourceUse, wl_display::WlDisplay, wl_surface::WlSurface},
};

use crate::clients::{ClientAction, DataSourceClientEvent, EventWriter};

#[derive(Debug)]
pub struct WlDataDevice {
    id: WlDataDeviceId,
    seat_id: WlSeatId,
    version: u32,
    selection_offer: Option<WlDataOfferId>,
    drag_offer: Option<WlDataOfferId>,
}

impl WlDataDevice {
    pub fn new(id: WlDataDeviceId, seat_id: WlSeatId, version: u32) -> Self {
        Self {
            id,
            seat_id,
            version,
            selection_offer: None,
            drag_offer: None,
        }
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn take_selection_offer(&mut self) -> Option<WlDataOfferId> {
        self.selection_offer.take()
    }

    pub fn set_selection_offer(&mut self, offer_id: Option<WlDataOfferId>) {
        self.selection_offer = offer_id;
    }

    pub fn take_drag_offer(&mut self) -> Option<WlDataOfferId> {
        self.drag_offer.take()
    }

    pub fn set_drag_offer(&mut self, offer_id: Option<WlDataOfferId>) {
        self.drag_offer = offer_id;
    }

    pub fn send_data_offer(&self, id: WlDataOfferId, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_data_device::Event::DataOffer { id }, dispatcher);
    }

    pub fn send_enter(
        &self,
        serial: u32,
        surface: WlSurfaceId,
        x: f32,
        y: f32,
        id: WlDataOfferId,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(wl_data_device::Event::Enter { serial, surface, x, y, id }, dispatcher);
    }

    pub fn send_leave(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_data_device::Event::Leave, dispatcher);
    }

    pub fn send_motion(&self, time: u32, x: f32, y: f32, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_data_device::Event::Motion { time, x, y }, dispatcher);
    }

    pub fn send_drop(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_data_device::Event::Drop, dispatcher);
    }

    pub fn send_selection(&self, id: WlDataOfferId, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_data_device::Event::Selection { id }, dispatcher);
    }
}

impl Object for WlDataDevice {
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
        use wl_data_device::Request::*;

        match client.parse::<Self>(req).unwrap() {
            StartDrag { source, origin, icon, serial } => {
                start_drag(self, client, state, dispatcher, source, origin, icon, serial);
            }
            SetSelection { source, serial } => {
                set_selection(self, client, state, dispatcher, source, serial);
            }
            Release => {
                state
                    .seat
                    .unregister_data_device(client.client_id(), self.id);
                if let Some(offer_id) = self.selection_offer.take() {
                    if client.contains_object(offer_id, state) {
                        client.destroy_data_offer(state, dispatcher, offer_id);
                    }
                }
                if let Some(offer_id) = self.drag_offer.take() {
                    if client.contains_object(offer_id, state) {
                        client.destroy_data_offer(state, dispatcher, offer_id);
                    }
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
    }
}

fn set_selection(
    data_device: &mut WlDataDevice,
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    source: WlDataSourceId,
    serial: u32,
) {
    if !state
        .seat
        .keyboard_focus()
        .is_some_and(|focus| focus.client_id == client.client_id())
    {
        return;
    }
    if !state.seat.validate_selection_serial(serial) {
        return;
    }
    if !state
        .seat
        .validate_client_serial(client.client_id(), serial)
    {
        return;
    }

    let source_raw: u32 = source.into();
    let next_selection = if source_raw == 0 {
        None
    } else {
        if !client.object_is(source, state) {
            client.protocol_violation_disconnect(
                dispatcher,
                data_device.id.into(),
                crate::protocol_generated::wl_display::Error::InvalidObject as u32,
                "invalid wl_data_source for selection",
            );
            return;
        }

        let mut source_object = client.grab_object(source, state);
        if source_object.dnd_actions() != 0 {
            client.protocol_violation_disconnect(
                dispatcher,
                source.into(),
                crate::protocol_generated::wl_data_source::Error::InvalidSource as u32,
                "selection source must not set drag-and-drop actions",
            );
            return;
        }
        // Real clients sometimes reuse the same wl_data_source object for multiple
        // selection operations. wlroots and Smithay are intentionally permissive
        // here, so we keep the source usable instead of disconnecting.
        source_object.set_used(SourceUse::Selection);
        Some(crate::state::SelectionSourceState {
            source_object_id: client.object_id(source),
            source_id: source,
            owner_client_id: client.client_id(),
        })
    };

    let previous = state.seat.replace_selection_source(next_selection, serial);
    if let Some(previous) = previous {
        dispatcher.dispatch_to(
            previous.owner_client_id,
            ClientAction::DataSourceEvent {
                source_id: previous.source_id,
                event: DataSourceClientEvent::Cancelled,
            },
        );
    }

    if let Some(focus) = state.seat.keyboard_focus() {
        dispatcher.dispatch_to(focus.client_id, ClientAction::RefreshDataDeviceSelection);
    }
}

fn start_drag(
    data_device: &mut WlDataDevice,
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    source: WlDataSourceId,
    origin: WlSurfaceId,
    icon: WlSurfaceId,
    serial: u32,
) {
    if !client.object_is(origin, state) {
        client.protocol_violation_disconnect(
            dispatcher,
            data_device.id.into(),
            crate::protocol_generated::wl_display::Error::InvalidObject as u32,
            "invalid drag origin surface",
        );
        return;
    }
    if !state
        .seat
        .validate_data_transfer_serial(client.client_id(), origin, serial)
    {
        return;
    }

    let source_raw: u32 = source.into();
    let (source_object_id, source_id) = if source_raw == 0 {
        (None, None)
    } else {
        if !client.object_is(source, state) {
            client.protocol_violation_disconnect(
                dispatcher,
                data_device.id.into(),
                crate::protocol_generated::wl_display::Error::InvalidObject as u32,
                "invalid wl_data_source for drag-and-drop",
            );
            return;
        }
        let mut source_object = client.grab_object(source, state);
        // Some clients incorrectly recycle a wl_data_source across multiple drags.
        // We stay compatible with wlroots/Smithay and accept that reuse instead of
        // disconnecting the client for a protocol nicety.
        source_object.set_used(SourceUse::Drag);
        (Some(client.object_id(source)), Some(source))
    };

    let icon_raw: u32 = icon.into();
    let (icon_surface_object_id, icon_surface_id) = if icon_raw == 0 {
        (None, None)
    } else {
        if !client.object_is(icon, state) {
            client.protocol_violation_disconnect(
                dispatcher,
                data_device.id.into(),
                crate::protocol_generated::wl_display::Error::InvalidObject as u32,
                "invalid drag icon surface",
            );
            return;
        }
        let mut icon_surface = client.grab_object(icon, state);
        if icon_surface.set_dnd_icon_role().is_err() {
            client.protocol_violation_disconnect(
                dispatcher,
                data_device.id.into(),
                wl_data_device::Error::Role as u32,
                "drag icon surface already has another role",
            );
            return;
        }
        (Some(client.object_id(icon)), Some(icon))
    };

    if state.seat.active_drag().is_some() {
        seat::cancel_active_drag(state, dispatcher, None);
    }

    state.seat.set_active_drag(crate::state::DragState {
        source_object_id,
        source_id,
        source_client_id: client.client_id(),
        origin_surface_id: origin,
        icon_surface_object_id,
        icon_surface_id,
        target: None,
        negotiation: Default::default(),
        phase: crate::state::DragPhase::Dragging,
    });
    if icon_surface_object_id.is_some() {
        dispatcher.dispatch_to(client.client_id(), ClientAction::RefreshSurfaceOutputs);
        crate::drm::schedule_active_output(state, dispatcher);
    }
    seat::refresh_active_drag_focus(state, dispatcher, 0);
}

#[cfg(test)]
mod tests {
    use {
        super::{set_selection, start_drag, WlDataDevice},
        crate::{
            clients::{Client, ClientAction, ClientId, DataSourceClientEvent, RawEvent},
            egl::EGL,
            globals::Globals,
            object::Object,
            protocol_generated::{WlDataDeviceId, WlDataSourceId, WlSeatId, WlSurfaceId},
            runner::Dispatcher,
            seat::{handle_keyboard_key, set_keyboard_focus_from_surface, SurfaceFocus},
            state::{
                DataOfferRole, DragNegotiationState, DragPhase, DragState, DragTargetState, State,
            },
            wayland_objects::{
                wl_data_offer::WlDataOffer, wl_data_source::WlDataSource, wl_surface::WlSurface,
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

    fn make_client(id: u64) -> (Client, Dispatcher) {
        (Client::new_for_tests(ClientId::from_raw(id)), Dispatcher::default())
    }

    fn release_request(data_device_id: WlDataDeviceId) -> crate::clients::RawRequest<'static> {
        let event = RawEvent::new(data_device_id.into(), 2, 0);
        (&*event.into_bytes()).try_into().unwrap()
    }

    #[test]
    fn set_selection_requires_keyboard_focus() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client(1);
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let mut data_device = WlDataDevice::new(WlDataDeviceId::from(2), WlSeatId::from(2), 3);
        let source_id = WlDataSourceId::from(3);

        client.insert_object(&mut state, WlDataSource::new(source_id, client.client_id(), 3));

        set_selection(
            &mut data_device,
            &mut client,
            &mut state,
            &mut client_dispatcher,
            source_id,
            41,
        );

        assert!(state.seat.selection_source().is_none());
    }

    #[test]
    fn start_drag_rejects_keyboard_serial_without_pointer_grab() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client(1);
        let mut setup_dispatcher = client.test_dispatcher(&mut dispatcher);
        let mut data_device = WlDataDevice::new(WlDataDeviceId::from(2), WlSeatId::from(2), 3);
        let origin_id = WlSurfaceId::from(10);

        client.insert_object(&mut state, WlSurface::new(origin_id, client.client_id(), 6));
        set_keyboard_focus_from_surface(
            &mut state,
            &mut setup_dispatcher,
            Some(SurfaceFocus { client_id: client.client_id(), surface_id: origin_id }),
        );
        drop(setup_dispatcher);

        handle_keyboard_key(&mut state, &mut dispatcher, 30, 1, 100);
        let serial = state.seat.last_keyboard_trigger_serial().unwrap();

        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        start_drag(
            &mut data_device,
            &mut client,
            &mut state,
            &mut client_dispatcher,
            WlDataSourceId::from(0),
            origin_id,
            WlSurfaceId::from(0),
            serial,
        );

        assert!(state.seat.active_drag().is_none());
    }

    #[test]
    fn set_selection_accepts_older_issued_serial() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client(1);
        let mut setup_dispatcher = client.test_dispatcher(&mut dispatcher);
        let mut data_device = WlDataDevice::new(WlDataDeviceId::from(2), WlSeatId::from(2), 3);
        let source_id = WlDataSourceId::from(3);
        let surface_id = WlSurfaceId::from(10);

        client.insert_object(&mut state, WlDataSource::new(source_id, client.client_id(), 3));
        client.insert_object(&mut state, WlSurface::new(surface_id, client.client_id(), 6));
        set_keyboard_focus_from_surface(
            &mut state,
            &mut setup_dispatcher,
            Some(SurfaceFocus { client_id: client.client_id(), surface_id }),
        );
        drop(setup_dispatcher);

        state.seat.record_client_serial(client.client_id(), 40);
        state.seat.record_client_serial(client.client_id(), 41);
        state.seat.record_client_serial(client.client_id(), 42);

        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        set_selection(
            &mut data_device,
            &mut client,
            &mut state,
            &mut client_dispatcher,
            source_id,
            40,
        );

        assert!(state.seat.selection_source().is_some());
    }

    #[test]
    fn release_runs_drag_offer_cleanup() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client(2);
        let data_device_id = WlDataDeviceId::from(2);
        let offer_id = crate::protocol_generated::WlDataOfferId::from(20);
        let source_id = WlDataSourceId::from(10);
        let source_client_id = ClientId::from_raw(1);

        let ((_, data_device_object_id), _) = client
            .insert_object(&mut state, WlDataDevice::new(data_device_id, WlSeatId::from(2), 2));
        let _ = client.insert_object(
            &mut state,
            WlDataOffer::new(
                offer_id,
                2,
                DataOfferRole::Drag,
                None,
                Some(source_id),
                Some(source_client_id),
            ),
        );

        state
            .seat
            .register_data_device(client.client_id(), data_device_id);
        state.seat.set_active_drag(DragState {
            source_object_id: None,
            source_id: Some(source_id),
            source_client_id,
            origin_surface_id: 30u32.into(),
            icon_surface_object_id: None,
            icon_surface_id: None,
            target: Some(DragTargetState {
                client_id: client.client_id(),
                surface_id: 31u32.into(),
            }),
            negotiation: DragNegotiationState {
                offer_ids: vec![offer_id],
                accepted_mime: Some("text/plain".to_string()),
                destination_actions: 0,
                preferred_action: 0,
                compositor_action: 0,
                chosen_action: crate::protocol_generated::wl_data_device_manager::DndAction::Copy
                    as u32,
                finished: false,
                has_finish_capable_offer: false,
            },
            phase: DragPhase::DropSent,
        });

        {
            let mut data_device = state
                .objects
                .grab_object::<WlDataDevice>(data_device_object_id);
            data_device.set_drag_offer(Some(offer_id));
        }

        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let mut data_device = state
            .objects
            .grab_object::<WlDataDevice>(data_device_object_id);
        data_device.on(
            &mut client,
            &mut state,
            release_request(data_device_id),
            &mut client_dispatcher,
        );
        drop(data_device);

        assert!(state.seat.active_drag().is_none());
        assert!(dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            source_client_id,
            |action| {
                matches!(
                    action,
                    ClientAction::DataSourceEvent {
                        source_id: queued_source_id,
                        event: DataSourceClientEvent::DndFinished,
                    } if *queued_source_id == source_id
                )
            }
        ));
    }
}
