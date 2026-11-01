//! Seat-owned pointer confinement.
//!
//! Activation matches pointer lock: the surface has both focuses and the pointer is
//! inside the committed effective region, unless a fullscreen surface can be warped
//! in first. Motion keeps the unclipped relative delta and slides the logical pointer
//! along the region boundary, following wlroots' `wlr_region_confine`. A commit that
//! leaves the pointer outside warps it to the nearest contained point with absolute
//! motion and no relative event. One-shot confinement becomes terminal on release.
//! Persistent confinement can activate again. The constraints global stays unpublished.

use crate::{
    clients::Resolved,
    protocol_generated::{WlSurfaceId, ZwpConfinedPointerV1Id},
    region::RegionValue,
    runner::Dispatcher,
    seat::{dispatch_client_event, SeatClientEvent},
    state::State,
    wayland_objects::{
        pointer_constraint::{ConstraintLifetime, ConstraintStatus, SurfacePointerConstraint},
        zwp_confined_pointer_v1::ZwpConfinedPointerV1,
    },
};

use super::SurfaceFocusPick;

struct ConfineFields {
    id: Resolved<ZwpConfinedPointerV1Id>,
    surface: Resolved<WlSurfaceId>,
    lifetime: ConstraintLifetime,
    region: Option<RegionValue>,
}

pub(crate) fn maybe_activate_pointer_confine(state: &mut State, dispatcher: &mut Dispatcher) {
    if state.seat.active_pointer_lock.is_some() || state.seat.active_pointer_confine.is_some() {
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
    let Some((input_region, confine_id)) = (|| {
        let surface = surface_id.try_ref(state)?;
        if surface.logical_size().is_none() {
            return None;
        }
        let SurfacePointerConstraint::Confine(confine_id) = surface.pointer_constraint()? else {
            return None;
        };
        Some((surface.committed_input_region().clone(), confine_id))
    })() else {
        return;
    };
    let Some(region) = (|| {
        let confine = confine_id.try_ref(state)?;
        if confine.status() != ConstraintStatus::Inactive {
            return None;
        }
        Some(confine.region().cloned())
    })() else {
        return;
    };
    let effective = RegionValue::effective(region.as_ref(), &input_region);
    let local = local_position(pointer_focus, state.seat.pointer_pos);
    if !effective.contains(local.0.floor() as i32, local.1.floor() as i32) {
        if !surface_is_fullscreen(state, surface_id) {
            return;
        }
        let Some(point) = effective.nearest_point(local) else {
            return;
        };
        warp_pointer(state, dispatcher, pointer_focus, point);
    }
    {
        let Some(confine) = confine_id.try_ref_mut(state) else {
            return;
        };
        confine.set_status(ConstraintStatus::Active);
    }
    state.seat.active_pointer_confine = Some(confine_id);
    dispatch_client_event(
        dispatcher,
        pointer_focus.target.client_id(),
        SeatClientEvent::PointerConfineConfined { confine: confine_id },
    );
}

pub(crate) fn recheck_pointer_confinement(state: &mut State, dispatcher: &mut Dispatcher) {
    maybe_activate_pointer_confine(state, dispatcher);
    warp_active_confine_into_region(state, dispatcher);
}

pub(crate) fn confine_pointer_motion(state: &mut State, from: (f64, f64)) {
    let Some(confine_id) = state.seat.active_pointer_confine else {
        return;
    };
    let Some(fields) = confine_fields(state, confine_id) else {
        return;
    };
    let Some(placed) = placed_region(state, &fields) else {
        return;
    };
    let from_local = local_from_origin(placed.origin, from);
    let to_local = local_from_origin(placed.origin, state.seat.pointer_pos);
    let confined = placed.effective.confine_motion(from_local, to_local);
    state.seat.pointer_pos = global_from_origin(placed.origin, confined);
}

pub(crate) fn release_active_pointer_confine(state: &mut State, dispatcher: &mut Dispatcher) {
    release_pointer_confine_unless_surface(state, dispatcher, None);
}

pub(crate) fn release_pointer_confine_unless_surface(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    kept: Option<Resolved<WlSurfaceId>>,
) {
    let Some(id) = state.seat.active_pointer_confine else {
        return;
    };
    let Some(fields) = confine_fields(state, id) else {
        state.seat.active_pointer_confine = None;
        return;
    };
    if kept == Some(fields.surface) {
        return;
    }
    end_confine(state, dispatcher, fields, true);
}

pub(crate) fn release_pointer_confine_for_surface(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    surface: Resolved<WlSurfaceId>,
    send_unconfined: bool,
) {
    let Some(id) = state.seat.active_pointer_confine else {
        return;
    };
    let Some(fields) = confine_fields(state, id) else {
        state.seat.active_pointer_confine = None;
        return;
    };
    if fields.surface != surface {
        return;
    }
    end_confine(state, dispatcher, fields, send_unconfined);
}

pub(crate) fn release_grabbed_pointer_confine(
    confine: &ZwpConfinedPointerV1,
    id: Resolved<ZwpConfinedPointerV1Id>,
    state: &mut State,
    dispatcher: &mut Dispatcher,
    send_unconfined: bool,
) {
    if state.seat.active_pointer_confine != Some(id) {
        return;
    }
    let fields = ConfineFields {
        id,
        surface: confine.surface(),
        lifetime: confine.lifetime(),
        region: confine.region().cloned(),
    };
    end_confine(state, dispatcher, fields, send_unconfined);
}

fn warp_active_confine_into_region(state: &mut State, dispatcher: &mut Dispatcher) {
    let Some(id) = state.seat.active_pointer_confine else {
        return;
    };
    let Some(fields) = confine_fields(state, id) else {
        return;
    };
    let Some(pointer_focus) = state.seat.pointer_focus else {
        return;
    };
    if pointer_focus.target.surface != fields.surface {
        return;
    }
    let Some(placed) = placed_region(state, &fields) else {
        return;
    };
    let local = local_from_origin(placed.origin, state.seat.pointer_pos);
    if placed
        .effective
        .contains(local.0.floor() as i32, local.1.floor() as i32)
    {
        return;
    }
    let Some(point) = placed.effective.nearest_point(local) else {
        end_confine(state, dispatcher, fields, true);
        return;
    };
    warp_pointer(state, dispatcher, pointer_focus, point);
}

fn end_confine(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    fields: ConfineFields,
    send_unconfined: bool,
) {
    if state.seat.active_pointer_confine != Some(fields.id) {
        return;
    }
    state.seat.active_pointer_confine = None;
    if let Some(confine) = fields.id.try_ref_mut(state) {
        let status = match fields.lifetime {
            ConstraintLifetime::Oneshot => ConstraintStatus::Terminal,
            ConstraintLifetime::Persistent => ConstraintStatus::Inactive,
        };
        confine.set_status(status);
    }
    if send_unconfined {
        dispatch_client_event(
            dispatcher,
            fields.surface.client_id(),
            SeatClientEvent::PointerConfineUnconfined { confine: fields.id },
        );
    }
}

struct PlacedRegion {
    origin: (i32, i32),
    effective: RegionValue,
}

fn placed_region(state: &State, fields: &ConfineFields) -> Option<PlacedRegion> {
    let surface = fields.surface.try_ref(state)?;
    let input = surface.committed_input_region().clone();
    let origin = state
        .seat
        .pointer_focus
        .filter(|focus| focus.target.surface == fields.surface)
        .map(|focus| (focus.rect.x, focus.rect.y))
        .unwrap_or_else(|| surface.root_position());
    Some(PlacedRegion {
        origin,
        effective: RegionValue::effective(fields.region.as_ref(), &input),
    })
}

fn confine_fields(state: &State, id: Resolved<ZwpConfinedPointerV1Id>) -> Option<ConfineFields> {
    let confine = id.try_ref(state)?;
    Some(ConfineFields {
        id,
        surface: confine.surface(),
        lifetime: confine.lifetime(),
        region: confine.region().cloned(),
    })
}

fn local_position(focus: SurfaceFocusPick, pointer: (f64, f64)) -> (f64, f64) {
    local_from_origin((focus.rect.x, focus.rect.y), pointer)
}

fn local_from_origin(origin: (i32, i32), pointer: (f64, f64)) -> (f64, f64) {
    (pointer.0 - origin.0 as f64, pointer.1 - origin.1 as f64)
}

fn global_from_origin(origin: (i32, i32), local: (f64, f64)) -> (f64, f64) {
    (origin.0 as f64 + local.0, origin.1 as f64 + local.1)
}

fn warp_pointer(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    focus: SurfaceFocusPick,
    local: (f64, f64),
) {
    state.seat.pointer_pos = global_from_origin((focus.rect.x, focus.rect.y), local);
    crate::seat::clamp_pointer_to_outputs(state);
    dispatch_client_event(
        dispatcher,
        focus.target.client_id(),
        SeatClientEvent::PointerMotion {
            time: monotonic_time_msec(),
            x: local.0 as f32,
            y: local.1 as f32,
        },
    );
    dispatch_client_event(dispatcher, focus.target.client_id(), SeatClientEvent::PointerFrame);
}

fn surface_is_fullscreen(state: &State, surface: Resolved<WlSurfaceId>) -> bool {
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
    use crate::{
        clients::{Client, ClientAction, ClientId, Resolved},
        input::{set_keyboard_focus, SurfaceFocus, SurfaceFocusPick, SurfaceRect},
        protocol_generated::{WlPointerId, WlSurfaceId, ZwpConfinedPointerV1Id},
        region::{RectI, RegionSpec, RegionValue},
        runner::Dispatcher,
        seat::{handle_pointer_motion, quiesce_for_session_pause, SeatClientEvent},
        state::{ObjectId, State},
        test_support::{state_for_tests, test_client_and_dispatcher},
        wayland_objects::{
            pointer_constraint::{ConstraintLifetime, ConstraintStatus, SurfacePointerConstraint},
            wl_pointer::WlPointer,
            wl_surface::WlSurface,
            zwp_confined_pointer_v1::ZwpConfinedPointerV1,
        },
    };

    use super::recheck_pointer_confinement;

    struct ConfineSetup {
        state: State,
        client: Client,
        dispatcher: Dispatcher,
        surface: Resolved<WlSurfaceId>,
        pointer: Resolved<WlPointerId>,
        confine: Resolved<ZwpConfinedPointerV1Id>,
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
        size: (i32, i32),
    ) -> ConfineSetup {
        let mut state = state_for_tests();
        let (mut client, dispatcher) = test_client_and_dispatcher(4);
        let (surface, _) = client.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(4), ClientId::from_raw(4), 6),
        );
        surface.try_ref_mut(&mut state).unwrap().test_map(
            ObjectId::from(41usize),
            0,
            0,
            size.0,
            size.1,
        );
        let (pointer_id, _) =
            client.insert_object(&mut state, WlPointer::new(WlPointerId::from(5), 9));
        let (confine, _) = client.insert_object(
            &mut state,
            ZwpConfinedPointerV1::new(
                ZwpConfinedPointerV1Id::from(9),
                surface,
                pointer_id,
                lifetime,
                region,
            ),
        );
        surface
            .try_ref_mut(&mut state)
            .unwrap()
            .set_pointer_constraint(SurfacePointerConstraint::Confine(confine));
        state.register_root_surface(surface);
        state.seat.pointer_pos = pointer;
        state.seat.pointer_focus = Some(SurfaceFocusPick {
            target: SurfaceFocus::new(surface),
            rect: SurfaceRect { x: 0, y: 0, width: size.0, height: size.1 },
        });
        ConfineSetup {
            state,
            client,
            dispatcher,
            surface,
            pointer: pointer_id,
            confine,
        }
    }

    fn focus_keyboard(setup: &mut ConfineSetup) {
        set_keyboard_focus(
            &mut setup.state,
            &mut setup.dispatcher,
            Some(SurfaceFocus::new(setup.surface)),
        );
    }

    fn confine_events(dispatcher: &Dispatcher, confined: bool) -> usize {
        dispatcher.test_count_queued_action::<ClientAction>(|action| match action {
            ClientAction::SeatEvent { event: SeatClientEvent::PointerConfineConfined { .. } } => {
                confined
            }
            ClientAction::SeatEvent { event: SeatClientEvent::PointerConfineUnconfined { .. } } => {
                !confined
            }
            _ => false,
        })
    }

    fn relative_events(dispatcher: &Dispatcher) -> usize {
        dispatcher.test_count_queued_action::<ClientAction>(|action| {
            matches!(
                action,
                ClientAction::SeatEvent { event: SeatClientEvent::RelativePointerMotion { .. } }
            )
        })
    }

    #[test]
    fn confinement_activates_inside_the_effective_region_only() {
        let mut inside = prepare(ConstraintLifetime::Persistent, None, (4.0, 6.0), (100, 80));
        inside
            .surface
            .try_ref_mut(&mut inside.state)
            .unwrap()
            .test_set_input_region(rect_region(0, 0, 10, 10));
        assert!(inside.state.seat.cursor_is_visible());
        focus_keyboard(&mut inside);
        assert_eq!(inside.state.seat.active_pointer_confine, Some(inside.confine));
        assert_eq!(confine_events(&inside.dispatcher, true), 1);
        assert!(inside.state.seat.cursor_is_visible());

        let mut outside = prepare(ConstraintLifetime::Persistent, None, (40.0, 40.0), (100, 80));
        outside
            .surface
            .try_ref_mut(&mut outside.state)
            .unwrap()
            .test_set_input_region(rect_region(0, 0, 10, 10));
        focus_keyboard(&mut outside);
        assert!(outside.state.seat.active_pointer_confine.is_none());

        let mut empty =
            prepare(ConstraintLifetime::Oneshot, Some(RegionValue::empty()), (4.0, 4.0), (100, 80));
        focus_keyboard(&mut empty);
        assert!(empty.state.seat.active_pointer_confine.is_none());
    }

    #[test]
    fn confined_motion_slides_on_the_boundary_and_keeps_the_relative_delta() {
        let mut setup = prepare(
            ConstraintLifetime::Persistent,
            Some(rect_region(0, 0, 100, 50)),
            (90.0, 40.0),
            (100, 80),
        );
        let (relative, _) = setup.client.insert_object(
            &mut setup.state,
            crate::wayland_objects::zwp_relative_pointer_v1::ZwpRelativePointerV1::new(
                crate::protocol_generated::ZwpRelativePointerV1Id::from(8),
                setup.pointer,
            ),
        );
        setup
            .state
            .seat
            .register_relative_pointer(setup.pointer, relative);
        focus_keyboard(&mut setup);
        handle_pointer_motion(
            &mut setup.state,
            &mut setup.dispatcher,
            30.0,
            -30.0,
            3.0,
            -1.0,
            20,
            20,
        );
        assert_eq!(setup.state.seat.pointer_pos.0, 99.0);
        assert_eq!(setup.state.seat.pointer_pos.1, 10.0);
        assert_eq!(setup.state.seat.active_pointer_confine, Some(setup.confine));
        assert!(setup
            .dispatcher
            .test_has_queued_action_for::<ClientAction, ClientId>(
                ClientId::from_raw(4),
                |action| matches!(
                    action,
                    ClientAction::SeatEvent {
                        event: SeatClientEvent::RelativePointerMotion { dx, dy, .. }
                    } if *dx == 30.0 && *dy == -30.0
                ),
            ));
    }

    #[test]
    fn a_commit_that_excludes_the_pointer_warps_without_relative_motion() {
        let mut setup = prepare(
            ConstraintLifetime::Persistent,
            Some(rect_region(0, 0, 100, 80)),
            (10.0, 10.0),
            (100, 80),
        );
        focus_keyboard(&mut setup);
        setup
            .confine
            .try_ref_mut(&mut setup.state)
            .unwrap()
            .core_mut()
            .apply_region(Some(rect_region(0, 0, 50, 50)));
        recheck_pointer_confinement(&mut setup.state, &mut setup.dispatcher);
        assert_eq!(setup.state.seat.pointer_pos, (10.0, 10.0));
        assert_eq!(relative_events(&setup.dispatcher), 0);
        assert_eq!(
            setup
                .dispatcher
                .test_count_queued_action::<ClientAction>(|action| {
                    matches!(
                        action,
                        ClientAction::SeatEvent { event: SeatClientEvent::PointerMotion { .. } }
                    )
                }),
            0
        );
        setup
            .confine
            .try_ref_mut(&mut setup.state)
            .unwrap()
            .core_mut()
            .apply_region(Some(rect_region(0, 0, 5, 5)));
        recheck_pointer_confinement(&mut setup.state, &mut setup.dispatcher);
        let (x, y) = setup.state.seat.pointer_pos;
        assert!(x < 5.0 && y < 5.0);
        assert!(setup.state.seat.active_pointer_confine.is_some());
        assert_eq!(relative_events(&setup.dispatcher), 0);
        assert!(
            setup
                .dispatcher
                .test_count_queued_action::<ClientAction>(|action| {
                    matches!(
                        action,
                        ClientAction::SeatEvent { event: SeatClientEvent::PointerMotion { .. } }
                    )
                })
                >= 1
        );
    }

    #[test]
    fn oneshot_confinement_ends_on_focus_loss_and_persistent_returns() {
        let mut oneshot = prepare(ConstraintLifetime::Oneshot, None, (10.0, 10.0), (100, 80));
        focus_keyboard(&mut oneshot);
        set_keyboard_focus(&mut oneshot.state, &mut oneshot.dispatcher, None);
        assert!(oneshot.state.seat.active_pointer_confine.is_none());
        assert_eq!(
            oneshot.confine.try_ref(&oneshot.state).unwrap().status(),
            ConstraintStatus::Terminal
        );
        assert_eq!(confine_events(&oneshot.dispatcher, false), 1);
        handle_pointer_motion(
            &mut oneshot.state,
            &mut oneshot.dispatcher,
            15.0,
            0.0,
            15.0,
            0.0,
            30,
            30,
        );
        assert_eq!(oneshot.state.seat.pointer_pos.0, 25.0);

        let mut persistent = prepare(ConstraintLifetime::Persistent, None, (10.0, 10.0), (100, 80));
        focus_keyboard(&mut persistent);
        set_keyboard_focus(&mut persistent.state, &mut persistent.dispatcher, None);
        assert_eq!(
            persistent
                .confine
                .try_ref(&persistent.state)
                .unwrap()
                .status(),
            ConstraintStatus::Inactive
        );
        focus_keyboard(&mut persistent);
        assert_eq!(persistent.state.seat.active_pointer_confine, Some(persistent.confine));
        assert_eq!(confine_events(&persistent.dispatcher, true), 2);
    }

    #[test]
    fn session_pause_unconfines_the_pointer() {
        let mut setup = prepare(ConstraintLifetime::Oneshot, None, (10.0, 10.0), (100, 80));
        focus_keyboard(&mut setup);
        quiesce_for_session_pause(&mut setup.state, &mut setup.dispatcher);
        assert!(setup.state.seat.active_pointer_confine.is_none());
        assert_eq!(confine_events(&setup.dispatcher, false), 1);
        assert_eq!(
            setup.confine.try_ref(&setup.state).unwrap().status(),
            ConstraintStatus::Terminal
        );
    }

    #[test]
    fn output_edge_stops_a_region_that_extends_past_the_output() {
        let mut setup = prepare(
            ConstraintLifetime::Persistent,
            Some(rect_region(0, 0, 200, 80)),
            (90.0, 10.0),
            (200, 80),
        );
        setup
            .state
            .insert_output(crate::test_support::test_output_drm0_wh(100, 80));
        focus_keyboard(&mut setup);
        handle_pointer_motion(
            &mut setup.state,
            &mut setup.dispatcher,
            40.0,
            0.0,
            40.0,
            0.0,
            20,
            20,
        );
        assert_eq!(setup.state.seat.pointer_pos.0, 99.0);

        let mut inner = prepare(
            ConstraintLifetime::Persistent,
            Some(rect_region(0, 0, 50, 80)),
            (40.0, 10.0),
            (200, 80),
        );
        inner
            .state
            .insert_output(crate::test_support::test_output_drm0_wh(100, 80));
        focus_keyboard(&mut inner);
        handle_pointer_motion(
            &mut inner.state,
            &mut inner.dispatcher,
            30.0,
            0.0,
            30.0,
            0.0,
            20,
            20,
        );
        assert_eq!(inner.state.seat.pointer_pos.0, 49.0);
    }
}
