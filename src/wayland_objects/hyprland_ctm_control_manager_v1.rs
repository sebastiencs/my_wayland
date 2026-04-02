use std::collections::{BTreeMap, BTreeSet};

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    drm::schedule_outputs,
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{
        hyprland_ctm_control_manager_v1, wl_display, Constants, HyprlandCtmControlManagerV1Id,
        NewId, WaylandId,
    },
    state::{ColorTransformMatrix, OutputId, State},
    wayland_objects::{wl_display::WlDisplay, wl_output::WlOutput},
};

#[derive(Debug)]
pub struct HyprlandCtmControlManagerV1Global;

impl HyprlandCtmControlManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for HyprlandCtmControlManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (
            HyprlandCtmControlManagerV1::INTERFACE_NAME,
            HyprlandCtmControlManagerV1::INTERFACE_VERSION,
        )
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        if !state.client_allows_ctm_control(client) {
            client.protocol_violation_disconnect(
                dispatcher,
                WaylandId::from(1u32),
                wl_display::Error::InvalidObject as u32,
                "client is not allowed to bind hyprland_ctm_control_manager_v1",
            );
            return;
        }
        let id = HyprlandCtmControlManagerV1Id::from(id);
        let blocked = state.objects.iter().any(|(_, object)| {
            object
                .downcast_ref::<HyprlandCtmControlManagerV1>()
                .is_some_and(|manager| !manager.is_blocked())
        });
        let ((_, _object_id), manager) =
            client.insert_object(state, HyprlandCtmControlManagerV1::new(id, version, blocked));
        if blocked {
            manager.send_blocked(dispatcher);
        }
    }
}

#[derive(Debug)]
pub struct HyprlandCtmControlManagerV1 {
    id: HyprlandCtmControlManagerV1Id,
    version: u32,
    blocked: bool,
    pending_ctms: BTreeMap<OutputId, ColorTransformMatrix>,
    current_ctms: BTreeMap<OutputId, ColorTransformMatrix>,
    applied_outputs: BTreeSet<OutputId>,
}

impl HyprlandCtmControlManagerV1 {
    pub fn new(id: HyprlandCtmControlManagerV1Id, version: u32, blocked: bool) -> Self {
        Self {
            id,
            version,
            blocked,
            pending_ctms: BTreeMap::new(),
            current_ctms: BTreeMap::new(),
            applied_outputs: BTreeSet::new(),
        }
    }

    pub fn is_blocked(&self) -> bool {
        self.blocked
    }

    pub fn send_blocked(&self, dispatcher: &mut ClientDispatcher) {
        if self.version >= hyprland_ctm_control_manager_v1::BLOCKED_SINCE {
            self.send_event(hyprland_ctm_control_manager_v1::Event::Blocked, dispatcher);
        }
    }

    fn validate_matrix(
        &self,
        client: &mut Client,
        dispatcher: &mut ClientDispatcher,
        values: [f32; 9],
    ) -> Option<ColorTransformMatrix> {
        if values
            .into_iter()
            .all(|value| value.is_finite() && value >= 0.0)
        {
            return Some(ColorTransformMatrix { values });
        }
        client.protocol_violation_disconnect(
            dispatcher,
            self.id.into(),
            hyprland_ctm_control_manager_v1::Error::InvalidMatrix as u32,
            "CTM entries must be finite and non-negative",
        );
        None
    }

    fn resolve_output_id(
        &self,
        client: &mut Client,
        state: &State,
        output: crate::protocol_generated::WlOutputId,
        dispatcher: &mut ClientDispatcher,
    ) -> Option<OutputId> {
        let Some(output_object_id) = client.try_object_id(output, state) else {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                "hyprland_ctm_control requires a live wl_output",
            );
            return None;
        };
        let Some(wl_output) = state.objects.try_ref_object::<WlOutput>(output_object_id) else {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                "CTM target is not a wl_output",
            );
            return None;
        };
        let Some(output_id) = wl_output.output_id() else {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                "CTM target output has been removed",
            );
            return None;
        };
        if !state.output_supports_ctm_control(output_id) {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                "target output does not support CTM",
            );
            return None;
        }
        Some(output_id)
    }

    fn apply_pending(&mut self, state: &mut State, dispatcher: &mut ClientDispatcher) {
        if self.blocked {
            return;
        }
        // Match Hyprland's commit semantics: each commit replaces the whole
        // active CTM set, so outputs omitted from the pending state reset.
        self.current_ctms = std::mem::take(&mut self.pending_ctms);

        let supported_outputs = state
            .outputs
            .iter()
            .filter_map(|(output_id, output)| output.enabled.then_some((output_id, output)))
            .filter_map(|(output_id, output)| output.ctm_supported.then_some(output_id))
            .collect::<Vec<_>>();

        for output_id in &supported_outputs {
            let desired = self.current_ctms.get(output_id).copied();
            let _ = state.set_output_ctm(*output_id, desired);
        }
        schedule_outputs(state, supported_outputs.iter().copied(), dispatcher);
        self.applied_outputs = supported_outputs.into_iter().collect();
    }

    fn reset_applied_outputs(&mut self, state: &mut State, dispatcher: &mut ClientDispatcher) {
        if self.blocked {
            self.pending_ctms.clear();
            self.current_ctms.clear();
            self.applied_outputs.clear();
            return;
        }
        let output_ids = self.applied_outputs.iter().copied().collect::<Vec<_>>();
        for output_id in &output_ids {
            let _ = state.set_output_ctm(*output_id, None);
        }
        schedule_outputs(state, output_ids, dispatcher);
        self.applied_outputs.clear();
        self.pending_ctms.clear();
        self.current_ctms.clear();
    }

    pub fn forget_output(&mut self, output_id: OutputId) {
        self.pending_ctms.remove(&output_id);
        self.current_ctms.remove(&output_id);
        self.applied_outputs.remove(&output_id);
    }
}

impl Object for HyprlandCtmControlManagerV1 {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> crate::object::RequestResult {
        use hyprland_ctm_control_manager_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                self.reset_applied_outputs(state, dispatcher);
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            Commit => self.apply_pending(state, dispatcher),
            SetCtmForOutput { output, mat0, mat1, mat2, mat3, mat4, mat5, mat6, mat7, mat8 } => {
                if self.blocked {
                    return Ok(());
                }
                let Some(output_id) = self.resolve_output_id(client, state, output, dispatcher)
                else {
                    return Ok(());
                };
                let Some(matrix) = self.validate_matrix(
                    client,
                    dispatcher,
                    [mat0, mat1, mat2, mat3, mat4, mat5, mat6, mat7, mat8],
                ) else {
                    return Ok(());
                };
                self.pending_ctms.insert(output_id, matrix);
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
        self.reset_applied_outputs(state, dispatcher);
    }
}
