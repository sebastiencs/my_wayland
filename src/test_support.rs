//! Shared helpers for `#[cfg(test)]` modules under `src/`.

use crate::{
    clients::{Client, ClientId, RawEvent, RawRequest, Resolved},
    config::RuntimeConfig,
    drm::ARGB8888,
    egl::EGL,
    globals::Globals,
    input::SurfaceFocus,
    protocol_generated::{WlBufferId, WlSubsurfaceId, WlSurfaceId, XdgSurfaceId, XdgToplevelId},
    runner::Dispatcher,
    seat::SeatState,
    state::{BackendOutputRef, DrmId, FractionalScale, ObjectId, Output, OutputMode, State},
    wayland_objects::{
        wl_buffer::{NewWlBufferParams, WlBuffer},
        wl_shm_pool::shared_memory_from_bytes,
        wl_subsurface::WlSubsurface,
        wl_surface::WlSurface,
    },
    xkb::Xkb,
};

pub fn state_for_tests() -> State {
    state_for_tests_with_config(RuntimeConfig::default())
}

pub fn state_for_tests_with_config(config: RuntimeConfig) -> State {
    let xkb = Xkb::new();
    let keyboard = xkb.create_keyboard_state();
    let mut globals = Globals::new();
    globals.initialize();
    State {
        objects: Default::default(),
        root_surfaces: Default::default(),
        root_surface_process_info: Default::default(),
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
        closing_snapshots: Vec::new(),
        next_closing_snapshot_id: 1,
        realize_closing_snapshots_in_tests: false,
        root_geometry_cache: Default::default(),
        client_process_info: Default::default(),
        xdg_toplevel_roles: Default::default(),
        ctm_control_owner: None,
        data_offers_by_source: Default::default(),
        next_workspace_serial: 1,
        backend_syncobj_globals: Default::default(),
        session_active: true,
        frame_scheduler: Default::default(),
        presentation: Default::default(),
        globals,
        activation_tokens: Default::default(),
        egl: EGL::new(),
        config,
        seat: SeatState::default(),
        keyboard,
        xkb,
    }
}

pub fn resolved_wl_surface(state: &State, object_id: ObjectId) -> Resolved<WlSurfaceId> {
    let surface = state
        .objects
        .try_ref_object::<WlSurface>(object_id)
        .unwrap();
    Resolved::new(surface.client_id(), surface.surface_id(), object_id)
}

/// Root `WlSurface` for client 1, wl object version 6.
pub fn insert_root_surface(state: &mut State, wl_surface_id: u32) -> ObjectId {
    let wl_id = WlSurfaceId::from(wl_surface_id);
    let object_id = state
        .insert_object(WlSurface::new(wl_id, ClientId::from_raw(1), 6))
        .1;
    state.register_root_surface(Resolved::new(ClientId::from_raw(1), wl_id, object_id));
    object_id
}

pub fn insert_wl_subsurface(
    state: &mut State,
    client_id: ClientId,
    id: WlSubsurfaceId,
    surface_object_id: ObjectId,
    parent_object_id: ObjectId,
) -> Resolved<WlSubsurfaceId> {
    let object_id = state
        .insert_object(WlSubsurface::new(
            id,
            resolved_wl_surface(state, surface_object_id),
            resolved_wl_surface(state, parent_object_id),
        ))
        .1;
    Resolved::new(client_id, id, object_id)
}

pub fn insert_wl_subsurface_client1(
    state: &mut State,
    id: WlSubsurfaceId,
    surface_object_id: ObjectId,
    parent_object_id: ObjectId,
) -> Resolved<WlSubsurfaceId> {
    insert_wl_subsurface(state, ClientId::from_raw(1), id, surface_object_id, parent_object_id)
}

/// Synthetic [`Output`] for unit tests (60Hz mode, no gamma/CTM, scale 1).
pub fn test_output(
    backend: BackendOutputRef,
    drm_crtc_id: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    name: impl Into<String>,
) -> Output {
    Output {
        backend,
        drm_crtc_id,
        global_name: None,
        gamma_lut_size: 0,
        ctm_supported: false,
        vrr_capable: false,
        vrr_policy_enabled: false,
        vrr_enabled: false,
        vrr_range: None,
        gamma_control_object_id: None,
        x,
        y,
        physical_width: 0,
        physical_height: 0,
        scale: FractionalScale::ONE,
        transform: 0,
        make: String::new(),
        model: String::new(),
        name: name.into(),
        description: String::new(),
        enabled: true,
        current_mode: OutputMode { width, height, refresh_millihz: 60_000 },
        preferred_mode: None,
    }
}

/// Same as [`test_output`], with display name `output-{drm_crtc_id}`.
pub fn test_output_with_crtc_name(
    backend: BackendOutputRef,
    drm_crtc_id: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Output {
    test_output(backend, drm_crtc_id, x, y, width, height, format!("output-{drm_crtc_id}"))
}

/// Output on DRM backend id **1**, CRTC **1**, name `output-1`.
pub fn test_output_drm1(x: i32, y: i32, width: i32, height: i32) -> Output {
    test_output(BackendOutputRef::new(DrmId::from(1usize)), 1, x, y, width, height, "output-1")
}

/// Output on DRM backend id **1** with a chosen CRTC id; name `output-{drm_crtc_id}`.
pub fn test_output_drm1_crtc(drm_crtc_id: u32, x: i32, y: i32, width: i32, height: i32) -> Output {
    test_output_with_crtc_name(
        BackendOutputRef::new(DrmId::from(1usize)),
        drm_crtc_id,
        x,
        y,
        width,
        height,
    )
}

/// Output on DRM backend id **0**, origin, name `test-output` (common XDG / workspace tests).
pub fn test_output_drm0_wh(width: i32, height: i32) -> Output {
    test_output(BackendOutputRef::new(DrmId::from(0usize)), 1, 0, 0, width, height, "test-output")
}

pub fn test_resolved_wl_surface(
    client_id: ClientId,
    surface_id: u32,
    object_id: ObjectId,
) -> Resolved<WlSurfaceId> {
    Resolved::new(client_id, surface_id.into(), object_id)
}

/// `ClientId::from_raw(1)`, `WlSurfaceId` and [`ObjectId`] all from the same `raw` index.
pub fn test_resolved_wl_surface_client1(raw: usize) -> Resolved<WlSurfaceId> {
    Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(raw as u32), ObjectId::from(raw))
}

pub fn test_client_and_dispatcher(raw: u64) -> (Client, Dispatcher) {
    (Client::new_for_tests(ClientId::from_raw(raw)), Dispatcher::default())
}

/// Minimal ARGB8888 SHM buffer (`stride = 4 * width`).
pub fn insert_test_shm_buffer(
    state: &mut State,
    client_id: ClientId,
    buffer_id: WlBufferId,
    width: i32,
    height: i32,
) -> ObjectId {
    let byte_len = (width.max(1) * height.max(1) * 4) as usize;
    let bytes = vec![0u8; byte_len];
    state
        .insert_object(
            WlBuffer::new(NewWlBufferParams {
                id: buffer_id,
                owner_client_id: client_id,
                offset: 0,
                width,
                height,
                stride: width.saturating_mul(4),
                format: &ARGB8888,
                mem: shared_memory_from_bytes(&bytes),
            })
            .unwrap(),
        )
        .1
}

/// Finalize a [`RawEvent`] built in tests (mirrors `(&*event.into_bytes()).try_into().unwrap()`).
pub fn raw_request_from_event(event: RawEvent) -> RawRequest<'static> {
    (&*event.into_bytes()).try_into().unwrap()
}

/// `xdg_surface.get_toplevel` request bytes for tests.
pub fn get_toplevel_request(
    xdg_surface_id: XdgSurfaceId,
    toplevel_id: XdgToplevelId,
) -> RawRequest<'static> {
    let mut event = RawEvent::new(xdg_surface_id.into(), 1, 1);
    event.id(toplevel_id);
    raw_request_from_event(event)
}

pub fn focus_for_surface(
    client_id: ClientId,
    surface_id: WlSurfaceId,
    object_id: ObjectId,
) -> SurfaceFocus {
    SurfaceFocus::new(Resolved::new(client_id, surface_id, object_id))
}

pub fn fake_focus(client_id: ClientId, surface_id: WlSurfaceId) -> SurfaceFocus {
    focus_for_surface(client_id, surface_id, ObjectId::from(0usize))
}
