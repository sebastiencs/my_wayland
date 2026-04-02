use std::collections::{BTreeMap, BTreeSet};

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser},
    protocol_generated::{
        ext_workspace_group_handle_v1, wl_display, ExtWorkspaceGroupHandleV1Id,
        ExtWorkspaceHandleV1Id, ExtWorkspaceManagerV1Id, WaylandId, WlOutputId,
    },
    state::{State, WorkspaceGroupId, WorkspaceId},
    wayland_objects::{
        ext_workspace_manager_v1::{ExtWorkspaceManagerV1, PendingWorkspaceRequest},
        wl_display::WlDisplay,
    },
};

#[derive(Debug)]
pub struct ExtWorkspaceGroupHandleV1 {
    id: ExtWorkspaceGroupHandleV1Id,
    manager_id: ExtWorkspaceManagerV1Id,
    group_id: WorkspaceGroupId,
    version: u32,
    known_output_objects: BTreeSet<WlOutputId>,
    known_workspaces: BTreeSet<WorkspaceId>,
    sent_capabilities: Option<u32>,
    removed: bool,
}

impl ExtWorkspaceGroupHandleV1 {
    pub fn new(
        id: ExtWorkspaceGroupHandleV1Id,
        manager_id: ExtWorkspaceManagerV1Id,
        group_id: WorkspaceGroupId,
        version: u32,
    ) -> Self {
        Self {
            id,
            manager_id,
            group_id,
            version,
            known_output_objects: BTreeSet::new(),
            known_workspaces: BTreeSet::new(),
            sent_capabilities: None,
            removed: false,
        }
    }

    pub fn group_id(&self) -> WorkspaceGroupId {
        self.group_id
    }

    fn capability_bits(&self, state: &State) -> Option<u32> {
        state
            .workspace_groups
            .try_get(self.group_id)
            .map(|_| ext_workspace_group_handle_v1::GroupCapabilities::CreateWorkspace as u32)
    }

    pub fn sync(
        &mut self,
        client: &Client,
        state: &State,
        dispatcher: &mut ClientDispatcher,
        workspace_handles: &BTreeMap<WorkspaceId, ExtWorkspaceHandleV1Id>,
    ) {
        let Some(group) = state.workspace_groups.try_get(self.group_id) else {
            if !self.removed {
                let known_workspaces = std::mem::take(&mut self.known_workspaces);
                for workspace_id in known_workspaces {
                    if let Some(handle_id) = workspace_handles.get(&workspace_id).copied() {
                        self.send_event(
                            ext_workspace_group_handle_v1::Event::WorkspaceLeave {
                                workspace: handle_id,
                            },
                            dispatcher,
                        );
                    }
                }
                self.known_output_objects.clear();
                self.send_event(ext_workspace_group_handle_v1::Event::Removed, dispatcher);
                self.removed = true;
            }
            return;
        };

        if self.removed {
            return;
        }

        let capabilities = self.capability_bits(state).unwrap_or(0);
        if self.sent_capabilities != Some(capabilities) {
            self.send_event(
                ext_workspace_group_handle_v1::Event::Capabilities { capabilities },
                dispatcher,
            );
            self.sent_capabilities = Some(capabilities);
        }

        self.known_output_objects
            .retain(|output_id| client.object_id_for_wayland(*output_id).is_some());
        let current_output_objects = group
            .outputs
            .iter()
            .flat_map(|output_id| client.output_object_ids_for_output(state, *output_id))
            .collect::<BTreeSet<_>>();
        for output in current_output_objects.difference(&self.known_output_objects) {
            self.send_event(
                ext_workspace_group_handle_v1::Event::OutputEnter { output: *output },
                dispatcher,
            );
        }
        for output in self
            .known_output_objects
            .difference(&current_output_objects)
        {
            self.send_event(
                ext_workspace_group_handle_v1::Event::OutputLeave { output: *output },
                dispatcher,
            );
        }
        self.known_output_objects = current_output_objects;

        let current_workspaces = group.workspaces.iter().copied().collect::<BTreeSet<_>>();
        for workspace_id in current_workspaces.difference(&self.known_workspaces) {
            let Some(handle_id) = workspace_handles.get(workspace_id).copied() else {
                continue;
            };
            self.send_event(
                ext_workspace_group_handle_v1::Event::WorkspaceEnter { workspace: handle_id },
                dispatcher,
            );
        }
        for workspace_id in self.known_workspaces.difference(&current_workspaces) {
            let Some(handle_id) = workspace_handles.get(workspace_id).copied() else {
                continue;
            };
            self.send_event(
                ext_workspace_group_handle_v1::Event::WorkspaceLeave { workspace: handle_id },
                dispatcher,
            );
        }
        self.known_workspaces = current_workspaces;
    }
}

impl Object for ExtWorkspaceGroupHandleV1 {
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
        use ext_workspace_group_handle_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            CreateWorkspace { workspace } => {
                if self.removed {
                    return Ok(());
                }
                let Some(manager_object_id) = client.object_id_for_wayland(self.manager_id) else {
                    return Ok(());
                };
                let Some(mut manager) = state
                    .objects
                    .try_grab_object::<ExtWorkspaceManagerV1>(manager_object_id)
                else {
                    return Ok(());
                };
                manager.queue_request(PendingWorkspaceRequest::CreateWorkspace {
                    group_id: self.group_id,
                    name: workspace,
                });
            }
            Destroy => {
                if let Some(manager_object_id) = client.object_id_for_wayland(self.manager_id) {
                    if let Some(mut manager) = state
                        .objects
                        .try_grab_object::<ExtWorkspaceManagerV1>(manager_object_id)
                    {
                        manager.release_group_handle(self.group_id);
                    }
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }
}
