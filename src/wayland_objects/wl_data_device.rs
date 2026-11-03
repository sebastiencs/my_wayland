use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_data_device, WaylandId, WlDataDeviceId, WlDataOfferId, WlDataSourceId, WlSeatId,
        WlSurfaceId,
    },
    seat,
    state::{GrabbedObject, SelectionSourceState, State},
    wayland_objects::{wl_data_source::SourceUse, wl_display::WlDisplay, wl_surface::WlSurface},
};

use crate::clients::{ClientAction, DataSourceClientEvent, EventWriter};

#[derive(Debug)]
pub struct WlDataDevice {
    id: WlDataDeviceId,
    seat_id: WlSeatId,
    version: u32,
    selection_offer: Option<Resolved<WlDataOfferId>>,
    drag_offer: Option<Resolved<WlDataOfferId>>,
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

    pub fn take_selection_offer(&mut self) -> Option<Resolved<WlDataOfferId>> {
        self.selection_offer.take()
    }

    pub fn set_selection_offer(&mut self, offer_id: Option<Resolved<WlDataOfferId>>) {
        self.selection_offer = offer_id;
    }

    pub fn take_drag_offer(&mut self) -> Option<Resolved<WlDataOfferId>> {
        self.drag_offer.take()
    }

    pub fn set_drag_offer(&mut self, offer_id: Option<Resolved<WlDataOfferId>>) {
        self.drag_offer = offer_id;
    }

    pub fn send_data_offer(
        &self,
        offer: Resolved<WlDataOfferId>,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(wl_data_device::Event::DataOffer { id: offer.wayland_id() }, dispatcher);
    }

    pub fn send_enter(
        &self,
        serial: u32,
        surface: Resolved<WlSurfaceId>,
        x: f32,
        y: f32,
        offer: Option<Resolved<WlDataOfferId>>,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            wl_data_device::Event::Enter {
                serial,
                surface: surface.wayland_id(),
                x,
                y,
                id: offer
                    .map(|offer| offer.wayland_id())
                    .unwrap_or_else(|| WlDataOfferId::from(0u32)),
            },
            dispatcher,
        );
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

    pub fn send_selection(
        &self,
        offer: Option<Resolved<WlDataOfferId>>,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            wl_data_device::Event::Selection {
                id: offer
                    .map(|offer| offer.wayland_id())
                    .unwrap_or_else(|| WlDataOfferId::from(0u32)),
            },
            dispatcher,
        );
    }
}

impl Object for WlDataDevice {
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
        use wl_data_device::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            StartDrag { source, origin, icon, serial } => {
                start_drag(&mut self, client, state, dispatcher, source, origin, icon, serial)?;
            }
            SetSelection { source, serial } => {
                set_selection(&mut self, client, state, dispatcher, source, serial)?;
            }
            Release => {
                if let Some(data_device) = client.resolve(self.id, state) {
                    state.seat.unregister_data_device(data_device);
                }
                if let Some(offer) = self.selection_offer.take() {
                    if offer.exists(state) {
                        client.destroy_data_offer(state, dispatcher, offer);
                    }
                }
                if let Some(offer) = self.drag_offer.take() {
                    if offer.exists(state) {
                        client.destroy_data_offer(state, dispatcher, offer);
                    }
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }
}

fn set_selection(
    data_device: &mut WlDataDevice,
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    source: WlDataSourceId,
    serial: u32,
) -> RequestResult {
    if !state
        .seat
        .keyboard_focus()
        .is_some_and(|focus| focus.client_id() == client.client_id())
    {
        return Ok(());
    }
    if !state.seat.validate_selection_serial(serial) {
        return Ok(());
    }
    if !state
        .seat
        .validate_client_serial(client.client_id(), serial)
    {
        return Ok(());
    }

    let next_selection = match source.to_opt() {
        None => None,
        Some(source) => {
            let source = client.resolve_or_disconnect(source, state, dispatcher)?;
            let mut source_object = source.grab(state);
            if source_object.dnd_actions() != 0 {
                client.protocol_violation_disconnect(
                    dispatcher,
                    source.wayland_id().into(),
                    crate::protocol_generated::wl_data_source::Error::InvalidSource as u32,
                    "selection source must not set drag-and-drop actions",
                );
                return Ok(());
            }
            // Real clients sometimes reuse the same wl_data_source object for multiple
            // selection operations. wlroots and Smithay are intentionally permissive
            // here, so we keep the source usable instead of disconnecting.
            source_object.set_used(SourceUse::Selection);
            Some(SelectionSourceState::WlData(source))
        }
    };

    let previous = state.seat.replace_selection_source(next_selection, serial);
    if let Some(previous) = previous {
        previous.cancelled(dispatcher);
    }

    if let Some(focus) = state.seat.keyboard_focus() {
        dispatcher.dispatch_to(focus.client_id(), ClientAction::RefreshDataDeviceSelection);
    }

    Ok(())
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
) -> RequestResult {
    use crate::state::{DragPhase, DragSource, DragState};

    if state.session_lock.input_gated() {
        return Ok(());
    }

    let origin = client.resolve_or_disconnect(origin, state, dispatcher)?;
    if !state.seat.validate_data_transfer_serial(origin, serial) {
        return Ok(());
    }

    let source: Option<Resolved<WlDataSourceId>> = match source.to_opt() {
        None => None,
        Some(source) => {
            let source = client.resolve_or_disconnect(source, state, dispatcher)?;
            let source_object = source.ref_mut(state);
            // Some clients incorrectly recycle a wl_data_source across multiple drags.
            // We stay compatible with wlroots/Smithay and accept that reuse instead of
            // disconnecting the client for a protocol nicety.
            source_object.set_used(SourceUse::Drag);
            Some(source)
        }
    };

    let icon: Option<Resolved<WlSurfaceId>> = match icon.to_opt() {
        None => None,
        Some(icon) => {
            let icon = client.resolve_or_disconnect(icon, state, dispatcher)?;
            let icon_surface = icon.ref_mut(state);
            if icon_surface.set_dnd_icon_role().is_err() {
                client.protocol_violation_disconnect(
                    dispatcher,
                    data_device.id.into(),
                    wl_data_device::Error::Role as u32,
                    "drag icon surface already has another role",
                );
                return Ok(());
            }
            Some(icon)
        }
    };

    if state.seat.active_drag().is_some() {
        seat::cancel_active_drag(state, dispatcher, None);
    }

    crate::input::pointer_lock::release_pointer_capture(state, dispatcher);
    let client_id = client.client_id();
    state.seat.set_active_drag(DragState {
        source: source
            .map(|source| DragSource::DataSource { source })
            .unwrap_or(DragSource::None { initiator_client_id: client_id }),
        origin_surface: origin,
        icon_surface: icon,
        target: None,
        negotiation: Default::default(),
        phase: DragPhase::Dragging,
    });
    if icon.is_some() {
        dispatcher.dispatch_to(client_id, ClientAction::RefreshSurfaceOutputs);
        crate::drm::request_active_output(state, dispatcher);
    }
    seat::refresh_active_drag_focus(state, dispatcher, 0);

    Ok(())
}

#[cfg(test)]
mod tests {
    use {
        super::{set_selection, start_drag, WlDataDevice},
        crate::{
            clients::{Client, ClientAction, ClientId, DataSourceClientEvent, RawEvent, Resolved},
            object::{Object, RequestResult},
            protocol_generated::{WlDataDeviceId, WlDataSourceId, WlSeatId, WlSurfaceId},
            runner::Dispatcher,
            seat::{handle_keyboard_key, set_keyboard_focus_from_surface, SurfaceFocus},
            state::{
                DataOfferRole, DragNegotiationState, DragPhase, DragSource, DragState,
                DragTargetState, ObjectId, State,
            },
            test_support::{state_for_tests, test_client_and_dispatcher, test_resolved_wl_surface},
            wayland_objects::{
                wl_data_offer::WlDataOffer, wl_data_source::WlDataSource, wl_surface::WlSurface,
            },
        },
    };

    fn release_request(data_device_id: WlDataDeviceId) -> crate::clients::RawRequest<'static> {
        let event = RawEvent::new(data_device_id.into(), 2, 0);
        (&*event.into_bytes()).try_into().unwrap()
    }

    #[test]
    fn set_selection_requires_keyboard_focus() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(1);
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
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(1);
        let mut setup_dispatcher = client.test_dispatcher(&mut dispatcher);
        let mut data_device = WlDataDevice::new(WlDataDeviceId::from(2), WlSeatId::from(2), 3);
        let origin_id = WlSurfaceId::from(10);

        client.insert_object(&mut state, WlSurface::new(origin_id, client.client_id(), 6));
        set_keyboard_focus_from_surface(
            &mut state,
            &mut setup_dispatcher,
            Some(SurfaceFocus::new(Resolved::new(
                client.client_id(),
                origin_id,
                client.object_id(origin_id),
            ))),
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
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(1);
        let mut setup_dispatcher = client.test_dispatcher(&mut dispatcher);
        let mut data_device = WlDataDevice::new(WlDataDeviceId::from(2), WlSeatId::from(2), 3);
        let source_id = WlDataSourceId::from(3);
        let surface_id = WlSurfaceId::from(10);

        client.insert_object(&mut state, WlDataSource::new(source_id, client.client_id(), 3));
        client.insert_object(&mut state, WlSurface::new(surface_id, client.client_id(), 6));
        set_keyboard_focus_from_surface(
            &mut state,
            &mut setup_dispatcher,
            Some(SurfaceFocus::new(Resolved::new(
                client.client_id(),
                surface_id,
                client.object_id(surface_id),
            ))),
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
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(2);
        let data_device_id = WlDataDeviceId::from(2);
        let offer_id = crate::protocol_generated::WlDataOfferId::from(20);
        let source_id = WlDataSourceId::from(10);
        let source_client_id = ClientId::from_raw(1);

        let (data_device_resolved, _) = client
            .insert_object(&mut state, WlDataDevice::new(data_device_id, WlSeatId::from(2), 2));
        let data_device_object_id = data_device_resolved.object_id();
        let _ = client.insert_object(
            &mut state,
            WlDataOffer::new(
                offer_id,
                2,
                DataOfferRole::Drag,
                Some(Resolved::new(source_client_id, source_id, ObjectId::from(0usize))),
            ),
        );

        state.seat.register_data_device(Resolved::new(
            client.client_id(),
            data_device_id,
            data_device_object_id,
        ));
        state.seat.set_active_drag(DragState {
            source: DragSource::DataSource {
                source: Resolved::new(source_client_id, source_id, ObjectId::from(0usize)),
            },
            origin_surface: test_resolved_wl_surface(source_client_id, 30, ObjectId::from(30usize)),
            icon_surface: None,
            target: Some(DragTargetState {
                surface: test_resolved_wl_surface(client.client_id(), 31, ObjectId::from(0usize)),
            }),
            negotiation: DragNegotiationState {
                offer_ids: vec![Resolved::new(
                    client.client_id(),
                    offer_id,
                    client.object_id(offer_id),
                )],
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
            data_device.set_drag_offer(Some(Resolved::new(
                client.client_id(),
                offer_id,
                client.object_id(offer_id),
            )));
        }

        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let data_device = state
            .objects
            .grab_object::<WlDataDevice>(data_device_object_id);
        data_device.on(
            &mut client,
            &mut state,
            release_request(data_device_id),
            &mut client_dispatcher,
        );

        assert!(state.seat.active_drag().is_none());
        assert!(dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            source_client_id,
            |action| {
                matches!(
                    action,
                    ClientAction::DataSourceEvent {
                        source: queued_source,
                        event: DataSourceClientEvent::DndFinished,
                    } if queued_source.wayland_id() == source_id
                )
            }
        ));
    }
}
