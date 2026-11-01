//! Session-lock lifecycle. While locking, locked, or abandoned, input stays on lock surfaces.
//! `locked` is not emitted here: confirmation waits until every active output has presented.

use {
    crate::{
        clients::{ClientId, Resolved},
        input::types::SurfaceFocus,
        protocol_generated::{ExtSessionLockSurfaceV1Id, ExtSessionLockV1Id, WlSurfaceId},
        state::{OutputId, State},
        wayland_objects::wl_surface::StackEntry,
    },
    std::collections::BTreeMap,
};

/// Compositor-owned coverage. Black while a locker is alive, red after it dies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockBlank {
    Black,
    Red,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionLockPhase {
    Unlocked,
    Locking,
    Locked,
    Abandoned,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PreLockFocus {
    pub keyboard: Option<SurfaceFocus>,
    pub pointer: Option<SurfaceFocus>,
}

#[derive(Clone, Copy, Debug)]
struct LockOutput {
    surface: Option<Resolved<ExtSessionLockSurfaceV1Id>>,
    /// A commit matched the last acked configure. Destroying that surface exposes the fallback.
    mapped: bool,
    presented: bool,
    /// Enabled when the lock was accepted. A surface on any other output does not join this set.
    required: bool,
}

#[derive(Clone, Debug)]
pub struct SessionLock {
    phase: SessionLockPhase,
    owner: Option<ClientId>,
    lock: Option<Resolved<ExtSessionLockV1Id>>,
    outputs: BTreeMap<OutputId, LockOutput>,
    focus: PreLockFocus,
    locked_sent: bool,
    /// Bumped when the protected coverage changes. Epoch 0 is the unlocked desktop.
    coverage_epoch: u64,
    /// `(epoch, full frames still owed)` per output. Two frames cover both render buffers.
    coverage_applied: BTreeMap<OutputId, (u64, u8)>,
}

impl Default for SessionLock {
    fn default() -> Self {
        Self {
            phase: SessionLockPhase::Unlocked,
            owner: None,
            lock: None,
            outputs: BTreeMap::new(),
            focus: PreLockFocus::default(),
            locked_sent: false,
            coverage_epoch: 0,
            coverage_applied: BTreeMap::new(),
        }
    }
}

impl SessionLock {
    pub fn phase(&self) -> SessionLockPhase {
        self.phase
    }

    pub fn owner(&self) -> Option<ClientId> {
        self.owner
    }

    pub fn lock(&self) -> Option<Resolved<ExtSessionLockV1Id>> {
        self.lock
    }

    pub fn locked_sent(&self) -> bool {
        self.locked_sent
    }

    pub fn pre_lock_focus(&self) -> PreLockFocus {
        self.focus
    }

    pub fn surface_for_output(
        &self,
        output_id: OutputId,
    ) -> Option<Resolved<ExtSessionLockSurfaceV1Id>> {
        self.outputs
            .get(&output_id)
            .and_then(|output| output.surface)
    }

    pub fn accepts_new_lock(&self) -> bool {
        matches!(self.phase, SessionLockPhase::Unlocked | SessionLockPhase::Abandoned)
    }

    /// Locking, locked, and abandoned all hide the desktop. Bindings and normal clients stay out.
    pub fn input_gated(&self) -> bool {
        !matches!(self.phase, SessionLockPhase::Unlocked)
    }

    /// Accept the first lock, or the single replacement after the previous locker died.
    /// `keep_focus` preserves the desktop focus captured by the original lock.
    pub fn install(
        &mut self,
        client_id: ClientId,
        lock: Resolved<ExtSessionLockV1Id>,
        keep_focus: bool,
        focus: PreLockFocus,
        outputs: impl IntoIterator<Item = OutputId>,
    ) {
        if !keep_focus {
            self.focus = focus;
        }
        self.phase = SessionLockPhase::Locking;
        self.owner = Some(client_id);
        self.lock = Some(lock);
        self.locked_sent = false;
        self.outputs = outputs
            .into_iter()
            .map(|output_id| {
                (
                    output_id,
                    LockOutput {
                        surface: None,
                        mapped: false,
                        presented: false,
                        required: true,
                    },
                )
            })
            .collect();
        self.bump_coverage();
    }

    pub fn surfaces(&self) -> Vec<Resolved<ExtSessionLockSurfaceV1Id>> {
        self.outputs
            .values()
            .filter_map(|output| output.surface)
            .collect()
    }

    /// `destroy` or a disconnect before `locked` uncovers the desktop.
    pub fn cancel_pending(&mut self) {
        if self.phase != SessionLockPhase::Locking || self.locked_sent {
            return;
        }
        let epoch = self.coverage_epoch.saturating_add(1);
        *self = Self::default();
        self.coverage_epoch = epoch;
    }

    /// The confirmed locker disconnected. The desktop stays covered for one replacement.
    pub fn abandon(&mut self) {
        if !self.locked_sent {
            return;
        }
        self.phase = SessionLockPhase::Abandoned;
        self.owner = None;
        self.lock = None;
        for output in self.outputs.values_mut() {
            output.surface = None;
            output.mapped = false;
            output.presented = false;
        }
        self.bump_coverage();
    }

    /// `unlock_and_destroy` after `locked`. The caller restores focus and clears the logind hint.
    pub fn unlock(&mut self) {
        if !self.locked_sent {
            return;
        }
        let epoch = self.coverage_epoch.saturating_add(1);
        *self = Self::default();
        self.coverage_epoch = epoch;
    }

    /// A resumed session redraws protected pixels before any desktop frame.
    pub fn note_session_resumed(&mut self) {
        if self.input_gated() {
            self.bump_coverage();
        }
    }

    /// An output that appears while protected is covered, but it does not join the
    /// acceptance barrier. An empty barrier still cannot be confirmed by that output.
    pub fn cover_output(&mut self, output_id: OutputId) -> bool {
        if !self.input_gated() || self.outputs.contains_key(&output_id) {
            return false;
        }
        self.outputs.insert(
            output_id,
            LockOutput {
                surface: None,
                mapped: false,
                presented: false,
                required: false,
            },
        );
        self.bump_coverage();
        true
    }

    pub fn covered_outputs(&self) -> Vec<OutputId> {
        self.outputs.keys().copied().collect()
    }

    /// Opaque coverage for the current phase. `None` while the desktop is visible.
    pub fn blank(&self) -> Option<LockBlank> {
        match self.phase {
            SessionLockPhase::Unlocked => None,
            SessionLockPhase::Locking | SessionLockPhase::Locked => Some(LockBlank::Black),
            SessionLockPhase::Abandoned => Some(LockBlank::Red),
        }
    }

    /// Lock surface and its subsurfaces, bottom to top. Popups are not part of this tree.
    pub fn paint_order(&self, state: &State, output_id: OutputId) -> Vec<Resolved<WlSurfaceId>> {
        if !self.shows_lock_surface(output_id) {
            return Vec::new();
        }
        let Some(root) = self
            .surface_for_output(output_id)
            .and_then(|surface| surface.try_ref(state))
            .map(|surface| surface.wl_surface())
        else {
            return Vec::new();
        };
        let mut order = Vec::new();
        append_paint_order(state, root, &mut order);
        order
    }

    /// The next rendered frames of `output_id` must repaint every pixel.
    ///
    /// Two frames cover the pair of render buffers, so a buffer that still holds the desktop
    /// cannot be preserved by buffer age after the coverage changes.
    pub fn consume_full_damage(&mut self, output_id: OutputId) -> bool {
        let epoch = self.coverage_epoch;
        if let Some((applied_epoch, remaining)) = self.coverage_applied.get_mut(&output_id) {
            if *applied_epoch == epoch {
                if *remaining == 0 {
                    return false;
                }
                *remaining -= 1;
                return true;
            }
        }
        let frames = if epoch == 0 { 0 } else { 2 };
        if frames == 0 {
            self.coverage_applied.insert(output_id, (0, 0));
            return false;
        }
        self.coverage_applied.insert(output_id, (epoch, frames - 1));
        true
    }

    fn bump_coverage(&mut self) {
        self.coverage_epoch = self.coverage_epoch.saturating_add(1);
    }

    pub fn assign_surface(
        &mut self,
        output_id: OutputId,
        surface: Resolved<ExtSessionLockSurfaceV1Id>,
    ) -> bool {
        let entry = self.outputs.entry(output_id).or_insert(LockOutput {
            surface: None,
            mapped: false,
            presented: false,
            required: false,
        });
        if entry.surface.is_some() {
            return false;
        }
        entry.surface = Some(surface);
        true
    }

    /// Returns whether a mapped lock surface was removed and the fallback must be repainted.
    pub fn clear_surface(&mut self, output_id: OutputId) -> bool {
        let was_mapped = self.outputs.get_mut(&output_id).is_some_and(|output| {
            let was_mapped = output.mapped;
            output.surface = None;
            output.mapped = false;
            was_mapped
        });
        if was_mapped {
            self.bump_coverage();
        }
        was_mapped
    }

    /// Returns whether this is the commit that first maps the lock surface.
    pub fn note_mapped(&mut self, output_id: OutputId) -> bool {
        let mapped_now = self.outputs.get_mut(&output_id).is_some_and(|output| {
            if output.surface.is_some() && !output.mapped {
                output.mapped = true;
                true
            } else {
                false
            }
        });
        if mapped_now {
            self.bump_coverage();
        }
        mapped_now
    }

    pub fn shows_lock_surface(&self, output_id: OutputId) -> bool {
        self.outputs
            .get(&output_id)
            .is_some_and(|output| output.mapped && output.surface.is_some())
    }

    /// The output is gone. The lock surface stays alive and becomes inert for its client.
    pub fn detach_output(
        &mut self,
        output_id: OutputId,
    ) -> Option<Resolved<ExtSessionLockSurfaceV1Id>> {
        self.outputs
            .remove(&output_id)
            .and_then(|output| output.surface)
    }

    pub fn mark_output_presented(&mut self, output_id: OutputId) -> bool {
        if let Some(output) = self.outputs.get_mut(&output_id) {
            output.presented = true;
        }
        self.barrier_satisfied()
    }

    /// Every output that was active at acceptance has presented. An empty set does not.
    pub fn barrier_satisfied(&self) -> bool {
        self.outputs.values().any(|output| output.required)
            && self
                .outputs
                .values()
                .filter(|output| output.required)
                .all(|output| output.presented)
    }

    /// Move `Locking` to `Locked` once the barrier is met. The caller sends the event.
    pub fn confirm_if_presented(&mut self) -> Option<Resolved<ExtSessionLockV1Id>> {
        if self.phase != SessionLockPhase::Locking || !self.barrier_satisfied() {
            return None;
        }
        self.phase = SessionLockPhase::Locked;
        self.locked_sent = true;
        self.lock
    }

    #[cfg(test)]
    pub fn mark_locked_for_tests(&mut self) {
        self.phase = SessionLockPhase::Locked;
        self.locked_sent = true;
    }
}

fn append_paint_order(
    state: &State,
    surface: Resolved<WlSurfaceId>,
    order: &mut Vec<Resolved<WlSurfaceId>>,
) {
    let stack = surface
        .try_ref(state)
        .map(|wl_surface| wl_surface.current_stack().to_vec())
        .unwrap_or_default();
    for entry in stack {
        match entry {
            StackEntry::Child(child) => append_paint_order(state, child, order),
            StackEntry::Parent => order.push(surface),
        }
    }
}
