//! `ext_idle_notifier_v1` version 2.
//!
//! Destroying the manager leaves notification objects alive. `get_input_idle_notification`
//! exists only on version 2 and ignores inhibitors; `get_idle_notification` obeys them.
//! Inhibitor visibility is a later phase, so both kinds currently see the same activity.

use std::time::Instant;

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_idle_notifier_v1 as proto, wl_display, Constants, ExtIdleNotifierV1Id, NewId, WaylandId,
    },
    state::{GrabbedObject, State},
    wayland_objects::{ext_idle_notification_v1::create_notification, wl_display::WlDisplay},
};

#[derive(Debug)]
pub struct ExtIdleNotifierV1Global;

impl ExtIdleNotifierV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ExtIdleNotifierV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (ExtIdleNotifierV1::INTERFACE_NAME, ExtIdleNotifierV1::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        client
            .insert_object(state, ExtIdleNotifierV1 { id: ExtIdleNotifierV1Id::from(id), version });
    }
}

#[derive(Debug)]
pub struct ExtIdleNotifierV1 {
    id: ExtIdleNotifierV1Id,
    version: u32,
}

impl Object for ExtIdleNotifierV1 {
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
            proto::Request::Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            proto::Request::GetIdleNotification { id, timeout, seat } => {
                create_notification(
                    client,
                    state,
                    dispatcher,
                    id,
                    timeout,
                    seat,
                    true,
                    Instant::now(),
                )?;
            }
            proto::Request::GetInputIdleNotification { id, timeout, seat } => {
                if self.version < proto::GET_INPUT_IDLE_NOTIFICATION_SINCE {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidMethod as u32,
                        "ext_idle_notifier_v1.get_input_idle_notification requires version >= 2",
                    );
                    return Ok(());
                }
                create_notification(
                    client,
                    state,
                    dispatcher,
                    id,
                    timeout,
                    seat,
                    false,
                    Instant::now(),
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, time::Duration};

    use crate::{
        clients::{Arguments, Client, ClientAction, ClientId, ClientManagerAction, RawRequest},
        globals::Globals,
        object::Object,
        protocol_generated::{Constants, ExtIdleNotificationV1Id, ExtIdleNotifierV1Id, WlSeatId},
        runner::{Dispatcher, Model},
        scheduler::idle_scheduler::{IdleScheduler, IdleSchedulerAction},
        test_support::state_for_tests,
        wayland_objects::{
            ext_idle_notification_v1::{evaluate, on_activity, IdleNotice},
            wl_seat::WlSeat,
        },
    };

    use super::ExtIdleNotifierV1;

    struct Fixture {
        state: crate::state::State,
        client: Client,
        dispatcher: Dispatcher,
        manager: crate::clients::Resolved<ExtIdleNotifierV1Id>,
        seat: crate::clients::Resolved<WlSeatId>,
    }

    fn new_fixture(version: u32) -> Fixture {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let (seat, _) = client.insert_object(&mut state, WlSeat::new(WlSeatId::from(3)));
        let (manager, _) = client.insert_object(
            &mut state,
            ExtIdleNotifierV1 { id: ExtIdleNotifierV1Id::from(6), version },
        );
        Fixture {
            state,
            client,
            dispatcher: Dispatcher::default(),
            manager,
            seat,
        }
    }

    fn words(values: &[u32]) -> RawRequest<'static> {
        RawRequest {
            object_id: 0,
            request_id: 0,
            arguments: Arguments::from_words(VecDeque::from(values.to_vec())),
            fds: None,
        }
    }

    fn dispatch(fixture: &mut Fixture, object_id: u32, request_id: u16, arguments: &[u32]) {
        let mut request = words(arguments);
        request.object_id = object_id;
        request.request_id = request_id;
        let mut client_dispatcher = fixture.client.test_dispatcher(&mut fixture.dispatcher);
        client_dispatcher.set_requester_object_id_for_tests(object_id.into());
        fixture
            .state
            .objects
            .grab(fixture.manager.object_id())
            .on(&mut fixture.client, &mut fixture.state, request, &mut client_dispatcher)
            .unwrap();
    }

    fn get_idle(fixture: &mut Fixture, id: u32, timeout_msec: u32) {
        dispatch(fixture, 6, 1, &[id, timeout_msec, 3]);
    }

    fn notices(fixture: &Fixture) -> Vec<IdleNotice> {
        let mut notices = Vec::new();
        fixture
            .dispatcher
            .test_matching_actions_for::<ClientAction, _>(ClientId::from_raw(1), |action| {
                if let ClientAction::IdleNotification { notice, .. } = action {
                    notices.push(*notice);
                }
            });
        notices
    }

    fn notification<'a>(
        fixture: &'a Fixture,
        id: u32,
    ) -> &'a crate::wayland_objects::ext_idle_notification_v1::ExtIdleNotificationV1 {
        fixture
            .client
            .resolve(ExtIdleNotificationV1Id::from(id), &fixture.state)
            .unwrap()
            .rref(&fixture.state)
    }

    #[test]
    fn the_idle_notifier_global_is_advertised_at_version_two() {
        let mut globals = Globals::new();
        globals.initialize();
        let (name, version) = globals
            .iter()
            .find_map(|(_, global)| {
                (global.interface().0 == ExtIdleNotifierV1::INTERFACE_NAME)
                    .then(|| global.interface())
            })
            .unwrap();
        assert_eq!(name, "ext_idle_notifier_v1");
        assert_eq!(version, 2);
        assert!(!globals.contains_interface("zwp_idle_inhibit_manager_v1"));
    }

    #[test]
    fn a_notification_idles_at_its_deadline_and_resumes_once() {
        let mut fixture = new_fixture(2);
        get_idle(&mut fixture, 7, 5_000);
        let idle = notification(&fixture, 7);
        assert!(!idle.is_idle());
        assert!(idle.obeys_inhibitors());
        assert_eq!(idle.seat(), fixture.seat);
        let deadline = idle.deadline().unwrap();
        assert!(fixture
            .dispatcher
            .test_has_queued_action(|action: &IdleSchedulerAction| {
                matches!(action, IdleSchedulerAction::SetDeadline { at, .. } if *at == deadline)
            }));

        evaluate(&mut fixture.state, &mut fixture.dispatcher, deadline - Duration::from_millis(1));
        assert!(!notification(&fixture, 7).is_idle());
        assert!(notices(&fixture).is_empty());

        let mut scheduler = IdleScheduler::new();
        scheduler
            .process(
                &mut fixture.state,
                IdleSchedulerAction::Evaluate { now: deadline },
                &mut fixture.dispatcher,
            )
            .unwrap();
        assert!(notification(&fixture, 7).is_idle());
        assert_eq!(notices(&fixture), vec![IdleNotice::Idled]);

        scheduler
            .process(
                &mut fixture.state,
                IdleSchedulerAction::Evaluate { now: deadline + Duration::from_secs(1) },
                &mut fixture.dispatcher,
            )
            .unwrap();
        assert_eq!(notices(&fixture), vec![IdleNotice::Idled]);

        on_activity(&mut fixture.state, &mut fixture.dispatcher, deadline);
        assert!(!notification(&fixture, 7).is_idle());
        assert_eq!(notices(&fixture), vec![IdleNotice::Idled, IdleNotice::Resumed]);
        let restarted = notification(&fixture, 7).deadline().unwrap();
        assert!(restarted > deadline);

        on_activity(&mut fixture.state, &mut fixture.dispatcher, restarted);
        assert_eq!(notices(&fixture), vec![IdleNotice::Idled, IdleNotice::Resumed]);
        let restarted_again = notification(&fixture, 7).deadline().unwrap();

        evaluate(&mut fixture.state, &mut fixture.dispatcher, restarted_again);
        assert_eq!(
            notices(&fixture),
            vec![IdleNotice::Idled, IdleNotice::Resumed, IdleNotice::Idled]
        );
    }

    #[test]
    fn a_zero_timeout_idles_immediately_and_idles_again_after_resume() {
        let mut fixture = new_fixture(2);
        get_idle(&mut fixture, 7, 0);
        assert!(notification(&fixture, 7).is_idle());
        assert!(notification(&fixture, 7).deadline().is_none());
        assert_eq!(notices(&fixture), vec![IdleNotice::Idled]);
        assert!(!fixture
            .dispatcher
            .test_has_queued_action(|action: &IdleSchedulerAction| {
                matches!(action, IdleSchedulerAction::SetDeadline { .. })
            }));

        on_activity(&mut fixture.state, &mut fixture.dispatcher, std::time::Instant::now());
        assert!(notification(&fixture, 7).is_idle());
        assert_eq!(
            notices(&fixture),
            vec![IdleNotice::Idled, IdleNotice::Resumed, IdleNotice::Idled]
        );
    }

    #[test]
    fn input_idle_ignores_the_inhibitor_flag_and_version_one_cannot_create_it() {
        let mut fixture = new_fixture(2);
        dispatch(&mut fixture, 6, 2, &[7, 1_000, 3]);
        assert!(!notification(&fixture, 7).obeys_inhibitors());

        let mut old = new_fixture(1);
        let mut request = words(&[8, 1_000, 3]);
        request.object_id = 6;
        request.request_id = 2;
        let mut client_dispatcher = old.client.test_dispatcher(&mut old.dispatcher);
        client_dispatcher.set_requester_object_id_for_tests(6.into());
        old.state
            .objects
            .grab(old.manager.object_id())
            .on(&mut old.client, &mut old.state, request, &mut client_dispatcher)
            .unwrap();
        assert!(!old
            .client
            .contains_object(ExtIdleNotificationV1Id::from(8), &old.state));
        assert!(old
            .dispatcher
            .test_has_queued_action(|action: &ClientManagerAction| {
                matches!(action, ClientManagerAction::Close { id } if *id == ClientId::from_raw(1))
            }));
    }

    #[test]
    fn version_one_can_still_create_an_inhibitor_aware_notification() {
        let mut fixture = new_fixture(1);
        get_idle(&mut fixture, 7, 1_000);
        assert!(notification(&fixture, 7).obeys_inhibitors());
        assert!(fixture
            .client
            .contains_object(ExtIdleNotificationV1Id::from(7), &fixture.state));
    }

    #[test]
    fn an_unknown_seat_disconnects_without_a_notification() {
        let mut fixture = new_fixture(2);
        let mut request = words(&[7, 1_000, 99]);
        request.object_id = 6;
        request.request_id = 1;
        let mut client_dispatcher = fixture.client.test_dispatcher(&mut fixture.dispatcher);
        client_dispatcher.set_requester_object_id_for_tests(6.into());
        let result = fixture.state.objects.grab(fixture.manager.object_id()).on(
            &mut fixture.client,
            &mut fixture.state,
            request,
            &mut client_dispatcher,
        );
        assert!(result.is_err());
        assert!(!fixture
            .client
            .contains_object(ExtIdleNotificationV1Id::from(7), &fixture.state));
        assert!(fixture.state.idle_notifications.is_empty());
        assert!(fixture
            .dispatcher
            .test_has_queued_action(|action: &ClientManagerAction| {
                matches!(action, ClientManagerAction::Close { id } if *id == ClientId::from_raw(1))
            }));
    }

    #[test]
    fn destroying_the_manager_leaves_the_notification_running() {
        let mut fixture = new_fixture(2);
        get_idle(&mut fixture, 7, 5_000);
        let deadline = notification(&fixture, 7).deadline().unwrap();
        dispatch(&mut fixture, 6, 0, &[]);
        assert!(!fixture
            .client
            .contains_object(ExtIdleNotifierV1Id::from(6), &fixture.state));
        assert!(fixture
            .client
            .contains_object(ExtIdleNotificationV1Id::from(7), &fixture.state));
        evaluate(&mut fixture.state, &mut fixture.dispatcher, deadline);
        assert_eq!(notices(&fixture), vec![IdleNotice::Idled]);
    }

    #[test]
    fn destroying_a_notification_sends_nothing_and_drops_its_deadline() {
        let mut fixture = new_fixture(2);
        get_idle(&mut fixture, 7, 5_000);
        let notification_id = fixture
            .client
            .resolve(ExtIdleNotificationV1Id::from(7), &fixture.state)
            .unwrap();
        let mut request = words(&[]);
        request.object_id = 7;
        request.request_id = 0;
        let mut client_dispatcher = fixture.client.test_dispatcher(&mut fixture.dispatcher);
        client_dispatcher.set_requester_object_id_for_tests(7.into());
        fixture
            .state
            .objects
            .grab(notification_id.object_id())
            .on(&mut fixture.client, &mut fixture.state, request, &mut client_dispatcher)
            .unwrap();
        assert!(!fixture
            .client
            .contains_object(ExtIdleNotificationV1Id::from(7), &fixture.state));
        assert!(fixture.state.idle_notifications.is_empty());
        assert!(notices(&fixture).is_empty());
        assert!(fixture
            .dispatcher
            .test_has_queued_action(|action: &IdleSchedulerAction| {
                matches!(action, IdleSchedulerAction::ClearDeadline { .. })
            }));
    }

    #[test]
    fn client_disconnect_drops_the_notification_without_an_event() {
        let mut fixture = new_fixture(2);
        get_idle(&mut fixture, 7, 5_000);
        let notification_id = fixture
            .client
            .resolve(ExtIdleNotificationV1Id::from(7), &fixture.state)
            .unwrap();
        {
            let mut notification = fixture
                .state
                .objects
                .try_grab_object::<crate::wayland_objects::ext_idle_notification_v1::ExtIdleNotificationV1>(
                    notification_id.object_id(),
                )
                .unwrap();
            let mut client_dispatcher = fixture.client.test_dispatcher(&mut fixture.dispatcher);
            notification.on_client_disconnect(
                &mut fixture.client,
                &mut fixture.state,
                &mut client_dispatcher,
            );
        }
        assert!(fixture.state.idle_notifications.is_empty());
        assert!(notices(&fixture).is_empty());
        assert!(fixture
            .dispatcher
            .test_has_queued_action(|action: &IdleSchedulerAction| {
                matches!(action, IdleSchedulerAction::ClearDeadline { .. })
            }));
    }

    #[test]
    fn the_sooner_of_two_notifications_idles_alone() {
        let mut fixture = new_fixture(2);
        get_idle(&mut fixture, 7, 1_000);
        get_idle(&mut fixture, 8, 3_600_000);
        let sooner = notification(&fixture, 7).deadline().unwrap();
        let later = notification(&fixture, 8).deadline().unwrap();
        assert!(sooner < later);
        evaluate(&mut fixture.state, &mut fixture.dispatcher, sooner);
        assert!(notification(&fixture, 7).is_idle());
        assert!(!notification(&fixture, 8).is_idle());
        assert_eq!(notices(&fixture), vec![IdleNotice::Idled]);
    }

    #[test]
    fn a_paused_session_still_delivers_idled() {
        let mut fixture = new_fixture(2);
        get_idle(&mut fixture, 7, 5_000);
        let deadline = notification(&fixture, 7).deadline().unwrap();
        fixture.state.session_active = false;
        evaluate(&mut fixture.state, &mut fixture.dispatcher, deadline);
        assert_eq!(notices(&fixture), vec![IdleNotice::Idled]);
    }

    #[test]
    fn the_client_writes_an_idled_event() {
        let mut fixture = new_fixture(2);
        get_idle(&mut fixture, 7, 0);
        fixture
            .client
            .process(
                &mut fixture.state,
                ClientAction::IdleNotification {
                    id: ExtIdleNotificationV1Id::from(7),
                    notice: IdleNotice::Idled,
                },
                &mut fixture.dispatcher,
            )
            .unwrap();
        assert!(fixture.dispatcher.test_has_queued_action(
            |action: &crate::io_uring::model::IoUringAction| {
                matches!(action, crate::io_uring::model::IoUringAction::Sendmsg { .. })
            }
        ));
    }
}
