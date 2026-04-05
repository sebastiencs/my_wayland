use std::os::fd::FromRawFd;

use crate::{
    clients::{
        Client, ClientAction, ClientDispatcher, ClientId, DataSourceClientEvent, EventWriter,
        RawRequest, Resolved,
    },
    object::{Object, Parser},
    protocol_generated::{
        wl_data_device_manager, wl_data_offer, WaylandId, WlDataOfferId, WlDataSourceId,
    },
    seat,
    state::{DataOfferRole, DragPhase, ObjectId, State},
    wayland_objects::wl_data_source::WlDataSource,
};

#[derive(Debug)]
pub struct WlDataOffer {
    id: WlDataOfferId,
    version: u32,
    role: DataOfferRole,
    source: Option<Resolved<WlDataSourceId>>,
    finished: bool,
}

impl WlDataOffer {
    pub fn new(
        id: WlDataOfferId,
        version: u32,
        role: DataOfferRole,
        source: Option<Resolved<WlDataSourceId>>,
    ) -> Self {
        Self { id, version, role, source, finished: false }
    }

    pub fn source_object_id(&self) -> Option<ObjectId> {
        self.source.map(|src| src.object_id())
    }

    pub fn clear_source(&mut self) {
        self.source = None;
    }

    pub(crate) fn source(&self) -> Option<Resolved<WlDataSourceId>> {
        self.source
    }

    pub fn send_offer(&self, mime_type: String, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_data_offer::Event::Offer { mime_type }, dispatcher);
    }

    pub fn send_source_actions(&self, source_actions: u32, dispatcher: &mut ClientDispatcher) {
        if self.version < wl_data_offer::SOURCE_ACTIONS_SINCE {
            return;
        }
        self.send_event(wl_data_offer::Event::SourceActions { source_actions }, dispatcher);
    }

    pub fn send_action(&self, dnd_action: u32, dispatcher: &mut ClientDispatcher) {
        if self.version < wl_data_offer::ACTION_SINCE {
            return;
        }
        self.send_event(wl_data_offer::Event::Action { dnd_action }, dispatcher);
    }

    fn is_active_drag_offer(
        &self,
        state: &State,
        client_id: ClientId,
        offer: Resolved<WlDataOfferId>,
    ) -> bool {
        self.role == DataOfferRole::Drag && state.seat.active_drag_contains_offer(client_id, offer)
    }

    pub fn before_destroy(
        &mut self,
        owner_client_id: ClientId,
        offer: Resolved<WlDataOfferId>,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if self.role != DataOfferRole::Drag
            || !state
                .seat
                .unregister_active_drag_offer(owner_client_id, offer)
            || self.finished
        {
            return;
        }
        if let Some(drag) = state.seat.active_drag() {
            if drag.negotiation.offer_ids.is_empty() {
                if drag.phase == DragPhase::DropSent && !drag.negotiation.has_finish_capable_offer {
                    self.finished = true;
                    if let Some(source) = self.source() {
                        if drag.negotiation.chosen_action != 0 {
                            dispatcher.dispatch_to(
                                source.client_id(),
                                ClientAction::DataSourceEvent {
                                    source,
                                    event: DataSourceClientEvent::Action {
                                        dnd_action: drag.negotiation.chosen_action,
                                    },
                                },
                            );
                        }
                        dispatcher.dispatch_to(
                            source.client_id(),
                            ClientAction::DataSourceEvent {
                                source,
                                event: DataSourceClientEvent::DndFinished,
                            },
                        );
                    }
                    seat::finish_active_drag(state, dispatcher);
                } else {
                    seat::cancel_active_drag(state, dispatcher, None);
                }
            }
        }
    }
}

fn choose_dnd_action(
    source_actions: u32,
    destination_actions: u32,
    preferred_action: u32,
    compositor_action: u32,
) -> u32 {
    let available_actions = source_actions & destination_actions;
    if available_actions == 0 {
        return 0;
    }
    // Keep compositor policy opt-in. A zero compositor action means we intentionally
    // stay permissive and let the source and destination negotiate the outcome.
    if compositor_action != 0 && compositor_action & available_actions != 0 {
        return compositor_action;
    }
    if preferred_action != 0 && preferred_action & available_actions != 0 {
        return preferred_action;
    }
    if available_actions & wl_data_device_manager::DndAction::Copy as u32 != 0 {
        wl_data_device_manager::DndAction::Copy as u32
    } else if available_actions & wl_data_device_manager::DndAction::Move as u32 != 0 {
        wl_data_device_manager::DndAction::Move as u32
    } else if available_actions & wl_data_device_manager::DndAction::Ask as u32 != 0 {
        wl_data_device_manager::DndAction::Ask as u32
    } else {
        0
    }
}

impl Object for WlDataOffer {
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
        use wl_data_offer::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Accept { serial: _, mime_type } => {
                if self.finished {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_data_offer::Error::InvalidFinish as u32,
                        "wl_data_offer.accept is invalid after finish",
                    );
                    return Ok(());
                }
                // Some clients accept or request MIME types that do not exactly match the
                // advertised list. Be intentionally permissive here and let the source
                // client decide whether it can actually serve the requested MIME type.
                let accepted_mime = (!mime_type.is_empty()).then_some(mime_type);
                if self.role == DataOfferRole::Drag {
                    let Some(offer) = client.resolve(self.id, state) else {
                        return Ok(());
                    };
                    if !self.is_active_drag_offer(state, client.client_id(), offer) {
                        return Ok(());
                    }
                    let _ = state.seat.set_active_drag_accepted_mime(
                        client.client_id(),
                        offer,
                        accepted_mime.clone(),
                    );
                    if let Some(source) = self.source() {
                        dispatcher.dispatch_to(
                            source.client_id(),
                            ClientAction::DataSourceEvent {
                                source,
                                event: DataSourceClientEvent::Target(accepted_mime),
                            },
                        );
                    }
                }
            }
            Receive { mime_type, fd } => {
                if self.finished {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_data_offer::Error::InvalidFinish as u32,
                        "wl_data_offer.receive is invalid after finish",
                    );
                    let _ = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
                    return Ok(());
                }
                if self.role == DataOfferRole::Drag
                    && client.resolve(self.id, state).is_none_or(|offer| {
                        !self.is_active_drag_offer(state, client.client_id(), offer)
                    })
                {
                    let _ = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
                    return Ok(());
                }
                if let Some(source) = self.source() {
                    // Be intentionally permissive for compatibility: even if the MIME type
                    // was not previously advertised, forward the request to the source
                    // instead of rejecting it in the compositor.
                    dispatcher.dispatch_to(
                        source.client_id(),
                        ClientAction::DataSourceEvent {
                            source,
                            event: DataSourceClientEvent::Send { mime_type, fd },
                        },
                    );
                } else {
                    let _ = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
                }
            }
            Destroy => {
                if let Some(offer) = client.resolve(self.id, state) {
                    self.before_destroy(client.client_id(), offer, state, dispatcher);
                }
                client.delete_id(
                    &crate::wayland_objects::wl_display::WlDisplay::default(),
                    state,
                    dispatcher,
                    self.id.into(),
                );
            }
            Finish => {
                if self.role != DataOfferRole::Drag {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_data_offer::Error::InvalidFinish as u32,
                        "wl_data_offer.finish is only valid for drag-and-drop offers",
                    );
                    return Ok(());
                }
                let Some(offer) = client.resolve(self.id, state) else {
                    return Ok(());
                };
                if !self.is_active_drag_offer(state, client.client_id(), offer) {
                    return Ok(());
                }
                let Some(drag) = state.seat.active_drag() else {
                    return Ok(());
                };
                if drag.negotiation.accepted_mime.is_none() || drag.negotiation.chosen_action == 0 {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_data_offer::Error::InvalidFinish as u32,
                        "wl_data_offer.finish requires an accepted mime type and action",
                    );
                    return Ok(());
                }
                if drag.negotiation.chosen_action == wl_data_device_manager::DndAction::Ask as u32 {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_data_offer::Error::InvalidOffer as u32,
                        "wl_data_offer.finish requires a resolved drag-and-drop action",
                    );
                    return Ok(());
                }
                if drag.phase != DragPhase::DropSent {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_data_offer::Error::InvalidFinish as u32,
                        "wl_data_offer.finish before drop completion",
                    );
                    return Ok(());
                }
                self.finished = true;
                let _ = state
                    .seat
                    .set_active_drag_finished(client.client_id(), offer, true);
                if let Some(source) = self.source() {
                    dispatcher.dispatch_to(
                        source.client_id(),
                        ClientAction::DataSourceEvent {
                            source,
                            event: DataSourceClientEvent::Action {
                                dnd_action: drag.negotiation.chosen_action,
                            },
                        },
                    );
                    dispatcher.dispatch_to(
                        source.client_id(),
                        ClientAction::DataSourceEvent {
                            source,
                            event: DataSourceClientEvent::DndFinished,
                        },
                    );
                }
                seat::finish_active_drag(state, dispatcher);
            }
            SetActions { dnd_actions, preferred_action } => {
                if self.finished {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_data_offer::Error::InvalidFinish as u32,
                        "wl_data_offer.set_actions is invalid after finish",
                    );
                    return Ok(());
                }
                if self.role != DataOfferRole::Drag {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_data_offer::Error::InvalidOffer as u32,
                        "wl_data_offer.set_actions is only valid for drag-and-drop offers",
                    );
                    return Ok(());
                }
                let Some(offer) = client.resolve(self.id, state) else {
                    return Ok(());
                };
                if !self.is_active_drag_offer(state, client.client_id(), offer) {
                    return Ok(());
                }
                const VALID_MASK: u32 = wl_data_device_manager::DndAction::Copy as u32
                    | wl_data_device_manager::DndAction::Move as u32
                    | wl_data_device_manager::DndAction::Ask as u32;
                if dnd_actions & !VALID_MASK != 0 {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_data_offer::Error::InvalidActionMask as u32,
                        "invalid drag-and-drop destination action mask",
                    );
                    return Ok(());
                }
                if preferred_action & !VALID_MASK != 0
                    || preferred_action.count_ones() > 1
                    || (preferred_action != 0 && preferred_action & dnd_actions == 0)
                {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_data_offer::Error::InvalidAction as u32,
                        "invalid preferred drag-and-drop action",
                    );
                    return Ok(());
                }

                let source_actions = self
                    .source
                    .map(|src| src.object_id())
                    .and_then(|source_object_id| {
                        state
                            .objects
                            .try_ref_object::<WlDataSource>(source_object_id)
                    })
                    .map(|source| source.effective_dnd_actions())
                    .unwrap_or(0);
                let compositor_action = state
                    .seat
                    .active_drag()
                    .map(|drag| drag.negotiation.compositor_action)
                    .unwrap_or(0);
                let chosen_action = choose_dnd_action(
                    source_actions,
                    dnd_actions,
                    preferred_action,
                    compositor_action,
                );
                let _ = state.seat.set_active_drag_negotiation_actions(
                    client.client_id(),
                    offer,
                    dnd_actions,
                    preferred_action,
                    chosen_action,
                );
                self.send_action(chosen_action, dispatcher);
                for other_offer in state.seat.active_drag_offer_ids(client.client_id()) {
                    if other_offer == offer || !other_offer.exists(state) {
                        continue;
                    }
                    other_offer
                        .rref(state)
                        .send_action(chosen_action, dispatcher);
                }
                if let Some(source) = self.source() {
                    dispatcher.dispatch_to(
                        source.client_id(),
                        ClientAction::DataSourceEvent {
                            source,
                            event: DataSourceClientEvent::Action { dnd_action: chosen_action },
                        },
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {
        super::WlDataOffer,
        crate::{
            clients::{
                Client, ClientAction, ClientId, ClientManagerAction, DataSourceClientEvent,
                RawEvent, Resolved,
            },
            object::Object,
            protocol_generated::{
                wl_data_device_manager, WlDataOfferId, WlDataSourceId, WlSurfaceId,
            },
            runner::Dispatcher,
            state::{
                DataOfferRole, DragNegotiationState, DragPhase, DragSource, DragState,
                DragTargetState, ObjectId, State,
            },
            test_support::{state_for_tests, test_resolved_wl_surface},
            wayland_objects::wl_data_source::WlDataSource,
        },
        std::os::fd::{FromRawFd, IntoRawFd},
    };

    fn destroy_request(offer_id: WlDataOfferId) -> crate::clients::RawRequest<'static> {
        let event = RawEvent::new(offer_id.into(), 2, 0);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn finish_request(offer_id: WlDataOfferId) -> crate::clients::RawRequest<'static> {
        let event = RawEvent::new(offer_id.into(), 3, 0);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn receive_request(
        offer_id: WlDataOfferId,
        mime_type: &str,
        fd: i32,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(offer_id.into(), 1, 1);
        event.string(mime_type.to_string());
        let bytes = event.into_bytes();
        let mut request: crate::clients::RawRequest<'static> = (&*bytes).try_into().unwrap();
        let fds =
            std::collections::VecDeque::from([unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) }]);
        request.fds = Some(Box::leak(Box::new(fds)));
        request
    }

    fn accept_request(
        offer_id: WlDataOfferId,
        serial: u32,
        mime_type: &str,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(offer_id.into(), 0, 2);
        event.uint(serial);
        event.string(mime_type.to_string());
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn set_actions_request(
        offer_id: WlDataOfferId,
        dnd_actions: u32,
        preferred_action: u32,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(offer_id.into(), 4, 2);
        event.uint(dnd_actions);
        event.uint(preferred_action);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn fake_offer(
        client_id: ClientId,
        offer_id: WlDataOfferId,
        object_id: ObjectId,
    ) -> Resolved<WlDataOfferId> {
        Resolved::new(client_id, offer_id, object_id)
    }

    fn source_offer_request(
        source_id: WlDataSourceId,
        mime_type: &str,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(source_id.into(), 0, 1);
        event.string(mime_type.to_string());
        (&*event.into_bytes()).try_into().unwrap()
    }

    #[test]
    fn legacy_drag_offer_destroy_after_drop_finishes_source() {
        let mut state = state_for_tests();
        let source_client_id = ClientId::from_raw(1);
        let mut target = Client::new_for_tests(ClientId::from_raw(2));
        let offer_id = WlDataOfferId::from(20);
        let source_id = WlDataSourceId::from(10);
        let (offer_resolved, _) = target.insert_object(
            &mut state,
            WlDataOffer::new(
                offer_id,
                2,
                DataOfferRole::Drag,
                Some(Resolved::new(source_client_id, source_id, ObjectId::from(0usize))),
            ),
        );
        let offer_object_id = offer_resolved.object_id();

        state.seat.set_active_drag(DragState {
            source: DragSource::DataSource {
                source: Resolved::new(source_client_id, source_id, ObjectId::from(0usize)),
            },
            origin_surface: test_resolved_wl_surface(source_client_id, 30, ObjectId::from(30usize)),
            icon_surface: None,
            target: Some(DragTargetState {
                surface: test_resolved_wl_surface(target.client_id(), 31, ObjectId::from(0usize)),
            }),
            negotiation: DragNegotiationState {
                offer_ids: vec![fake_offer(target.client_id(), offer_id, offer_object_id)],
                accepted_mime: Some("text/plain".to_string()),
                destination_actions: 0,
                preferred_action: 0,
                compositor_action: 0,
                chosen_action: wl_data_device_manager::DndAction::Copy as u32,
                finished: false,
                has_finish_capable_offer: false,
            },
            phase: DragPhase::DropSent,
        });

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = target.test_dispatcher(&mut dispatcher);
        let mut offer = state.objects.grab_object::<WlDataOffer>(offer_object_id);
        offer.on(&mut target, &mut state, destroy_request(offer_id), &mut client_dispatcher);
        drop(offer);

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
        assert!(!dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            source_client_id,
            |action| {
                matches!(
                    action,
                    ClientAction::DataSourceEvent {
                        source: queued_source,
                        event: DataSourceClientEvent::Cancelled,
                    } if queued_source.wayland_id() == source_id
                )
            }
        ));
    }

    #[test]
    fn set_actions_rejects_preferred_action_outside_destination_mask() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(7));
        let offer_id = WlDataOfferId::from(20);
        let (offer_resolved, _) = client
            .insert_object(&mut state, WlDataOffer::new(offer_id, 3, DataOfferRole::Drag, None));
        let offer_object_id = offer_resolved.object_id();
        state.seat.set_active_drag(DragState {
            source: DragSource::None { initiator_client_id: ClientId::from_raw(9) },
            origin_surface: test_resolved_wl_surface(
                ClientId::from_raw(9),
                30,
                ObjectId::from(30usize),
            ),
            icon_surface: None,
            target: Some(DragTargetState {
                surface: test_resolved_wl_surface(client.client_id(), 31, ObjectId::from(0usize)),
            }),
            negotiation: DragNegotiationState {
                offer_ids: vec![fake_offer(client.client_id(), offer_id, offer_object_id)],
                accepted_mime: None,
                destination_actions: 0,
                preferred_action: 0,
                compositor_action: 0,
                chosen_action: 0,
                finished: false,
                has_finish_capable_offer: true,
            },
            phase: DragPhase::Dragging,
        });

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let mut offer = state.objects.grab_object::<WlDataOffer>(offer_object_id);
        offer.on(
            &mut client,
            &mut state,
            set_actions_request(
                offer_id,
                wl_data_device_manager::DndAction::Copy as u32,
                wl_data_device_manager::DndAction::Move as u32,
            ),
            &mut client_dispatcher,
        );

        assert!(dispatcher.test_has_queued_action::<ClientManagerAction>(|action| {
            matches!(action, ClientManagerAction::Close { id } if *id == client.client_id())
        }));
    }

    #[test]
    fn destroying_one_sibling_drag_offer_keeps_shared_drag_alive() {
        let mut state = state_for_tests();
        let source_client_id = ClientId::from_raw(1);
        let mut target = Client::new_for_tests(ClientId::from_raw(2));
        let offer_a_id = WlDataOfferId::from(20);
        let offer_b_id = WlDataOfferId::from(21);
        let (offer_a_resolved, _) = target.insert_object(
            &mut state,
            WlDataOffer::new(
                offer_a_id,
                3,
                DataOfferRole::Drag,
                Some(Resolved::new(
                    source_client_id,
                    WlDataSourceId::from(10),
                    ObjectId::from(0usize),
                )),
            ),
        );
        let offer_a_object_id = offer_a_resolved.object_id();
        let (offer_b_resolved, _) = target.insert_object(
            &mut state,
            WlDataOffer::new(
                offer_b_id,
                3,
                DataOfferRole::Drag,
                Some(Resolved::new(
                    source_client_id,
                    WlDataSourceId::from(10),
                    ObjectId::from(0usize),
                )),
            ),
        );
        let offer_b_object_id = offer_b_resolved.object_id();
        state.seat.set_active_drag(DragState {
            source: DragSource::DataSource {
                source: Resolved::new(
                    source_client_id,
                    WlDataSourceId::from(10),
                    ObjectId::from(0usize),
                ),
            },
            origin_surface: test_resolved_wl_surface(source_client_id, 30, ObjectId::from(30usize)),
            icon_surface: None,
            target: Some(DragTargetState {
                surface: test_resolved_wl_surface(target.client_id(), 31, ObjectId::from(0usize)),
            }),
            negotiation: DragNegotiationState {
                offer_ids: vec![
                    fake_offer(target.client_id(), offer_a_id, offer_a_object_id),
                    fake_offer(target.client_id(), offer_b_id, offer_b_object_id),
                ],
                accepted_mime: Some("text/plain".to_string()),
                destination_actions: wl_data_device_manager::DndAction::Copy as u32,
                preferred_action: wl_data_device_manager::DndAction::Copy as u32,
                compositor_action: 0,
                chosen_action: wl_data_device_manager::DndAction::Copy as u32,
                finished: false,
                has_finish_capable_offer: true,
            },
            phase: DragPhase::Dragging,
        });

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = target.test_dispatcher(&mut dispatcher);
        let mut offer = state.objects.grab_object::<WlDataOffer>(offer_a_object_id);
        offer.on(&mut target, &mut state, destroy_request(offer_a_id), &mut client_dispatcher);

        let drag = state.seat.active_drag().expect("drag should stay active");
        assert_eq!(
            drag.negotiation.offer_ids,
            vec![fake_offer(target.client_id(), offer_b_id, offer_b_object_id)]
        );
        assert!(!dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            source_client_id,
            |action| matches!(
                action,
                ClientAction::DataSourceEvent { event: DataSourceClientEvent::Cancelled, .. }
            )
        ));
    }

    #[test]
    fn stale_drag_offer_finish_is_ignored() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(7));
        let stale_offer_id = WlDataOfferId::from(20);
        let live_offer_id = WlDataOfferId::from(21);
        let (stale_offer_resolved, _) = client.insert_object(
            &mut state,
            WlDataOffer::new(stale_offer_id, 3, DataOfferRole::Drag, None),
        );
        let stale_offer_object_id = stale_offer_resolved.object_id();
        state.seat.set_active_drag(DragState {
            source: DragSource::None { initiator_client_id: ClientId::from_raw(9) },
            origin_surface: test_resolved_wl_surface(
                ClientId::from_raw(9),
                30,
                ObjectId::from(30usize),
            ),
            icon_surface: None,
            target: Some(DragTargetState {
                surface: test_resolved_wl_surface(client.client_id(), 31, ObjectId::from(0usize)),
            }),
            negotiation: DragNegotiationState {
                offer_ids: vec![fake_offer(
                    client.client_id(),
                    live_offer_id,
                    ObjectId::from(21usize),
                )],
                accepted_mime: Some("text/plain".to_string()),
                destination_actions: wl_data_device_manager::DndAction::Copy as u32,
                preferred_action: wl_data_device_manager::DndAction::Copy as u32,
                compositor_action: 0,
                chosen_action: wl_data_device_manager::DndAction::Copy as u32,
                finished: false,
                has_finish_capable_offer: true,
            },
            phase: DragPhase::DropSent,
        });

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let mut offer = state
            .objects
            .grab_object::<WlDataOffer>(stale_offer_object_id);
        offer.on(&mut client, &mut state, finish_request(stale_offer_id), &mut client_dispatcher);

        let drag = state
            .seat
            .active_drag()
            .expect("stale finish should be ignored");
        assert_eq!(
            drag.negotiation.offer_ids,
            vec![fake_offer(client.client_id(), live_offer_id, ObjectId::from(21usize),)]
        );
        assert!(!dispatcher.test_has_queued_action::<ClientManagerAction>(|action| {
            matches!(action, ClientManagerAction::Close { id } if *id == client.client_id())
        }));
    }

    #[test]
    fn selection_accept_does_not_mutate_drag_acceptance() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(7));
        let source_id = WlDataSourceId::from(10);
        let selection_offer_id = WlDataOfferId::from(20);
        let (source_resolved, _) =
            client.insert_object(&mut state, WlDataSource::new(source_id, client.client_id(), 3));
        let source_object_id = source_resolved.object_id();
        let (selection_offer_resolved, _) = client.insert_object(
            &mut state,
            WlDataOffer::new(
                selection_offer_id,
                3,
                DataOfferRole::Selection,
                Some(Resolved::new(client.client_id(), source_id, source_object_id)),
            ),
        );
        let selection_offer_object_id = selection_offer_resolved.object_id();
        state.seat.set_active_drag(DragState {
            source: DragSource::DataSource {
                source: Resolved::new(
                    ClientId::from_raw(9),
                    WlDataSourceId::from(99),
                    ObjectId::from(0usize),
                ),
            },
            origin_surface: test_resolved_wl_surface(
                ClientId::from_raw(9),
                30,
                ObjectId::from(30usize),
            ),
            icon_surface: None,
            target: Some(DragTargetState {
                surface: test_resolved_wl_surface(client.client_id(), 31, ObjectId::from(0usize)),
            }),
            negotiation: DragNegotiationState {
                offer_ids: vec![fake_offer(
                    client.client_id(),
                    WlDataOfferId::from(99),
                    ObjectId::from(99usize),
                )],
                accepted_mime: None,
                destination_actions: 0,
                preferred_action: 0,
                compositor_action: 0,
                chosen_action: 0,
                finished: false,
                has_finish_capable_offer: true,
            },
            phase: DragPhase::Dragging,
        });

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        {
            let mut source = state.objects.grab_object::<WlDataSource>(source_object_id);
            source.on(
                &mut client,
                &mut state,
                source_offer_request(source_id, "text/plain"),
                &mut client_dispatcher,
            );
        }
        let mut offer = state
            .objects
            .grab_object::<WlDataOffer>(selection_offer_object_id);
        offer.on(
            &mut client,
            &mut state,
            accept_request(selection_offer_id, 1, "text/plain"),
            &mut client_dispatcher,
        );

        let drag = state.seat.active_drag().expect("drag should remain active");
        assert!(drag.negotiation.accepted_mime.is_none());
    }

    #[test]
    fn finish_rejects_ask_action() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(7));
        let offer_id = WlDataOfferId::from(20);
        let source_id = WlDataSourceId::from(10);
        let (offer_resolved, _) = client.insert_object(
            &mut state,
            WlDataOffer::new(
                offer_id,
                3,
                DataOfferRole::Drag,
                Some(Resolved::new(ClientId::from_raw(9), source_id, ObjectId::from(0usize))),
            ),
        );
        let offer_object_id = offer_resolved.object_id();

        state.seat.set_active_drag(DragState {
            source: DragSource::DataSource {
                source: Resolved::new(ClientId::from_raw(9), source_id, ObjectId::from(0usize)),
            },
            origin_surface: test_resolved_wl_surface(
                ClientId::from_raw(9),
                30,
                ObjectId::from(30usize),
            ),
            icon_surface: None,
            target: Some(DragTargetState {
                surface: test_resolved_wl_surface(client.client_id(), 31, ObjectId::from(0usize)),
            }),
            negotiation: DragNegotiationState {
                offer_ids: vec![fake_offer(client.client_id(), offer_id, offer_object_id)],
                accepted_mime: Some("text/plain".to_string()),
                destination_actions: 0,
                preferred_action: 0,
                compositor_action: 0,
                chosen_action: wl_data_device_manager::DndAction::Ask as u32,
                finished: false,
                has_finish_capable_offer: true,
            },
            phase: DragPhase::DropSent,
        });

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let mut offer = state.objects.grab_object::<WlDataOffer>(offer_object_id);
        offer.on(&mut client, &mut state, finish_request(offer_id), &mut client_dispatcher);

        assert!(dispatcher.test_has_queued_action::<ClientManagerAction>(|action| {
            matches!(action, ClientManagerAction::Close { id } if *id == client.client_id())
        }));
    }

    #[test]
    fn accept_rejects_after_finish() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(7));
        let offer_id = WlDataOfferId::from(20);
        let (offer_resolved, _) = client
            .insert_object(&mut state, WlDataOffer::new(offer_id, 3, DataOfferRole::Drag, None));
        let offer_object_id = offer_resolved.object_id();

        {
            let offer = state
                .objects
                .get_mut(offer_object_id)
                .downcast_mut::<WlDataOffer>()
                .unwrap();
            offer.finished = true;
        }

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let mut offer = state.objects.grab_object::<WlDataOffer>(offer_object_id);
        offer.on(
            &mut client,
            &mut state,
            accept_request(offer_id, 1, "text/plain"),
            &mut client_dispatcher,
        );

        assert!(dispatcher.test_has_queued_action::<ClientManagerAction>(|action| {
            matches!(action, ClientManagerAction::Close { id } if *id == client.client_id())
        }));
    }

    #[test]
    fn accept_allows_unadvertised_mime_for_compatibility() {
        let mut state = state_for_tests();
        let source_client_id = ClientId::from_raw(8);
        let mut target = Client::new_for_tests(ClientId::from_raw(7));
        let source_id = WlDataSourceId::from(10);
        let offer_id = WlDataOfferId::from(20);
        let (offer_resolved, _) = target.insert_object(
            &mut state,
            WlDataOffer::new(
                offer_id,
                3,
                DataOfferRole::Drag,
                Some(Resolved::new(source_client_id, source_id, ObjectId::from(0usize))),
            ),
        );
        let offer_object_id = offer_resolved.object_id();
        state.seat.set_active_drag(DragState {
            source: DragSource::DataSource {
                source: Resolved::new(source_client_id, source_id, ObjectId::from(0usize)),
            },
            origin_surface: test_resolved_wl_surface(source_client_id, 30, ObjectId::from(30usize)),
            icon_surface: None,
            target: Some(DragTargetState {
                surface: test_resolved_wl_surface(target.client_id(), 31, ObjectId::from(0usize)),
            }),
            negotiation: DragNegotiationState {
                offer_ids: vec![fake_offer(target.client_id(), offer_id, offer_object_id)],
                accepted_mime: None,
                destination_actions: 0,
                preferred_action: 0,
                compositor_action: 0,
                chosen_action: 0,
                finished: false,
                has_finish_capable_offer: true,
            },
            phase: DragPhase::Dragging,
        });

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = target.test_dispatcher(&mut dispatcher);
        let mut offer = state.objects.grab_object::<WlDataOffer>(offer_object_id);
        offer.on(
            &mut target,
            &mut state,
            accept_request(offer_id, 1, "application/x-unadvertised"),
            &mut client_dispatcher,
        );

        assert!(dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            source_client_id,
            |action| {
                matches!(
                    action,
                    ClientAction::DataSourceEvent {
                        source: queued_source,
                        event: DataSourceClientEvent::Target(Some(mime_type)),
                    } if queued_source.wayland_id() == source_id
                        && mime_type == "application/x-unadvertised"
                )
            }
        ));
        let drag = state.seat.active_drag().expect("drag should stay active");
        assert_eq!(drag.negotiation.accepted_mime.as_deref(), Some("application/x-unadvertised"));
    }

    #[test]
    fn receive_forwards_unadvertised_mime_to_source_for_compatibility() {
        let mut state = state_for_tests();
        let source_client_id = ClientId::from_raw(8);
        let mut target = Client::new_for_tests(ClientId::from_raw(7));
        let source_id = WlDataSourceId::from(10);
        let offer_id = WlDataOfferId::from(20);
        let (offer_resolved, _) = target.insert_object(
            &mut state,
            WlDataOffer::new(
                offer_id,
                3,
                DataOfferRole::Drag,
                Some(Resolved::new(source_client_id, source_id, ObjectId::from(0usize))),
            ),
        );
        let offer_object_id = offer_resolved.object_id();
        state.seat.set_active_drag(DragState {
            source: DragSource::DataSource {
                source: Resolved::new(source_client_id, source_id, ObjectId::from(0usize)),
            },
            origin_surface: test_resolved_wl_surface(source_client_id, 30, ObjectId::from(30usize)),
            icon_surface: None,
            target: Some(DragTargetState {
                surface: test_resolved_wl_surface(target.client_id(), 31, ObjectId::from(0usize)),
            }),
            negotiation: DragNegotiationState {
                offer_ids: vec![fake_offer(target.client_id(), offer_id, offer_object_id)],
                accepted_mime: Some("application/x-unadvertised".to_string()),
                destination_actions: wl_data_device_manager::DndAction::Copy as u32,
                preferred_action: wl_data_device_manager::DndAction::Copy as u32,
                compositor_action: 0,
                chosen_action: wl_data_device_manager::DndAction::Copy as u32,
                finished: false,
                has_finish_capable_offer: true,
            },
            phase: DragPhase::Dragging,
        });

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = target.test_dispatcher(&mut dispatcher);
        let mut offer = state.objects.grab_object::<WlDataOffer>(offer_object_id);
        offer.on(
            &mut target,
            &mut state,
            receive_request(
                offer_id,
                "application/x-unadvertised",
                std::fs::File::open("/dev/null").unwrap().into_raw_fd(),
            ),
            &mut client_dispatcher,
        );

        assert!(dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            source_client_id,
            |action| {
                matches!(
                    action,
                    ClientAction::DataSourceEvent {
                        source: queued_source,
                        event: DataSourceClientEvent::Send { mime_type, .. },
                    } if queued_source.wayland_id() == source_id
                        && mime_type == "application/x-unadvertised"
                )
            }
        ));
    }
}
