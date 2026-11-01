//! `ext_idle_notification_v1` objects.
//!
//! The timeout starts when the notification is created, as the protocol
//! requires and as wlroots, Smithay, and Hyprland do (`update(0)` in
//! Hyprland). Jay turns a zero timeout into one millisecond; a zero timeout
//! here sends `idled` immediately. Activity restarts the full timeout from
//! that event. `resumed` is sent only after `idled`, and never twice in a row.
//!
//! wlroots, Smithay, and Hyprland arm one timer per notification. Those
//! deadlines share this compositor's single idle timerfd. Inhibition is
//! recorded on the object and applied in a later phase; nothing is inhibited yet.

use std::time::{Duration, Instant};

use crate::{
    clients::{
        Client, ClientAction, ClientDispatcher, ClientId, EventWriter, RawRequest, Resolved,
    },
    object::{Object, Parser, RequestResult},
    protocol_generated::{ext_idle_notification_v1, ExtIdleNotificationV1Id, WaylandId, WlSeatId},
    runner::Dispatcher,
    scheduler::idle_scheduler::{IdleDeadlineId, IdleSchedulerAction},
    state::{GrabbedObject, ObjectId, State},
    wayland_objects::wl_display::WlDisplay,
};

/// Which idle transition to deliver. Stored on [`ClientAction`] so the client
/// model performs the write when it is not inside this object's handler.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IdleNotice {
    Idled,
    Resumed,
}

#[derive(Debug)]
pub struct ExtIdleNotificationV1 {
    id: ExtIdleNotificationV1Id,
    client_id: ClientId,
    object_id: ObjectId,
    seat: Resolved<WlSeatId>,
    timeout: Duration,
    obey_inhibitors: bool,
    idle: bool,
    deadline: Option<Instant>,
    deadline_id: IdleDeadlineId,
}

impl ExtIdleNotificationV1 {
    fn new(
        id: ExtIdleNotificationV1Id,
        client_id: ClientId,
        seat: Resolved<WlSeatId>,
        timeout_msec: u32,
        obey_inhibitors: bool,
    ) -> Self {
        Self {
            id,
            client_id,
            object_id: ObjectId::from_parts(0, 0),
            seat,
            timeout: Duration::from_millis(u64::from(timeout_msec)),
            obey_inhibitors,
            idle: false,
            deadline: None,
            deadline_id: IdleDeadlineId::from_raw(0),
        }
    }

    fn attach(&mut self, object_id: ObjectId) {
        self.object_id = object_id;
        self.deadline_id = IdleDeadlineId::from_raw(object_id.as_u64());
    }

    pub(crate) fn seat(&self) -> Resolved<WlSeatId> {
        self.seat
    }

    pub(crate) fn obeys_inhibitors(&self) -> bool {
        self.obey_inhibitors
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.idle
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub(crate) fn deadline_id(&self) -> IdleDeadlineId {
        self.deadline_id
    }

    pub(crate) fn send_notice(&self, notice: IdleNotice, dispatcher: &mut ClientDispatcher) {
        let event = match notice {
            IdleNotice::Idled => ext_idle_notification_v1::Event::Idled,
            IdleNotice::Resumed => ext_idle_notification_v1::Event::Resumed,
        };
        self.send_event(event, dispatcher);
    }

    fn emit(&self, dispatcher: &mut Dispatcher, notice: IdleNotice) {
        dispatcher
            .dispatch_to(self.client_id, ClientAction::IdleNotification { id: self.id, notice });
    }

    /// Arm from `now`. A zero timeout becomes idle immediately and does not
    /// start `resumed`; the object is not idle until this runs.
    fn arm_from(&mut self, dispatcher: &mut Dispatcher, now: Instant) {
        if self.timeout.is_zero() {
            self.mark_idle(dispatcher);
            return;
        }
        let at = now + self.timeout;
        self.deadline = Some(at);
        dispatcher.dispatch(IdleSchedulerAction::SetDeadline { id: self.deadline_id, at });
    }

    fn mark_idle(&mut self, dispatcher: &mut Dispatcher) {
        if self.idle {
            return;
        }
        self.idle = true;
        self.deadline = None;
        self.emit(dispatcher, IdleNotice::Idled);
        dispatcher.dispatch(IdleSchedulerAction::ClearDeadline { id: self.deadline_id });
    }

    fn handle_activity(&mut self, dispatcher: &mut Dispatcher, now: Instant) {
        if self.idle {
            self.idle = false;
            self.deadline = None;
            self.emit(dispatcher, IdleNotice::Resumed);
        }
        self.arm_from(dispatcher, now);
    }

    fn evaluate_at(&mut self, dispatcher: &mut Dispatcher, now: Instant) {
        let Some(deadline) = self.deadline else {
            return;
        };
        if self.idle || deadline > now {
            return;
        }
        self.mark_idle(dispatcher);
    }

    fn release_deadline(&mut self, dispatcher: &mut Dispatcher) {
        self.deadline = None;
        dispatcher.dispatch(IdleSchedulerAction::ClearDeadline { id: self.deadline_id });
    }

    fn unregister(&self, state: &mut State) {
        let object_id = self.object_id;
        state
            .idle_notifications
            .retain(|notification| notification.object_id() != object_id);
    }
}

/// Real input on the compositor seat. Every live notification restarts from
/// `now`. There is one seat, so a notification stays tied to the `wl_seat` it
/// was created with without waiting for that resource to remain alive.
pub(crate) fn on_activity(state: &mut State, dispatcher: &mut Dispatcher, now: Instant) {
    let object_ids: Vec<ObjectId> = state
        .idle_notifications
        .iter()
        .map(|notification| notification.object_id())
        .collect();
    for object_id in object_ids {
        let Some(mut notification) = state
            .objects
            .try_grab_object::<ExtIdleNotificationV1>(object_id)
        else {
            continue;
        };
        notification.handle_activity(dispatcher, now);
    }
}

/// Send `idled` for notifications whose deadline is at or before `now`.
pub(crate) fn evaluate(state: &mut State, dispatcher: &mut Dispatcher, now: Instant) {
    let object_ids: Vec<ObjectId> = state
        .idle_notifications
        .iter()
        .map(|notification| notification.object_id())
        .collect();
    for object_id in object_ids {
        let Some(mut notification) = state
            .objects
            .try_grab_object::<ExtIdleNotificationV1>(object_id)
        else {
            continue;
        };
        notification.evaluate_at(dispatcher, now);
    }
}

pub(crate) fn create_notification(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    id: ExtIdleNotificationV1Id,
    timeout_msec: u32,
    seat: WlSeatId,
    obey_inhibitors: bool,
    now: Instant,
) -> RequestResult {
    let seat = client.resolve_or_disconnect(seat, state, dispatcher)?;
    let resolved = {
        let (resolved, notification) = client.insert_object(
            state,
            ExtIdleNotificationV1::new(id, client.client_id(), seat, timeout_msec, obey_inhibitors),
        );
        notification.attach(resolved.object_id());
        notification.arm_from(dispatcher, now);
        resolved
    };
    state.idle_notifications.push(resolved);
    Ok(())
}

impl Object for ExtIdleNotificationV1 {
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
        match client.parse::<Self>(req, dispatcher)? {
            ext_idle_notification_v1::Request::Destroy => {
                self.release_deadline(dispatcher);
                self.unregister(state);
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        _client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.release_deadline(dispatcher);
        self.unregister(state);
    }
}
