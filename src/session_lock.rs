//! Session-lock lifecycle. Presentation, input gating, and restoration arrive in later phases.
//! `locked` is not emitted here: confirmation waits until every active output has presented.

use {
    crate::{
        clients::{ClientId, Resolved},
        input::types::SurfaceFocus,
        protocol_generated::{ExtSessionLockSurfaceV1Id, ExtSessionLockV1Id},
        state::OutputId,
    },
    std::collections::BTreeMap,
};

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
        *self = Self::default();
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
    }

    /// `unlock_and_destroy` after `locked`. Focus restoration is a later phase.
    pub fn unlock(&mut self) {
        if !self.locked_sent {
            return;
        }
        *self = Self::default();
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

    pub fn clear_surface(&mut self, output_id: OutputId) {
        if let Some(output) = self.outputs.get_mut(&output_id) {
            output.surface = None;
            output.mapped = false;
        }
    }

    pub fn note_mapped(&mut self, output_id: OutputId) {
        if let Some(output) = self.outputs.get_mut(&output_id) {
            if output.surface.is_some() {
                output.mapped = true;
            }
        }
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
