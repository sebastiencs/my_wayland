//! Whether any idle inhibitor is honored.
//!
//! Sway honors an application inhibitor when `view_is_visible` is true, and a
//! layer-shell inhibitor whenever that layer is mapped on an enabled output,
//! even if something else covers its pixels. This compositor uses the same
//! split. Workspace windows follow [`crate::scene::visibility`]: mapped, on an
//! enabled output, protocol-visible, and not covered by another fullscreen
//! window on that workspace. Subsurfaces and popups use their root. A mapped
//! layer surface inhibits on its own and is not suppressed by fullscreen.
//!
//! Hyprland's `setInhibit` calls `update(0)`: an already-idle inhibitor-aware
//! notification resumes immediately, and lifting inhibition starts a fresh
//! timeout. Input-idle notifications ignore the bit.
//!
//! Milestone 6 can reject inhibition while a session is locked by changing
//! [`any_honored_inhibitor`] before the visibility walk.

use std::time::Instant;

use crate::{
    clients::{ClientDispatcher, Resolved},
    object::PostAction,
    protocol_generated::{WlSurfaceId, ZwpIdleInhibitorV1Id},
    runner::Dispatcher,
    scene::visibility::root_has_protocol_visibility,
    state::{OutputId, State},
    wayland_objects::ext_idle_notification_v1::ExtIdleNotificationV1,
};

pub(crate) fn register_inhibitor(state: &mut State, inhibitor: Resolved<ZwpIdleInhibitorV1Id>) {
    if !state.idle_inhibitors.contains(&inhibitor) {
        state.idle_inhibitors.push(inhibitor);
    }
}

pub(crate) fn unregister_inhibitor(state: &mut State, inhibitor: Resolved<ZwpIdleInhibitorV1Id>) {
    state
        .idle_inhibitors
        .retain(|existing| *existing != inhibitor);
}

pub(crate) fn any_honored_inhibitor(state: &State) -> bool {
    let surfaces = state
        .idle_inhibitors
        .iter()
        .filter_map(|inhibitor| {
            inhibitor
                .try_ref(state)
                .and_then(|inhibitor| inhibitor.surface())
        })
        .collect::<Vec<_>>();
    surfaces
        .into_iter()
        .any(|surface| inhibitor_is_honored(state, surface))
}

pub(crate) fn inhibitor_is_honored(state: &State, surface: Resolved<WlSurfaceId>) -> bool {
    let Some(root) = state.root_surface_for_surface(surface) else {
        return false;
    };
    if layer_is_honored(state, root) {
        return true;
    }
    if root
        .try_ref(state)
        .is_some_and(|surface| surface.layer_surface().is_some())
    {
        return false;
    }
    state.root_surface_is_mapped(root)
        && root_has_protocol_visibility(state, root)
        && workspace_output_enabled(state, root)
}

pub(crate) fn refresh(state: &mut State, dispatcher: &mut Dispatcher, now: Instant) {
    let honored = any_honored_inhibitor(state);
    if state.idle_inhibitors_honored == honored {
        return;
    }
    state.idle_inhibitors_honored = honored;
    let object_ids = state
        .idle_notifications
        .iter()
        .map(|notification| notification.object_id())
        .collect::<Vec<_>>();
    for object_id in object_ids {
        let Some(mut notification) = state
            .objects
            .try_grab_object::<ExtIdleNotificationV1>(object_id)
        else {
            continue;
        };
        if honored {
            notification.inhibit(dispatcher);
        } else {
            notification.resume_after_inhibition(dispatcher, now);
        }
    }
}

pub(crate) fn queue_refresh(dispatcher: &mut ClientDispatcher) {
    dispatcher.dispatch_post_action(RefreshIdleInhibition);
}

struct RefreshIdleInhibition;

impl PostAction for RefreshIdleInhibition {
    fn execute(
        self: Box<Self>,
        _client: &mut crate::clients::Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        refresh(state, dispatcher, Instant::now());
    }
}

fn layer_is_honored(state: &State, root: Resolved<WlSurfaceId>) -> bool {
    let Some(surface) = root.try_ref(state) else {
        return false;
    };
    let Some(layer_id) = surface.layer_surface() else {
        return false;
    };
    let Some(layer) = layer_id.try_ref(state) else {
        return false;
    };
    if !layer.is_mapped() {
        return false;
    }
    let Some(output_id) = layer.resolved_output_id(state) else {
        return false;
    };
    output_enabled(state, output_id)
}

fn workspace_output_enabled(state: &State, root: Resolved<WlSurfaceId>) -> bool {
    let Some(workspace_id) = state.root_surface_workspace(root) else {
        return true;
    };
    state
        .workspace_output_ids(workspace_id)
        .into_iter()
        .any(|output_id| output_enabled(state, output_id))
}

fn output_enabled(state: &State, output_id: OutputId) -> bool {
    state.output_bounds(output_id).is_some()
        && state
            .outputs
            .try_get(output_id)
            .is_some_and(|output| output.enabled)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::{
        clients::{Client, ClientAction, ClientId, Resolved},
        protocol_generated::{
            WlSeatId, WlSurfaceId, ZwlrLayerShellV1Id, ZwlrLayerSurfaceV1Id, ZwpIdleInhibitorV1Id,
        },
        runner::Dispatcher,
        state::{BackendOutputRef, DrmId, ObjectId, State},
        test_support::{state_for_tests, test_output_with_crtc_name},
        wayland_objects::{
            ext_idle_notification_v1::{self, IdleNotice},
            wl_seat::WlSeat,
            wl_surface::WlSurface,
            zwlr_layer_surface_v1::{LayerShellLayer, ZwlrLayerSurfaceV1},
            zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1,
        },
    };

    use super::{inhibitor_is_honored, refresh, register_inhibitor};

    fn output_and_root(state: &mut State, raw: u32) -> Resolved<WlSurfaceId> {
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        if state.outputs.iter().next().is_none() {
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        }
        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(raw), ClientId::from_raw(1), 6));
        let surface = Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(raw), object_id);
        state.register_root_surface(surface);
        surface
    }

    fn map_root(state: &mut State, surface: Resolved<WlSurfaceId>) {
        state
            .managed_root_surfaces
            .get_mut(&surface)
            .unwrap()
            .mapped = true;
    }

    fn track_inhibitor(
        state: &mut State,
        surface: Resolved<WlSurfaceId>,
        raw: u32,
    ) -> Resolved<ZwpIdleInhibitorV1Id> {
        let (_, object_id) =
            state.insert_object(ZwpIdleInhibitorV1::new(ZwpIdleInhibitorV1Id::from(raw), surface));
        let inhibitor =
            Resolved::new(ClientId::from_raw(1), ZwpIdleInhibitorV1Id::from(raw), object_id);
        surface
            .try_ref_mut(state)
            .unwrap()
            .add_idle_inhibitor(inhibitor);
        register_inhibitor(state, inhibitor);
        inhibitor
    }

    #[test]
    fn only_a_mapped_visible_window_inhibits() {
        let mut state = state_for_tests();
        let root = output_and_root(&mut state, 10);
        track_inhibitor(&mut state, root, 20);
        assert!(!inhibitor_is_honored(&state, root));

        map_root(&mut state, root);
        assert!(inhibitor_is_honored(&state, root));

        let workspace_id = state.root_surface_workspace(root).unwrap();
        let group_id = state
            .workspaces
            .try_get(workspace_id)
            .unwrap()
            .group
            .unwrap();
        let hidden = state.create_workspace(Some(group_id), Some("hidden".to_string()));
        assert!(state.assign_root_surface_to_workspace(root, hidden));
        map_root(&mut state, root);
        assert!(!inhibitor_is_honored(&state, root));
    }

    #[test]
    fn a_fullscreen_window_hides_other_workspace_inhibitors() {
        let mut state = state_for_tests();
        let covered = output_and_root(&mut state, 11);
        let fullscreen = output_and_root(&mut state, 12);
        map_root(&mut state, covered);
        map_root(&mut state, fullscreen);
        let workspace_id = state.root_surface_workspace(covered).unwrap();
        state
            .workspaces
            .try_get_mut(workspace_id)
            .unwrap()
            .layout
            .set_fullscreen(Some(fullscreen));

        assert!(!inhibitor_is_honored(&state, covered));
        assert!(inhibitor_is_honored(&state, fullscreen));

        let (_, child_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(13), ClientId::from_raw(1), 6));
        let child = Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(13), child_id);
        state
            .objects
            .get_mut(child_id)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_mark_subsurface_at(covered, (0, 0));
        assert!(!inhibitor_is_honored(&state, child));

        state
            .objects
            .get_mut(child_id)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_mark_subsurface_at(fullscreen, (0, 0));
        assert!(inhibitor_is_honored(&state, child));
    }

    #[test]
    fn a_mapped_layer_inhibits_while_a_fullscreen_window_covers_the_workspace() {
        let mut state = state_for_tests();
        let window = output_and_root(&mut state, 14);
        map_root(&mut state, window);
        let output_id = state.outputs.iter().next().unwrap().0;
        let workspace_id = state.root_surface_workspace(window).unwrap();
        state
            .workspaces
            .try_get_mut(workspace_id)
            .unwrap()
            .layout
            .set_fullscreen(Some(window));

        let (_, layer_root_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(15), ClientId::from_raw(1), 6));
        let layer_root = Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(15), layer_root_id);
        let manager = Resolved::new(
            ClientId::from_raw(1),
            ZwlrLayerShellV1Id::from(1),
            ObjectId::from(1usize),
        );
        let (_, layer_object_id) = state.insert_object(ZwlrLayerSurfaceV1::new(
            ZwlrLayerSurfaceV1Id::from(16),
            manager,
            4,
            layer_root,
            Some(output_id),
            LayerShellLayer::Overlay,
            "bar".to_string(),
        ));
        let layer =
            Resolved::new(ClientId::from_raw(1), ZwlrLayerSurfaceV1Id::from(16), layer_object_id);
        state
            .objects
            .get_mut(layer_root_id)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_layer_surface_impl(layer);
        assert!(!inhibitor_is_honored(&state, layer_root));
        state
            .objects
            .get_mut(layer.object_id())
            .downcast_mut::<ZwlrLayerSurfaceV1>()
            .unwrap()
            .test_set_mapped(true);
        assert!(inhibitor_is_honored(&state, layer_root));
    }

    struct NotifyFixture {
        state: State,
        client: Client,
        dispatcher: Dispatcher,
        seat: Resolved<WlSeatId>,
    }

    fn notify_fixture() -> NotifyFixture {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let (seat, _) = client.insert_object(&mut state, WlSeat::new(WlSeatId::from(3)));
        NotifyFixture { state, client, dispatcher: Dispatcher::default(), seat }
    }

    fn create_notice(
        fixture: &mut NotifyFixture,
        id: u32,
        timeout_msec: u32,
        obey_inhibitors: bool,
        now: Instant,
    ) {
        let mut client_dispatcher = fixture.client.test_dispatcher(&mut fixture.dispatcher);
        ext_idle_notification_v1::create_notification(
            &mut fixture.client,
            &mut fixture.state,
            &mut client_dispatcher,
            crate::protocol_generated::ExtIdleNotificationV1Id::from(id),
            timeout_msec,
            fixture.seat.wayland_id(),
            obey_inhibitors,
            now,
        )
        .unwrap();
    }

    fn notices(fixture: &FixtureClock<'_>) -> Vec<IdleNotice> {
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

    struct FixtureClock<'a> {
        dispatcher: &'a Dispatcher,
    }

    fn clock(fixture: &NotifyFixture) -> FixtureClock<'_> {
        FixtureClock { dispatcher: &fixture.dispatcher }
    }

    #[test]
    fn the_first_honored_inhibitor_resumes_and_the_last_restarts_the_timeout() {
        let mut fixture = notify_fixture();
        let now = Instant::now();
        create_notice(&mut fixture, 7, 5_000, true, now);
        create_notice(&mut fixture, 8, 5_000, false, now);
        let aware_deadline = fixture
            .client
            .resolve(crate::protocol_generated::ExtIdleNotificationV1Id::from(7), &fixture.state)
            .unwrap()
            .rref(&fixture.state)
            .deadline()
            .unwrap();
        ext_idle_notification_v1::evaluate(
            &mut fixture.state,
            &mut fixture.dispatcher,
            aware_deadline,
        );
        assert_eq!(notices(&clock(&fixture)), vec![IdleNotice::Idled, IdleNotice::Idled]);

        let root = output_and_root(&mut fixture.state, 30);
        map_root(&mut fixture.state, root);
        track_inhibitor(&mut fixture.state, root, 40);
        let resumed_at = aware_deadline + Duration::from_secs(3);
        refresh(&mut fixture.state, &mut fixture.dispatcher, resumed_at);
        assert_eq!(
            notices(&clock(&fixture)),
            vec![IdleNotice::Idled, IdleNotice::Idled, IdleNotice::Resumed]
        );
        let aware = fixture
            .client
            .resolve(crate::protocol_generated::ExtIdleNotificationV1Id::from(7), &fixture.state)
            .unwrap()
            .rref(&fixture.state);
        assert!(!aware.is_idle());
        assert!(aware.deadline().is_none());
        let input_idle = fixture
            .client
            .resolve(crate::protocol_generated::ExtIdleNotificationV1Id::from(8), &fixture.state)
            .unwrap()
            .rref(&fixture.state);
        assert!(input_idle.is_idle());
        assert!(input_idle.deadline().is_none());

        map_root(&mut fixture.state, root);
        fixture
            .state
            .managed_root_surfaces
            .get_mut(&root)
            .unwrap()
            .mapped = false;
        let released_at = resumed_at + Duration::from_secs(9);
        refresh(&mut fixture.state, &mut fixture.dispatcher, released_at);
        let aware = fixture
            .client
            .resolve(crate::protocol_generated::ExtIdleNotificationV1Id::from(7), &fixture.state)
            .unwrap()
            .rref(&fixture.state);
        assert_eq!(aware.deadline(), Some(released_at + Duration::from_millis(5_000)));
        assert!(!aware.is_idle());
    }

    #[test]
    fn activity_while_inhibited_does_not_arm_and_a_zero_timeout_waits() {
        let mut fixture = notify_fixture();
        let now = Instant::now();
        let root = output_and_root(&mut fixture.state, 31);
        map_root(&mut fixture.state, root);
        track_inhibitor(&mut fixture.state, root, 41);
        refresh(&mut fixture.state, &mut fixture.dispatcher, now);

        create_notice(&mut fixture, 7, 0, true, now);
        assert!(notices(&clock(&fixture)).is_empty());
        let aware = fixture
            .client
            .resolve(crate::protocol_generated::ExtIdleNotificationV1Id::from(7), &fixture.state)
            .unwrap()
            .rref(&fixture.state);
        assert!(!aware.is_idle());
        assert!(aware.deadline().is_none());

        let during = now + Duration::from_secs(4);
        ext_idle_notification_v1::on_activity(&mut fixture.state, &mut fixture.dispatcher, during);
        let aware = fixture
            .client
            .resolve(crate::protocol_generated::ExtIdleNotificationV1Id::from(7), &fixture.state)
            .unwrap()
            .rref(&fixture.state);
        assert!(aware.deadline().is_none());

        fixture
            .state
            .managed_root_surfaces
            .get_mut(&root)
            .unwrap()
            .mapped = false;
        let released_at = during + Duration::from_secs(2);
        refresh(&mut fixture.state, &mut fixture.dispatcher, released_at);
        assert_eq!(notices(&clock(&fixture)), vec![IdleNotice::Idled]);
    }
}
