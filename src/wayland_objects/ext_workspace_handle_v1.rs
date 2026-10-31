use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_workspace_handle_v1, wl_display, Array, ExtWorkspaceGroupHandleV1Id,
        ExtWorkspaceHandleV1Id, ExtWorkspaceManagerV1Id, WaylandId,
    },
    state::{GrabbedObject, State, WorkspaceGroupId, WorkspaceId},
    wayland_objects::{
        ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
        ext_workspace_manager_v1::{ExtWorkspaceManagerV1, PendingWorkspaceRequest},
        wl_display::WlDisplay,
    },
};

#[derive(Debug)]
pub struct ExtWorkspaceHandleV1 {
    id: ExtWorkspaceHandleV1Id,
    manager: Resolved<ExtWorkspaceManagerV1Id>,
    workspace_id: WorkspaceId,
    version: u32,
    sent_name: Option<String>,
    sent_coordinates: Option<Vec<u32>>,
    sent_state: Option<u32>,
    sent_capabilities: Option<u32>,
    removed: bool,
}

impl ExtWorkspaceHandleV1 {
    pub fn new(
        id: ExtWorkspaceHandleV1Id,
        manager: Resolved<ExtWorkspaceManagerV1Id>,
        workspace_id: WorkspaceId,
        version: u32,
    ) -> Self {
        Self {
            id,
            manager,
            workspace_id,
            version,
            sent_name: None,
            sent_coordinates: None,
            sent_state: None,
            sent_capabilities: None,
            removed: false,
        }
    }

    pub fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    fn state_bits(&self, state: &State) -> Option<u32> {
        state.workspaces.try_get(self.workspace_id).map(|_| {
            let mut bits = 0;
            if state.workspace_is_active(self.workspace_id) {
                bits |= ext_workspace_handle_v1::State::Active as u32;
            }
            bits
        })
    }

    fn capability_bits(&self, state: &State) -> Option<u32> {
        state
            .workspaces
            .try_get(self.workspace_id)
            .map(|workspace| {
                let mut bits = ext_workspace_handle_v1::WorkspaceCapabilities::Assign as u32;
                if workspace.group.is_some() && !state.workspace_is_active(self.workspace_id) {
                    bits |= ext_workspace_handle_v1::WorkspaceCapabilities::Activate as u32;
                }
                if state.can_remove_workspace(self.workspace_id) {
                    bits |= ext_workspace_handle_v1::WorkspaceCapabilities::Remove as u32;
                }
                bits
            })
    }

    pub fn sync(&mut self, state: &State, dispatcher: &mut ClientDispatcher) {
        let Some(workspace) = state.workspaces.try_get(self.workspace_id) else {
            if !self.removed {
                self.send_event(ext_workspace_handle_v1::Event::Removed, dispatcher);
                self.removed = true;
            }
            return;
        };

        if self.removed {
            return;
        }

        if self.sent_name.as_ref() != Some(&workspace.name) {
            self.send_event(
                ext_workspace_handle_v1::Event::Name { name: workspace.name.clone() },
                dispatcher,
            );
            self.sent_name = Some(workspace.name.clone());
        }

        if self.sent_coordinates.as_ref() != Some(&workspace.coordinates) {
            self.send_event(
                ext_workspace_handle_v1::Event::Coordinates {
                    coordinates: Array(workspace.coordinates.clone()),
                },
                dispatcher,
            );
            self.sent_coordinates = Some(workspace.coordinates.clone());
        }

        let state_bits = self.state_bits(state).unwrap_or(0);
        if self.sent_state != Some(state_bits) {
            self.send_event(
                ext_workspace_handle_v1::Event::State { state: state_bits },
                dispatcher,
            );
            self.sent_state = Some(state_bits);
        }

        let capabilities = self.capability_bits(state).unwrap_or(0);
        if self.sent_capabilities != Some(capabilities) {
            self.send_event(
                ext_workspace_handle_v1::Event::Capabilities { capabilities },
                dispatcher,
            );
            self.sent_capabilities = Some(capabilities);
        }
    }
}

impl Object for ExtWorkspaceHandleV1 {
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
        use ext_workspace_handle_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                if let Some(mut manager) = self.manager.try_ref_mut(state) {
                    manager.release_workspace_handle(self.workspace_id);
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            Activate => {
                if self.removed {
                    return Ok(());
                }
                let Some(mut manager) = self.manager.try_ref_mut(state) else {
                    return Ok(());
                };
                manager.queue_request(PendingWorkspaceRequest::Activate {
                    workspace_id: self.workspace_id,
                });
            }
            Deactivate => {}
            Assign { workspace_group } => {
                if self.removed {
                    return Ok(());
                }
                let group = client
                    .resolve_or_disconnect(workspace_group, state, dispatcher)?
                    .rref(state);
                let group_id = group.group_id();
                let Some(mut manager) = self.manager.try_ref_mut(state) else {
                    return Ok(());
                };
                manager.queue_request(PendingWorkspaceRequest::Assign {
                    workspace_id: self.workspace_id,
                    group_id,
                });
            }
            Remove => {
                if self.removed {
                    return Ok(());
                }
                let Some(mut manager) = self.manager.try_ref_mut(state) else {
                    return Ok(());
                };
                manager.queue_request(PendingWorkspaceRequest::Remove {
                    workspace_id: self.workspace_id,
                });
            }
        }
        Ok(())
    }
}
