//! Seat-owned pointer lock.
//!
//! A lock activates when its surface has both pointer and keyboard focus and the pointer
//! lies in the committed constraint region intersected with the surface input region.
//! Sway activates from keyboard focus alone; this compositor also requires pointer focus
//! and the region. A fullscreen surface may be warped into that region first, and that
//! warp sends `wl_pointer.motion` without a relative event.
//!
//! While locked, the logical pointer stays put and `wl_pointer.motion` is suppressed.
//! Relative motion, buttons, and scroll still reach the focused client. Ending a lock
//! sends `unlocked`. A one-shot lock becomes terminal; a persistent lock returns to
//! inactive and can activate again. A valid committed hint is applied as an absolute
//! warp with no relative event, matching Sway's cursor hint. Weston drops the constraint
//! when the session goes inactive; session pause does the same here.

use crate::{
    clients::Resolved,
    object::PostAction,
    protocol_generated::{WlSurfaceId, ZwpLockedPointerV1Id},
    region::RegionValue,
    runner::Dispatcher,
    seat::{dispatch_client_event, SeatClientEvent},
    state::State,
    wayland_objects::{
        pointer_constraint::{
            point_in_effective_region, sample_effective_point, ConstraintLifetime,
            ConstraintStatus, SurfacePointerConstraint,
        },
        zwp_locked_pointer_v1::ZwpLockedPointerV1,
    },
};

use super::SurfaceFocusPick;

#[derive(Clone, Debug)]
pub(crate) struct PointerLockWarpGeometry {
    pub origin: (i32, i32),
    pub input_region: RegionValue,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PointerLockRelease {
    pub send_unlocked: bool,
    pub apply_lifetime: bool,
    pub warp: bool,
}

const COMPOSITOR_RELEASE: PointerLockRelease =
    PointerLockRelease { send_unlocked: true, apply_lifetime: true, warp: true };

struct LockFields {
    id: Resolved<ZwpLockedPointerV1Id>,
    surface: Resolved<WlSurfaceId>,
    lifetime: ConstraintLifetime,
    region: Option<RegionValue>,
    hint: Option<(f32, f32)>,
}

pub(crate) struct RecheckPointerLock;

impl PostAction for RecheckPointerLock {
    fn execute(
        self: Box<Self>,
        _client: &mut crate::clients::Client,
        state: &mut State,
        dispatcher: &mut crate::clients::ClientDispatcher,
    ) {
        maybe_activate_pointer_lock(state, dispatcher);
        crate::input::pointer_confine::recheck_pointer_confinement(state, dispatcher);
    }
}

pub(crate) fn maybe_activate_pointer_lock(state: &mut State, dispatcher: &mut Dispatcher) {
    if compositor_blocks_pointer_capture(state)
        || state.seat.active_pointer_lock.is_some()
        || state.seat.active_pointer_confine.is_some()
    {
        return;
    }
    let Some(pointer_focus) = state.seat.pointer_focus else {
        return;
    };
    let Some(keyboard_focus) = state.seat.keyboard_focus else {
        return;
    };
    if pointer_focus.target.surface != keyboard_focus.surface {
        return;
    }
    let surface_id = pointer_focus.target.surface;
    let Some((input_region, lock_id)) = (|| {
        let surface = surface_id.try_ref(state)?;
        if surface.logical_size().is_none() {
            return None;
        }
        let SurfacePointerConstraint::Lock(lock_id) = surface.pointer_constraint()? else {
            return None;
        };
        Some((surface.committed_input_region().clone(), lock_id))
    })() else {
        return;
    };
    let Some((region, hint)) = (|| {
        let lock = lock_id.try_ref(state)?;
        if lock.status() != ConstraintStatus::Inactive {
            return None;
        }
        Some((lock.region().cloned(), lock.committed_hint()))
    })() else {
        return;
    };

    let rect = pointer_focus.rect;
    let local_x = (state.seat.pointer_pos.0 - rect.x as f64).floor() as i32;
    let local_y = (state.seat.pointer_pos.1 - rect.y as f64).floor() as i32;
    if !point_in_effective_region(region.as_ref(), &input_region, local_x, local_y) {
        if !lock_surface_is_fullscreen(state, surface_id) {
            return;
        }
        let Some((sample_x, sample_y)) =
            fullscreen_warp_target(region.as_ref(), &input_region, hint)
        else {
            return;
        };
        warp_pointer(state, dispatcher, pointer_focus, sample_x, sample_y);
    }

    {
        let Some(lock) = lock_id.try_ref_mut(state) else {
            return;
        };
        lock.set_status(ConstraintStatus::Active);
    }
    state.seat.active_pointer_lock = Some(lock_id);
    dispatch_client_event(
        dispatcher,
        pointer_focus.target.client_id(),
        SeatClientEvent::PointerLockLocked { lock: lock_id },
    );
}

pub(crate) fn release_active_pointer_lock(state: &mut State, dispatcher: &mut Dispatcher) {
    release_pointer_lock_unless_surface(state, dispatcher, None);
}

/// Drop whichever constraint is active before a compositor action takes the pointer
/// or changes focus. One-shot constraints become terminal. Persistent ones can
/// activate again once the action is finished and both focuses are back inside
/// the effective region.
pub(crate) fn release_pointer_capture(state: &mut State, dispatcher: &mut Dispatcher) {
    release_active_pointer_lock(state, dispatcher);
    crate::input::pointer_confine::release_active_pointer_confine(state, dispatcher);
}

pub(crate) fn recheck_pointer_capture(state: &mut State, dispatcher: &mut Dispatcher) {
    maybe_activate_pointer_lock(state, dispatcher);
    crate::input::pointer_confine::recheck_pointer_confinement(state, dispatcher);
}

pub(crate) fn compositor_blocks_pointer_capture(state: &State) -> bool {
    state.session_lock.input_gated()
        || state.seat.pointer_interactive_grab.is_some()
        || state.seat.popup_grab_client().is_some()
        || state.seat.active_drag().is_some()
}

pub(crate) fn release_pointer_lock_unless_surface(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    kept: Option<Resolved<WlSurfaceId>>,
) {
    let Some(fields) = active_lock_fields(state) else {
        return;
    };
    if kept == Some(fields.surface) {
        return;
    }
    end_lock(state, dispatcher, fields, COMPOSITOR_RELEASE, None);
}

pub(crate) fn release_pointer_lock_for_surface(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    surface: Resolved<WlSurfaceId>,
    geometry: Option<PointerLockWarpGeometry>,
    release: PointerLockRelease,
) {
    let Some(fields) = active_lock_fields(state) else {
        return;
    };
    if fields.surface != surface {
        return;
    }
    end_lock(state, dispatcher, fields, release, geometry);
}

pub(crate) fn release_grabbed_pointer_lock(
    lock: &ZwpLockedPointerV1,
    id: Resolved<ZwpLockedPointerV1Id>,
    state: &mut State,
    dispatcher: &mut Dispatcher,
    release: PointerLockRelease,
) {
    if state.seat.active_pointer_lock != Some(id) {
        return;
    }
    let fields = LockFields {
        id,
        surface: lock.surface(),
        lifetime: lock.lifetime(),
        region: lock.region().cloned(),
        hint: lock.committed_hint(),
    };
    end_lock(state, dispatcher, fields, release, None);
}

fn active_lock_fields(state: &State) -> Option<LockFields> {
    let id = state.seat.active_pointer_lock?;
    let lock = id.try_ref(state)?;
    Some(LockFields {
        id,
        surface: lock.surface(),
        lifetime: lock.lifetime(),
        region: lock.region().cloned(),
        hint: lock.committed_hint(),
    })
}

fn end_lock(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    fields: LockFields,
    release: PointerLockRelease,
    geometry: Option<PointerLockWarpGeometry>,
) {
    if state.seat.active_pointer_lock != Some(fields.id) {
        return;
    }
    state.seat.active_pointer_lock = None;
    if release.apply_lifetime {
        if let Some(lock) = fields.id.try_ref_mut(state) {
            let status = match fields.lifetime {
                ConstraintLifetime::Oneshot => ConstraintStatus::Terminal,
                ConstraintLifetime::Persistent => ConstraintStatus::Inactive,
            };
            lock.set_status(status);
        }
    }
    if release.send_unlocked {
        dispatch_client_event(
            dispatcher,
            fields.surface.client_id(),
            SeatClientEvent::PointerLockUnlocked { lock: fields.id },
        );
    }
    if release.warp {
        warp_to_committed_hint(state, dispatcher, &fields, geometry);
    }
}

fn warp_to_committed_hint(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    fields: &LockFields,
    geometry: Option<PointerLockWarpGeometry>,
) {
    let Some((hint_x, hint_y)) = fields.hint else {
        return;
    };
    let geometry = geometry.or_else(|| warp_geometry_from_surface(state, fields.surface));
    let Some(geometry) = geometry else {
        return;
    };
    let local_x = hint_x.floor() as i32;
    let local_y = hint_y.floor() as i32;
    if !point_in_effective_region(fields.region.as_ref(), &geometry.input_region, local_x, local_y)
    {
        return;
    }
    let focus = state
        .seat
        .pointer_focus
        .filter(|focus| focus.target.surface == fields.surface);
    let origin = focus
        .map(|focus| (focus.rect.x, focus.rect.y))
        .unwrap_or(geometry.origin);
    let pointer_focus = focus.unwrap_or(SurfaceFocusPick {
        target: crate::input::SurfaceFocus::new(fields.surface),
        rect: super::SurfaceRect { x: origin.0, y: origin.1, width: 0, height: 0 },
    });
    warp_pointer(state, dispatcher, pointer_focus, hint_x as f64, hint_y as f64);
}

fn warp_geometry_from_surface(
    state: &State,
    surface: Resolved<WlSurfaceId>,
) -> Option<PointerLockWarpGeometry> {
    let surface = surface.try_ref(state)?;
    Some(PointerLockWarpGeometry {
        origin: surface.root_position(),
        input_region: surface.committed_input_region().clone(),
    })
}

fn warp_pointer(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    focus: SurfaceFocusPick,
    local_x: f64,
    local_y: f64,
) {
    state.seat.pointer_pos = (focus.rect.x as f64 + local_x, focus.rect.y as f64 + local_y);
    crate::seat::clamp_pointer_to_outputs(state);
    let (x, y) = (local_x as f32, local_y as f32);
    dispatch_client_event(
        dispatcher,
        focus.target.client_id(),
        SeatClientEvent::PointerWarp { time: monotonic_time_msec(), x, y },
    );
    dispatch_client_event(dispatcher, focus.target.client_id(), SeatClientEvent::PointerFrame);
}

fn fullscreen_warp_target(
    region: Option<&RegionValue>,
    input_region: &RegionValue,
    hint: Option<(f32, f32)>,
) -> Option<(f64, f64)> {
    if let Some((hint_x, hint_y)) = hint {
        let local_x = hint_x.floor() as i32;
        let local_y = hint_y.floor() as i32;
        if point_in_effective_region(region, input_region, local_x, local_y) {
            return Some((hint_x as f64, hint_y as f64));
        }
    }
    sample_effective_point(region, input_region).map(|(x, y)| (x as f64, y as f64))
}

fn lock_surface_is_fullscreen(state: &State, surface: Resolved<WlSurfaceId>) -> bool {
    let root = state.root_surface_for_surface(surface).unwrap_or(surface);
    state.root_surface_is_fullscreen(root)
        || state
            .managed_root_surface_state(root)
            .is_some_and(|managed| managed.fullscreen)
}

fn monotonic_time_msec() -> u32 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
    }
    (ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000) as u32
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use crate::{
        clients::{Arguments, Client, ClientAction, ClientId, RawRequest, Resolved},
        input::{set_keyboard_focus, SurfaceFocus, SurfaceFocusPick, SurfaceRect},
        object::Object,
        protocol_generated::{WlPointerId, WlSurfaceId, ZwpLockedPointerV1Id},
        region::{RectI, RegionSpec, RegionValue},
        runner::Dispatcher,
        seat::{
            handle_pointer_button, handle_pointer_motion, handle_pointer_motion_absolute,
            handle_pointer_scroll, quiesce_for_session_pause, PointerAxisChange,
            PointerScrollFrame, PointerScrollSource, SeatClientEvent,
        },
        state::{ManagedRootSurfaceState, ManagedWindowMode, ObjectId, State, WorkspaceId},
        test_support::{state_for_tests, test_client_and_dispatcher},
        visual_animation::{
            FocusVisualAnimation, WindowGeometryAnimation, WindowLifecycleAnimation,
            WindowVisualBase,
        },
        wayland_objects::{
            pointer_constraint::{ConstraintLifetime, ConstraintStatus, SurfacePointerConstraint},
            wl_pointer::WlPointer,
            wl_surface::WlSurface,
            zwp_locked_pointer_v1::ZwpLockedPointerV1,
        },
    };

    use super::maybe_activate_pointer_lock;

    struct LockSetup {
        state: State,
        client: Client,
        dispatcher: Dispatcher,
        surface: Resolved<WlSurfaceId>,
        lock: Resolved<ZwpLockedPointerV1Id>,
    }

    fn rect_region(x: i32, y: i32, width: i32, height: i32) -> RegionValue {
        let mut spec = RegionSpec::default();
        spec.add_rect(RectI::new(x, y, width, height).unwrap());
        RegionValue::from_spec(spec)
    }

    fn prepare(
        lifetime: ConstraintLifetime,
        region: Option<RegionValue>,
        pointer: (f64, f64),
    ) -> LockSetup {
        let mut state = state_for_tests();
        let (mut client, dispatcher) = test_client_and_dispatcher(3);
        let (surface, _) = client.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(4), ClientId::from_raw(3), 6),
        );
        surface
            .try_ref_mut(&mut state)
            .unwrap()
            .test_map(ObjectId::from(40usize), 0, 0, 100, 80);
        let (pointer_id, _) =
            client.insert_object(&mut state, WlPointer::new(WlPointerId::from(5), 9));
        let (lock, _) = client.insert_object(
            &mut state,
            ZwpLockedPointerV1::new(
                ZwpLockedPointerV1Id::from(9),
                surface,
                pointer_id,
                lifetime,
                region,
            ),
        );
        surface
            .try_ref_mut(&mut state)
            .unwrap()
            .set_pointer_constraint(SurfacePointerConstraint::Lock(lock));
        state.register_root_surface(surface);
        state.seat.pointer_pos = pointer;
        state.seat.pointer_focus = Some(SurfaceFocusPick {
            target: SurfaceFocus::new(surface),
            rect: SurfaceRect { x: 0, y: 0, width: 100, height: 80 },
        });
        LockSetup { state, client, dispatcher, surface, lock }
    }

    fn focus_keyboard(setup: &mut LockSetup) {
        set_keyboard_focus(
            &mut setup.state,
            &mut setup.dispatcher,
            Some(SurfaceFocus::new(setup.surface)),
        );
    }

    fn lock_events(dispatcher: &Dispatcher, locked: bool) -> usize {
        dispatcher.test_count_queued_action::<ClientAction>(|action| match action {
            ClientAction::SeatEvent { event: SeatClientEvent::PointerLockLocked { .. } } => locked,
            ClientAction::SeatEvent { event: SeatClientEvent::PointerLockUnlocked { .. } } => {
                !locked
            }
            _ => false,
        })
    }

    #[test]
    fn lock_activates_only_with_both_focuses_inside_the_effective_region() {
        let mut setup = prepare(ConstraintLifetime::Persistent, None, (4.0, 6.0));
        setup
            .surface
            .try_ref_mut(&mut setup.state)
            .unwrap()
            .test_set_input_region(rect_region(0, 0, 10, 10));
        assert!(setup.state.seat.cursor_is_visible());
        maybe_activate_pointer_lock(&mut setup.state, &mut setup.dispatcher);
        assert!(setup.state.seat.active_pointer_lock.is_none());

        focus_keyboard(&mut setup);
        assert_eq!(setup.state.seat.active_pointer_lock, Some(setup.lock));
        assert_eq!(setup.lock.try_ref(&setup.state).unwrap().status(), ConstraintStatus::Active);
        assert_eq!(lock_events(&setup.dispatcher, true), 1);
        assert!(setup.state.seat.cursor_is_visible());

        let mut outside = prepare(ConstraintLifetime::Persistent, None, (40.0, 40.0));
        outside
            .surface
            .try_ref_mut(&mut outside.state)
            .unwrap()
            .test_set_input_region(rect_region(0, 0, 10, 10));
        focus_keyboard(&mut outside);
        assert!(outside.state.seat.active_pointer_lock.is_none());
        assert_eq!(lock_events(&outside.dispatcher, true), 0);

        let mut keyboard_only = prepare(ConstraintLifetime::Oneshot, None, (4.0, 4.0));
        keyboard_only.state.seat.pointer_focus = None;
        focus_keyboard(&mut keyboard_only);
        assert!(keyboard_only.state.seat.active_pointer_lock.is_none());
    }

    #[test]
    fn fullscreen_lock_warps_into_the_region_then_activates() {
        let mut setup =
            prepare(ConstraintLifetime::Persistent, Some(rect_region(10, 10, 20, 20)), (1.0, 1.0));
        focus_keyboard(&mut setup);
        assert!(setup.state.seat.active_pointer_lock.is_none());
        assert_eq!(setup.state.seat.pointer_pos, (1.0, 1.0));

        setup.state.managed_root_surfaces.insert(
            setup.surface,
            ManagedRootSurfaceState {
                workspace_id: WorkspaceId::from_parts(1, 1),
                mode: ManagedWindowMode::Floating,
                fullscreen: true,
                mapped: true,
                awaiting_initial_layout: false,
                remembered_floating_state: None,
                restore_floating_state_on_next_configure: false,
                focus_animation: FocusVisualAnimation::settled(WindowVisualBase {
                    content_alpha: 255,
                    dim_alpha: 0,
                    shadow_alpha: 0,
                    border_alpha: 0,
                }),
                lifecycle_animation: WindowLifecycleAnimation::idle(),
                geometry_animation: WindowGeometryAnimation::unarmed(),
            },
        );
        maybe_activate_pointer_lock(&mut setup.state, &mut setup.dispatcher);
        assert_eq!(setup.state.seat.pointer_pos, (20.0, 20.0));
        assert_eq!(setup.state.seat.active_pointer_lock, Some(setup.lock));
        assert_eq!(lock_events(&setup.dispatcher, true), 1);
        assert!(setup
            .dispatcher
            .test_has_queued_action_for::<ClientAction, ClientId>(
                ClientId::from_raw(3),
                |action| matches!(
                    action,
                    ClientAction::SeatEvent { event: SeatClientEvent::PointerWarp { x, y, .. } }
                        if *x == 20.0 && *y == 20.0
                ),
            ));
        assert_eq!(
            setup
                .dispatcher
                .test_count_queued_action::<ClientAction>(|action| {
                    matches!(
                        action,
                        ClientAction::SeatEvent {
                            event: SeatClientEvent::RelativePointerMotion { .. }
                        }
                    )
                }),
            0
        );
    }

    #[test]
    fn locked_pointer_keeps_relative_motion_buttons_and_scroll() {
        let mut setup = prepare(ConstraintLifetime::Oneshot, None, (10.0, 12.0));
        let pointer = setup.lock.try_ref(&setup.state).unwrap().pointer();
        let (relative, _) = setup.client.insert_object(
            &mut setup.state,
            crate::wayland_objects::zwp_relative_pointer_v1::ZwpRelativePointerV1::new(
                crate::protocol_generated::ZwpRelativePointerV1Id::from(8),
                pointer,
            ),
        );
        setup
            .state
            .seat
            .register_relative_pointer(pointer, relative);
        focus_keyboard(&mut setup);
        let motion_before = setup
            .dispatcher
            .test_count_queued_action::<ClientAction>(|action| {
                matches!(
                    action,
                    ClientAction::SeatEvent { event: SeatClientEvent::PointerMotion { .. } }
                )
            });

        handle_pointer_motion(
            &mut setup.state,
            &mut setup.dispatcher,
            30.0,
            -4.0,
            3.0,
            -1.0,
            20,
            20,
        );
        assert_eq!(setup.state.seat.pointer_pos, (10.0, 12.0));
        assert!(setup
            .dispatcher
            .test_has_queued_action_for::<ClientAction, ClientId>(
                ClientId::from_raw(3),
                |action| matches!(
                    action,
                    ClientAction::SeatEvent {
                        event: SeatClientEvent::RelativePointerMotion { dx, dy, .. }
                    } if *dx == 30.0 && *dy == -4.0
                ),
            ));
        assert_eq!(
            setup
                .dispatcher
                .test_count_queued_action::<ClientAction>(|action| {
                    matches!(
                        action,
                        ClientAction::SeatEvent { event: SeatClientEvent::PointerMotion { .. } }
                    )
                }),
            motion_before
        );

        handle_pointer_button(&mut setup.state, &mut setup.dispatcher, 0x111, 1, 21);
        assert!(setup
            .dispatcher
            .test_has_queued_action_for::<ClientAction, ClientId>(
                ClientId::from_raw(3),
                |action| matches!(
                    action,
                    ClientAction::SeatEvent {
                        event: SeatClientEvent::PointerButton { button: 0x111, state: 1, .. }
                    }
                ),
            ));
        handle_pointer_scroll(
            &mut setup.state,
            &mut setup.dispatcher,
            PointerScrollFrame {
                time: 22,
                source: PointerScrollSource::Wheel,
                vertical: Some(PointerAxisChange::Scroll {
                    value: 1.0,
                    discrete: None,
                    value120: None,
                    relative_direction: None,
                }),
                horizontal: None,
            },
        );
        assert!(setup
            .dispatcher
            .test_has_queued_action_for::<ClientAction, ClientId>(
                ClientId::from_raw(3),
                |action| matches!(
                    action,
                    ClientAction::SeatEvent { event: SeatClientEvent::PointerScroll(_) }
                ),
            ));

        handle_pointer_motion_absolute(&mut setup.state, &mut setup.dispatcher, 80.0, 70.0, 23);
        assert_eq!(setup.state.seat.pointer_pos, (10.0, 12.0));
        assert!(setup.state.seat.cursor_is_visible());
    }

    #[test]
    fn each_locked_motion_sends_one_relative_event_and_no_absolute_motion() {
        let mut setup = prepare(ConstraintLifetime::Oneshot, None, (10.0, 12.0));
        let pointer = setup.lock.try_ref(&setup.state).unwrap().pointer();
        let (relative, _) = setup.client.insert_object(
            &mut setup.state,
            crate::wayland_objects::zwp_relative_pointer_v1::ZwpRelativePointerV1::new(
                crate::protocol_generated::ZwpRelativePointerV1Id::from(8),
                pointer,
            ),
        );
        setup
            .state
            .seat
            .register_relative_pointer(pointer, relative);
        focus_keyboard(&mut setup);
        let absolute_before = setup
            .dispatcher
            .test_count_queued_action::<ClientAction>(|action| {
                matches!(
                    action,
                    ClientAction::SeatEvent { event: SeatClientEvent::PointerMotion { .. } }
                )
            });

        handle_pointer_motion(&mut setup.state, &mut setup.dispatcher, 4.0, 1.0, 2.0, 1.0, 20, 20);
        handle_pointer_motion(
            &mut setup.state,
            &mut setup.dispatcher,
            5.0,
            -2.0,
            5.0,
            -2.0,
            21,
            21,
        );

        assert_eq!(setup.state.seat.pointer_pos, (10.0, 12.0));
        assert_eq!(
            setup
                .dispatcher
                .test_count_queued_action::<ClientAction>(|action| {
                    matches!(
                        action,
                        ClientAction::SeatEvent {
                            event: SeatClientEvent::RelativePointerMotion { .. }
                        }
                    )
                }),
            2
        );
        assert_eq!(
            setup
                .dispatcher
                .test_count_queued_action::<ClientAction>(|action| {
                    matches!(
                        action,
                        ClientAction::SeatEvent { event: SeatClientEvent::PointerMotion { .. } }
                    )
                }),
            absolute_before
        );
        assert_eq!(lock_events(&setup.dispatcher, true), 1);
    }

    #[test]
    fn unlock_warps_to_a_valid_hint_and_oneshot_stays_terminal() {
        let mut setup = prepare(ConstraintLifetime::Oneshot, None, (10.0, 12.0));
        setup
            .lock
            .try_ref_mut(&mut setup.state)
            .unwrap()
            .core_mut()
            .apply_hint((12.5, 8.0));
        focus_keyboard(&mut setup);
        set_keyboard_focus(&mut setup.state, &mut setup.dispatcher, None);
        assert!(setup.state.seat.active_pointer_lock.is_none());
        assert_eq!(setup.state.seat.pointer_pos, (12.5, 8.0));
        assert_eq!(lock_events(&setup.dispatcher, false), 1);
        assert_eq!(setup.lock.try_ref(&setup.state).unwrap().status(), ConstraintStatus::Terminal);
        assert_eq!(
            setup
                .dispatcher
                .test_count_queued_action::<ClientAction>(|action| {
                    matches!(
                        action,
                        ClientAction::SeatEvent {
                            event: SeatClientEvent::RelativePointerMotion { .. }
                        }
                    )
                }),
            0
        );

        set_keyboard_focus(
            &mut setup.state,
            &mut setup.dispatcher,
            Some(SurfaceFocus::new(setup.surface)),
        );
        assert!(setup.state.seat.active_pointer_lock.is_none());
        assert_eq!(setup.lock.try_ref(&setup.state).unwrap().status(), ConstraintStatus::Terminal);
    }

    #[test]
    fn an_invalid_hint_does_not_move_the_pointer() {
        let mut setup =
            prepare(ConstraintLifetime::Persistent, Some(rect_region(0, 0, 5, 5)), (1.0, 1.0));
        setup
            .lock
            .try_ref_mut(&mut setup.state)
            .unwrap()
            .core_mut()
            .apply_hint((50.0, 50.0));
        focus_keyboard(&mut setup);
        set_keyboard_focus(&mut setup.state, &mut setup.dispatcher, None);
        assert_eq!(setup.state.seat.pointer_pos, (1.0, 1.0));
        assert_eq!(setup.lock.try_ref(&setup.state).unwrap().status(), ConstraintStatus::Inactive);
    }

    #[test]
    fn persistent_lock_reactivates_after_keyboard_focus_returns() {
        let mut setup = prepare(ConstraintLifetime::Persistent, None, (10.0, 10.0));
        focus_keyboard(&mut setup);
        set_keyboard_focus(&mut setup.state, &mut setup.dispatcher, None);
        assert_eq!(setup.lock.try_ref(&setup.state).unwrap().status(), ConstraintStatus::Inactive);
        focus_keyboard(&mut setup);
        assert_eq!(setup.state.seat.active_pointer_lock, Some(setup.lock));
        assert_eq!(lock_events(&setup.dispatcher, true), 2);
        assert_eq!(setup.lock.try_ref(&setup.state).unwrap().status(), ConstraintStatus::Active);
    }

    #[test]
    fn session_pause_unlocks_and_later_motion_moves_the_pointer() {
        let mut setup = prepare(ConstraintLifetime::Oneshot, None, (10.0, 10.0));
        focus_keyboard(&mut setup);
        setup.state.seat.clear_cursor();
        quiesce_for_session_pause(&mut setup.state, &mut setup.dispatcher);
        assert!(setup.state.seat.cursor_is_visible());
        assert!(setup.state.seat.active_pointer_lock.is_none());
        assert_eq!(lock_events(&setup.dispatcher, false), 1);
        assert_eq!(setup.lock.try_ref(&setup.state).unwrap().status(), ConstraintStatus::Terminal);
        handle_pointer_motion(&mut setup.state, &mut setup.dispatcher, 5.0, 0.0, 5.0, 0.0, 30, 30);
        assert_eq!(setup.state.seat.pointer_pos.0, 15.0);
    }

    #[test]
    fn persistent_lock_returns_when_focus_is_restored_after_session_pause() {
        let mut setup = prepare(ConstraintLifetime::Persistent, None, (10.0, 10.0));
        focus_keyboard(&mut setup);
        quiesce_for_session_pause(&mut setup.state, &mut setup.dispatcher);
        assert!(setup.state.seat.active_pointer_lock.is_none());
        assert_eq!(setup.lock.try_ref(&setup.state).unwrap().status(), ConstraintStatus::Inactive);
        assert_eq!(lock_events(&setup.dispatcher, false), 1);

        setup.state.seat.pointer_focus = Some(SurfaceFocusPick {
            target: SurfaceFocus::new(setup.surface),
            rect: SurfaceRect { x: 0, y: 0, width: 100, height: 80 },
        });
        focus_keyboard(&mut setup);
        assert_eq!(setup.state.seat.active_pointer_lock, Some(setup.lock));
        assert_eq!(lock_events(&setup.dispatcher, true), 2);
    }

    #[test]
    fn a_workspace_switch_with_nowhere_to_go_keeps_the_lock() {
        let mut setup = prepare(ConstraintLifetime::Oneshot, None, (10.0, 10.0));
        focus_keyboard(&mut setup);
        crate::config::execute_action(
            &mut setup.state,
            &mut setup.dispatcher,
            crate::config::CompositorAction::WorkspaceNext,
        );
        assert_eq!(setup.state.seat.active_pointer_lock, Some(setup.lock));
        assert_eq!(lock_events(&setup.dispatcher, true), 1);
        assert_eq!(lock_events(&setup.dispatcher, false), 0);
    }

    #[test]
    fn destroying_the_surface_sends_unlocked_and_unfreezes_the_pointer() {
        let mut setup = prepare(ConstraintLifetime::Persistent, None, (10.0, 10.0));
        setup
            .lock
            .try_ref_mut(&mut setup.state)
            .unwrap()
            .core_mut()
            .apply_hint((4.0, 4.0));
        focus_keyboard(&mut setup);
        {
            let mut client_dispatcher = setup.client.test_dispatcher(&mut setup.dispatcher);
            {
                let mut surface = setup
                    .state
                    .objects
                    .grab_object::<WlSurface>(setup.surface.object_id());
                surface.delete_self(&mut setup.client, &mut setup.state, &mut client_dispatcher);
            }
            client_dispatcher.drain_post_actions(&mut setup.client, &mut setup.state);
        }
        assert!(setup.state.seat.active_pointer_lock.is_none());
        assert!(setup.state.seat.pointer_focus.is_none());
        assert!(setup.state.seat.keyboard_focus().is_none());
        assert_eq!(lock_events(&setup.dispatcher, false), 1);
        assert_eq!(setup.state.seat.pointer_pos, (4.0, 4.0));
        handle_pointer_motion(&mut setup.state, &mut setup.dispatcher, 2.0, 0.0, 2.0, 0.0, 30, 30);
        assert_eq!(setup.state.seat.pointer_pos.0, 6.0);
    }

    #[test]
    fn client_disconnect_clears_the_lock_without_an_unlocked_event() {
        let mut setup = prepare(ConstraintLifetime::Persistent, None, (10.0, 10.0));
        focus_keyboard(&mut setup);
        {
            let mut client_dispatcher = setup.client.test_dispatcher(&mut setup.dispatcher);
            let mut lock = setup
                .state
                .objects
                .grab_object::<ZwpLockedPointerV1>(setup.lock.object_id());
            lock.on_client_disconnect(&mut setup.client, &mut setup.state, &mut client_dispatcher);
        }
        assert!(setup.state.seat.active_pointer_lock.is_none());
        assert_eq!(lock_events(&setup.dispatcher, false), 0);
        assert_eq!(setup.lock.try_ref(&setup.state).unwrap().status(), ConstraintStatus::Terminal);
        handle_pointer_motion(&mut setup.state, &mut setup.dispatcher, 3.0, 0.0, 3.0, 0.0, 30, 30);
        assert_eq!(setup.state.seat.pointer_pos.0, 13.0);
    }

    #[test]
    fn destroying_the_lock_object_warps_without_an_unlocked_event() {
        let mut setup = prepare(ConstraintLifetime::Persistent, None, (10.0, 10.0));
        setup
            .lock
            .try_ref_mut(&mut setup.state)
            .unwrap()
            .core_mut()
            .apply_hint((6.0, 7.0));
        focus_keyboard(&mut setup);
        let request = RawRequest {
            object_id: 9,
            request_id: 0,
            arguments: Arguments::from_words(VecDeque::new()),
            fds: None,
        };
        {
            let mut client_dispatcher = setup.client.test_dispatcher(&mut setup.dispatcher);
            client_dispatcher.set_requester_object_id_for_tests(setup.lock.wayland_id().into());
            setup
                .state
                .objects
                .grab(setup.lock.object_id())
                .on(&mut setup.client, &mut setup.state, request, &mut client_dispatcher)
                .unwrap();
        }
        assert!(setup.state.seat.active_pointer_lock.is_none());
        assert_eq!(setup.state.seat.pointer_pos, (6.0, 7.0));
        assert_eq!(lock_events(&setup.dispatcher, false), 0);
    }

    fn move_target(surface: Resolved<WlSurfaceId>) -> crate::input::WindowTarget {
        crate::input::WindowTarget {
            root_surface: surface,
            role: crate::input::WindowRoleSnapshot::Unsupported,
            geometry: crate::root_geometry::WindowTargetGeometry {
                root_pos: (0, 0),
                resize_size: (100, 80),
                target_rect: RectI::new(0, 0, 100, 80).unwrap(),
                binding_rect: RectI::new(0, 0, 100, 80).unwrap(),
            },
        }
    }

    #[test]
    fn a_move_grab_unlocks_before_it_takes_the_pointer() {
        let mut setup = prepare(ConstraintLifetime::Oneshot, None, (10.0, 10.0));
        focus_keyboard(&mut setup);
        assert!(crate::input::apply_interaction_intent(
            &mut setup.state,
            &mut setup.dispatcher,
            crate::input::InteractionIntent::StartMove(move_target(setup.surface)),
            crate::input::PROTOCOL_INTERACTION_START_OPTIONS,
        ));
        assert!(setup.state.seat.active_pointer_lock.is_none());
        assert_eq!(setup.lock.try_ref(&setup.state).unwrap().status(), ConstraintStatus::Terminal);
        assert_eq!(lock_events(&setup.dispatcher, false), 1);
        handle_pointer_motion(&mut setup.state, &mut setup.dispatcher, 8.0, 0.0, 8.0, 0.0, 40, 40);
        assert_eq!(setup.state.seat.pointer_pos.0, 18.0);
        crate::input::finish_pointer_interactive_grab(&mut setup.state, &mut setup.dispatcher, 50);
        handle_pointer_motion(&mut setup.state, &mut setup.dispatcher, 4.0, 0.0, 4.0, 0.0, 60, 60);
        assert_eq!(setup.state.seat.pointer_pos.0, 22.0);
        assert!(setup.state.seat.active_pointer_lock.is_none());
    }

    #[test]
    fn a_popup_grab_unlocks_and_a_persistent_lock_returns_when_it_ends() {
        let mut setup = prepare(ConstraintLifetime::Persistent, None, (10.0, 10.0));
        focus_keyboard(&mut setup);
        let popup = Resolved::new(
            ClientId::from_raw(3),
            crate::protocol_generated::XdgPopupId::from(40),
            ObjectId::from(90usize),
        );
        crate::seat::begin_popup_grab(
            &mut setup.state,
            &mut setup.dispatcher,
            ClientId::from_raw(3),
            popup,
            setup.surface,
        );
        assert!(setup.state.seat.active_pointer_lock.is_none());
        assert_eq!(setup.lock.try_ref(&setup.state).unwrap().status(), ConstraintStatus::Inactive);
        maybe_activate_pointer_lock(&mut setup.state, &mut setup.dispatcher);
        assert!(setup.state.seat.active_pointer_lock.is_none());
        crate::seat::remove_popup_grab(
            &mut setup.state,
            &mut setup.dispatcher,
            popup,
            Some(SurfaceFocus::new(setup.surface)),
        );
        assert_eq!(setup.state.seat.active_pointer_lock, Some(setup.lock));
        assert_eq!(lock_events(&setup.dispatcher, true), 2);
    }
}
