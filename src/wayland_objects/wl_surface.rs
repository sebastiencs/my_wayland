use crate::{
    clients::{Client, ClientDispatcher, ClientId, EventWriter, RawRequest},
    drm::schedule_active_output,
    object::{Object, Parser},
    protocol_generated::{
        wl_surface, wp_linux_drm_syncobj_surface_v1, wp_viewport, xdg_wm_base, WaylandId,
        WlBufferId, WlCallbackId, WlRegionId, WlSubsurfaceId, WlSurfaceId, XdgSurfaceId,
    },
    region::RegionValue,
    state::{ObjectId, State},
    sync::{self, AcquireReadyWaiter, RetiredBufferCandidate, SurfaceSyncState},
    transform::{self, BufferTransform, SizeI, SurfaceViewport},
    wayland_objects::{wl_callback::WlCallback, wl_region::WlRegion, xdg_surface::XdgSurface},
};

use {super::wl_buffer::WlBuffer, crate::protocol_generated::wl_display};

static LOG_WL_SURFACE_DAMAGE_IGNORED: std::sync::Once = std::sync::Once::new();

fn log_wl_surface_damage_ignored_once() {
    LOG_WL_SURFACE_DAMAGE_IGNORED.call_once(|| {
        log::warn!(
            "wl_surface::damage and wl_surface::damage_buffer are ignored; repainting full buffers on commit until damage tracking is implemented"
        );
    });
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Role {
    None,
    XdgSurface,
    Subsurface,
}

#[derive(Clone, Debug)]
struct CachedCommit {
    payload: SurfaceCommitPayload,
    frame_callbacks: Vec<WlCallbackId>,
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
    impl_id: Option<XdgSurfaceId>,
    version: u32,
    /// back buffer
    pending: Pending,
    /// front buffer
    on_screen: OnScreen,
    blocked: Option<BlockedSurfaceState>,
    blocked_waiter_id: Option<u64>,
    syncobj_surface_object: Option<crate::protocol_generated::WpLinuxDrmSyncobjSurfaceV1Id>,
    viewport_object: Option<crate::protocol_generated::WpViewportId>,
    retired_buffers: Vec<RetiredBufferCandidate>,
    /// `wl_surface.frame` callbacks to complete on the next `commit`.
    pending_frame_callbacks: Vec<WlCallbackId>,
    cached_commit: Option<CachedCommit>,
    subsurface: Option<SubsurfaceState>,
    tree: SurfaceTreeState,
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
    x: i32,
    y: i32,
    sync: SurfaceSyncState,
    buffer_transform: BufferTransform,
    buffer_scale: i32,
    viewport: SurfaceViewport,
    logical_size: Option<SizeI>,
    sample_transform: transform::Mat3,
    input_region: RegionValue,
    opaque_region: RegionValue,
}

impl Default for OnScreen {
    fn default() -> Self {
        Self {
            buffer_object_id: None,
            x: 0,
            y: 0,
            sync: SurfaceSyncState::default(),
            buffer_transform: BufferTransform::Normal,
            buffer_scale: 1,
            viewport: SurfaceViewport::default(),
            logical_size: None,
            sample_transform: transform::Mat3::identity(),
            input_region: RegionValue::infinite(),
            opaque_region: RegionValue::empty(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SurfaceRenderState {
    pub buffer_object_id: ObjectId,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub tex_transform: transform::Mat3,
    pub sync: SurfaceSyncState,
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

impl WlSurface {
    pub fn new(id: WlSurfaceId, client_id: ClientId, version: u32) -> Self {
        Self {
            id,
            client_id,
            role: Role::None,
            impl_id: None,
            version,
            pending: Pending::default(),
            on_screen: OnScreen::default(),
            blocked: None,
            blocked_waiter_id: None,
            syncobj_surface_object: None,
            viewport_object: None,
            retired_buffers: Vec::new(),
            pending_frame_callbacks: Vec::new(),
            cached_commit: None,
            subsurface: None,
            tree: SurfaceTreeState::default(),
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
            let Some(object) = state.objects.try_get(object_id) else {
                return false;
            };
            let Some(surface) = object.downcast_ref::<WlSurface>() else {
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

    pub fn set_impl(&mut self, id: XdgSurfaceId) {
        self.impl_id = Some(id);
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
        if self.version >= 5 {
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
        Some(client.get_object(region_id, state).snapshot())
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
        self.cancel_blocked(client, state, dispatcher);
        self.retire_current();
        self.on_screen = OnScreen {
            buffer_object_id: None,
            x: self.on_screen.x,
            y: self.on_screen.y,
            sync: SurfaceSyncState::default(),
            buffer_transform: self.on_screen.buffer_transform,
            buffer_scale: self.on_screen.buffer_scale,
            viewport: self.on_screen.viewport,
            logical_size: None,
            sample_transform: transform::Mat3::identity(),
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
    ) {
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
            }
            None => {
                self.cached_commit = Some(CachedCommit { payload, frame_callbacks });
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
        let Some(object) = state.objects.try_get(buffer_object_id) else {
            return true;
        };
        let Some(buffer) = object.downcast_ref::<WlBuffer>() else {
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
        let object = state.objects.try_get(buffer_object_id)?;
        let buffer = object.downcast_ref::<WlBuffer>()?;
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
        let object = state.objects.try_get(buffer_object_id)?;
        let buffer = object.downcast_ref::<WlBuffer>()?;
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

    fn parent_behaves_synchronized(&self, state: &State) -> bool {
        let Some(parent_object_id) = self.parent_surface_object_id() else {
            return false;
        };
        let Some(object) = state.objects.try_get(parent_object_id) else {
            return false;
        };
        let Some(parent) = object.downcast_ref::<WlSurface>() else {
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
            let Some(mut child_object) = state.objects.try_grab(child_surface_object_id) else {
                continue;
            };
            let Some(child_surface) = child_object.downcast_mut::<WlSurface>() else {
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
        for cb in callbacks {
            {
                let callback = client.grab_object(cb, state);
                callback.done(dispatcher);
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

        match payload.buffer {
            None => {}
            Some(None) => {
                self.cancel_blocked(client, state, dispatcher);
                self.retire_current();
                self.on_screen = OnScreen {
                    buffer_object_id: None,
                    x: self.on_screen.x,
                    y: self.on_screen.y,
                    sync: SurfaceSyncState::default(),
                    buffer_transform: payload.buffer_transform,
                    buffer_scale: payload.buffer_scale,
                    viewport: payload.viewport,
                    logical_size: None,
                    sample_transform: transform::Mat3::identity(),
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

        let damage_scaffold = self
            .on_screen
            .buffer_object_id
            .zip(self.on_screen.logical_size)
            .and_then(|(buffer_object_id, logical_size)| {
                self.pending_damage_scaffold(state, buffer_object_id, logical_size, payload)
            });
        if let Some(damage) = damage_scaffold.as_ref() {
            if !damage.surface_to_buffer.is_empty() || !damage.buffer_to_surface.is_empty() {
                log::debug!(
                    "wl_surface commit damage scaffold surface={:?} transform={:?} scale={} viewport={:?} surface_to_buffer={:?} buffer_to_surface={:?}",
                    self.id,
                    self.on_screen.buffer_transform,
                    self.on_screen.buffer_scale,
                    self.on_screen.viewport,
                    damage.surface_to_buffer,
                    damage.buffer_to_surface
                );
            }
        }

        buffer_changed || mapping_changed
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
        self.apply_surface_update(
            client,
            state,
            dispatcher,
            cached_commit.payload,
            cached_commit.frame_callbacks,
            surface_behaves_synchronized,
        )
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
                let Some(object) = state.objects.try_get(child_surface_object_id) else {
                    continue;
                };
                let Some(child_surface) = object.downcast_ref::<WlSurface>() else {
                    continue;
                };
                surface_behaves_synchronized
                    || child_surface.sync_mode() == Some(SubsurfaceSyncMode::Sync)
            };
            if !child_behaves_synchronized {
                continue;
            }
            let Some(mut child_object) = state.objects.try_grab(child_surface_object_id) else {
                continue;
            };
            let Some(child_surface) = child_object.downcast_mut::<WlSurface>() else {
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
        surface_behaves_synchronized: bool,
    ) -> bool {
        let mut changed = self.apply_surface_payload(client, state, dispatcher, &payload);
        changed |= self.apply_pending_tree_state(state);
        changed |= self.release_synchronized_children(
            client,
            state,
            dispatcher,
            surface_behaves_synchronized,
        );
        self.flush_frame_callbacks_list(frame_callbacks, client, state, dispatcher);
        changed
    }

    fn on_commit(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(xdg_surface_id) = self.impl_id {
            let mut xdg_surface = client.grab_object(xdg_surface_id, state);
            xdg_surface.on_commit(client, state, dispatcher);
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

        if self.behaves_synchronized(state) {
            self.cache_commit_payload(payload, frame_callbacks);
            return;
        }

        let has_cached_commit = self.cached_commit.is_some();
        let payload = if has_cached_commit {
            self.cache_commit_payload(payload, frame_callbacks);
            let cached_commit = self.cached_commit.take().unwrap();
            self.apply_surface_update(
                client,
                state,
                dispatcher,
                cached_commit.payload,
                cached_commit.frame_callbacks,
                false,
            )
        } else {
            self.apply_surface_update(client, state, dispatcher, payload, frame_callbacks, false)
        };

        if payload {
            schedule_active_output(state, dispatcher);
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

        let Some(object) = state.objects.try_get(buffer_object_id) else {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                "pending wl_buffer disappeared before commit",
            );
            return false;
        };
        let Some(buffer) = object.downcast_ref::<WlBuffer>() else {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                "pending buffer object is not a wl_buffer",
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

    pub fn render_state(&self) -> Option<SurfaceRenderState> {
        self.render_state_at(0, 0)
    }

    pub fn render_state_at(&self, base_x: i32, base_y: i32) -> Option<SurfaceRenderState> {
        let logical_size = self.on_screen.logical_size?;
        Some(SurfaceRenderState {
            buffer_object_id: self.on_screen.buffer_object_id?,
            x: base_x + self.on_screen.x,
            y: base_y + self.on_screen.y,
            width: logical_size.width,
            height: logical_size.height,
            tex_transform: self.on_screen.sample_transform,
            sync: self.on_screen.sync.clone(),
        })
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
            x,
            y,
            sync: SurfaceSyncState::default(),
            buffer_transform: BufferTransform::Normal,
            buffer_scale: 1,
            viewport: SurfaceViewport::default(),
            logical_size: Some(SizeI { width, height }),
            sample_transform: transform::Mat3::identity(),
            input_region: RegionValue::infinite(),
            opaque_region: RegionValue::empty(),
        };
    }

    #[cfg(test)]
    pub(crate) fn test_set_input_region(&mut self, region: RegionValue) {
        self.on_screen.input_region = region;
    }

    pub fn activate_blocked_if_waiter_ready(&mut self, waiter_id: u64) -> bool {
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
            dispatcher.stop_model::<_, AcquireReadyWaiter>(waiter_id);
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
        self.retire_current();
        self.on_screen = OnScreen {
            buffer_object_id: Some(next.buffer_object_id),
            x: next.x,
            y: next.y,
            sync: next.sync,
            buffer_transform: next.buffer_transform,
            buffer_scale: next.buffer_scale,
            viewport: next.viewport,
            logical_size: Some(SizeI { width: next.width, height: next.height }),
            sample_transform: next.tex_transform,
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
    }

    fn cancel_blocked(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(waiter_id) = self.blocked_waiter_id.take() {
            dispatcher.stop_model::<_, AcquireReadyWaiter>(waiter_id);
        }
        let Some(blocked) = self.blocked.take() else {
            return;
        };
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
        waiter_id: u64,
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
            schedule_active_output(state, dispatcher);
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
            let Some(child_object) = state.objects.try_get(child_surface_object_id) else {
                return;
            };
            let Some(child_surface) = child_object.downcast_ref::<WlSurface>() else {
                return;
            };
            (child_surface.child_surface_ids(), child_surface.subsurface_object())
        };

        for grandchild_surface_object_id in grandchildren {
            Self::detach_subsurface_tree(grandchild_surface_object_id, client, state, dispatcher);
        }

        {
            let Some(mut child_object) = state.objects.try_grab(child_surface_object_id) else {
                return;
            };
            let Some(child_surface) = child_object.downcast_mut::<WlSurface>() else {
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
        for child_surface_object_id in self.child_surface_ids() {
            Self::detach_subsurface_tree(child_surface_object_id, client, state, dispatcher);
        }
        if let Some(parent_surface_object_id) = self.parent_surface_object_id() {
            if let Some(mut parent_object) = state.objects.try_grab(parent_surface_object_id) {
                if let Some(parent_surface) = parent_object.downcast_mut::<WlSurface>() {
                    parent_surface.remove_child_from_tree(client.object_id(self.id));
                }
            }
        }
        state.unregister_root_surface(client.object_id(self.id));
        use crate::wayland_objects::wl_display::WlDisplay;
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
        schedule_active_output(state, dispatcher);
    }

    pub fn client_id(&self) -> ClientId {
        self.client_id
    }

    pub fn surface_id(&self) -> WlSurfaceId {
        self.id
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
                log_wl_surface_damage_ignored_once();
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
                self.pending
                    .damage_buffer
                    .push(DamageRect { x, y, width, height });
                log_wl_surface_damage_ignored_once();
            }
            Offset { x, y } => {
                if self.role != Role::Subsurface {
                    self.pending.offset = (x, y);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::{DamageRect, OnScreen, SurfaceCommitPayload, WlSurface},
        crate::{
            clients::{Client, ClientId},
            egl::EGL,
            globals::Globals,
            protocol_generated::{WlSubsurfaceId, WlSurfaceId},
            region::{RectI, RegionSpec, RegionValue},
            runner::Dispatcher,
            state::{ObjectId, State},
            sync::SurfaceSyncState,
            transform::{self, BufferTransform, SizeI, SurfaceViewport},
            xkb::Xkb,
        },
    };

    fn make_surface() -> WlSurface {
        let mut surface = WlSurface::new(WlSurfaceId::from(1), ClientId::from_raw(7), 6);
        surface.on_screen = OnScreen {
            buffer_object_id: Some(ObjectId::from(1usize)),
            x: 10,
            y: 20,
            sync: SurfaceSyncState::default(),
            buffer_transform: BufferTransform::Normal,
            buffer_scale: 1,
            viewport: SurfaceViewport::default(),
            logical_size: Some(SizeI { width: 100, height: 80 }),
            sample_transform: transform::Mat3::identity(),
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
            inputs: Default::default(),
            drm: Default::default(),
            globals,
            egl: EGL::new(),
            seat: Default::default(),
            keyboard,
            xkb,
        }
    }

    fn insert_surface(state: &mut State, surface: WlSurface) -> ObjectId {
        let object_id = state.insert_object(surface).1;
        state.register_root_surface(object_id);
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
            let mut parent_object = state.objects.grab(parent_object_id);
            let parent = parent_object.downcast_mut::<WlSurface>().unwrap();
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
            let mut parent_object = state.objects.grab(parent_object_id);
            let parent = parent_object.downcast_mut::<WlSurface>().unwrap();
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
            let mut parent_object = state.objects.grab(parent_object_id);
            let parent = parent_object.downcast_mut::<WlSurface>().unwrap();
            assert!(parent.apply_pending_tree_state(&mut state));
        }
        {
            let mut parent_object = state.objects.grab(parent_object_id);
            let parent = parent_object.downcast_mut::<WlSurface>().unwrap();
            assert!(!parent.apply_pending_tree_state(&mut state));
        }
    }

    #[test]
    fn repeated_synchronized_cache_updates_compose_latest_state() {
        let mut surface = WlSurface::new(WlSurfaceId::from(1), ClientId::from_raw(1), 6);
        surface.cache_commit_payload(
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
        );
        surface.cache_commit_payload(
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
        );

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
            let mut parent_object = state.objects.grab(parent_object_id);
            let parent = parent_object.downcast_mut::<WlSurface>().unwrap();
            assert!(parent.apply_pending_tree_state(&mut state));
        }
        {
            let mut child_object = state.objects.grab(child_object_id);
            let child = child_object.downcast_mut::<WlSurface>().unwrap();
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
}
