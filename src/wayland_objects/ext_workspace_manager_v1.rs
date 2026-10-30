use std::collections::BTreeMap;

use crate::{
    clients::{Client, ClientDispatcher, ClientManagerAction, EventWriter, RawRequest, Resolved},
    drm::{kick_window_animation_manager, schedule_outputs},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_workspace_manager_v1, Constants, ExtWorkspaceGroupHandleV1Id, ExtWorkspaceHandleV1Id,
        ExtWorkspaceManagerV1Id, NewId, WaylandId,
    },
    runner::Dispatcher,
    seat,
    state::{GrabbedObject, OutputId, State, WorkspaceGroupId, WorkspaceId},
    wayland_objects::{
        ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
        ext_workspace_handle_v1::ExtWorkspaceHandleV1, wl_display::WlDisplay,
    },
};

#[derive(Debug)]
pub struct ExtWorkspaceManagerV1Global;

impl ExtWorkspaceManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

pub fn publish_workspace_state_changed(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    affected_outputs: impl IntoIterator<Item = OutputId>,
) {
    let opened = state.relayout_dirty_workspaces();
    if !opened.is_empty() {
        kick_window_animation_manager(dispatcher);
    }
    let mut affected_outputs = affected_outputs.into_iter().collect::<Vec<_>>();
    affected_outputs.sort_unstable();
    affected_outputs.dedup();

    if !affected_outputs.is_empty() {
        schedule_outputs(state, affected_outputs.iter().copied(), dispatcher);
    }
    seat::refresh_scene_focus(state, dispatcher);
    dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
}

impl Global for ExtWorkspaceManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (ExtWorkspaceManagerV1::INTERFACE_NAME, ExtWorkspaceManagerV1::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let id = ExtWorkspaceManagerV1Id::from(id);
        let (manager, _) = client.insert_object(state, ExtWorkspaceManagerV1::new(id, version));
        let mut manager = manager.grab(state);
        manager.sync(client, state, dispatcher, true);
    }
}

#[derive(Clone, Debug)]
pub enum PendingWorkspaceRequest {
    CreateWorkspace { group_id: WorkspaceGroupId, name: String },
    Activate { workspace_id: WorkspaceId },
    Assign { workspace_id: WorkspaceId, group_id: WorkspaceGroupId },
    Remove { workspace_id: WorkspaceId },
}

#[derive(Debug)]
pub struct ExtWorkspaceManagerV1 {
    id: ExtWorkspaceManagerV1Id,
    version: u32,
    group_handles: BTreeMap<WorkspaceGroupId, Resolved<ExtWorkspaceGroupHandleV1Id>>,
    workspace_handles: BTreeMap<WorkspaceId, Resolved<ExtWorkspaceHandleV1Id>>,
    pending_requests: Vec<PendingWorkspaceRequest>,
}

impl ExtWorkspaceManagerV1 {
    pub fn new(id: ExtWorkspaceManagerV1Id, version: u32) -> Self {
        Self {
            id,
            version,
            group_handles: BTreeMap::new(),
            workspace_handles: BTreeMap::new(),
            pending_requests: Vec::new(),
        }
    }

    pub fn queue_request(&mut self, request: PendingWorkspaceRequest) {
        self.pending_requests.push(request);
    }

    pub fn group_handle_id(
        &self,
        group_id: WorkspaceGroupId,
    ) -> Option<Resolved<ExtWorkspaceGroupHandleV1Id>> {
        self.group_handles.get(&group_id).copied()
    }

    pub fn workspace_handle_id(
        &self,
        workspace_id: WorkspaceId,
    ) -> Option<Resolved<ExtWorkspaceHandleV1Id>> {
        self.workspace_handles.get(&workspace_id).copied()
    }

    /// Clears the manager's mapping so the next `sync` can emit a new `workspace_group` and
    /// `new_id` for this logical group (client destroyed their `ext_workspace_group_handle_v1`).
    pub(crate) fn release_group_handle(&mut self, group_id: WorkspaceGroupId) {
        self.group_handles.remove(&group_id);
    }

    /// Clears the manager's mapping so the next `sync` can emit a new `workspace` and `new_id`
    /// for this logical workspace (client destroyed their `ext_workspace_handle_v1`).
    pub(crate) fn release_workspace_handle(&mut self, workspace_id: WorkspaceId) {
        self.workspace_handles.remove(&workspace_id);
    }

    fn create_missing_group_resources(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        for group_id in state.workspace_group_ids() {
            if self.group_handles.contains_key(&group_id) {
                continue;
            }
            let resource_id = client.allocate_server_object_id::<ExtWorkspaceGroupHandleV1Id>();
            client.insert_object(
                state,
                ExtWorkspaceGroupHandleV1::new(
                    resource_id,
                    Resolved::new(client.client_id(), self.id, client.object_id(self.id)),
                    group_id,
                    self.version,
                ),
            );
            self.group_handles.insert(
                group_id,
                Resolved::new(client.client_id(), resource_id, client.object_id(resource_id)),
            );
            self.send_event(
                ext_workspace_manager_v1::Event::WorkspaceGroup { workspace_group: resource_id },
                dispatcher,
            );
        }
    }

    fn create_missing_workspace_resources(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let mut new_workspace_object_ids = Vec::new();
        for workspace_id in state.workspace_ids() {
            if self.workspace_handles.contains_key(&workspace_id) {
                continue;
            }
            let resource_id = client.allocate_server_object_id::<ExtWorkspaceHandleV1Id>();
            let this = client.resolve(self.id, state).unwrap();
            let (workspace_handle, _) = client.insert_object(
                state,
                ExtWorkspaceHandleV1::new(resource_id, this, workspace_id, self.version),
            );
            self.workspace_handles
                .insert(workspace_id, workspace_handle);
            self.send_event(
                ext_workspace_manager_v1::Event::Workspace { workspace: resource_id },
                dispatcher,
            );
            new_workspace_object_ids.push(workspace_handle);
        }

        for object_id in new_workspace_object_ids {
            let mut workspace = object_id.grab(state);
            workspace.sync(state, dispatcher);
        }
    }

    pub fn sync(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        send_done: bool,
    ) {
        self.create_missing_group_resources(client, state, dispatcher);
        self.create_missing_workspace_resources(client, state, dispatcher);

        let workspace_handles = self.workspace_handles.clone();
        let group_object_ids = self.group_handles.values().copied().collect::<Vec<_>>();
        for resource_id in group_object_ids {
            let Some(mut group) = resource_id.try_grab(state) else {
                continue;
            };
            group.sync(client, state, dispatcher, &workspace_handles);
        }

        let workspace_object_ids = self.workspace_handles.values().copied().collect::<Vec<_>>();
        for resource_id in workspace_object_ids {
            let Some(mut workspace) = resource_id.try_grab(state) else {
                continue;
            };
            workspace.sync(state, dispatcher);
        }

        if send_done {
            self.send_event(ext_workspace_manager_v1::Event::Done, dispatcher);
        }
    }

    fn apply_pending_requests(&mut self, state: &mut State) -> (Vec<crate::state::OutputId>, bool) {
        let requests = std::mem::take(&mut self.pending_requests);
        let had_requests = !requests.is_empty();
        let mut affected_outputs = Vec::new();
        for request in requests {
            match request {
                PendingWorkspaceRequest::CreateWorkspace { group_id, name } => {
                    if state.workspace_groups.try_get(group_id).is_none() {
                        continue;
                    }
                    let workspace_id = state.create_workspace(Some(group_id), Some(name));
                    let _ = state.activate_workspace(workspace_id);
                    affected_outputs.extend(state.workspace_output_ids(workspace_id));
                }
                PendingWorkspaceRequest::Activate { workspace_id } => {
                    if state.workspaces.try_get(workspace_id).is_none() {
                        continue;
                    }
                    affected_outputs.extend(state.workspace_output_ids(workspace_id));
                    let _ = state.activate_workspace(workspace_id);
                }
                PendingWorkspaceRequest::Assign { workspace_id, group_id } => {
                    if state.workspaces.try_get(workspace_id).is_none()
                        || state.workspace_groups.try_get(group_id).is_none()
                    {
                        continue;
                    }
                    affected_outputs.extend(state.workspace_output_ids(workspace_id));
                    state.move_workspace_to_group_output(workspace_id, group_id);
                    affected_outputs.extend(state.workspace_output_ids(workspace_id));
                }
                PendingWorkspaceRequest::Remove { workspace_id } => {
                    if state.workspaces.try_get(workspace_id).is_none() {
                        continue;
                    }
                    affected_outputs.extend(state.workspace_output_ids(workspace_id));
                    let _ = state.remove_workspace(workspace_id);
                }
            }
        }
        affected_outputs.sort_unstable();
        affected_outputs.dedup();
        (affected_outputs, had_requests)
    }

    fn stop(&mut self, client: &mut Client, state: &mut State, dispatcher: &mut ClientDispatcher) {
        let group_ids = self.group_handles.values().copied().collect::<Vec<_>>();
        let workspace_ids = self.workspace_handles.values().copied().collect::<Vec<_>>();
        self.send_event(ext_workspace_manager_v1::Event::Finished, dispatcher);
        for resource_id in group_ids {
            if resource_id.exists(state) {
                client.delete_id(
                    &WlDisplay::default(),
                    state,
                    dispatcher,
                    resource_id.wayland_id().into(),
                );
            }
        }
        for resource_id in workspace_ids {
            if resource_id.exists(state) {
                client.delete_id(
                    &WlDisplay::default(),
                    state,
                    dispatcher,
                    resource_id.wayland_id().into(),
                );
            }
        }
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }
}

impl Object for ExtWorkspaceManagerV1 {
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
        use ext_workspace_manager_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Commit => {
                let (affected_outputs, had_requests) = self.apply_pending_requests(state);
                if !had_requests {
                    self.send_event(ext_workspace_manager_v1::Event::Done, dispatcher);
                    return Ok(());
                }
                publish_workspace_state_changed(state, dispatcher, affected_outputs);
            }
            Stop => self.stop(client, state, dispatcher),
        }
        Ok(())
    }
}
