use std::collections::BTreeSet;

use crate::{
    clients::{Client, ClientDispatcher, ClientId, ClientsAction, EventWriter, RawRequest},
    drm::{schedule_active_output, schedule_outputs, schedule_outputs_for_rect},
    object::{Object, Parser},
    protocol_generated::{
        wl_pointer, wl_surface, wp_fractional_scale_v1, wp_linux_drm_syncobj_surface_v1,
        wp_viewport, xdg_wm_base, WaylandId, WlBufferId, WlCallbackId, WlOutputId, WlRegionId,
        WlSubsurfaceId, WlSurfaceId, WpPresentationFeedbackId, XdgSurfaceId, ZwlrLayerSurfaceV1Id,
    },
    region::{DamageRegion, RectI, RegionValue},
    state::{FractionalScale, ObjectId, OutputId, State},
    sync::{self, AcquireReadyWaiter, RetiredBufferCandidate, SurfaceSyncState},
    transform::{self, BufferTransform, SizeI, SurfaceViewport},
    wayland_objects::{
        wl_callback::WlCallback,
        wl_region::WlRegion,
        wp_fractional_scale_v1::WpFractionalScaleV1,
        xdg_surface::XdgSurface,
        zwlr_layer_surface_v1::{
            output_usable_bounds, output_usable_bounds_with_snapshot, LayerLayoutObjectSnapshot,
            ZwlrLayerSurfaceV1,
        },
    },
    ENABLE_DAMAGE,
};

use {super::wl_buffer::WlBuffer, crate::protocol_generated::wl_display};

fn monotonic_time_msec() -> u32 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
        return 0;
    }
    let millis = (ts.tv_sec as u64)
        .saturating_mul(1000)
        .saturating_add((ts.tv_nsec as u64) / 1_000_000);
    millis.min(u64::from(u32::MAX)) as u32
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfacePresentationFeedback {
    pub id: WpPresentationFeedbackId,
    pub version: u32,
    pub output_id: Option<OutputId>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Role {
    None,
    Cursor,
    DndIcon,
    XdgSurface,
    LayerSurface,
    Subsurface,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShellSurfaceObject {
    XdgSurface(XdgSurfaceId),
    LayerSurface(ZwlrLayerSurfaceV1Id),
}

#[derive(Clone, Debug)]
struct CachedCommit {
    payload: SurfaceCommitPayload,
    frame_callbacks: Vec<WlCallbackId>,
    presentation_feedbacks: Vec<SurfacePresentationFeedback>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SubsurfaceSyncMode {
    Sync,
    Desync,
}

#[derive(Clone, Debug)]
struct SubsurfaceState {
    object_id: WlSubsurfaceId,
    parent_surface_object_id: ObjectId,
    pending_position: (i32, i32),
    current_position: (i32, i32),
    sync_mode: SubsurfaceSyncMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StackEntry {
    Parent,
    Child(ObjectId),
}

#[derive(Clone, Debug)]
struct SurfaceTreeState {
    pending_stack: Vec<StackEntry>,
    current_stack: Vec<StackEntry>,
}

impl Default for SurfaceTreeState {
    fn default() -> Self {
        let stack = vec![StackEntry::Parent];
        Self { pending_stack: stack.clone(), current_stack: stack }
    }
}

#[derive(Debug)]
pub struct WlSurface {
    id: WlSurfaceId,
    client_id: ClientId,
    role: Role,
    shell_surface: Option<ShellSurfaceObject>,
    popup_parent_layer_surface: Option<ZwlrLayerSurfaceV1Id>,
    version: u32,
    /// back buffer
    pending: Pending,
    /// front buffer
    on_screen: OnScreen,
    blocked: Option<BlockedSurfaceState>,
    blocked_waiter_id: Option<crate::sync::AcquireReadyWaiterId>,
    syncobj_surface_object: Option<crate::protocol_generated::WpLinuxDrmSyncobjSurfaceV1Id>,
    fractional_scale_object: Option<ObjectId>,
    viewport_object: Option<crate::protocol_generated::WpViewportId>,
    retired_buffers: Vec<RetiredBufferCandidate>,
    /// `wl_surface.frame` callbacks to complete on the next `commit`.
    pending_frame_callbacks: Vec<WlCallbackId>,
    pending_presentation_feedbacks: Vec<SurfacePresentationFeedback>,
    cached_commit: Option<CachedCommit>,
    subsurface: Option<SubsurfaceState>,
    tree: SurfaceTreeState,
    output_membership: BTreeSet<OutputId>,
    active_output: Option<OutputId>,
    preferred_fractional_scale_sent: Option<u32>,
    preferred_buffer_scale_sent: Option<i32>,
    preferred_buffer_transform_sent: Option<u32>,
}

#[derive(Clone, Debug)]
struct Pending {
    /// `None` = no attach this commit; `Some(None)` = detach; `Some(Some(id))` = attach.
    buffer: Option<Option<ObjectId>>,
    offset: (i32, i32),
    sync: SurfaceSyncState,
    buffer_transform: BufferTransform,
    buffer_scale: i32,
    viewport: SurfaceViewport,
    damage_surface: Vec<DamageRect>,
    damage_buffer: Vec<DamageRect>,
    input_region: Option<RegionValue>,
    opaque_region: Option<RegionValue>,
}

impl Default for Pending {
    fn default() -> Self {
        Self {
            buffer: None,
            offset: (0, 0),
            sync: SurfaceSyncState::default(),
            buffer_transform: BufferTransform::Normal,
            buffer_scale: 1,
            viewport: SurfaceViewport::default(),
            damage_surface: Vec::new(),
            damage_buffer: Vec::new(),
            input_region: None,
            opaque_region: None,
        }
    }
}

#[derive(Clone, Debug)]
struct SurfaceCommitPayload {
    buffer: Option<Option<ObjectId>>,
    offset: (i32, i32),
    sync: SurfaceSyncState,
    buffer_transform: BufferTransform,
    buffer_scale: i32,
    viewport: SurfaceViewport,
    damage_surface: Vec<DamageRect>,
    damage_buffer: Vec<DamageRect>,
    input_region: RegionValue,
    opaque_region: RegionValue,
}

#[derive(Clone, Debug)]
struct OnScreen {
    buffer_object_id: Option<ObjectId>,
    content_revision: u64,
    x: i32,
    y: i32,
    sync: SurfaceSyncState,
    buffer_transform: BufferTransform,
    buffer_scale: i32,
    viewport: SurfaceViewport,
    logical_size: Option<SizeI>,
    sample_transform: transform::Mat3,
    damage: DamageRegion,
    frame_callbacks: Vec<WlCallbackId>,
    presentation_feedbacks: Vec<SurfacePresentationFeedback>,
    input_region: RegionValue,
    opaque_region: RegionValue,
}

impl Default for OnScreen {
    fn default() -> Self {
        Self {
            buffer_object_id: None,
            content_revision: 0,
            x: 0,
            y: 0,
            sync: SurfaceSyncState::default(),
            buffer_transform: BufferTransform::Normal,
            buffer_scale: 1,
            viewport: SurfaceViewport::default(),
            logical_size: None,
            sample_transform: transform::Mat3::identity(),
            damage: DamageRegion::new(),
            frame_callbacks: Vec::new(),
            presentation_feedbacks: Vec::new(),
            input_region: RegionValue::infinite(),
            opaque_region: RegionValue::empty(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SurfaceRenderState {
    pub surface_client_id: ClientId,
    pub surface_id: WlSurfaceId,
    pub buffer_object_id: ObjectId,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub tex_transform: transform::Mat3,
    pub damage: DamageRegion,
    pub opaque_region: DamageRegion,
    pub sync: SurfaceSyncState,
    pub presentation_feedbacks: Vec<SurfacePresentationFeedback>,
}

#[derive(Clone, Debug)]
struct BlockedSurfaceState {
    buffer_object_id: ObjectId,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    tex_transform: transform::Mat3,
    sync: SurfaceSyncState,
    frame_callbacks: Vec<WlCallbackId>,
    presentation_feedbacks: Vec<SurfacePresentationFeedback>,
    buffer_transform: BufferTransform,
    buffer_scale: i32,
    viewport: SurfaceViewport,
    input_region: RegionValue,
    opaque_region: RegionValue,
}

struct SurfaceChanges {
    buffer_changed: bool,
    mapping_changed: bool,
}

#[derive(Clone, Copy, Debug)]
struct DamageRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Debug)]
struct DamageScaffold {
    surface_to_buffer: Vec<transform::RectF>,
    buffer_to_surface: Vec<transform::RectF>,
}

#[derive(Debug, Default)]
pub struct SurfaceOutputDelta {
    pub entered: Vec<OutputId>,
    pub left: Vec<OutputId>,
    pub active_output_changed: bool,
}

impl WlSurface {
    pub fn new(id: WlSurfaceId, client_id: ClientId, version: u32) -> Self {
        Self {
            id,
            client_id,
            role: Role::None,
            shell_surface: None,
            popup_parent_layer_surface: None,
            version,
            pending: Pending::default(),
            on_screen: OnScreen::default(),
            blocked: None,
            blocked_waiter_id: None,
            syncobj_surface_object: None,
            fractional_scale_object: None,
            viewport_object: None,
            retired_buffers: Vec::new(),
            pending_frame_callbacks: Vec::new(),
            pending_presentation_feedbacks: Vec::new(),
            cached_commit: None,
            subsurface: None,
            tree: SurfaceTreeState::default(),
            output_membership: BTreeSet::new(),
            active_output: None,
            preferred_fractional_scale_sent: None,
            preferred_buffer_scale_sent: None,
            preferred_buffer_transform_sent: None,
        }
    }

    pub fn set_syncobj_surface_object(
        &mut self,
        object: crate::protocol_generated::WpLinuxDrmSyncobjSurfaceV1Id,
    ) -> Result<(), ()> {
        if self.syncobj_surface_object.is_some() {
            return Err(());
        }
        self.syncobj_surface_object = Some(object);
        Ok(())
    }

    pub fn clear_syncobj_surface_object(
        &mut self,
        object: crate::protocol_generated::WpLinuxDrmSyncobjSurfaceV1Id,
    ) {
        if self.syncobj_surface_object == Some(object) {
            self.syncobj_surface_object = None;
        }
    }

    pub fn set_fractional_scale_object(&mut self, object_id: ObjectId) -> Result<(), ()> {
        if self.fractional_scale_object.is_some() {
            return Err(());
        }
        self.fractional_scale_object = Some(object_id);
        Ok(())
    }

    pub fn clear_fractional_scale_object(&mut self, object_id: ObjectId) {
        if self.fractional_scale_object == Some(object_id) {
            self.fractional_scale_object = None;
            self.preferred_fractional_scale_sent = None;
        }
    }

    pub fn has_fractional_scale_object(&self) -> bool {
        self.fractional_scale_object.is_some()
    }

    fn delete_owned_object(
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        id: WaylandId,
    ) {
        if client.object_id_for_wayland(id).is_some() {
            use crate::wayland_objects::wl_display::WlDisplay;
            client.delete_id(&WlDisplay::default(), state, dispatcher, id);
        }
    }

    fn cleanup_extension_objects(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(object_id) = self.fractional_scale_object.take() {
            self.preferred_fractional_scale_sent = None;
            let fractional_id = state
                .objects
                .try_ref_object::<WpFractionalScaleV1>(object_id)
                .map(|object| object.id());
            if let Some(fractional_id) = fractional_id {
                Self::delete_owned_object(client, state, dispatcher, fractional_id);
            }
        }

        if let Some(viewport_id) = self.viewport_object.take() {
            self.pending.viewport = SurfaceViewport::default();
            Self::delete_owned_object(client, state, dispatcher, viewport_id.into());
        }

        if let Some(syncobj_surface_id) = self.syncobj_surface_object.take() {
            Self::delete_owned_object(client, state, dispatcher, syncobj_surface_id.into());
        }
    }

    pub fn set_viewport_object(
        &mut self,
        object: crate::protocol_generated::WpViewportId,
    ) -> Result<(), ()> {
        if self.viewport_object.is_some() {
            return Err(());
        }
        self.viewport_object = Some(object);
        Ok(())
    }

    pub fn clear_viewport_object(&mut self, object: crate::protocol_generated::WpViewportId) {
        if self.viewport_object == Some(object) {
            self.viewport_object = None;
            self.pending.viewport = SurfaceViewport::default();
        }
    }

    pub fn has_role(&self) -> bool {
        self.role != Role::None
    }

    pub fn has_cursor_role(&self) -> bool {
        self.role == Role::Cursor
    }

    pub fn has_dnd_icon_role(&self) -> bool {
        self.role == Role::DndIcon
    }

    pub fn has_pending_buffer_attach(&self) -> bool {
        matches!(self.pending.buffer, Some(Some(_)))
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn has_committed_buffer(&self) -> bool {
        self.on_screen.buffer_object_id.is_some()
    }

    pub fn has_attached_or_committed_buffer(&self) -> bool {
        self.has_pending_buffer_attach() || self.has_committed_buffer()
    }

    pub fn xdg_surface_id(&self) -> Option<XdgSurfaceId> {
        match self.shell_surface {
            Some(ShellSurfaceObject::XdgSurface(id)) => Some(id),
            _ => None,
        }
    }

    pub fn layer_surface_id(&self) -> Option<ZwlrLayerSurfaceV1Id> {
        match self.shell_surface {
            Some(ShellSurfaceObject::LayerSurface(id)) => Some(id),
            _ => None,
        }
    }

    pub fn clear_xdg_surface_role(&mut self, xdg_surface_id: XdgSurfaceId) {
        if self.shell_surface == Some(ShellSurfaceObject::XdgSurface(xdg_surface_id)) {
            self.shell_surface = None;
            self.popup_parent_layer_surface = None;
            if self.role == Role::XdgSurface {
                self.role = Role::None;
            }
        }
    }

    pub fn clear_layer_surface_role(&mut self, layer_surface_id: ZwlrLayerSurfaceV1Id) {
        if self.shell_surface == Some(ShellSurfaceObject::LayerSurface(layer_surface_id)) {
            self.shell_surface = None;
            if self.role == Role::LayerSurface {
                self.role = Role::None;
            }
        }
    }

    pub fn set_popup_parent_layer_surface(
        &mut self,
        layer_surface_id: Option<ZwlrLayerSurfaceV1Id>,
    ) {
        self.popup_parent_layer_surface = layer_surface_id;
    }

    pub fn popup_parent_layer_surface_id(&self) -> Option<ZwlrLayerSurfaceV1Id> {
        self.popup_parent_layer_surface
    }

    pub fn assign_subsurface(
        &mut self,
        object_id: WlSubsurfaceId,
        parent_surface_object_id: ObjectId,
    ) {
        self.role = Role::Subsurface;
        self.subsurface = Some(SubsurfaceState {
            object_id,
            parent_surface_object_id,
            pending_position: (0, 0),
            current_position: (0, 0),
            sync_mode: SubsurfaceSyncMode::Sync,
        });
    }

    pub fn subsurface_object(&self) -> Option<WlSubsurfaceId> {
        self.subsurface
            .as_ref()
            .map(|subsurface| subsurface.object_id)
    }

    pub fn parent_surface_object_id(&self) -> Option<ObjectId> {
        self.subsurface
            .as_ref()
            .map(|subsurface| subsurface.parent_surface_object_id)
    }

    pub fn is_root(&self) -> bool {
        self.subsurface.is_none()
    }

    pub(crate) fn current_stack(&self) -> &[StackEntry] {
        &self.tree.current_stack
    }

    pub fn current_subsurface_position(&self) -> (i32, i32) {
        self.subsurface
            .as_ref()
            .map(|subsurface| subsurface.current_position)
            .unwrap_or((0, 0))
    }

    pub fn root_position(&self) -> (i32, i32) {
        (self.on_screen.x, self.on_screen.y)
    }

    pub fn content_revision(&self) -> u64 {
        self.on_screen.content_revision
    }

    pub fn active_output_id(&self) -> Option<OutputId> {
        self.active_output
    }

    pub fn contains_output(&self, output_id: OutputId) -> bool {
        self.output_membership.contains(&output_id)
    }

    pub fn output_membership_ids(&self) -> Vec<OutputId> {
        self.output_membership.iter().copied().collect()
    }

    pub fn update_output_membership(
        &mut self,
        outputs_in_priority_order: Vec<OutputId>,
    ) -> SurfaceOutputDelta {
        let mut next_membership = BTreeSet::new();
        let mut next_active_output = None;
        for output_id in outputs_in_priority_order {
            if next_membership.insert(output_id) && next_active_output.is_none() {
                next_active_output = Some(output_id);
            }
        }

        let left = self
            .output_membership
            .difference(&next_membership)
            .copied()
            .collect();
        let entered = next_membership
            .difference(&self.output_membership)
            .copied()
            .collect();
        let active_output_changed = self.active_output != next_active_output;

        self.output_membership = next_membership;
        self.active_output = next_active_output;

        SurfaceOutputDelta { entered, left, active_output_changed }
    }

    pub fn send_output_enter(&self, output: WlOutputId, dispatcher: &mut ClientDispatcher) {
        log::debug!("sending wl_surface.enter surface={:?} output={:?}", self.id, output);
        self.send_event(wl_surface::Event::Enter { output }, dispatcher);
    }

    pub fn send_output_leave(&self, output: WlOutputId, dispatcher: &mut ClientDispatcher) {
        log::debug!("sending wl_surface.leave surface={:?} output={:?}", self.id, output);
        self.send_event(wl_surface::Event::Leave { output }, dispatcher);
    }

    pub fn preferred_output_id(&self, state: &State) -> Option<OutputId> {
        state.output_id_for_surface(self)
    }

    pub fn preferred_output_id_for_object(
        &self,
        state: &State,
        surface_object_id: ObjectId,
    ) -> Option<OutputId> {
        state.output_id_for_surface_object(surface_object_id, self)
    }

    pub fn configure_bounds(&self, state: &State) -> Option<(i32, i32)> {
        self.configure_bounds_with_snapshot(state, None)
    }

    pub fn configure_bounds_for_object(
        &self,
        state: &State,
        surface_object_id: ObjectId,
    ) -> Option<(i32, i32)> {
        self.configure_bounds_with_snapshot_for_object(state, surface_object_id, None)
    }

    pub fn configure_bounds_with_snapshot(
        &self,
        state: &State,
        snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
    ) -> Option<(i32, i32)> {
        self.preferred_output_id(state)
            .and_then(|output_id| output_usable_bounds_with_snapshot(state, output_id, snapshot))
            .map(|bounds| (bounds.width, bounds.height))
    }

    pub fn configure_bounds_with_snapshot_for_object(
        &self,
        state: &State,
        surface_object_id: ObjectId,
        snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
    ) -> Option<(i32, i32)> {
        self.preferred_output_id_for_object(state, surface_object_id)
            .and_then(|output_id| output_usable_bounds_with_snapshot(state, output_id, snapshot))
            .map(|bounds| (bounds.width, bounds.height))
    }

    pub fn send_preferred_buffer_state(
        &mut self,
        state: &State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let preferred_output_id = self.preferred_output_id(state);
        self.send_preferred_buffer_state_with_output(preferred_output_id, state, dispatcher);
    }

    pub fn send_preferred_buffer_state_for_object(
        &mut self,
        state: &State,
        surface_object_id: ObjectId,
        dispatcher: &mut ClientDispatcher,
    ) {
        let preferred_output_id = self.preferred_output_id_for_object(state, surface_object_id);
        self.send_preferred_buffer_state_with_output(preferred_output_id, state, dispatcher);
    }

    fn send_preferred_buffer_state_with_output(
        &mut self,
        preferred_output_id: Option<OutputId>,
        state: &State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let Some(output) =
            preferred_output_id.and_then(|output_id| state.outputs.try_get(output_id))
        else {
            return;
        };
        self.send_preferred_buffer_state_for_output(
            output.legacy_scale().max(1),
            output.transform as u32,
            dispatcher,
        );
    }

    pub fn send_preferred_fractional_scale(
        &mut self,
        state: &State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let preferred_output_id = self.preferred_output_id(state);
        self.send_preferred_fractional_scale_with_output(preferred_output_id, state, dispatcher);
    }

    pub fn send_preferred_fractional_scale_for_object(
        &mut self,
        state: &State,
        surface_object_id: ObjectId,
        dispatcher: &mut ClientDispatcher,
    ) {
        let preferred_output_id = self.preferred_output_id_for_object(state, surface_object_id);
        self.send_preferred_fractional_scale_with_output(preferred_output_id, state, dispatcher);
    }

    fn send_preferred_fractional_scale_with_output(
        &mut self,
        preferred_output_id: Option<OutputId>,
        state: &State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let Some(output) =
            preferred_output_id.and_then(|output_id| state.outputs.try_get(output_id))
        else {
            return;
        };
        self.send_preferred_fractional_scale_for_output(output.scale, state, dispatcher);
    }

    pub fn send_preferred_fractional_scale_for_output(
        &mut self,
        scale: FractionalScale,
        state: &State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let Some(object_id) = self.fractional_scale_object else {
            return;
        };
        let scale = scale.as_v120();
        if self.preferred_fractional_scale_sent == Some(scale) {
            return;
        }
        let Some(object) = state
            .objects
            .try_ref_object::<WpFractionalScaleV1>(object_id)
        else {
            return;
        };
        object.send_preferred_scale(scale, dispatcher);
        self.preferred_fractional_scale_sent = Some(scale);
    }

    pub fn send_preferred_buffer_state_for_output(
        &mut self,
        scale: i32,
        transform: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        if self.version >= wl_surface::PREFERRED_BUFFER_SCALE_SINCE {
            if self.preferred_buffer_scale_sent != Some(scale) {
                self.send_event(
                    wl_surface::Event::PreferredBufferScale { factor: scale },
                    dispatcher,
                );
                self.preferred_buffer_scale_sent = Some(scale);
            }
        }

        if self.version >= wl_surface::PREFERRED_BUFFER_TRANSFORM_SINCE {
            if self.preferred_buffer_transform_sent != Some(transform) {
                self.send_event(
                    wl_surface::Event::PreferredBufferTransform { transform },
                    dispatcher,
                );
                self.preferred_buffer_transform_sent = Some(transform);
            }
        }
    }

    pub fn set_root_position(&mut self, x: i32, y: i32) {
        self.on_screen.x = x;
        self.on_screen.y = y;
        if let Some(blocked) = self.blocked.as_mut() {
            blocked.x = x;
            blocked.y = y;
        }
    }

    fn pending_subsurface_position(&self) -> (i32, i32) {
        self.subsurface
            .as_ref()
            .map(|subsurface| subsurface.pending_position)
            .unwrap_or((0, 0))
    }

    fn sync_mode(&self) -> Option<SubsurfaceSyncMode> {
        self.subsurface
            .as_ref()
            .map(|subsurface| subsurface.sync_mode)
    }

    pub fn set_subsurface_position(&mut self, x: i32, y: i32) {
        if let Some(subsurface) = self.subsurface.as_mut() {
            subsurface.pending_position = (x, y);
        }
    }

    pub fn set_subsurface_sync_mode(&mut self, mode: bool) {
        if let Some(subsurface) = self.subsurface.as_mut() {
            subsurface.sync_mode =
                if mode { SubsurfaceSyncMode::Sync } else { SubsurfaceSyncMode::Desync };
        }
    }

    pub fn destroy_subsurface_role(&mut self) {
        self.cached_commit = None;
        self.subsurface = None;
        if self.role == Role::Subsurface {
            self.role = Role::None;
        }
    }

    pub fn add_pending_child(&mut self, child_surface_object_id: ObjectId) {
        self.ensure_pending_stack_parent();
        Self::remove_child_from_stack(&mut self.tree.pending_stack, child_surface_object_id);
        let parent_index = self
            .tree
            .pending_stack
            .iter()
            .position(|entry| *entry == StackEntry::Parent)
            .unwrap_or(self.tree.pending_stack.len());
        self.tree
            .pending_stack
            .insert(parent_index, StackEntry::Child(child_surface_object_id));
    }

    pub fn remove_child_from_tree(&mut self, child_surface_object_id: ObjectId) {
        Self::remove_child_from_stack(&mut self.tree.pending_stack, child_surface_object_id);
        Self::remove_child_from_stack(&mut self.tree.current_stack, child_surface_object_id);
        self.ensure_pending_stack_parent();
        self.ensure_current_stack_parent();
    }

    pub fn restack_child_relative(
        &mut self,
        child_surface_object_id: ObjectId,
        sibling_surface_object_id: Option<ObjectId>,
        place_above: bool,
    ) -> bool {
        self.ensure_pending_stack_parent();
        let target = match sibling_surface_object_id {
            Some(object_id) => StackEntry::Child(object_id),
            None => StackEntry::Parent,
        };
        if !self.tree.pending_stack.contains(&target) {
            return false;
        }
        Self::remove_child_from_stack(&mut self.tree.pending_stack, child_surface_object_id);
        let Some(index) = self
            .tree
            .pending_stack
            .iter()
            .position(|entry| *entry == target)
        else {
            return false;
        };
        let insert_at = if place_above { index + 1 } else { index };
        self.tree
            .pending_stack
            .insert(insert_at, StackEntry::Child(child_surface_object_id));
        true
    }

    pub fn is_ancestor_of(
        &self,
        self_object_id: ObjectId,
        state: &State,
        descendant_surface_object_id: ObjectId,
    ) -> bool {
        let mut current = Some(descendant_surface_object_id);
        while let Some(object_id) = current {
            if object_id == self_object_id {
                return true;
            }
            let Some(surface) = state.objects.try_ref_object::<WlSurface>(object_id) else {
                return false;
            };
            current = surface.parent_surface_object_id();
        }
        false
    }

    fn send_viewport_error(
        &self,
        client: &Client,
        dispatcher: &mut ClientDispatcher,
        code: u32,
        message: impl AsRef<str>,
    ) {
        let Some(viewport) = self.viewport_object else {
            log::warn!(
                "viewport protocol error without viewport object on wl_surface {:?}: {}",
                self.id,
                message.as_ref()
            );
            return;
        };
        client.send_protocol_error(dispatcher, viewport.into(), code, message);
    }

    pub fn pending_sync_mut(&mut self) -> &mut SurfaceSyncState {
        &mut self.pending.sync
    }

    pub fn pending_viewport_mut(&mut self) -> &mut SurfaceViewport {
        &mut self.pending.viewport
    }

    pub fn has_syncobj_surface_object(&self) -> bool {
        self.syncobj_surface_object.is_some()
    }

    fn send_syncobj_surface_error(
        &self,
        client: &Client,
        dispatcher: &mut ClientDispatcher,
        code: u32,
        message: impl AsRef<str>,
    ) {
        let Some(syncobj_surface) = self.syncobj_surface_object else {
            log::warn!(
                "syncobj protocol error without syncobj surface object on wl_surface {:?}: {}",
                self.id,
                message.as_ref()
            );
            return;
        };
        client.send_protocol_error(dispatcher, syncobj_surface.into(), code, message);
    }

    pub fn set_role(
        &mut self,
        new_role: Role,
        dispatcher: &mut ClientDispatcher,
    ) -> Result<(), xdg_wm_base::Error> {
        if self.role != Role::None && self.role != new_role {
            self.send_error(
                dispatcher,
                xdg_wm_base::Error::Role as u32,
                "invalid xdg_surface role",
            );
            return Err(xdg_wm_base::Error::Role);
        }
        self.role = new_role;
        Ok(())
    }

    pub fn set_cursor_role(
        &mut self,
        dispatcher: &mut ClientDispatcher,
    ) -> Result<(), wl_pointer::Error> {
        if self.role != Role::None && self.role != Role::Cursor {
            self.send_error(dispatcher, wl_pointer::Error::Role as u32, "invalid cursor role");
            return Err(wl_pointer::Error::Role);
        }
        self.role = Role::Cursor;
        Ok(())
    }

    pub fn set_dnd_icon_role(&mut self) -> Result<(), ()> {
        if self.role != Role::None && self.role != Role::DndIcon {
            return Err(());
        }
        self.role = Role::DndIcon;
        Ok(())
    }

    pub fn set_xdg_surface_impl(&mut self, id: XdgSurfaceId) {
        self.shell_surface = Some(ShellSurfaceObject::XdgSurface(id));
    }

    pub fn set_layer_surface_impl(&mut self, id: ZwlrLayerSurfaceV1Id) {
        self.shell_surface = Some(ShellSurfaceObject::LayerSurface(id));
    }

    fn on_attach(
        &mut self,
        client: &mut Client,
        state: &mut State,
        buffer_id: WlBufferId,
        x: i32,
        y: i32,
        dispatcher: &mut ClientDispatcher,
    ) -> Result<(), wl_surface::Error> {
        if self.version >= wl_surface::OFFSET_SINCE {
            if x != 0 || y != 0 {
                self.send_error(
                    dispatcher,
                    wl_surface::Error::InvalidOffset as u32,
                    "non-zero attach offset not allowed",
                );
                return Err(wl_surface::Error::InvalidOffset);
            }
        } else {
            self.pending.offset = (x, y);
        }

        let raw: u32 = buffer_id.into();
        if raw == 0 {
            self.pending.buffer = Some(None);
            return Ok(());
        }

        if !client.contains_object(buffer_id, state) {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                "invalid wl_buffer for wl_surface.attach",
            );
            return Ok(());
        }

        self.pending.buffer = Some(Some(client.object_id(buffer_id)));

        Ok(())
    }

    fn snapshot_region_request(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        region_id: WlRegionId,
        null_region: RegionValue,
        request_name: &'static str,
    ) -> Option<RegionValue> {
        let raw: u32 = region_id.into();
        if raw == 0 {
            return Some(null_region);
        }
        if !client.object_is(region_id, state) {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                format!("invalid wl_region for {request_name}"),
            );
            return None;
        }
        Some(client.ref_object(region_id, state).snapshot())
    }

    fn on_set_opaque_region(
        &mut self,
        client: &mut Client,
        state: &mut State,
        region_id: WlRegionId,
        dispatcher: &mut ClientDispatcher,
    ) {
        let Some(region) = self.snapshot_region_request(
            client,
            state,
            dispatcher,
            region_id,
            RegionValue::empty(),
            "wl_surface.set_opaque_region",
        ) else {
            return;
        };
        self.pending.opaque_region = Some(region);
    }

    fn on_set_input_region(
        &mut self,
        client: &mut Client,
        state: &mut State,
        region_id: WlRegionId,
        dispatcher: &mut ClientDispatcher,
    ) {
        let Some(region) = self.snapshot_region_request(
            client,
            state,
            dispatcher,
            region_id,
            RegionValue::infinite(),
            "wl_surface.set_input_region",
        ) else {
            return;
        };
        self.pending.input_region = Some(region);
    }

    fn ensure_pending_stack_parent(&mut self) {
        if !self.tree.pending_stack.contains(&StackEntry::Parent) {
            self.tree.pending_stack.insert(0, StackEntry::Parent);
        }
    }

    fn ensure_current_stack_parent(&mut self) {
        if !self.tree.current_stack.contains(&StackEntry::Parent) {
            self.tree.current_stack.insert(0, StackEntry::Parent);
        }
    }

    fn remove_child_from_stack(stack: &mut Vec<StackEntry>, child_surface_object_id: ObjectId) {
        stack.retain(|entry| *entry != StackEntry::Child(child_surface_object_id));
    }

    fn child_surface_ids(&self) -> Vec<ObjectId> {
        let mut children = Vec::new();
        for stack in [&self.tree.pending_stack, &self.tree.current_stack] {
            for entry in stack {
                let StackEntry::Child(child_surface_object_id) = *entry else {
                    continue;
                };
                if !children.contains(&child_surface_object_id) {
                    children.push(child_surface_object_id);
                }
            }
        }
        children
    }

    pub(crate) fn unmap_due_to_parent_loss(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.discard_all_presentation_feedbacks(client, state, dispatcher);
        self.cancel_blocked(client, state, dispatcher);
        self.retire_current();
        self.on_screen = OnScreen {
            buffer_object_id: None,
            content_revision: self.next_content_revision(),
            x: self.on_screen.x,
            y: self.on_screen.y,
            sync: SurfaceSyncState::default(),
            buffer_transform: self.on_screen.buffer_transform,
            buffer_scale: self.on_screen.buffer_scale,
            viewport: self.on_screen.viewport,
            logical_size: None,
            sample_transform: transform::Mat3::identity(),
            damage: DamageRegion::new(),
            frame_callbacks: std::mem::take(&mut self.on_screen.frame_callbacks),
            presentation_feedbacks: Vec::new(),
            input_region: self.on_screen.input_region.clone(),
            opaque_region: self.on_screen.opaque_region.clone(),
        };
    }

    fn build_commit_payload(&mut self) -> SurfaceCommitPayload {
        SurfaceCommitPayload {
            buffer: self.pending.buffer.take(),
            offset: std::mem::take(&mut self.pending.offset),
            sync: std::mem::take(&mut self.pending.sync),
            buffer_transform: self.pending.buffer_transform,
            buffer_scale: self.pending.buffer_scale,
            viewport: self.pending.viewport,
            damage_surface: std::mem::take(&mut self.pending.damage_surface),
            damage_buffer: std::mem::take(&mut self.pending.damage_buffer),
            input_region: self
                .pending
                .input_region
                .take()
                .unwrap_or_else(|| self.on_screen.input_region.clone()),
            opaque_region: self
                .pending
                .opaque_region
                .take()
                .unwrap_or_else(|| self.on_screen.opaque_region.clone()),
        }
    }

    fn cache_commit_payload(
        &mut self,
        payload: SurfaceCommitPayload,
        frame_callbacks: Vec<WlCallbackId>,
        presentation_feedbacks: Vec<SurfacePresentationFeedback>,
    ) -> Vec<SurfacePresentationFeedback> {
        // Follow a Smithay-like "compose cached state until the apply root commits"
        // model rather than treating the first cached commit as immutable.
        match self.cached_commit.as_mut() {
            Some(cached) => {
                if payload.buffer.is_some() {
                    cached.payload.buffer = payload.buffer;
                    cached.payload.offset = payload.offset;
                    cached.payload.sync = payload.sync;
                }
                cached.payload.buffer_transform = payload.buffer_transform;
                cached.payload.buffer_scale = payload.buffer_scale;
                cached.payload.viewport = payload.viewport;
                cached.payload.damage_surface.extend(payload.damage_surface);
                cached.payload.damage_buffer.extend(payload.damage_buffer);
                cached.payload.input_region = payload.input_region;
                cached.payload.opaque_region = payload.opaque_region;
                cached.frame_callbacks.extend(frame_callbacks);
                return std::mem::replace(
                    &mut cached.presentation_feedbacks,
                    presentation_feedbacks,
                );
            }
            None => {
                self.cached_commit =
                    Some(CachedCommit { payload, frame_callbacks, presentation_feedbacks });
                Vec::new()
            }
        }
    }

    fn clear_failed_surface_commit(&mut self) {
        self.pending = Pending::default();
    }

    fn get_changes_for_payload(&self, payload: &SurfaceCommitPayload) -> SurfaceChanges {
        SurfaceChanges {
            buffer_changed: payload.buffer.is_some(),
            mapping_changed: payload.buffer_transform != self.on_screen.buffer_transform
                || payload.buffer_scale != self.on_screen.buffer_scale
                || payload.viewport != self.on_screen.viewport,
        }
    }

    fn validate_pending_mapping(
        &self,
        client: &Client,
        state: &State,
        dispatcher: &mut ClientDispatcher,
    ) -> bool {
        self.validate_payload_mapping(
            client,
            state,
            dispatcher,
            self.pending.buffer,
            self.pending.buffer_transform,
            self.pending.buffer_scale,
            self.pending.viewport,
        )
    }

    fn validate_payload_mapping(
        &self,
        client: &Client,
        state: &State,
        dispatcher: &mut ClientDispatcher,
        buffer: Option<Option<ObjectId>>,
        buffer_transform: BufferTransform,
        buffer_scale: i32,
        viewport: SurfaceViewport,
    ) -> bool {
        let Some(buffer_object_id) = buffer.flatten().or(self.on_screen.buffer_object_id) else {
            return true;
        };
        let Some(buffer) = state.objects.try_ref_object::<WlBuffer>(buffer_object_id) else {
            return true;
        };

        let buffer_size = buffer.dimensions();
        let buffer_size = SizeI { width: buffer_size.0, height: buffer_size.1 };
        let base =
            transform::pre_viewport_logical_size(buffer_size, buffer_transform, buffer_scale);
        if let Some(src) = viewport.src {
            if src.width <= 0.0 || src.height <= 0.0 || src.x < 0.0 || src.y < 0.0 {
                self.send_viewport_error(
                    client,
                    dispatcher,
                    wp_viewport::Error::BadValue as u32,
                    "viewport source must have positive size and non-negative origin",
                );
                return false;
            }
            if viewport.dst.is_none() && (src.width.fract() != 0.0 || src.height.fract() != 0.0) {
                self.send_viewport_error(
                    client,
                    dispatcher,
                    wp_viewport::Error::BadSize as u32,
                    "viewport source must have integer size when destination is unset",
                );
                return false;
            }
            let content =
                transform::RectF { x: 0.0, y: 0.0, width: base.width, height: base.height };
            if !transform::rect_contains(content, src) {
                self.send_viewport_error(
                    client,
                    dispatcher,
                    wp_viewport::Error::OutOfBuffer as u32,
                    "viewport source rectangle extends outside the transformed/scaled buffer",
                );
                return false;
            }
        }
        true
    }

    fn mapped_state_for_buffer(
        &self,
        state: &State,
        buffer_object_id: ObjectId,
        x: i32,
        y: i32,
        payload: &SurfaceCommitPayload,
    ) -> Option<BlockedSurfaceState> {
        let buffer = state.objects.try_ref_object::<WlBuffer>(buffer_object_id)?;
        let buffer_size = buffer.dimensions();
        let buffer_size = SizeI { width: buffer_size.0, height: buffer_size.1 };
        let logical_size = transform::derive_logical_size(
            buffer_size,
            payload.buffer_transform,
            payload.buffer_scale,
            payload.viewport,
        );
        let tex_transform = transform::surface_sample_transform(
            buffer_size,
            payload.buffer_transform,
            payload.buffer_scale,
            payload.viewport,
        );
        Some(BlockedSurfaceState {
            buffer_object_id,
            x,
            y,
            width: logical_size.width,
            height: logical_size.height,
            tex_transform,
            sync: payload.sync.clone(),
            frame_callbacks: Vec::new(),
            presentation_feedbacks: Vec::new(),
            buffer_transform: payload.buffer_transform,
            buffer_scale: payload.buffer_scale,
            viewport: payload.viewport,
            input_region: payload.input_region.clone(),
            opaque_region: payload.opaque_region.clone(),
        })
    }

    fn pending_damage_scaffold(
        &self,
        state: &State,
        buffer_object_id: ObjectId,
        logical_size: SizeI,
        payload: &SurfaceCommitPayload,
    ) -> Option<DamageScaffold> {
        let buffer = state.objects.try_ref_object::<WlBuffer>(buffer_object_id)?;
        let raw_size = buffer.dimensions();
        let raw_size = SizeI { width: raw_size.0, height: raw_size.1 };
        let surface_to_buffer = payload
            .damage_surface
            .iter()
            .map(|rect| {
                transform::surface_rect_to_buffer_rect(
                    transform::RectF {
                        x: rect.x as f32,
                        y: rect.y as f32,
                        width: rect.width as f32,
                        height: rect.height as f32,
                    },
                    logical_size,
                    raw_size,
                    payload.buffer_transform,
                    payload.buffer_scale,
                    payload.viewport,
                )
            })
            .collect();
        let buffer_to_surface = payload
            .damage_buffer
            .iter()
            .filter_map(|rect| {
                transform::buffer_rect_to_surface_rect(
                    transform::RectF {
                        x: rect.x as f32,
                        y: rect.y as f32,
                        width: rect.width as f32,
                        height: rect.height as f32,
                    },
                    logical_size,
                    raw_size,
                    payload.buffer_transform,
                    payload.buffer_scale,
                    payload.viewport,
                )
            })
            .collect();
        Some(DamageScaffold { surface_to_buffer, buffer_to_surface })
    }

    fn full_damage_for_logical_size(logical_size: SizeI) -> DamageRegion {
        let mut damage = DamageRegion::new();
        if let Some(rect) = RectI::new(0, 0, logical_size.width, logical_size.height) {
            damage.add_rect(rect);
        }
        damage
    }

    fn explicit_damage_region(
        &self,
        state: &State,
        payload: &SurfaceCommitPayload,
        logical_size: SizeI,
        buffer_object_id: ObjectId,
    ) -> DamageRegion {
        let mut damage = DamageRegion::new();
        for rect in &payload.damage_surface {
            if let Some(rect) = RectI::new(rect.x, rect.y, rect.width, rect.height) {
                damage.add_rect(rect);
            }
        }
        if let Some(scaffold) =
            self.pending_damage_scaffold(state, buffer_object_id, logical_size, payload)
        {
            for rect in scaffold.buffer_to_surface {
                if let Some(rect) = RectI::from_f32_bounds(rect) {
                    damage.add_rect(rect);
                }
            }
        }
        if let Some(bounds) = RectI::new(0, 0, logical_size.width, logical_size.height) {
            damage.intersect_rect(bounds);
        }
        damage
    }

    fn committed_damage_region(
        &self,
        state: &State,
        payload: &SurfaceCommitPayload,
        buffer_changed: bool,
        mapping_changed: bool,
    ) -> DamageRegion {
        let Some(buffer_object_id) = self.on_screen.buffer_object_id else {
            return DamageRegion::new();
        };
        let Some(logical_size) = self.on_screen.logical_size else {
            return DamageRegion::new();
        };
        if !ENABLE_DAMAGE {
            if buffer_changed
                || mapping_changed
                || !payload.damage_surface.is_empty()
                || !payload.damage_buffer.is_empty()
            {
                return Self::full_damage_for_logical_size(logical_size);
            }
            return DamageRegion::new();
        }
        if buffer_changed || mapping_changed {
            return Self::full_damage_for_logical_size(logical_size);
        }
        self.explicit_damage_region(state, payload, logical_size, buffer_object_id)
    }

    fn parent_behaves_synchronized(&self, state: &State) -> bool {
        let Some(parent_object_id) = self.parent_surface_object_id() else {
            return false;
        };
        let Some(parent) = state.objects.try_ref_object::<WlSurface>(parent_object_id) else {
            return false;
        };
        parent.behaves_synchronized(state)
    }

    fn behaves_synchronized(&self, state: &State) -> bool {
        match self.sync_mode() {
            None => false,
            Some(SubsurfaceSyncMode::Sync) => true,
            Some(SubsurfaceSyncMode::Desync) => self.parent_behaves_synchronized(state),
        }
    }

    pub(crate) fn apply_pending_tree_state(&mut self, state: &mut State) -> bool {
        let order_changed = self.tree.pending_stack != self.tree.current_stack;
        let mut child_ids = Vec::new();
        for stack in [&self.tree.pending_stack, &self.tree.current_stack] {
            for entry in stack {
                let StackEntry::Child(child_surface_object_id) = *entry else {
                    continue;
                };
                if !child_ids.contains(&child_surface_object_id) {
                    child_ids.push(child_surface_object_id);
                }
            }
        }
        self.tree.current_stack = self.tree.pending_stack.clone();
        self.ensure_current_stack_parent();

        let mut position_changed = false;
        for child_surface_object_id in child_ids {
            let Some(mut child_surface) = state
                .objects
                .try_grab_object::<WlSurface>(child_surface_object_id)
            else {
                continue;
            };
            let old_position = child_surface.current_subsurface_position();
            let new_position = child_surface.pending_subsurface_position();
            if old_position != new_position {
                position_changed = true;
            }
            if let Some(subsurface) = child_surface.subsurface.as_mut() {
                subsurface.current_position = subsurface.pending_position;
            }
        }

        order_changed || position_changed
    }

    fn flush_frame_callbacks_list(
        &mut self,
        callbacks: Vec<WlCallbackId>,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        use crate::wayland_objects::wl_display::WlDisplay;
        let display = WlDisplay::default();
        let time_msec = monotonic_time_msec();
        for cb in callbacks {
            {
                let callback = client.ref_object(cb, state);
                callback.done(time_msec, dispatcher);
            }
            client.delete_id(&display, state, dispatcher, cb.into());
        }
    }

    fn apply_surface_payload(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        payload: &SurfaceCommitPayload,
    ) -> bool {
        let changes = self.get_changes_for_payload(payload);
        let mut commit_blocked = false;
        let mut committed_damage = DamageRegion::new();
        let previous_feedbacks = std::mem::take(&mut self.on_screen.presentation_feedbacks);

        match payload.buffer {
            None => {}
            Some(None) => {
                self.cancel_blocked(client, state, dispatcher);
                self.retire_current();
                self.on_screen = OnScreen {
                    buffer_object_id: None,
                    content_revision: self.next_content_revision(),
                    x: self.on_screen.x,
                    y: self.on_screen.y,
                    sync: SurfaceSyncState::default(),
                    buffer_transform: payload.buffer_transform,
                    buffer_scale: payload.buffer_scale,
                    viewport: payload.viewport,
                    logical_size: None,
                    sample_transform: transform::Mat3::identity(),
                    damage: DamageRegion::new(),
                    frame_callbacks: std::mem::take(&mut self.on_screen.frame_callbacks),
                    presentation_feedbacks: Vec::new(),
                    input_region: payload.input_region.clone(),
                    opaque_region: payload.opaque_region.clone(),
                };
            }
            Some(Some(buffer_object_id)) => {
                let Some(next) = self.mapped_state_for_buffer(
                    state,
                    buffer_object_id,
                    self.on_screen.x + payload.offset.0,
                    self.on_screen.y + payload.offset.1,
                    payload,
                ) else {
                    log::warn!("failed to derive mapped state for buffer {:?}", buffer_object_id);
                    self.on_screen.presentation_feedbacks = previous_feedbacks;
                    return false;
                };

                let should_block =
                    self.syncobj_surface_object.is_some() && !next.sync.acquire.is_ready_now();
                if should_block {
                    commit_blocked = true;
                    if let Err(error) = self.install_blocked_surface(
                        next.clone(),
                        self.client_id,
                        client.object_id(self.id),
                        dispatcher,
                    ) {
                        log::warn!(
                            "failed to install acquire waiter for surface {:?}: {:?}; keeping previous contents active",
                            self.id,
                            error
                        );
                        self.cancel_blocked(client, state, dispatcher);
                        self.on_screen.presentation_feedbacks = previous_feedbacks;
                        return false;
                    }
                } else {
                    self.cancel_blocked(client, state, dispatcher);
                    self.activate_blocked_state(next);
                }
            }
        }

        if !changes.buffer_changed
            && changes.mapping_changed
            && self.on_screen.buffer_object_id.is_some()
        {
            let Some(next) = self.mapped_state_for_buffer(
                state,
                self.on_screen.buffer_object_id.unwrap(),
                self.on_screen.x,
                self.on_screen.y,
                &SurfaceCommitPayload {
                    buffer: None,
                    offset: (0, 0),
                    sync: self.on_screen.sync.clone(),
                    buffer_transform: payload.buffer_transform,
                    buffer_scale: payload.buffer_scale,
                    viewport: payload.viewport,
                    damage_surface: Vec::new(),
                    damage_buffer: Vec::new(),
                    input_region: self.on_screen.input_region.clone(),
                    opaque_region: self.on_screen.opaque_region.clone(),
                },
            ) else {
                log::warn!("failed to recompute mapped state for current surface {:?}", self.id);
                self.on_screen.presentation_feedbacks = previous_feedbacks;
                return false;
            };
            self.on_screen.buffer_transform = next.buffer_transform;
            self.on_screen.buffer_scale = next.buffer_scale;
            self.on_screen.viewport = next.viewport;
            self.on_screen.logical_size = Some(SizeI { width: next.width, height: next.height });
            self.on_screen.sample_transform = next.tex_transform;
        }

        if !commit_blocked {
            self.on_screen.input_region = payload.input_region.clone();
            self.on_screen.opaque_region = payload.opaque_region.clone();
            committed_damage = self.committed_damage_region(
                state,
                payload,
                changes.buffer_changed,
                changes.mapping_changed,
            );
            self.on_screen.damage.add_region(&committed_damage);
            if changes.buffer_changed || changes.mapping_changed || !committed_damage.is_empty() {
                self.on_screen.content_revision = self.next_content_revision();
            }
        }

        let SurfaceChanges { buffer_changed, mapping_changed } = changes;
        if buffer_changed || mapping_changed {
            log::debug!(
                "wl_surface commit applied surface={:?} buffer={:?} logical_size={:?} pos=({}, {}) transform={:?} scale={} viewport={:?}",
                self.id,
                self.on_screen.buffer_object_id,
                self.on_screen.logical_size,
                self.on_screen.x,
                self.on_screen.y,
                self.on_screen.buffer_transform,
                self.on_screen.buffer_scale,
                self.on_screen.viewport
            );
        }

        if !committed_damage.is_empty() {
            log::debug!(
                "wl_surface commit damage applied surface={:?} local_damage={:?}",
                self.id,
                committed_damage.rects()
            );
        }

        self.discard_presentation_feedbacks(client, state, dispatcher, previous_feedbacks);

        buffer_changed || mapping_changed || !committed_damage.is_empty()
    }

    fn apply_cached_commit(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        surface_behaves_synchronized: bool,
    ) -> bool {
        let Some(cached_commit) = self.cached_commit.take() else {
            return false;
        };
        let (changed, needs_frame) = self.apply_surface_update(
            client,
            state,
            dispatcher,
            cached_commit.payload,
            cached_commit.frame_callbacks,
            cached_commit.presentation_feedbacks,
            surface_behaves_synchronized,
        );
        changed || needs_frame
    }

    fn release_synchronized_children(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        surface_behaves_synchronized: bool,
    ) -> bool {
        let mut changed = false;
        let stack = self.tree.current_stack.clone();
        for entry in stack {
            let StackEntry::Child(child_surface_object_id) = entry else {
                continue;
            };
            let child_behaves_synchronized = {
                let Some(child_surface) = state
                    .objects
                    .try_ref_object::<WlSurface>(child_surface_object_id)
                else {
                    continue;
                };
                surface_behaves_synchronized
                    || child_surface.sync_mode() == Some(SubsurfaceSyncMode::Sync)
            };
            if !child_behaves_synchronized {
                continue;
            }
            let Some(mut child_surface) = state
                .objects
                .try_grab_object::<WlSurface>(child_surface_object_id)
            else {
                continue;
            };
            changed |= child_surface.apply_cached_commit(
                client,
                state,
                dispatcher,
                child_behaves_synchronized,
            );
        }
        changed
    }

    fn apply_surface_update(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        payload: SurfaceCommitPayload,
        frame_callbacks: Vec<WlCallbackId>,
        presentation_feedbacks: Vec<SurfacePresentationFeedback>,
        surface_behaves_synchronized: bool,
    ) -> (bool, bool) {
        let mut changed = self.apply_surface_payload(client, state, dispatcher, &payload);
        changed |= state.seat.apply_cursor_surface_offset(
            client.object_id(self.id),
            payload.offset.0,
            payload.offset.1,
        );
        changed |= self.apply_pending_tree_state(state);
        changed |= self.release_synchronized_children(
            client,
            state,
            dispatcher,
            surface_behaves_synchronized,
        );
        let (discarded_feedbacks, needs_presentation_frame) =
            self.route_presentation_feedbacks(presentation_feedbacks);
        self.discard_presentation_feedbacks(client, state, dispatcher, discarded_feedbacks);
        let (immediate_callbacks, needs_frame) = self.route_frame_callbacks(frame_callbacks);
        self.flush_frame_callbacks_list(immediate_callbacks, client, state, dispatcher);
        (changed, needs_frame || needs_presentation_frame)
    }

    fn on_commit(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let was_mapped = self.global_logical_bounds().is_some();
        if let Some(shell_surface) = self.shell_surface {
            let surface_object_id = client.object_id(self.id);
            self.send_preferred_fractional_scale_for_object(state, surface_object_id, dispatcher);
            self.send_preferred_buffer_state_for_object(state, surface_object_id, dispatcher);
            match shell_surface {
                ShellSurfaceObject::XdgSurface(xdg_surface_id) => {
                    let layout_snapshot =
                        LayerLayoutObjectSnapshot::for_surface(surface_object_id, self);
                    let configure_bounds = self.configure_bounds_with_snapshot_for_object(
                        state,
                        surface_object_id,
                        Some(layout_snapshot),
                    );
                    let mut xdg_surface = client.grab_object(xdg_surface_id, state);
                    if !xdg_surface.on_commit(
                        client,
                        state,
                        dispatcher,
                        self.has_pending_buffer_attach(),
                        matches!(self.pending.buffer, Some(None)),
                        configure_bounds,
                    ) {
                        self.clear_failed_surface_commit();
                        return;
                    }
                }
                ShellSurfaceObject::LayerSurface(layer_surface_id) => {
                    let surface_object_id = client.object_id(self.id);
                    let layer_surface_object_id = client.object_id(layer_surface_id);
                    let mut layer_surface = client.grab_object(layer_surface_id, state);
                    if !layer_surface.on_commit(
                        client,
                        state,
                        dispatcher,
                        surface_object_id,
                        self,
                        layer_surface_object_id,
                        self.has_pending_buffer_attach(),
                        matches!(self.pending.buffer, Some(None)),
                    ) {
                        self.clear_failed_surface_commit();
                        return;
                    }
                }
            }
        }

        if !self.validate_pending_mapping(client, state, dispatcher) {
            self.clear_failed_surface_commit();
            return;
        }

        if !self.validate_pending_sync(client, state, dispatcher) {
            self.clear_failed_surface_commit();
            return;
        }

        let payload = self.build_commit_payload();
        let frame_callbacks = std::mem::take(&mut self.pending_frame_callbacks);
        let presentation_feedbacks = std::mem::take(&mut self.pending_presentation_feedbacks);

        if self.behaves_synchronized(state) {
            let discarded =
                self.cache_commit_payload(payload, frame_callbacks, presentation_feedbacks);
            self.discard_presentation_feedbacks(client, state, dispatcher, discarded);
            return;
        }

        let has_cached_commit = self.cached_commit.is_some();
        let (changed, needs_frame) = if has_cached_commit {
            let discarded =
                self.cache_commit_payload(payload, frame_callbacks, presentation_feedbacks);
            self.discard_presentation_feedbacks(client, state, dispatcher, discarded);
            let cached_commit = self.cached_commit.take().unwrap();
            self.apply_surface_update(
                client,
                state,
                dispatcher,
                cached_commit.payload,
                cached_commit.frame_callbacks,
                cached_commit.presentation_feedbacks,
                false,
            )
        } else {
            self.apply_surface_update(
                client,
                state,
                dispatcher,
                payload,
                frame_callbacks,
                presentation_feedbacks,
                false,
            )
        };

        if changed {
            log::debug!(
                "wl_surface commit scheduling RefreshSurfaceOutputs client={:?} surface={:?} changed={}",
                client.client_id(),
                self.id,
                changed
            );
            dispatcher.dispatch_to(
                client.client_id(),
                crate::clients::ClientAction::RefreshSurfaceOutputs,
            );
        }
        let is_mapped = self.global_logical_bounds().is_some();
        if was_mapped != is_mapped {
            if let Some(ShellSurfaceObject::XdgSurface(xdg_surface_id)) = self.shell_surface {
                let surface_object_id = client.object_id(self.id);
                let xdg_surface_object_id = client.object_id(xdg_surface_id);
                if state
                    .objects
                    .try_ref_object::<XdgSurface>(xdg_surface_object_id)
                    .is_some_and(XdgSurface::is_toplevel_parent)
                {
                    if is_mapped {
                        state.confirm_root_surface_mapped(surface_object_id);
                    }
                    dispatcher.dispatch(ClientsAction::WorkspaceStateChanged);
                }
            }
        }

        if changed || needs_frame {
            if let Some(bounds) = self.global_logical_bounds() {
                schedule_outputs(state, self.output_membership_ids(), dispatcher);
                schedule_outputs_for_rect(state, bounds, dispatcher);
            } else {
                schedule_outputs(state, self.output_membership_ids(), dispatcher);
                schedule_active_output(state, dispatcher);
            }
        }

        if matches!(self.shell_surface, Some(ShellSurfaceObject::LayerSurface(_))) {
            dispatcher.dispatch(ClientsAction::LayerShellLayoutChanged);
        }
    }

    fn validate_pending_sync(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) -> bool {
        if self.syncobj_surface_object.is_none() {
            return true;
        }

        let pending_sync = &self.pending.sync;
        let sync_present = !pending_sync.acquire.is_none() || !pending_sync.release.is_none();
        let Some(pending_buffer) = self.pending.buffer else {
            if sync_present {
                self.send_syncobj_surface_error(
                    client,
                    dispatcher,
                    wp_linux_drm_syncobj_surface_v1::Error::NoBuffer as u32,
                    "explicit sync points require a pending non-null buffer",
                );
                return false;
            }
            return true;
        };

        let Some(buffer_object_id) = pending_buffer else {
            if sync_present {
                self.send_syncobj_surface_error(
                    client,
                    dispatcher,
                    wp_linux_drm_syncobj_surface_v1::Error::NoBuffer as u32,
                    "explicit sync points require a pending non-null buffer",
                );
                return false;
            }
            return true;
        };

        let Some(buffer) = state.objects.try_ref_object::<WlBuffer>(buffer_object_id) else {
            let msg = if state.objects.contains(buffer_object_id) {
                "pending buffer object is not a wl_buffer"
            } else {
                "pending wl_buffer disappeared before commit"
            };
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                msg,
            );
            return false;
        };
        if !buffer.supports_explicit_sync() {
            self.send_syncobj_surface_error(
                client,
                dispatcher,
                wp_linux_drm_syncobj_surface_v1::Error::UnsupportedBuffer as u32,
                "explicit sync currently only supports dma-buf wl_buffers",
            );
            return false;
        }
        if pending_sync.acquire.is_none() {
            self.send_syncobj_surface_error(
                client,
                dispatcher,
                wp_linux_drm_syncobj_surface_v1::Error::NoAcquirePoint as u32,
                "missing acquire point for explicit-sync surface commit",
            );
            return false;
        }
        if pending_sync.release.is_none() {
            self.send_syncobj_surface_error(
                client,
                dispatcher,
                wp_linux_drm_syncobj_surface_v1::Error::NoReleasePoint as u32,
                "missing release point for explicit-sync surface commit",
            );
            return false;
        }
        if let SurfaceSyncState {
            acquire:
                sync::AcquireSync::TimelinePoint { timeline: acquire_timeline, point: acquire_point },
            release:
                sync::ReleasePoint::TimelinePoint { timeline: release_timeline, point: release_point },
        } = pending_sync
        {
            if acquire_timeline.handle() == release_timeline.handle()
                && acquire_point >= release_point
            {
                self.send_syncobj_surface_error(
                    client,
                    dispatcher,
                    wp_linux_drm_syncobj_surface_v1::Error::ConflictingPoints as u32,
                    "acquire timeline point must be strictly less than release point",
                );
                return false;
            }
        }

        true
    }

    fn flush_frame_callbacks(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let callbacks = std::mem::take(&mut self.pending_frame_callbacks);
        self.flush_frame_callbacks_list(callbacks, client, state, dispatcher);
    }

    fn route_frame_callbacks(&mut self, callbacks: Vec<WlCallbackId>) -> (Vec<WlCallbackId>, bool) {
        if callbacks.is_empty() {
            return (Vec::new(), false);
        }
        if let Some(blocked) = self.blocked.as_mut() {
            blocked.frame_callbacks.extend(callbacks);
            return (Vec::new(), false);
        }
        if self.on_screen.logical_size.is_some() {
            self.on_screen.frame_callbacks.extend(callbacks);
            return (Vec::new(), true);
        }
        (callbacks, false)
    }

    fn route_presentation_feedbacks(
        &mut self,
        feedbacks: Vec<SurfacePresentationFeedback>,
    ) -> (Vec<SurfacePresentationFeedback>, bool) {
        if feedbacks.is_empty() {
            return (Vec::new(), false);
        }
        if let Some(blocked) = self.blocked.as_mut() {
            let superseded = std::mem::replace(&mut blocked.presentation_feedbacks, feedbacks);
            return (superseded, false);
        }
        if self.on_screen.logical_size.is_some() {
            let superseded =
                std::mem::replace(&mut self.on_screen.presentation_feedbacks, feedbacks);
            return (superseded, true);
        }
        (feedbacks, false)
    }

    fn discard_presentation_feedbacks(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        feedbacks: Vec<SurfacePresentationFeedback>,
    ) {
        use crate::wayland_objects::wl_display::WlDisplay;

        for feedback in feedbacks {
            if !client.contains_object(feedback.id, state) {
                continue;
            }
            {
                let feedback_object = client.ref_object(feedback.id, state);
                feedback_object.send_discarded(dispatcher);
            }
            client.delete_id(&WlDisplay::default(), state, dispatcher, feedback.id.into());
        }
    }

    pub fn discard_all_presentation_feedbacks(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let pending_feedbacks = std::mem::take(&mut self.pending_presentation_feedbacks);
        self.discard_presentation_feedbacks(client, state, dispatcher, pending_feedbacks);
        let cached_feedbacks = self
            .cached_commit
            .as_mut()
            .map(|cached| std::mem::take(&mut cached.presentation_feedbacks))
            .unwrap_or_default();
        self.discard_presentation_feedbacks(client, state, dispatcher, cached_feedbacks);
        let on_screen_feedbacks = std::mem::take(&mut self.on_screen.presentation_feedbacks);
        self.discard_presentation_feedbacks(client, state, dispatcher, on_screen_feedbacks);
        let blocked_feedbacks = self
            .blocked
            .as_mut()
            .map(|blocked| std::mem::take(&mut blocked.presentation_feedbacks))
            .unwrap_or_default();
        self.discard_presentation_feedbacks(client, state, dispatcher, blocked_feedbacks);
    }

    pub fn discard_presentation_feedbacks_for_backend(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        backend: crate::state::BackendOutputRef,
    ) {
        let split_feedbacks = |feedbacks: &mut Vec<SurfacePresentationFeedback>, state: &State| {
            let mut retained = Vec::new();
            let mut discarded = Vec::new();
            for feedback in feedbacks.drain(..) {
                let matches_backend = match feedback.output_id {
                    Some(output_id) => state.backend_for_output(output_id) == Some(backend),
                    None => true,
                };
                if matches_backend {
                    discarded.push(feedback);
                } else {
                    retained.push(feedback);
                }
            }
            *feedbacks = retained;
            discarded
        };

        let pending_feedbacks = split_feedbacks(&mut self.pending_presentation_feedbacks, state);
        self.discard_presentation_feedbacks(client, state, dispatcher, pending_feedbacks);
        let cached_feedbacks = self
            .cached_commit
            .as_mut()
            .map(|cached| split_feedbacks(&mut cached.presentation_feedbacks, state))
            .unwrap_or_default();
        self.discard_presentation_feedbacks(client, state, dispatcher, cached_feedbacks);
        let on_screen_feedbacks =
            split_feedbacks(&mut self.on_screen.presentation_feedbacks, state);
        self.discard_presentation_feedbacks(client, state, dispatcher, on_screen_feedbacks);
        let blocked_feedbacks = self
            .blocked
            .as_mut()
            .map(|blocked| split_feedbacks(&mut blocked.presentation_feedbacks, state))
            .unwrap_or_default();
        self.discard_presentation_feedbacks(client, state, dispatcher, blocked_feedbacks);
    }

    pub fn render_state(&self) -> Option<SurfaceRenderState> {
        self.render_state_at(0, 0)
    }

    pub fn take_frame_callbacks(&mut self) -> Vec<WlCallbackId> {
        std::mem::take(&mut self.on_screen.frame_callbacks)
    }

    pub fn render_state_at(&self, base_x: i32, base_y: i32) -> Option<SurfaceRenderState> {
        let logical_size = self.on_screen.logical_size?;
        let x = base_x + self.on_screen.x;
        let y = base_y + self.on_screen.y;
        let local_bounds = RectI::new(0, 0, logical_size.width, logical_size.height)?;
        Some(SurfaceRenderState {
            surface_client_id: self.client_id,
            surface_id: self.id,
            buffer_object_id: self.on_screen.buffer_object_id?,
            x,
            y,
            width: logical_size.width,
            height: logical_size.height,
            tex_transform: self.on_screen.sample_transform,
            damage: self.on_screen.damage.translated(x, y),
            opaque_region: self
                .on_screen
                .opaque_region
                .to_region_clipped(local_bounds)
                .translated(x, y),
            sync: self.on_screen.sync.clone(),
            presentation_feedbacks: Vec::new(),
        })
    }

    pub fn take_render_state_at(&mut self, base_x: i32, base_y: i32) -> Option<SurfaceRenderState> {
        let mut state = self.render_state_at(base_x, base_y)?;
        state.presentation_feedbacks = std::mem::take(&mut self.on_screen.presentation_feedbacks);
        self.on_screen.damage.clear();
        Some(state)
    }

    pub fn accepts_input_at(&self, x: f64, y: f64) -> bool {
        self.accepts_input_at_with_origin(x, y, 0, 0)
    }

    pub fn accepts_input_at_with_origin(&self, x: f64, y: f64, base_x: i32, base_y: i32) -> bool {
        let Some(surface_state) = self.render_state_at(base_x, base_y) else {
            return false;
        };
        if x < surface_state.x as f64
            || y < surface_state.y as f64
            || x >= (surface_state.x + surface_state.width) as f64
            || y >= (surface_state.y + surface_state.height) as f64
        {
            return false;
        }
        let local_x = (x - surface_state.x as f64).floor() as i32;
        let local_y = (y - surface_state.y as f64).floor() as i32;
        self.on_screen.input_region.contains(local_x, local_y)
    }

    pub fn logical_size(&self) -> Option<SizeI> {
        self.on_screen.logical_size
    }

    pub fn global_logical_bounds(&self) -> Option<RectI> {
        let logical_size = self.logical_size()?;
        let (x, y) = self.root_position();
        RectI::new(x, y, logical_size.width, logical_size.height)
    }

    pub fn force_unmap_for_role_dismissal(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.discard_all_presentation_feedbacks(client, state, dispatcher);
        self.cancel_blocked(client, state, dispatcher);
        self.retire_current();
        self.on_screen.logical_size = None;
        self.on_screen.damage.clear();
        dispatcher
            .dispatch_to(client.client_id(), crate::clients::ClientAction::RefreshSurfaceOutputs);
    }

    pub fn take_retired_buffers(&mut self) -> Vec<RetiredBufferCandidate> {
        std::mem::take(&mut self.retired_buffers)
    }

    #[cfg(test)]
    pub(crate) fn test_map(
        &mut self,
        buffer_object_id: ObjectId,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) {
        self.on_screen = OnScreen {
            buffer_object_id: Some(buffer_object_id),
            content_revision: 1,
            x,
            y,
            sync: SurfaceSyncState::default(),
            buffer_transform: BufferTransform::Normal,
            buffer_scale: 1,
            viewport: SurfaceViewport::default(),
            logical_size: Some(SizeI { width, height }),
            sample_transform: transform::Mat3::identity(),
            damage: Self::full_damage_for_logical_size(SizeI { width, height }),
            frame_callbacks: Vec::new(),
            presentation_feedbacks: Vec::new(),
            input_region: RegionValue::infinite(),
            opaque_region: RegionValue::empty(),
        };
    }

    #[cfg(test)]
    pub(crate) fn test_set_input_region(&mut self, region: RegionValue) {
        self.on_screen.input_region = region;
    }

    pub fn activate_blocked_if_waiter_ready(
        &mut self,
        waiter_id: crate::sync::AcquireReadyWaiterId,
    ) -> bool {
        if self.blocked_waiter_id != Some(waiter_id) {
            return false;
        }
        let Some(blocked) = self.blocked.take() else {
            return false;
        };
        self.blocked_waiter_id = None;
        self.activate_blocked_state(blocked);
        true
    }

    fn install_blocked_surface(
        &mut self,
        next: BlockedSurfaceState,
        client_id: ClientId,
        surface_object_id: ObjectId,
        dispatcher: &mut ClientDispatcher,
    ) -> std::io::Result<()> {
        self.blocked = Some(next);
        let acquire = self.blocked.as_ref().unwrap().sync.acquire.clone();
        let Some(poll_fd) = acquire.prepare_waiter_fd()? else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "missing acquire fd for blocked surface",
            ));
        };
        if let Some(waiter_id) = self.blocked_waiter_id.take() {
            dispatcher.stop_model(waiter_id);
        }
        let waiter_id = sync::fresh_waiter_id();
        self.blocked_waiter_id = Some(waiter_id);
        dispatcher.start_model(
            waiter_id,
            AcquireReadyWaiter::new(waiter_id, client_id, surface_object_id, poll_fd),
        );
        Ok(())
    }

    fn activate_blocked_state(&mut self, next: BlockedSurfaceState) {
        let mut frame_callbacks = std::mem::take(&mut self.on_screen.frame_callbacks);
        frame_callbacks.extend(next.frame_callbacks.iter().copied());
        self.retire_current();
        self.on_screen = OnScreen {
            buffer_object_id: Some(next.buffer_object_id),
            content_revision: self.next_content_revision(),
            x: next.x,
            y: next.y,
            sync: next.sync,
            buffer_transform: next.buffer_transform,
            buffer_scale: next.buffer_scale,
            viewport: next.viewport,
            logical_size: Some(SizeI { width: next.width, height: next.height }),
            sample_transform: next.tex_transform,
            damage: Self::full_damage_for_logical_size(SizeI {
                width: next.width,
                height: next.height,
            }),
            frame_callbacks,
            presentation_feedbacks: next.presentation_feedbacks,
            input_region: next.input_region,
            opaque_region: next.opaque_region,
        };
    }

    fn retire_current(&mut self) {
        if let Some(buffer_object_id) = self.on_screen.buffer_object_id.take() {
            self.retired_buffers.push(RetiredBufferCandidate {
                buffer_object_id,
                sync: self.on_screen.sync.clone(),
            });
        }
        self.on_screen.sync = SurfaceSyncState::default();
        self.on_screen.logical_size = None;
        self.on_screen.sample_transform = transform::Mat3::identity();
        self.on_screen.damage.clear();
    }

    fn next_content_revision(&self) -> u64 {
        self.on_screen.content_revision.saturating_add(1).max(1)
    }

    fn cancel_blocked(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(waiter_id) = self.blocked_waiter_id.take() {
            dispatcher.stop_model(waiter_id);
        }
        let Some(blocked) = self.blocked.take() else {
            return;
        };
        self.discard_presentation_feedbacks(
            client,
            state,
            dispatcher,
            blocked.presentation_feedbacks,
        );
        sync::resolve_unused_candidate_immediately(
            client,
            state,
            dispatcher,
            RetiredBufferCandidate {
                buffer_object_id: blocked.buffer_object_id,
                sync: blocked.sync,
            },
        );
    }

    pub fn cancel_blocked_if_waiter_matches(
        &mut self,
        waiter_id: crate::sync::AcquireReadyWaiterId,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if self.blocked_waiter_id != Some(waiter_id) {
            return;
        }
        self.cancel_blocked(client, state, dispatcher);
    }

    pub fn apply_cached_if_parent_desynchronized(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) -> bool {
        if self.sync_mode() != Some(SubsurfaceSyncMode::Desync)
            || self.parent_behaves_synchronized(state)
        {
            return false;
        }
        let changed = self.apply_cached_commit(client, state, dispatcher, false);
        if changed {
            if let Some(bounds) = self.global_logical_bounds() {
                schedule_outputs(state, self.output_membership_ids(), dispatcher);
                schedule_outputs_for_rect(state, bounds, dispatcher);
            } else {
                schedule_outputs(state, self.output_membership_ids(), dispatcher);
                schedule_active_output(state, dispatcher);
            }
        }
        changed
    }

    fn detach_subsurface_tree(
        child_surface_object_id: ObjectId,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let (grandchildren, subsurface_object_id) = {
            let Some(child_surface) = state
                .objects
                .try_ref_object::<WlSurface>(child_surface_object_id)
            else {
                return;
            };
            (child_surface.child_surface_ids(), child_surface.subsurface_object())
        };

        for grandchild_surface_object_id in grandchildren {
            Self::detach_subsurface_tree(grandchild_surface_object_id, client, state, dispatcher);
        }

        {
            let Some(mut child_surface) = state
                .objects
                .try_grab_object::<WlSurface>(child_surface_object_id)
            else {
                return;
            };
            child_surface.unmap_due_to_parent_loss(client, state, dispatcher);
            child_surface.destroy_subsurface_role();
        }

        if let Some(subsurface_object_id) = subsurface_object_id {
            use crate::wayland_objects::wl_display::WlDisplay;
            if client.contains_object(subsurface_object_id, state) {
                client.delete_id(
                    &WlDisplay::default(),
                    state,
                    dispatcher,
                    subsurface_object_id.into(),
                );
            }
        }

        state.register_root_surface(child_surface_object_id);
    }

    pub fn delete_self(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let object_id = client.object_id(self.id);
        state.seat.clear_cursor_surface(object_id);
        self.discard_all_presentation_feedbacks(client, state, dispatcher);
        for child_surface_object_id in self.child_surface_ids() {
            Self::detach_subsurface_tree(child_surface_object_id, client, state, dispatcher);
        }
        if let Some(parent_surface_object_id) = self.parent_surface_object_id() {
            if let Some(mut parent_surface) = state
                .objects
                .try_grab_object::<WlSurface>(parent_surface_object_id)
            {
                parent_surface.remove_child_from_tree(client.object_id(self.id));
            }
        }
        self.cleanup_extension_objects(client, state, dispatcher);
        state.unregister_root_surface(object_id);
        use crate::wayland_objects::wl_display::WlDisplay;
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
        if let Some(bounds) = self.global_logical_bounds() {
            schedule_outputs(state, self.output_membership_ids(), dispatcher);
            schedule_outputs_for_rect(state, bounds, dispatcher);
        } else {
            schedule_outputs(state, self.output_membership_ids(), dispatcher);
            schedule_active_output(state, dispatcher);
        }
    }

    pub fn client_id(&self) -> ClientId {
        self.client_id
    }

    pub fn surface_id(&self) -> WlSurfaceId {
        self.id
    }

    pub fn queue_presentation_feedback(
        &mut self,
        feedback: WpPresentationFeedbackId,
        version: u32,
        output_id: Option<OutputId>,
    ) {
        self.pending_presentation_feedbacks
            .push(SurfacePresentationFeedback { id: feedback, version, output_id });
    }
}

impl Object for WlSurface {
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
        use wl_surface::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => self.delete_self(client, state, dispatcher),
            Attach { buffer, x, y } => {
                self.on_attach(client, state, buffer, x, y, dispatcher);
            }
            Damage { x, y, width, height } => {
                self.pending
                    .damage_surface
                    .push(DamageRect { x, y, width, height });
            }
            Frame { callback } => {
                client.insert_object(state, WlCallback::new(callback));
                self.pending_frame_callbacks.push(callback);
            }
            SetOpaqueRegion { region } => {
                self.on_set_opaque_region(client, state, region, dispatcher);
            }
            SetInputRegion { region } => {
                self.on_set_input_region(client, state, region, dispatcher);
            }
            Commit => {
                self.on_commit(client, state, dispatcher);
            }
            SetBufferTransform { transform } => {
                if self.version < wl_surface::SET_BUFFER_TRANSFORM_SINCE {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidMethod as u32,
                        "wl_surface.set_buffer_transform requires wl_surface version >= 2",
                    );
                    return;
                }
                let Some(transform) = BufferTransform::from_wayland(transform) else {
                    self.send_error(
                        dispatcher,
                        wl_surface::Error::InvalidTransform as u32,
                        "invalid wl_surface buffer transform",
                    );
                    return;
                };
                self.pending.buffer_transform = transform;
            }
            SetBufferScale { scale } => {
                if self.version < wl_surface::SET_BUFFER_SCALE_SINCE {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidMethod as u32,
                        "wl_surface.set_buffer_scale requires wl_surface version >= 3",
                    );
                    return;
                }
                if scale < 1 {
                    self.send_error(
                        dispatcher,
                        wl_surface::Error::InvalidScale as u32,
                        "wl_surface buffer scale must be at least 1",
                    );
                    return;
                }
                self.pending.buffer_scale = scale;
            }
            DamageBuffer { x, y, width, height } => {
                if self.version < wl_surface::DAMAGE_BUFFER_SINCE {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidMethod as u32,
                        "wl_surface.damage_buffer requires wl_surface version >= 4",
                    );
                    return;
                }
                self.pending
                    .damage_buffer
                    .push(DamageRect { x, y, width, height });
            }
            Offset { x, y } => {
                if self.version < wl_surface::OFFSET_SINCE {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidMethod as u32,
                        "wl_surface.offset requires wl_surface version >= 5",
                    );
                    return;
                }
                if self.role != Role::Subsurface {
                    self.pending.offset = (x, y);
                }
            }
        }
    }

    fn on_client_disconnect(
        &mut self,
        _client: &mut Client,
        _state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(waiter_id) = self.blocked_waiter_id.take() {
            dispatcher.stop_model(waiter_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::{
            DamageRect, OnScreen, Role, SurfaceCommitPayload, SurfacePresentationFeedback,
            WlSurface,
        },
        crate::{
            clients::{Client, ClientId, ClientsAction, RawEvent, RawRequest},
            cursor::CursorSourceRef,
            drm::ARGB8888,
            egl::EGL,
            globals::Globals,
            io_uring::model::IoUringAction,
            object::Object,
            protocol_generated::{
                xdg_wm_base, WlBufferId, WlSubsurfaceId, WlSurfaceId, WpFractionalScaleV1Id,
                WpLinuxDrmSyncobjSurfaceV1Id, WpPresentationFeedbackId, WpViewportId, XdgSurfaceId,
                XdgToplevelId, XdgWmBaseId,
            },
            region::{DamageRegion, RectI, RegionSpec, RegionValue},
            runner::Dispatcher,
            state::{
                BackendOutputRef, DrmId, FractionalScale, ObjectId, Output, OutputMode, State,
            },
            sync::SurfaceSyncState,
            transform::{self, BufferTransform, SizeI, SurfaceViewport},
            wayland_objects::{
                wl_buffer::{NewWlBufferParams, WlBuffer},
                wl_shm_pool::shared_memory_from_bytes,
                wp_fractional_scale_v1::WpFractionalScaleV1,
                wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1,
                wp_viewport::WpViewport,
                xdg_surface::{xdg_toplevel::XdgToplevel, XdgSurface},
            },
            xkb::Xkb,
        },
    };

    fn make_surface() -> WlSurface {
        let mut surface = WlSurface::new(WlSurfaceId::from(1), ClientId::from_raw(7), 6);
        surface.on_screen = OnScreen {
            buffer_object_id: Some(ObjectId::from(1usize)),
            content_revision: 1,
            x: 10,
            y: 20,
            sync: SurfaceSyncState::default(),
            buffer_transform: BufferTransform::Normal,
            buffer_scale: 1,
            viewport: SurfaceViewport::default(),
            logical_size: Some(SizeI { width: 100, height: 80 }),
            sample_transform: transform::Mat3::identity(),
            damage: DamageRegion::new(),
            frame_callbacks: Vec::new(),
            presentation_feedbacks: Vec::new(),
            input_region: RegionValue::infinite(),
            opaque_region: RegionValue::empty(),
        };
        surface
    }

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

    fn make_output(
        backend: BackendOutputRef,
        drm_crtc_id: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Output {
        Output {
            backend,
            drm_crtc_id,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x,
            y,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: format!("output-{drm_crtc_id}"),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width, height, refresh_millihz: 60_000 },
            preferred_mode: None,
        }
    }

    fn insert_surface(state: &mut State, surface: WlSurface) -> ObjectId {
        let object_id = state.insert_object(surface).1;
        state.register_root_surface(object_id);
        object_id
    }

    fn attach_fractional_scale_object(state: &mut State, surface_object_id: ObjectId) -> ObjectId {
        let object_id = state
            .insert_object(WpFractionalScaleV1::new(
                WpFractionalScaleV1Id::from(77),
                surface_object_id,
            ))
            .1;
        let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
        surface.set_fractional_scale_object(object_id).unwrap();
        object_id
    }

    fn make_client() -> (Client, Dispatcher) {
        (Client::new_for_tests(ClientId::from_raw(9)), Dispatcher::default())
    }

    fn region_value(x: i32, y: i32, width: i32, height: i32) -> RegionValue {
        let mut spec = RegionSpec::default();
        spec.add_rect(RectI::new(x, y, width, height).unwrap());
        RegionValue::from_spec(spec)
    }

    fn feedback(id: u32, version: u32) -> SurfacePresentationFeedback {
        SurfacePresentationFeedback {
            id: WpPresentationFeedbackId::from(id),
            version,
            output_id: None,
        }
    }

    fn get_toplevel_request(
        xdg_surface_id: XdgSurfaceId,
        toplevel_id: XdgToplevelId,
    ) -> RawRequest<'static> {
        let mut event = RawEvent::new(xdg_surface_id.into(), 1, 1);
        event.id(toplevel_id);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn ack_configure_request(xdg_surface_id: XdgSurfaceId, serial: u32) -> RawRequest<'static> {
        let mut event = RawEvent::new(xdg_surface_id.into(), 4, 1);
        event.uint(serial);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn insert_test_buffer(state: &mut State, client_id: ClientId) -> ObjectId {
        let bytes = [0u8; 16];
        state
            .insert_object(
                WlBuffer::new(NewWlBufferParams {
                    id: WlBufferId::from(90),
                    owner_client_id: client_id,
                    offset: 0,
                    width: 2,
                    height: 2,
                    stride: 8,
                    format: &ARGB8888,
                    mem: shared_memory_from_bytes(&bytes),
                })
                .unwrap(),
            )
            .1
    }

    fn create_root_xdg_toplevel(
        state: &mut State,
        client: &mut Client,
        dispatcher: &mut Dispatcher,
        surface_id: WlSurfaceId,
        xdg_surface_id: XdgSurfaceId,
        toplevel_id: XdgToplevelId,
    ) -> ObjectId {
        let mut client_dispatcher = client.test_dispatcher(dispatcher);
        let ((_, surface_object_id), _) =
            client.insert_object(state, WlSurface::new(surface_id, client.client_id(), 6));
        state.register_root_surface(surface_object_id);
        {
            let surface = state
                .objects
                .get_mut(surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            assert!(surface
                .set_role(Role::XdgSurface, &mut client_dispatcher)
                .is_ok());
            surface.set_xdg_surface_impl(xdg_surface_id);
        }
        client.insert_object(
            state,
            XdgSurface::new(xdg_surface_id, XdgWmBaseId::from(1), surface_id, 7),
        );
        {
            let mut xdg_surface = client.grab_object(xdg_surface_id, state);
            xdg_surface.on(
                client,
                state,
                get_toplevel_request(xdg_surface_id, toplevel_id),
                &mut client_dispatcher,
            );
        }
        surface_object_id
    }

    fn commit_initial_xdg_toplevel(
        state: &mut State,
        client: &mut Client,
        dispatcher: &mut Dispatcher,
        surface_object_id: ObjectId,
        xdg_surface_id: XdgSurfaceId,
    ) {
        let mut client_dispatcher = client.test_dispatcher(dispatcher);
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            surface.on_commit(client, state, &mut client_dispatcher);
        }
        {
            let xdg_surface_object_id = client.object_id(xdg_surface_id);
            let mut xdg_surface = state
                .objects
                .grab_object::<XdgSurface>(xdg_surface_object_id);
            xdg_surface.on(
                client,
                state,
                ack_configure_request(xdg_surface_id, 1),
                &mut client_dispatcher,
            );
        }
    }

    #[test]
    fn cursor_role_conflicts_with_xdg_role() {
        let mut surface = WlSurface::new(WlSurfaceId::from(1), ClientId::from_raw(3), 6);
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        assert!(surface.set_cursor_role(&mut client_dispatcher).is_ok());
        assert!(surface.has_cursor_role());
        assert_eq!(
            surface.set_role(Role::XdgSurface, &mut client_dispatcher),
            Err(xdg_wm_base::Error::Role)
        );
    }

    #[test]
    fn deleting_active_cursor_surface_clears_seat_cursor_state() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let ((_, object_id), _) = client
            .insert_object(&mut state, WlSurface::new(WlSurfaceId::from(5), client.client_id(), 6));
        state.register_root_surface(object_id);
        let surface_id = WlSurfaceId::from(5);

        assert!(state
            .seat
            .set_cursor_surface(client.client_id(), surface_id, object_id, 1, 2,));
        assert!(matches!(
            state.seat.visible_cursor_source(),
            Some(CursorSourceRef::Surface { owner_client_id, cursor })
                if owner_client_id == client.client_id() && cursor.object_id == object_id
        ));

        let mut surface = state.objects.grab_object::<WlSurface>(object_id);
        surface.delete_self(&mut client, &mut state, &mut client_dispatcher);

        assert!(state.seat.visible_cursor_source().is_none());
    }

    #[test]
    fn deleting_surface_removes_surface_extension_objects() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let ((_, surface_object_id), _) = client
            .insert_object(&mut state, WlSurface::new(WlSurfaceId::from(5), client.client_id(), 6));
        state.register_root_surface(surface_object_id);

        let fractional_id = WpFractionalScaleV1Id::from(6);
        let ((_, fractional_object_id), _) = client
            .insert_object(&mut state, WpFractionalScaleV1::new(fractional_id, surface_object_id));
        let viewport_id = WpViewportId::from(7);
        let ((_, viewport_object_id), _) =
            client.insert_object(&mut state, WpViewport::new(viewport_id, surface_object_id));
        let syncobj_surface_id = WpLinuxDrmSyncobjSurfaceV1Id::from(8);
        let ((_, syncobj_surface_object_id), _) = client.insert_object(
            &mut state,
            WpLinuxDrmSyncobjSurfaceV1::new(syncobj_surface_id, surface_object_id),
        );

        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            surface
                .set_fractional_scale_object(fractional_object_id)
                .unwrap();
            surface.set_viewport_object(viewport_id).unwrap();
            surface
                .set_syncobj_surface_object(syncobj_surface_id)
                .unwrap();
            surface.delete_self(&mut client, &mut state, &mut client_dispatcher);
        }
        drop(client_dispatcher);

        assert!(!client.contains_object(WlSurfaceId::from(5), &state));
        assert!(!client.contains_object(fractional_id, &state));
        assert!(!client.contains_object(viewport_id, &state));
        assert!(!client.contains_object(syncobj_surface_id, &state));
        assert!(state
            .objects
            .try_ref_object::<WpFractionalScaleV1>(fractional_object_id)
            .is_none());
        assert!(state
            .objects
            .try_ref_object::<WpViewport>(viewport_object_id)
            .is_none());
        assert!(state
            .objects
            .try_ref_object::<WpLinuxDrmSyncobjSurfaceV1>(syncobj_surface_object_id)
            .is_none());
    }

    #[test]
    fn mapping_root_xdg_toplevel_schedules_workspace_relayout() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let _output_id = state.insert_output(make_output(backend, 1, 0, 0, 1280, 720));
        let (mut client, mut dispatcher) = make_client();

        let surface_id = WlSurfaceId::from(50);
        let xdg_surface_id = XdgSurfaceId::from(60);
        let toplevel_id = XdgToplevelId::from(70);
        let surface_object_id = create_root_xdg_toplevel(
            &mut state,
            &mut client,
            &mut dispatcher,
            surface_id,
            xdg_surface_id,
            toplevel_id,
        );
        commit_initial_xdg_toplevel(
            &mut state,
            &mut client,
            &mut dispatcher,
            surface_object_id,
            xdg_surface_id,
        );
        let buffer_object_id = insert_test_buffer(&mut state, client.client_id());
        {
            let surface = state
                .objects
                .get_mut(surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.pending.buffer = Some(Some(buffer_object_id));
        }

        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            surface.on_commit(&mut client, &mut state, &mut client_dispatcher);
        }

        assert!(dispatcher.test_has_queued_action::<ClientsAction>(|action| {
            matches!(action, ClientsAction::WorkspaceStateChanged)
        }));
    }

    #[test]
    fn tiled_root_initial_configure_uses_tile_size() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(2usize));
        let _output_id = state.insert_output(make_output(backend, 1, 0, 0, 1280, 720));
        let (mut client, mut dispatcher) = make_client();

        let surface_object_id = create_root_xdg_toplevel(
            &mut state,
            &mut client,
            &mut dispatcher,
            WlSurfaceId::from(80),
            XdgSurfaceId::from(81),
            XdgToplevelId::from(82),
        );

        commit_initial_xdg_toplevel(
            &mut state,
            &mut client,
            &mut dispatcher,
            surface_object_id,
            XdgSurfaceId::from(81),
        );

        let toplevel = state
            .objects
            .try_ref_object::<XdgToplevel>(client.object_id(XdgToplevelId::from(82)))
            .unwrap();
        assert_eq!(toplevel.test_current_size(), (1280, 720));
        let states = toplevel.test_current_states();
        assert!(
            states.contains(&(crate::protocol_generated::xdg_toplevel::State::TiledLeft as u32))
        );
        assert!(
            states.contains(&(crate::protocol_generated::xdg_toplevel::State::TiledRight as u32))
        );
        assert!(states.contains(&(crate::protocol_generated::xdg_toplevel::State::TiledTop as u32)));
        assert!(
            states.contains(&(crate::protocol_generated::xdg_toplevel::State::TiledBottom as u32))
        );
        assert!(state
            .managed_root_surface_state(surface_object_id)
            .is_some_and(|managed| managed.pending_initial_layout));
    }

    #[test]
    fn floating_root_initial_configure_stays_generic() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(3usize));
        let _output_id = state.insert_output(make_output(backend, 1, 0, 0, 1280, 720));
        let (mut client, mut dispatcher) = make_client();

        let surface_object_id = create_root_xdg_toplevel(
            &mut state,
            &mut client,
            &mut dispatcher,
            WlSurfaceId::from(90),
            XdgSurfaceId::from(91),
            XdgToplevelId::from(92),
        );
        assert!(state.set_root_surface_floating(surface_object_id, true));

        commit_initial_xdg_toplevel(
            &mut state,
            &mut client,
            &mut dispatcher,
            surface_object_id,
            XdgSurfaceId::from(91),
        );

        let toplevel = state
            .objects
            .try_ref_object::<XdgToplevel>(client.object_id(XdgToplevelId::from(92)))
            .unwrap();
        assert_eq!(toplevel.test_current_size(), (0, 0));
        let states = toplevel.test_current_states();
        assert!(
            !states.contains(&(crate::protocol_generated::xdg_toplevel::State::TiledLeft as u32))
        );
        assert!(!state
            .managed_root_surface_state(surface_object_id)
            .is_some_and(|managed| managed.pending_initial_layout));
    }

    #[test]
    fn fullscreen_root_initial_configure_uses_fullscreen_layout() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(4usize));
        let _output_id = state.insert_output(make_output(backend, 1, 0, 0, 1280, 720));
        let (mut client, mut dispatcher) = make_client();

        let surface_object_id = create_root_xdg_toplevel(
            &mut state,
            &mut client,
            &mut dispatcher,
            WlSurfaceId::from(100),
            XdgSurfaceId::from(101),
            XdgToplevelId::from(102),
        );
        assert!(state.set_root_surface_fullscreen(surface_object_id, true));

        commit_initial_xdg_toplevel(
            &mut state,
            &mut client,
            &mut dispatcher,
            surface_object_id,
            XdgSurfaceId::from(101),
        );

        let toplevel = state
            .objects
            .try_ref_object::<XdgToplevel>(client.object_id(XdgToplevelId::from(102)))
            .unwrap();
        assert_eq!(toplevel.test_current_size(), (1280, 720));
        let states = toplevel.test_current_states();
        assert!(
            states.contains(&(crate::protocol_generated::xdg_toplevel::State::Fullscreen as u32))
        );
    }

    #[test]
    fn first_map_clears_pending_initial_layout_state() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(5usize));
        let _output_id = state.insert_output(make_output(backend, 1, 0, 0, 1280, 720));
        let (mut client, mut dispatcher) = make_client();

        let surface_object_id = create_root_xdg_toplevel(
            &mut state,
            &mut client,
            &mut dispatcher,
            WlSurfaceId::from(110),
            XdgSurfaceId::from(111),
            XdgToplevelId::from(112),
        );
        commit_initial_xdg_toplevel(
            &mut state,
            &mut client,
            &mut dispatcher,
            surface_object_id,
            XdgSurfaceId::from(111),
        );
        assert!(state
            .managed_root_surface_state(surface_object_id)
            .is_some_and(|managed| managed.pending_initial_layout));

        let buffer_object_id = insert_test_buffer(&mut state, client.client_id());
        {
            let surface = state
                .objects
                .get_mut(surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.pending.buffer = Some(Some(buffer_object_id));
        }
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            surface.on_commit(&mut client, &mut state, &mut client_dispatcher);
        }

        assert!(!state
            .managed_root_surface_state(surface_object_id)
            .is_some_and(|managed| managed.pending_initial_layout));
    }

    #[test]
    fn stale_pending_root_does_not_shrink_next_initial_configure() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(6usize));
        let _output_id = state.insert_output(make_output(backend, 1, 0, 0, 1280, 720));
        let (mut client, mut dispatcher) = make_client();

        let first_surface_object_id = create_root_xdg_toplevel(
            &mut state,
            &mut client,
            &mut dispatcher,
            WlSurfaceId::from(120),
            XdgSurfaceId::from(121),
            XdgToplevelId::from(122),
        );
        commit_initial_xdg_toplevel(
            &mut state,
            &mut client,
            &mut dispatcher,
            first_surface_object_id,
            XdgSurfaceId::from(121),
        );
        assert!(state
            .managed_root_surface_state(first_surface_object_id)
            .is_some_and(|managed| managed.pending_initial_layout));

        let second_surface_object_id = create_root_xdg_toplevel(
            &mut state,
            &mut client,
            &mut dispatcher,
            WlSurfaceId::from(130),
            XdgSurfaceId::from(131),
            XdgToplevelId::from(132),
        );
        commit_initial_xdg_toplevel(
            &mut state,
            &mut client,
            &mut dispatcher,
            second_surface_object_id,
            XdgSurfaceId::from(131),
        );

        let second_toplevel = state
            .objects
            .try_ref_object::<XdgToplevel>(client.object_id(XdgToplevelId::from(132)))
            .unwrap();
        assert_eq!(second_toplevel.test_current_size(), (1280, 720));
        let states = second_toplevel.test_current_states();
        assert!(
            states.contains(&(crate::protocol_generated::xdg_toplevel::State::TiledLeft as u32))
        );
        assert!(
            states.contains(&(crate::protocol_generated::xdg_toplevel::State::TiledRight as u32))
        );
        assert!(states.contains(&(crate::protocol_generated::xdg_toplevel::State::TiledTop as u32)));
        assert!(
            states.contains(&(crate::protocol_generated::xdg_toplevel::State::TiledBottom as u32))
        );
    }

    #[test]
    fn surface_defaults_match_region_protocol_defaults() {
        let surface = WlSurface::new(WlSurfaceId::from(1), ClientId::from_raw(1), 6);
        assert_eq!(surface.on_screen.input_region, RegionValue::infinite());
        assert_eq!(surface.on_screen.opaque_region, RegionValue::empty());
    }

    #[test]
    fn hit_testing_uses_committed_input_region_not_pending_region() {
        let mut surface = make_surface();
        surface.pending.input_region = Some(region_value(0, 0, 10, 10));

        assert!(surface.accepts_input_at(50.0, 50.0));

        surface.on_screen.input_region = surface.pending.input_region.take().unwrap();

        assert!(surface.accepts_input_at(15.0, 25.0));
        assert!(!surface.accepts_input_at(50.0, 50.0));
    }

    #[test]
    fn committed_input_region_uses_surface_local_coordinates() {
        let mut surface = make_surface();
        surface.on_screen.input_region = region_value(5, 5, 10, 10);

        assert!(surface.accepts_input_at(15.0, 25.0));
        assert!(!surface.accepts_input_at(30.0, 25.0));
    }

    #[test]
    fn configure_bounds_falls_back_to_current_output_without_surface_geometry() {
        let mut state = make_state();
        let output_id = state.insert_output(make_output(
            BackendOutputRef::new(DrmId::from(1usize)),
            1,
            0,
            0,
            200,
            120,
        ));
        let surface = WlSurface::new(WlSurfaceId::from(3), ClientId::from_raw(4), 6);

        assert_eq!(surface.preferred_output_id(&state), Some(output_id));
        assert_eq!(surface.configure_bounds(&state), Some((200, 120)));
    }

    #[test]
    fn configure_bounds_for_object_works_while_surface_is_grabbed() {
        let mut state = make_state();
        let output_id = state.insert_output(make_output(
            BackendOutputRef::new(DrmId::from(1usize)),
            1,
            0,
            0,
            200,
            120,
        ));
        let (_, surface_object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(11u32), ClientId::from_raw(4), 6));

        let surface = state.objects.grab_object::<WlSurface>(surface_object_id);
        assert_eq!(
            surface.preferred_output_id_for_object(&state, surface_object_id),
            Some(output_id)
        );
        assert_eq!(
            surface.configure_bounds_for_object(&state, surface_object_id),
            Some((200, 120))
        );
    }

    #[test]
    fn preferred_buffer_state_sends_v6_hints_once_per_change() {
        let mut state = make_state();
        let output_id = state.insert_output(make_output(
            BackendOutputRef::new(DrmId::from(1usize)),
            1,
            0,
            0,
            200,
            120,
        ));
        {
            let output = state.outputs.try_get_mut(output_id).unwrap();
            output.scale = FractionalScale::from_integer(2);
            output.transform = 3;
        }
        let mut surface = WlSurface::new(WlSurfaceId::from(9), ClientId::from_raw(6), 6);
        let (client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        surface.send_preferred_buffer_state(&state, &mut client_dispatcher);
        drop(client_dispatcher);

        assert_eq!(surface.preferred_buffer_scale_sent, Some(2));
        assert_eq!(surface.preferred_buffer_transform_sent, Some(3));
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        surface.send_preferred_buffer_state(&state, &mut client_dispatcher);
        drop(client_dispatcher);

        assert!(!dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));
    }

    #[test]
    fn preferred_fractional_scale_sends_once_per_scale_value() {
        let mut state = make_state();
        let output_id = state.insert_output(make_output(
            BackendOutputRef::new(DrmId::from(1usize)),
            1,
            0,
            0,
            200,
            120,
        ));
        state.outputs.try_get_mut(output_id).unwrap().scale = FractionalScale::ONE_POINT_FIVE;

        let surface_object_id = insert_surface(
            &mut state,
            WlSurface::new(WlSurfaceId::from(10), ClientId::from_raw(6), 6),
        );
        let fractional_object_id = attach_fractional_scale_object(&mut state, surface_object_id);

        let mut surface = WlSurface::new(WlSurfaceId::from(10), ClientId::from_raw(6), 6);
        surface.update_output_membership(vec![output_id]);
        surface.fractional_scale_object = Some(fractional_object_id);

        let (client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        surface.send_preferred_fractional_scale(&state, &mut client_dispatcher);
        drop(client_dispatcher);

        assert_eq!(
            surface.preferred_fractional_scale_sent,
            Some(FractionalScale::ONE_POINT_FIVE.as_v120())
        );
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));

        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        surface.send_preferred_fractional_scale(&state, &mut client_dispatcher);
        drop(client_dispatcher);

        assert!(!dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));
    }

    #[test]
    fn preferred_fractional_scale_updates_when_surface_moves_outputs() {
        let mut state = make_state();
        let left_output = state.insert_output(make_output(
            BackendOutputRef::new(DrmId::from(1usize)),
            1,
            0,
            0,
            200,
            120,
        ));
        let right_output = state.insert_output(make_output(
            BackendOutputRef::new(DrmId::from(1usize)),
            2,
            200,
            0,
            200,
            120,
        ));
        state.outputs.try_get_mut(left_output).unwrap().scale = FractionalScale::ONE;
        state.outputs.try_get_mut(right_output).unwrap().scale = FractionalScale::TWO;

        let surface_object_id = insert_surface(
            &mut state,
            WlSurface::new(WlSurfaceId::from(11), ClientId::from_raw(7), 6),
        );
        let fractional_object_id = attach_fractional_scale_object(&mut state, surface_object_id);

        let mut surface = WlSurface::new(WlSurfaceId::from(11), ClientId::from_raw(7), 6);
        surface.fractional_scale_object = Some(fractional_object_id);
        surface.update_output_membership(vec![left_output]);

        let (client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        surface.send_preferred_fractional_scale(&state, &mut client_dispatcher);
        drop(client_dispatcher);
        assert_eq!(surface.preferred_fractional_scale_sent, Some(FractionalScale::ONE.as_v120()));

        let mut dispatcher = Dispatcher::default();
        surface.update_output_membership(vec![right_output]);
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        surface.send_preferred_fractional_scale(&state, &mut client_dispatcher);
        drop(client_dispatcher);

        assert_eq!(surface.preferred_fractional_scale_sent, Some(FractionalScale::TWO.as_v120()));
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));
    }

    #[test]
    fn pending_null_like_regions_can_be_committed_explicitly() {
        let mut surface = make_surface();
        surface.pending.input_region = Some(RegionValue::infinite());
        surface.pending.opaque_region = Some(RegionValue::empty());

        surface.on_screen.input_region = surface.pending.input_region.take().unwrap();
        surface.on_screen.opaque_region = surface.pending.opaque_region.take().unwrap();

        assert!(surface.accepts_input_at(50.0, 50.0));
        assert_eq!(surface.on_screen.opaque_region, RegionValue::empty());
    }

    #[test]
    fn parent_commit_applies_pending_child_positions_and_stack_order() {
        let mut state = make_state();
        let parent_object_id = insert_surface(
            &mut state,
            WlSurface::new(WlSurfaceId::from(1), ClientId::from_raw(1), 6),
        );
        let child_a_object_id = insert_surface(
            &mut state,
            WlSurface::new(WlSurfaceId::from(2), ClientId::from_raw(1), 6),
        );
        let child_b_object_id = insert_surface(
            &mut state,
            WlSurface::new(WlSurfaceId::from(3), ClientId::from_raw(1), 6),
        );

        {
            let parent = state
                .objects
                .get_mut(parent_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            parent.add_pending_child(child_a_object_id);
            parent.add_pending_child(child_b_object_id);
        }
        {
            let child_a = state
                .objects
                .get_mut(child_a_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_a.assign_subsurface(
                crate::protocol_generated::WlSubsurfaceId::from(10),
                parent_object_id,
            );
            child_a.set_subsurface_position(20, 30);
        }
        {
            let child_b = state
                .objects
                .get_mut(child_b_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_b.assign_subsurface(
                crate::protocol_generated::WlSubsurfaceId::from(11),
                parent_object_id,
            );
            child_b.set_subsurface_position(-5, 7);
        }

        {
            let parent = state
                .objects
                .get_mut(parent_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            assert!(parent.restack_child_relative(child_a_object_id, None, false));
        }
        {
            let mut parent = state.objects.grab_object::<WlSurface>(parent_object_id);
            assert!(parent.apply_pending_tree_state(&mut state));
            assert_eq!(
                parent.current_stack(),
                &[
                    super::StackEntry::Child(child_b_object_id),
                    super::StackEntry::Child(child_a_object_id),
                    super::StackEntry::Parent,
                ]
            );
        }

        let child_a = state
            .objects
            .get(child_a_object_id)
            .downcast_ref::<WlSurface>()
            .unwrap();
        let child_b = state
            .objects
            .get(child_b_object_id)
            .downcast_ref::<WlSurface>()
            .unwrap();
        assert_eq!(child_a.current_subsurface_position(), (20, 30));
        assert_eq!(child_b.current_subsurface_position(), (-5, 7));
    }

    #[test]
    fn synchronized_behavior_inherits_from_sync_ancestors() {
        let mut state = make_state();
        let root_object_id = insert_surface(
            &mut state,
            WlSurface::new(WlSurfaceId::from(1), ClientId::from_raw(1), 6),
        );
        let child_object_id = insert_surface(
            &mut state,
            WlSurface::new(WlSurfaceId::from(2), ClientId::from_raw(1), 6),
        );
        let grandchild_object_id = insert_surface(
            &mut state,
            WlSurface::new(WlSurfaceId::from(3), ClientId::from_raw(1), 6),
        );

        {
            let child = state
                .objects
                .get_mut(child_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child.assign_subsurface(
                crate::protocol_generated::WlSubsurfaceId::from(20),
                root_object_id,
            );
            child.set_subsurface_sync_mode(true);
        }
        {
            let grandchild = state
                .objects
                .get_mut(grandchild_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            grandchild.assign_subsurface(
                crate::protocol_generated::WlSubsurfaceId::from(21),
                child_object_id,
            );
            grandchild.set_subsurface_sync_mode(false);
        }

        let child = state
            .objects
            .get(child_object_id)
            .downcast_ref::<WlSurface>()
            .unwrap();
        let grandchild = state
            .objects
            .get(grandchild_object_id)
            .downcast_ref::<WlSurface>()
            .unwrap();
        assert!(child.behaves_synchronized(&state));
        assert!(grandchild.behaves_synchronized(&state));

        {
            let child = state
                .objects
                .get_mut(child_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child.set_subsurface_sync_mode(false);
        }

        let child = state
            .objects
            .get(child_object_id)
            .downcast_ref::<WlSurface>()
            .unwrap();
        let grandchild = state
            .objects
            .get(grandchild_object_id)
            .downcast_ref::<WlSurface>()
            .unwrap();
        assert!(!child.behaves_synchronized(&state));
        assert!(!grandchild.behaves_synchronized(&state));
    }

    #[test]
    fn restack_relative_to_sibling_updates_pending_order() {
        let mut state = make_state();
        let parent_object_id = insert_surface(
            &mut state,
            WlSurface::new(WlSurfaceId::from(1), ClientId::from_raw(1), 6),
        );
        let child_a_object_id = insert_surface(
            &mut state,
            WlSurface::new(WlSurfaceId::from(2), ClientId::from_raw(1), 6),
        );
        let child_b_object_id = insert_surface(
            &mut state,
            WlSurface::new(WlSurfaceId::from(3), ClientId::from_raw(1), 6),
        );
        let child_c_object_id = insert_surface(
            &mut state,
            WlSurface::new(WlSurfaceId::from(4), ClientId::from_raw(1), 6),
        );

        {
            let parent = state
                .objects
                .get_mut(parent_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            parent.add_pending_child(child_a_object_id);
            parent.add_pending_child(child_b_object_id);
            parent.add_pending_child(child_c_object_id);
            assert!(parent.restack_child_relative(
                child_c_object_id,
                Some(child_a_object_id),
                false
            ));
            assert!(parent.restack_child_relative(
                child_b_object_id,
                Some(child_c_object_id),
                true
            ));
        }
        {
            let child_a = state
                .objects
                .get_mut(child_a_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_a.assign_subsurface(
                crate::protocol_generated::WlSubsurfaceId::from(41),
                parent_object_id,
            );
        }
        {
            let child_b = state
                .objects
                .get_mut(child_b_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_b.assign_subsurface(
                crate::protocol_generated::WlSubsurfaceId::from(42),
                parent_object_id,
            );
        }
        {
            let child_c = state
                .objects
                .get_mut(child_c_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_c.assign_subsurface(
                crate::protocol_generated::WlSubsurfaceId::from(43),
                parent_object_id,
            );
        }
        {
            let mut parent = state.objects.grab_object::<WlSurface>(parent_object_id);
            assert!(parent.apply_pending_tree_state(&mut state));
            assert_eq!(
                parent.current_stack(),
                &[
                    super::StackEntry::Child(child_c_object_id),
                    super::StackEntry::Child(child_b_object_id),
                    super::StackEntry::Child(child_a_object_id),
                    super::StackEntry::Parent,
                ]
            );
        }
    }

    #[test]
    fn applying_unchanged_tree_state_is_idempotent() {
        let mut state = make_state();
        let parent_object_id = insert_surface(
            &mut state,
            WlSurface::new(WlSurfaceId::from(1), ClientId::from_raw(1), 6),
        );
        let child_object_id = insert_surface(
            &mut state,
            WlSurface::new(WlSurfaceId::from(2), ClientId::from_raw(1), 6),
        );

        {
            let parent = state
                .objects
                .get_mut(parent_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            parent.add_pending_child(child_object_id);
        }
        {
            let child = state
                .objects
                .get_mut(child_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child.assign_subsurface(
                crate::protocol_generated::WlSubsurfaceId::from(51),
                parent_object_id,
            );
            child.set_subsurface_position(12, 34);
        }
        {
            let mut parent = state.objects.grab_object::<WlSurface>(parent_object_id);
            assert!(parent.apply_pending_tree_state(&mut state));
        }
        {
            let mut parent = state.objects.grab_object::<WlSurface>(parent_object_id);
            assert!(!parent.apply_pending_tree_state(&mut state));
        }
    }

    #[test]
    fn repeated_synchronized_cache_updates_compose_latest_state() {
        let mut surface = WlSurface::new(WlSurfaceId::from(1), ClientId::from_raw(1), 6);
        let discarded = surface.cache_commit_payload(
            SurfaceCommitPayload {
                buffer: Some(Some(ObjectId::from(1usize))),
                offset: (10, 20),
                sync: SurfaceSyncState::default(),
                buffer_transform: BufferTransform::Normal,
                buffer_scale: 1,
                viewport: SurfaceViewport::default(),
                damage_surface: vec![DamageRect { x: 0, y: 0, width: 10, height: 10 }],
                damage_buffer: vec![],
                input_region: RegionValue::infinite(),
                opaque_region: RegionValue::empty(),
            },
            vec![crate::protocol_generated::WlCallbackId::from(7)],
            vec![feedback(17, 2)],
        );
        assert!(discarded.is_empty());
        let discarded = surface.cache_commit_payload(
            SurfaceCommitPayload {
                buffer: Some(Some(ObjectId::from(2usize))),
                offset: (30, 40),
                sync: SurfaceSyncState::default(),
                buffer_transform: BufferTransform::FlipRotate180,
                buffer_scale: 2,
                viewport: SurfaceViewport::default(),
                damage_surface: vec![DamageRect { x: 5, y: 6, width: 7, height: 8 }],
                damage_buffer: vec![],
                input_region: RegionValue::empty(),
                opaque_region: RegionValue::infinite(),
            },
            vec![crate::protocol_generated::WlCallbackId::from(8)],
            vec![feedback(18, 2)],
        );
        assert_eq!(discarded, vec![feedback(17, 2)]);

        let cached = surface.cached_commit.as_ref().unwrap();
        assert_eq!(cached.payload.buffer, Some(Some(ObjectId::from(2usize))));
        assert_eq!(cached.payload.offset, (30, 40));
        assert_eq!(cached.payload.buffer_transform, BufferTransform::FlipRotate180);
        assert_eq!(cached.payload.buffer_scale, 2);
        assert_eq!(cached.payload.damage_surface.len(), 2);
        assert_eq!(
            cached.frame_callbacks,
            vec![
                crate::protocol_generated::WlCallbackId::from(7),
                crate::protocol_generated::WlCallbackId::from(8),
            ]
        );
        assert_eq!(cached.presentation_feedbacks, vec![feedback(18, 2)]);
    }

    #[test]
    fn route_presentation_feedbacks_replaces_superseded_on_screen_commit() {
        let mut surface = make_surface();
        surface.on_screen.presentation_feedbacks = vec![feedback(21, 2)];

        let (discarded, needs_frame) = surface.route_presentation_feedbacks(vec![feedback(22, 2)]);

        assert_eq!(discarded, vec![feedback(21, 2)]);
        assert!(needs_frame);
        assert_eq!(surface.on_screen.presentation_feedbacks, vec![feedback(22, 2)]);
    }

    #[test]
    fn route_presentation_feedbacks_on_unmapped_surface_discards_immediately() {
        let mut surface = WlSurface::new(WlSurfaceId::from(1), ClientId::from_raw(1), 6);

        let (discarded, needs_frame) = surface.route_presentation_feedbacks(vec![feedback(23, 2)]);

        assert_eq!(discarded, vec![feedback(23, 2)]);
        assert!(!needs_frame);
    }

    #[test]
    fn take_render_state_drains_presentation_feedbacks() {
        let mut surface = make_surface();
        surface.on_screen.presentation_feedbacks = vec![feedback(31, 2), feedback(32, 1)];

        let render_state = surface.take_render_state_at(0, 0).unwrap();

        assert_eq!(render_state.presentation_feedbacks, vec![feedback(31, 2), feedback(32, 1)]);
        assert!(surface.on_screen.presentation_feedbacks.is_empty());
    }

    #[test]
    fn backend_discard_also_clears_unassigned_feedbacks() {
        let mut surface = make_surface();
        surface.pending_presentation_feedbacks = vec![feedback(41, 2)];
        let (mut client, mut dispatcher) = make_client();
        let mut dispatcher = client.test_dispatcher(&mut dispatcher);
        let mut state = make_state();
        let backend = crate::state::BackendOutputRef::new(crate::state::DrmId::from(0usize));

        surface.discard_presentation_feedbacks_for_backend(
            &mut client,
            &mut state,
            &mut dispatcher,
            backend,
        );

        assert!(surface.pending_presentation_feedbacks.is_empty());
    }

    #[test]
    fn deleting_parent_surface_detaches_and_unmaps_child_subtree() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut dispatcher = client.test_dispatcher(&mut dispatcher);

        let (_, parent_object_id) = client
            .insert_object(&mut state, WlSurface::new(WlSurfaceId::from(1), client.client_id(), 6))
            .0;
        state.register_root_surface(parent_object_id);
        let (_, child_object_id) = client
            .insert_object(&mut state, WlSurface::new(WlSurfaceId::from(2), client.client_id(), 6))
            .0;
        state.register_root_surface(child_object_id);
        let (_, grandchild_object_id) = client
            .insert_object(&mut state, WlSurface::new(WlSurfaceId::from(3), client.client_id(), 6))
            .0;
        state.register_root_surface(grandchild_object_id);

        {
            let parent = state
                .objects
                .get_mut(parent_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            parent.add_pending_child(child_object_id);
            parent.test_map(ObjectId::from(300usize), 0, 0, 40, 40);
        }
        {
            let child = state
                .objects
                .get_mut(child_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child.assign_subsurface(WlSubsurfaceId::from(61), parent_object_id);
            child.set_subsurface_position(5, 6);
            child.add_pending_child(grandchild_object_id);
            child.test_map(ObjectId::from(301usize), 0, 0, 20, 20);
        }
        {
            let grandchild = state
                .objects
                .get_mut(grandchild_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            grandchild.assign_subsurface(WlSubsurfaceId::from(62), child_object_id);
            grandchild.set_subsurface_position(7, 8);
            grandchild.test_map(ObjectId::from(302usize), 0, 0, 10, 10);
        }
        {
            let mut parent = state.objects.grab_object::<WlSurface>(parent_object_id);
            assert!(parent.apply_pending_tree_state(&mut state));
        }
        {
            let mut child = state.objects.grab_object::<WlSurface>(child_object_id);
            assert!(child.apply_pending_tree_state(&mut state));
        }

        {
            let mut parent_object = client.grab_object(WlSurfaceId::from(1), &mut state);
            parent_object.delete_self(&mut client, &mut state, &mut dispatcher);
        }

        let child = state
            .objects
            .get(child_object_id)
            .downcast_ref::<WlSurface>()
            .unwrap();
        let grandchild = state
            .objects
            .get(grandchild_object_id)
            .downcast_ref::<WlSurface>()
            .unwrap();
        assert!(child.is_root());
        assert!(grandchild.is_root());
        assert!(child.subsurface_object().is_none());
        assert!(grandchild.subsurface_object().is_none());
        assert!(child.render_state().is_none());
        assert!(grandchild.render_state().is_none());
        assert!(!client.contains_object(WlSubsurfaceId::from(61), &state));
        assert!(!client.contains_object(WlSubsurfaceId::from(62), &state));
    }

    #[test]
    fn client_disconnect_clears_blocked_surface_state() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut surface = make_surface();
        surface.blocked_waiter_id = Some(crate::sync::AcquireReadyWaiterId::from(42u64));

        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            crate::object::Object::on_client_disconnect(
                &mut surface,
                &mut client,
                &mut state,
                &mut client_dispatcher,
            );
        }

        assert_eq!(surface.blocked_waiter_id, None);
    }
}
