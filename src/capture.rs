//! Shared lifecycle for `zwlr_screencopy_v1` and `ext-image-copy-capture-v1`.
//!
//! Pixel copy, damage, and cursor compositing land in later phases. This registry only tracks
//! which protocol objects must stop when an output, toplevel, or pointer disappears, matching
//! wlroots screencopy/ext-image-copy source teardown. Globals stay unpublished until those
//! later phases pass. Hyprland's screencopy permission prompt is intentionally not copied.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    clients::{Client, ClientAction, ClientDispatcher, ClientId, Resolved},
    protocol_generated::{
        ExtForeignToplevelHandleV1Id, ExtForeignToplevelListV1Id, ExtImageCopyCaptureFrameV1Id,
        ExtImageCopyCaptureSessionV1Id, WlPointerId, XdgToplevelId, ZwlrScreencopyFrameV1Id,
    },
    runner::Dispatcher,
    state::{OutputId, State},
    wayland_objects::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
};

/// Stable mapped-toplevel identity. Values are never reused after [`close_toplevel`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ToplevelKey(u64);

impl ToplevelKey {
    pub fn identifier(self) -> String {
        format!("{:08x}", self.0)
    }
}

/// What an ext session or screencopy frame is capturing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureTarget {
    Output(OutputId),
    Toplevel(ToplevelKey),
}

#[derive(Clone, Debug)]
struct ToplevelRecord {
    closed: bool,
    title: String,
    app_id: String,
}

#[derive(Clone, Copy, Debug)]
struct SessionSlot {
    session: Resolved<ExtImageCopyCaptureSessionV1Id>,
    target: CaptureTarget,
    pointer: Option<Resolved<WlPointerId>>,
}

#[derive(Clone, Copy, Debug)]
struct ExtFrameSlot {
    frame: Resolved<ExtImageCopyCaptureFrameV1Id>,
    target: CaptureTarget,
    pointer: Option<Resolved<WlPointerId>>,
}

#[derive(Clone, Copy, Debug)]
struct ScreencopySlot {
    frame: Resolved<ZwlrScreencopyFrameV1Id>,
    output_id: OutputId,
}

#[derive(Clone, Debug)]
enum Pending {
    StopSession(Resolved<ExtImageCopyCaptureSessionV1Id>),
    FailExtFrame(Resolved<ExtImageCopyCaptureFrameV1Id>),
    FailScreencopy(Resolved<ZwlrScreencopyFrameV1Id>),
    CloseHandle(Resolved<ExtForeignToplevelHandleV1Id>),
    Announce { key: ToplevelKey, title: String, app_id: String },
    Retitle { key: ToplevelKey, title: String },
    Reapp { key: ToplevelKey, app_id: String },
}

#[derive(Debug, Default)]
pub struct CaptureRegistry {
    next_toplevel: u64,
    toplevels: BTreeMap<ToplevelKey, ToplevelRecord>,
    /// Mapped XDG toplevels. A new map after unmap gets a new [`ToplevelKey`].
    by_xdg: BTreeMap<Resolved<XdgToplevelId>, ToplevelKey>,
    /// Lists that already received a handle for a key. Destroying that handle does not create
    /// another one until the toplevel is unmapped and mapped again.
    announced: BTreeSet<(Resolved<ExtForeignToplevelListV1Id>, ToplevelKey)>,
    sessions: Vec<SessionSlot>,
    ext_frames: Vec<ExtFrameSlot>,
    screencopy: Vec<ScreencopySlot>,
    lists: Vec<Resolved<ExtForeignToplevelListV1Id>>,
    handles: Vec<(Resolved<ExtForeignToplevelHandleV1Id>, ToplevelKey)>,
    pending: BTreeMap<ClientId, Vec<Pending>>,
}

impl CaptureRegistry {
    pub(crate) fn track_session(
        &mut self,
        session: Resolved<ExtImageCopyCaptureSessionV1Id>,
        target: CaptureTarget,
        pointer: Option<Resolved<WlPointerId>>,
    ) {
        self.sessions.push(SessionSlot { session, target, pointer });
    }

    pub(crate) fn track_ext_frame(
        &mut self,
        frame: Resolved<ExtImageCopyCaptureFrameV1Id>,
        target: CaptureTarget,
        pointer: Option<Resolved<WlPointerId>>,
    ) {
        self.ext_frames
            .push(ExtFrameSlot { frame, target, pointer });
    }

    pub(crate) fn track_screencopy(
        &mut self,
        frame: Resolved<ZwlrScreencopyFrameV1Id>,
        output_id: OutputId,
    ) {
        self.screencopy.push(ScreencopySlot { frame, output_id });
    }

    pub(crate) fn track_list(&mut self, list: Resolved<ExtForeignToplevelListV1Id>) {
        self.lists.push(list);
    }

    pub(crate) fn track_handle(
        &mut self,
        handle: Resolved<ExtForeignToplevelHandleV1Id>,
        key: ToplevelKey,
    ) {
        self.handles.push((handle, key));
    }

    pub(crate) fn forget_session(&mut self, session: Resolved<ExtImageCopyCaptureSessionV1Id>) {
        self.sessions.retain(|slot| slot.session != session);
        self.retain_pending(|item| !matches!(item, Pending::StopSession(id) if *id == session));
    }

    pub(crate) fn forget_ext_frame(&mut self, frame: Resolved<ExtImageCopyCaptureFrameV1Id>) {
        self.ext_frames.retain(|slot| slot.frame != frame);
        self.retain_pending(|item| !matches!(item, Pending::FailExtFrame(id) if *id == frame));
    }

    pub(crate) fn forget_screencopy(&mut self, frame: Resolved<ZwlrScreencopyFrameV1Id>) {
        self.screencopy.retain(|slot| slot.frame != frame);
        self.retain_pending(|item| !matches!(item, Pending::FailScreencopy(id) if *id == frame));
    }

    pub(crate) fn forget_list(&mut self, list: Resolved<ExtForeignToplevelListV1Id>) {
        self.lists.retain(|candidate| *candidate != list);
        self.announced.retain(|(candidate, _)| *candidate != list);
    }

    pub(crate) fn forget_handle(&mut self, handle: Resolved<ExtForeignToplevelHandleV1Id>) {
        self.handles.retain(|(candidate, _)| *candidate != handle);
        self.retain_pending(|item| !matches!(item, Pending::CloseHandle(id) if *id == handle));
    }

    /// Drop every capture object and queued event owned by a disconnected client.
    pub fn drop_client(&mut self, client_id: ClientId) {
        self.sessions
            .retain(|slot| slot.session.client_id() != client_id);
        self.ext_frames
            .retain(|slot| slot.frame.client_id() != client_id);
        self.screencopy
            .retain(|slot| slot.frame.client_id() != client_id);
        self.lists.retain(|list| list.client_id() != client_id);
        self.handles
            .retain(|(handle, _)| handle.client_id() != client_id);
        self.announced
            .retain(|(list, _)| list.client_id() != client_id);
        self.pending.remove(&client_id);
    }

    pub(crate) fn toplevel_is_open(&self, key: ToplevelKey) -> bool {
        self.toplevels
            .get(&key)
            .is_some_and(|record| !record.closed)
    }

    pub(crate) fn handles_for(
        &self,
        key: ToplevelKey,
    ) -> Vec<Resolved<ExtForeignToplevelHandleV1Id>> {
        self.handles
            .iter()
            .filter(|(_, candidate)| *candidate == key)
            .map(|(handle, _)| *handle)
            .collect()
    }

    fn retain_pending(&mut self, mut keep: impl FnMut(&Pending) -> bool) {
        for queue in self.pending.values_mut() {
            queue.retain(|item| keep(item));
        }
        self.pending.retain(|_, queue| !queue.is_empty());
    }

    fn enqueue(&mut self, client_id: ClientId, item: Pending) {
        self.pending.entry(client_id).or_default().push(item);
    }
}

pub(crate) fn target_is_live(state: &State, target: CaptureTarget) -> bool {
    match target {
        CaptureTarget::Output(output_id) => state.outputs.try_get(output_id).is_some(),
        CaptureTarget::Toplevel(key) => state.capture.toplevel_is_open(key),
    }
}

pub(crate) fn output_removed(state: &mut State, dispatcher: &mut Dispatcher, output_id: OutputId) {
    let target = CaptureTarget::Output(output_id);
    let mut clients = BTreeSet::new();
    let sessions: Vec<_> = state
        .capture
        .sessions
        .iter()
        .filter(|slot| slot.target == target)
        .copied()
        .collect();
    for slot in sessions {
        let client_id = slot.session.client_id();
        state
            .capture
            .enqueue(client_id, Pending::StopSession(slot.session));
        clients.insert(client_id);
    }
    state.capture.sessions.retain(|slot| slot.target != target);

    let frames: Vec<_> = state
        .capture
        .ext_frames
        .iter()
        .filter(|slot| slot.target == target)
        .copied()
        .collect();
    for slot in frames {
        let client_id = slot.frame.client_id();
        state
            .capture
            .enqueue(client_id, Pending::FailExtFrame(slot.frame));
        clients.insert(client_id);
    }
    state
        .capture
        .ext_frames
        .retain(|slot| slot.target != target);

    let screencopy: Vec<_> = state
        .capture
        .screencopy
        .iter()
        .filter(|slot| slot.output_id == output_id)
        .copied()
        .collect();
    for slot in screencopy {
        let client_id = slot.frame.client_id();
        state
            .capture
            .enqueue(client_id, Pending::FailScreencopy(slot.frame));
        clients.insert(client_id);
    }
    state
        .capture
        .screencopy
        .retain(|slot| slot.output_id != output_id);
    dispatch_deliver(dispatcher, clients);
}

pub(crate) fn pointer_released(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    pointer: Resolved<WlPointerId>,
) {
    let mut clients = BTreeSet::new();
    let sessions: Vec<_> = state
        .capture
        .sessions
        .iter()
        .filter(|slot| slot.pointer == Some(pointer))
        .copied()
        .collect();
    for slot in sessions {
        let client_id = slot.session.client_id();
        state
            .capture
            .enqueue(client_id, Pending::StopSession(slot.session));
        clients.insert(client_id);
    }
    state
        .capture
        .sessions
        .retain(|slot| slot.pointer != Some(pointer));

    let frames: Vec<_> = state
        .capture
        .ext_frames
        .iter()
        .filter(|slot| slot.pointer == Some(pointer))
        .copied()
        .collect();
    for slot in frames {
        let client_id = slot.frame.client_id();
        state
            .capture
            .enqueue(client_id, Pending::FailExtFrame(slot.frame));
        clients.insert(client_id);
    }
    state
        .capture
        .ext_frames
        .retain(|slot| slot.pointer != Some(pointer));
    dispatch_deliver(dispatcher, clients);
}

/// Create one toplevel identity and queue its initial events for every live list.
pub(crate) fn announce_toplevel(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    title: impl Into<String>,
    app_id: impl Into<String>,
) -> ToplevelKey {
    state.capture.next_toplevel = state.capture.next_toplevel.saturating_add(1);
    let key = ToplevelKey(state.capture.next_toplevel);
    let title = title.into();
    let app_id = app_id.into();
    debug_assert!(key.identifier().len() <= 32);
    debug_assert!(key
        .identifier()
        .bytes()
        .all(|byte| byte.is_ascii() && !byte.is_ascii_control()));
    state.capture.toplevels.insert(
        key,
        ToplevelRecord { closed: false, title: title.clone(), app_id: app_id.clone() },
    );
    let lists = state.capture.lists.clone();
    let mut clients = BTreeSet::new();
    for list in lists {
        let finished = list.try_ref(state).is_none_or(|list| list.is_finished());
        if finished {
            continue;
        }
        state.capture.enqueue(
            list.client_id(),
            Pending::Announce { key, title: title.clone(), app_id: app_id.clone() },
        );
        clients.insert(list.client_id());
    }
    dispatch_deliver(dispatcher, clients);
    key
}

/// Unmap a toplevel: close its handles and stop sessions that captured it.
pub(crate) fn close_toplevel(state: &mut State, dispatcher: &mut Dispatcher, key: ToplevelKey) {
    let already_closed = match state.capture.toplevels.get_mut(&key) {
        Some(record) if !record.closed => {
            record.closed = true;
            false
        }
        _ => true,
    };
    if already_closed {
        return;
    }
    state.capture.retain_pending(|item| match item {
        Pending::Announce { key: announced, .. }
        | Pending::Retitle { key: announced, .. }
        | Pending::Reapp { key: announced, .. } => *announced != key,
        _ => true,
    });
    state
        .capture
        .announced
        .retain(|(_, announced)| *announced != key);
    state
        .capture
        .by_xdg
        .retain(|_, announced| *announced != key);

    let mut clients = BTreeSet::new();
    let handles = state.capture.handles_for(key);
    for handle in handles {
        state
            .capture
            .enqueue(handle.client_id(), Pending::CloseHandle(handle));
        clients.insert(handle.client_id());
    }
    let target = CaptureTarget::Toplevel(key);
    let sessions: Vec<_> = state
        .capture
        .sessions
        .iter()
        .filter(|slot| slot.target == target)
        .copied()
        .collect();
    for slot in sessions {
        state
            .capture
            .enqueue(slot.session.client_id(), Pending::StopSession(slot.session));
        clients.insert(slot.session.client_id());
    }
    state.capture.sessions.retain(|slot| slot.target != target);
    let frames: Vec<_> = state
        .capture
        .ext_frames
        .iter()
        .filter(|slot| slot.target == target)
        .copied()
        .collect();
    for slot in frames {
        state
            .capture
            .enqueue(slot.frame.client_id(), Pending::FailExtFrame(slot.frame));
        clients.insert(slot.frame.client_id());
    }
    state
        .capture
        .ext_frames
        .retain(|slot| slot.target != target);
    dispatch_deliver(dispatcher, clients);
}

pub fn deliver(client: &mut Client, state: &mut State, dispatcher: &mut ClientDispatcher) {
    let Some(pending) = state.capture.pending.remove(&client.client_id()) else {
        return;
    };
    for item in pending {
        match item {
            Pending::StopSession(session) => {
                if let Some(session) = session.try_ref_mut(state) {
                    session.mark_stopped(dispatcher);
                }
            }
            Pending::FailExtFrame(frame) => {
                if let Some(frame) = frame.try_ref_mut(state) {
                    frame.fail_stopped(dispatcher);
                }
            }
            Pending::FailScreencopy(frame) => {
                if let Some(frame) = frame.try_ref_mut(state) {
                    frame.fail(dispatcher);
                }
            }
            Pending::CloseHandle(handle) => {
                if let Some(handle) = handle.try_ref_mut(state) {
                    handle.send_closed(dispatcher);
                }
            }
            Pending::Announce { key, title, app_id } => {
                if state.capture.toplevel_is_open(key) {
                    announce_to_client(client, state, dispatcher, key, &title, &app_id);
                }
            }
            Pending::Retitle { key, title } => {
                for handle in state.capture.handles_for(key) {
                    if let Some(handle) = handle.try_ref_mut(state) {
                        handle.send_title(&title, dispatcher);
                    }
                }
            }
            Pending::Reapp { key, app_id } => {
                for handle in state.capture.handles_for(key) {
                    if let Some(handle) = handle.try_ref_mut(state) {
                        handle.send_app_id(&app_id, dispatcher);
                    }
                }
            }
        }
    }
}

fn announce_to_client(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    key: ToplevelKey,
    title: &str,
    app_id: &str,
) {
    let lists: Vec<_> = state
        .capture
        .lists
        .iter()
        .copied()
        .filter(|list| list.client_id() == client.client_id())
        .filter(|list| list.try_ref(state).is_some_and(|list| !list.is_finished()))
        .collect();
    let lists: Vec<_> = lists
        .into_iter()
        .filter(|list| !state.capture.announced.contains(&(*list, key)))
        .collect();
    for list in lists {
        let handle_id = client.allocate_server_object_id::<ExtForeignToplevelHandleV1Id>();
        let (handle, _) = client.insert_object(
            state,
            ExtForeignToplevelHandleV1::new(handle_id, key, title.to_string(), app_id.to_string()),
        );
        state.capture.track_handle(handle, key);
        state.capture.announced.insert((list, key));
        list.rref(state).send_toplevel(handle_id, dispatcher);
        handle.rref(state).send_initial(dispatcher);
    }
}

/// Publish one mapped XDG toplevel. A later map of the same toplevel gets a new identifier.
pub(crate) fn map_xdg_toplevel(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    toplevel: Resolved<XdgToplevelId>,
    title: impl Into<String>,
    app_id: impl Into<String>,
) {
    if state.capture.by_xdg.contains_key(&toplevel) {
        return;
    }
    let key = announce_toplevel(state, dispatcher, title, app_id);
    state.capture.by_xdg.insert(toplevel, key);
}

/// Stop listing and capturing an XDG toplevel that unmapped or was destroyed.
pub(crate) fn unmap_xdg_toplevel(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    toplevel: Resolved<XdgToplevelId>,
) {
    let Some(key) = state.capture.by_xdg.remove(&toplevel) else {
        return;
    };
    close_toplevel(state, dispatcher, key);
}

pub(crate) fn retitle_xdg_toplevel(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    toplevel: Resolved<XdgToplevelId>,
    title: String,
) {
    update_xdg_detail(state, dispatcher, toplevel, true, title);
}

pub(crate) fn reapp_xdg_toplevel(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    toplevel: Resolved<XdgToplevelId>,
    app_id: String,
) {
    update_xdg_detail(state, dispatcher, toplevel, false, app_id);
}

fn update_xdg_detail(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    toplevel: Resolved<XdgToplevelId>,
    title: bool,
    value: String,
) {
    let Some(key) = state.capture.by_xdg.get(&toplevel).copied() else {
        return;
    };
    let Some(record) = state.capture.toplevels.get_mut(&key) else {
        return;
    };
    if record.closed {
        return;
    }
    if title {
        if record.title == value {
            return;
        }
        record.title = value.clone();
    } else {
        if record.app_id == value {
            return;
        }
        record.app_id = value.clone();
    }
    let clients: BTreeSet<_> = state
        .capture
        .handles_for(key)
        .into_iter()
        .map(|handle| handle.client_id())
        .collect();
    for client_id in clients.iter().copied() {
        let item = if title {
            Pending::Retitle { key, title: value.clone() }
        } else {
            Pending::Reapp { key, app_id: value.clone() }
        };
        state.capture.enqueue(client_id, item);
    }
    dispatch_deliver(dispatcher, clients);
}

/// Send every currently mapped toplevel to a list that just bound.
pub(crate) fn sync_mapped_toplevels(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    list: Resolved<ExtForeignToplevelListV1Id>,
) {
    if list.try_ref(state).is_none_or(|list| list.is_finished()) {
        return;
    }
    let open: Vec<_> = state
        .capture
        .toplevels
        .iter()
        .filter(|(_, record)| !record.closed)
        .map(|(key, record)| (*key, record.title.clone(), record.app_id.clone()))
        .collect();
    if open.is_empty() {
        return;
    }
    let client_id = list.client_id();
    for (key, title, app_id) in open {
        state
            .capture
            .enqueue(client_id, Pending::Announce { key, title, app_id });
    }
    dispatch_deliver(dispatcher, BTreeSet::from([client_id]));
}

fn dispatch_deliver(dispatcher: &mut Dispatcher, clients: BTreeSet<ClientId>) {
    for client_id in clients {
        dispatcher.dispatch_to(client_id, ClientAction::DeliverCaptureEvents);
    }
}

#[cfg(test)]
pub(crate) fn test_request(object: u32, opcode: u16, words: u16) -> crate::clients::RawEvent {
    crate::clients::RawEvent::new(
        crate::protocol_generated::WaylandId::from(object),
        opcode,
        words.into(),
    )
}

#[cfg(test)]
pub(crate) fn test_dispatch<K>(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut Dispatcher,
    object: K,
    event: crate::clients::RawEvent,
) where
    K: crate::object::IdToObject + Into<crate::protocol_generated::WaylandId> + Copy,
{
    use crate::object::Object;
    let mut client_dispatcher = client.test_dispatcher(dispatcher);
    client_dispatcher.set_requester_object_id_for_tests(object.into());
    client
        .grab_object(object, state)
        .on(
            client,
            state,
            crate::test_support::raw_request_from_event(event),
            &mut client_dispatcher,
        )
        .unwrap();
}

#[cfg(test)]
pub(crate) fn test_sent(dispatcher: &Dispatcher, object: u32, opcode: u32) -> bool {
    use crate::io_uring::model::IoUringAction;
    dispatcher.test_has_queued_action::<IoUringAction>(|action| match action {
        IoUringAction::Sendmsg { buffer, .. } => {
            buffer.len() >= 8
                && u32::from_ne_bytes(buffer[0..4].try_into().unwrap()) == object
                && u32::from_ne_bytes(buffer[4..8].try_into().unwrap()) & 0xffff == opcode
        }
        _ => false,
    })
}

#[cfg(test)]
pub(crate) fn test_sent_arg(dispatcher: &Dispatcher, object: u32, opcode: u32, value: u32) -> bool {
    use crate::io_uring::model::IoUringAction;
    dispatcher.test_has_queued_action::<IoUringAction>(|action| match action {
        IoUringAction::Sendmsg { buffer, .. } => {
            buffer.len() >= 12
                && u32::from_ne_bytes(buffer[0..4].try_into().unwrap()) == object
                && u32::from_ne_bytes(buffer[4..8].try_into().unwrap()) & 0xffff == opcode
                && u32::from_ne_bytes(buffer[8..12].try_into().unwrap()) == value
        }
        _ => false,
    })
}

#[cfg(test)]
pub(crate) fn test_closed(dispatcher: &Dispatcher, client: &Client) -> bool {
    dispatcher.test_has_queued_action::<crate::clients::ClientManagerAction>(|action| {
        matches!(action, crate::clients::ClientManagerAction::Close { id } if *id == client.client_id())
    })
}

#[cfg(test)]
pub(crate) fn test_deliver(client: &mut Client, state: &mut State, dispatcher: &mut Dispatcher) {
    use crate::runner::Model;
    client
        .process(state, ClientAction::DeliverCaptureEvents, dispatcher)
        .unwrap();
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            clients::{ClientAction, RawEvent},
            protocol_generated::{
                Constants, ExtForeignToplevelImageCaptureSourceManagerV1Id,
                ExtForeignToplevelListV1Id, ExtImageCaptureSourceV1Id,
                ExtImageCopyCaptureCursorSessionV1Id, ExtImageCopyCaptureFrameV1Id,
                ExtImageCopyCaptureManagerV1Id, ExtImageCopyCaptureSessionV1Id,
                ExtOutputImageCaptureSourceManagerV1Id, WlBufferId, WlOutputId, WlPointerId,
                ZwlrScreencopyFrameV1Id, ZwlrScreencopyManagerV1Id,
            },
            runner::Model,
            test_support::{state_for_tests, test_client_and_dispatcher, test_output_drm1},
            wayland_objects::{
                ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
                ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
                ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1,
                ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1,
                ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1,
                ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
                ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
                wl_buffer::{NewWlBufferParams, WlBuffer},
                wl_output::WlOutput,
                wl_pointer::WlPointer,
                zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
                zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
            },
        },
    };

    const OUTPUT_OBJECT: u32 = 20;
    const BUFFER: u32 = 21;

    fn client_state() -> (crate::clients::Client, Dispatcher, State) {
        let (client, dispatcher) = test_client_and_dispatcher(1);
        (client, dispatcher, state_for_tests())
    }

    fn insert_output(client: &mut crate::clients::Client, state: &mut State) -> OutputId {
        let output_id = state.insert_output(test_output_drm1(0, 0, 640, 480));
        client.insert_object(state, WlOutput::new(WlOutputId::from(OUTPUT_OBJECT), output_id, 4));
        output_id
    }

    fn insert_buffer(client: &mut crate::clients::Client, state: &mut State) {
        let bytes = vec![0u8; 16];
        client.insert_object(
            state,
            WlBuffer::new(NewWlBufferParams {
                id: WlBufferId::from(BUFFER),
                owner_client_id: client.client_id(),
                offset: 0,
                width: 2,
                height: 2,
                stride: 8,
                format: &crate::drm::ARGB8888,
                mem: crate::wayland_objects::wl_shm_pool::shared_memory_from_bytes(&bytes),
            })
            .unwrap(),
        );
    }

    fn with_id(
        mut event: RawEvent,
        id: impl Into<crate::protocol_generated::WaylandId>,
    ) -> RawEvent {
        event.id(id.into());
        event
    }

    #[test]
    fn capture_globals_stay_unpublished() {
        let state = state_for_tests();
        for interface in [
            ZwlrScreencopyManagerV1::INTERFACE_NAME,
            ExtImageCopyCaptureManagerV1::INTERFACE_NAME,
            ExtOutputImageCaptureSourceManagerV1::INTERFACE_NAME,
            ExtForeignToplevelImageCaptureSourceManagerV1::INTERFACE_NAME,
            ExtForeignToplevelListV1::INTERFACE_NAME,
        ] {
            assert!(!state.globals.contains_interface(interface), "{interface}");
        }
    }

    #[test]
    fn screencopy_copy_is_one_shot_and_survives_manager_destruction() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let _output_id = insert_output(&mut client, &mut state);
        insert_buffer(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrScreencopyManagerV1::new(ZwlrScreencopyManagerV1Id::from(8), 3),
        );
        let mut event = test_request(8, 0, 3);
        event.id(ZwlrScreencopyFrameV1Id::from(9));
        event.int(1);
        event.id(WlOutputId::from(OUTPUT_OBJECT));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyManagerV1Id::from(8),
            event,
        );
        let frame = client
            .resolve(ZwlrScreencopyFrameV1Id::from(9), &state)
            .unwrap()
            .rref(&state);
        assert!(frame.overlay_cursor());
        assert!(frame.region().is_none());
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyManagerV1Id::from(8),
            test_request(8, 2, 0),
        );
        assert!(client
            .resolve(ZwlrScreencopyFrameV1Id::from(9), &state)
            .is_some());
        let copy = with_id(test_request(9, 0, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyFrameV1Id::from(9),
            copy,
        );
        assert!(!test_closed(&dispatcher, &client));
        let copy_again = with_id(test_request(9, 0, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyFrameV1Id::from(9),
            copy_again,
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn screencopy_region_is_kept_and_a_new_frame_can_replace_a_destroyed_one() {
        let (mut client, mut dispatcher, mut state) = client_state();
        insert_output(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrScreencopyManagerV1::new(ZwlrScreencopyManagerV1Id::from(8), 3),
        );
        let mut event = test_request(8, 1, 7);
        event.id(ZwlrScreencopyFrameV1Id::from(9));
        event.int(0);
        event.id(WlOutputId::from(OUTPUT_OBJECT));
        event.int(4);
        event.int(6);
        event.int(20);
        event.int(10);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyManagerV1Id::from(8),
            event,
        );
        assert_eq!(
            client
                .resolve(ZwlrScreencopyFrameV1Id::from(9), &state)
                .unwrap()
                .rref(&state)
                .region(),
            Some((4, 6, 20, 10))
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyFrameV1Id::from(9),
            test_request(9, 1, 0),
        );
        assert!(state.capture.screencopy.is_empty());
        let mut event = test_request(8, 0, 3);
        event.id(ZwlrScreencopyFrameV1Id::from(10));
        event.int(0);
        event.id(WlOutputId::from(OUTPUT_OBJECT));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyManagerV1Id::from(8),
            event,
        );
        assert!(client
            .resolve(ZwlrScreencopyFrameV1Id::from(10), &state)
            .is_some());
    }

    #[test]
    fn losing_the_output_fails_an_unfinished_screencopy_frame() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = insert_output(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrScreencopyManagerV1::new(ZwlrScreencopyManagerV1Id::from(8), 3),
        );
        let mut event = test_request(8, 0, 3);
        event.id(ZwlrScreencopyFrameV1Id::from(9));
        event.int(0);
        event.id(WlOutputId::from(OUTPUT_OBJECT));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyManagerV1Id::from(8),
            event,
        );
        assert!(state.remove_output(output_id, &mut dispatcher).is_some());
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(test_sent(&dispatcher, 9, 3));
        assert!(state.capture.screencopy.is_empty());
    }

    #[test]
    fn copy_with_damage_on_version_one_is_rejected() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = insert_output(&mut client, &mut state);
        insert_buffer(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrScreencopyFrameV1::new(ZwlrScreencopyFrameV1Id::from(9), 1, output_id, false, None),
        );
        let copy = with_id(test_request(9, 2, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyFrameV1Id::from(9),
            copy,
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn client_disconnect_drops_screencopy_tracking() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = insert_output(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrScreencopyFrameV1::new(ZwlrScreencopyFrameV1Id::from(9), 3, output_id, false, None),
        );
        state.capture.track_screencopy(
            client
                .resolve(ZwlrScreencopyFrameV1Id::from(9), &state)
                .unwrap(),
            output_id,
        );
        client
            .process(&mut state, ClientAction::Close, &mut dispatcher)
            .unwrap();
        assert!(state.capture.screencopy.is_empty());
        assert!(state.capture.pending.is_empty());
    }

    fn output_source(
        client: &mut crate::clients::Client,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) {
        insert_output(client, state);
        client.insert_object(
            state,
            ExtOutputImageCaptureSourceManagerV1::new(
                ExtOutputImageCaptureSourceManagerV1Id::from(30),
            ),
        );
        let mut event = test_request(30, 0, 2);
        event.id(ExtImageCaptureSourceV1Id::from(31));
        event.id(WlOutputId::from(OUTPUT_OBJECT));
        test_dispatch(
            client,
            state,
            dispatcher,
            ExtOutputImageCaptureSourceManagerV1Id::from(30),
            event,
        );
    }

    #[test]
    fn invalid_capture_option_disconnects() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(32)),
        );
        let mut event = test_request(32, 0, 3);
        event.id(ExtImageCopyCaptureSessionV1Id::from(33));
        event.id(ExtImageCaptureSourceV1Id::from(31));
        event.uint(2);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(32),
            event,
        );
        assert!(test_closed(&dispatcher, &client));
        assert!(client
            .resolve(ExtImageCopyCaptureSessionV1Id::from(33), &state)
            .is_none());
    }

    #[test]
    fn a_second_frame_on_the_same_session_disconnects() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        insert_buffer(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(32)),
        );
        let mut event = test_request(32, 0, 3);
        event.id(ExtImageCopyCaptureSessionV1Id::from(33));
        event.id(ExtImageCaptureSourceV1Id::from(31));
        event.uint(1);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(32),
            event,
        );
        assert!(client
            .resolve(ExtImageCopyCaptureSessionV1Id::from(33), &state)
            .unwrap()
            .rref(&state)
            .paint_cursors());
        let mut event = test_request(33, 0, 1);
        event.id(ExtImageCopyCaptureFrameV1Id::from(34));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(33),
            event,
        );
        let mut again = test_request(33, 0, 1);
        again.id(ExtImageCopyCaptureFrameV1Id::from(35));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(33),
            again,
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn capture_without_a_buffer_disconnects() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        insert_buffer(&mut client, &mut state);
        let output_id = state.outputs.iter().next().unwrap().0;
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureSessionV1::new(
                ExtImageCopyCaptureSessionV1Id::from(33),
                CaptureTarget::Output(output_id),
                false,
                None,
            ),
        );
        let session = client
            .resolve(ExtImageCopyCaptureSessionV1Id::from(33), &state)
            .unwrap();
        state
            .capture
            .track_session(session, session.rref(&state).target(), None);
        let mut event = test_request(33, 0, 1);
        event.id(ExtImageCopyCaptureFrameV1Id::from(34));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(33),
            event,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            test_request(34, 3, 0),
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn invalid_buffer_damage_disconnects() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = insert_output(&mut client, &mut state);
        insert_buffer(&mut client, &mut state);
        let session_id = ExtImageCopyCaptureSessionV1Id::from(33);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureSessionV1::new(
                session_id,
                CaptureTarget::Output(output_id),
                false,
                None,
            ),
        );
        let session = client.resolve(session_id, &state).unwrap();
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureFrameV1::new(ExtImageCopyCaptureFrameV1Id::from(34), session),
        );
        let mut damage = test_request(34, 2, 4);
        damage.int(-1);
        damage.int(0);
        damage.int(1);
        damage.int(1);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            damage,
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn output_loss_stops_the_session_and_fails_a_capturing_frame() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = insert_output(&mut client, &mut state);
        insert_buffer(&mut client, &mut state);
        let session_id = ExtImageCopyCaptureSessionV1Id::from(33);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureSessionV1::new(
                session_id,
                CaptureTarget::Output(output_id),
                false,
                None,
            ),
        );
        let session = client.resolve(session_id, &state).unwrap();
        state
            .capture
            .track_session(session, CaptureTarget::Output(output_id), None);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureFrameV1::new(ExtImageCopyCaptureFrameV1Id::from(34), session),
        );
        let frame = client
            .resolve(ExtImageCopyCaptureFrameV1Id::from(34), &state)
            .unwrap();
        state
            .capture
            .track_ext_frame(frame, CaptureTarget::Output(output_id), None);
        let attach = with_id(test_request(34, 1, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            attach,
        );
        let mut damage = test_request(34, 2, 4);
        damage.int(0);
        damage.int(0);
        damage.int(2);
        damage.int(2);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            damage,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            test_request(34, 3, 0),
        );
        assert!(!test_closed(&dispatcher, &client));
        assert!(state.remove_output(output_id, &mut dispatcher).is_some());
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(test_sent(&dispatcher, 33, 5));
        assert!(test_sent_arg(&dispatcher, 34, 4, 2));
    }

    #[test]
    fn destroying_a_source_leaves_the_session_usable_until_the_output_is_gone() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(32)),
        );
        let mut event = test_request(32, 0, 3);
        event.id(ExtImageCopyCaptureSessionV1Id::from(33));
        event.id(ExtImageCaptureSourceV1Id::from(31));
        event.uint(0);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(32),
            event,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCaptureSourceV1Id::from(31),
            test_request(31, 0, 0),
        );
        assert!(client
            .resolve(ExtImageCopyCaptureSessionV1Id::from(33), &state)
            .is_some());
        assert_eq!(state.capture.sessions.len(), 1);
    }

    #[test]
    fn cursor_capture_sessions_cannot_be_requested_twice() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        client.insert_object(&mut state, WlPointer::new(WlPointerId::from(40), 3));
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(32)),
        );
        let mut event = test_request(32, 1, 3);
        event.id(ExtImageCopyCaptureCursorSessionV1Id::from(41));
        event.id(ExtImageCaptureSourceV1Id::from(31));
        event.id(WlPointerId::from(40));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(32),
            event,
        );
        let mut event = test_request(41, 1, 1);
        event.id(ExtImageCopyCaptureSessionV1Id::from(33));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureCursorSessionV1Id::from(41),
            event,
        );
        let mut again = test_request(41, 1, 1);
        again.id(ExtImageCopyCaptureSessionV1Id::from(42));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureCursorSessionV1Id::from(41),
            again,
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn destroying_an_ext_frame_allows_another_and_a_second_capture_is_rejected() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = insert_output(&mut client, &mut state);
        insert_buffer(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureSessionV1::new(
                ExtImageCopyCaptureSessionV1Id::from(33),
                CaptureTarget::Output(output_id),
                false,
                None,
            ),
        );
        let mut event = test_request(33, 0, 1);
        event.id(ExtImageCopyCaptureFrameV1Id::from(34));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(33),
            event,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            test_request(34, 0, 0),
        );
        let mut event = test_request(33, 0, 1);
        event.id(ExtImageCopyCaptureFrameV1Id::from(35));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(33),
            event,
        );
        assert!(client
            .resolve(ExtImageCopyCaptureFrameV1Id::from(35), &state)
            .is_some());
        let attach = with_id(test_request(35, 1, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(35),
            attach,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(35),
            test_request(35, 3, 0),
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(35),
            test_request(35, 3, 0),
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn pointer_release_stops_a_cursor_capture_session() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        client.insert_object(&mut state, WlPointer::new(WlPointerId::from(40), 3));
        let pointer = client.resolve(WlPointerId::from(40), &state).unwrap();
        let output_id = state.outputs.iter().next().unwrap().0;
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureCursorSessionV1::new(
                ExtImageCopyCaptureCursorSessionV1Id::from(41),
                CaptureTarget::Output(output_id),
                pointer,
            ),
        );
        let mut event = test_request(41, 1, 1);
        event.id(ExtImageCopyCaptureSessionV1Id::from(33));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureCursorSessionV1Id::from(41),
            event,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            WlPointerId::from(40),
            test_request(40, 1, 0),
        );
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(test_sent(&dispatcher, 33, 5));
    }

    #[test]
    fn closing_a_toplevel_closes_its_handle_and_stops_capture() {
        let (mut client, mut dispatcher, mut state) = client_state();
        ExtForeignToplevelListV1::attach(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelListV1Id::from(50),
        );
        let key = announce_toplevel(&mut state, &mut dispatcher, "Terminal", "app");
        test_deliver(&mut client, &mut state, &mut dispatcher);
        let handle = state.capture.handles_for(key)[0];
        let handle_id: u32 = handle.wayland_id().into();
        assert!(test_sent(&dispatcher, handle_id, 4));
        assert!(!handle.rref(&state).is_closed());
        client.insert_object(
            &mut state,
            ExtForeignToplevelImageCaptureSourceManagerV1::new(
                ExtForeignToplevelImageCaptureSourceManagerV1Id::from(51),
            ),
        );
        let mut event = test_request(51, 0, 2);
        event.id(ExtImageCaptureSourceV1Id::from(52));
        event.id(handle.wayland_id());
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelImageCaptureSourceManagerV1Id::from(51),
            event,
        );
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(53)),
        );
        insert_buffer(&mut client, &mut state);
        let mut event = test_request(53, 0, 3);
        event.id(ExtImageCopyCaptureSessionV1Id::from(54));
        event.id(ExtImageCaptureSourceV1Id::from(52));
        event.uint(0);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(53),
            event,
        );
        let mut event = test_request(54, 0, 1);
        event.id(ExtImageCopyCaptureFrameV1Id::from(55));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(54),
            event,
        );
        let attach = with_id(test_request(55, 1, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(55),
            attach,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(55),
            test_request(55, 3, 0),
        );
        close_toplevel(&mut state, &mut dispatcher, key);
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(test_sent(&dispatcher, handle_id, 0));
        assert!(handle.rref(&state).is_closed());
        assert!(test_sent(&dispatcher, 54, 5));
        assert!(test_sent_arg(&dispatcher, 55, 4, 2));
        let mut event = test_request(51, 0, 2);
        event.id(ExtImageCaptureSourceV1Id::from(56));
        event.id(handle.wayland_id());
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelImageCaptureSourceManagerV1Id::from(51),
            event,
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn stopping_a_toplevel_list_sends_finished_once() {
        let (mut client, mut dispatcher, mut state) = client_state();
        ExtForeignToplevelListV1::attach(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelListV1Id::from(50),
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelListV1Id::from(50),
            test_request(50, 0, 0),
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelListV1Id::from(50),
            test_request(50, 0, 0),
        );
        assert!(test_sent(&dispatcher, 50, 1));
        let key = announce_toplevel(&mut state, &mut dispatcher, "Hidden", "app");
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(state.capture.handles_for(key).is_empty());
    }

    fn insert_toplevel(
        client: &mut crate::clients::Client,
        state: &mut State,
    ) -> Resolved<crate::protocol_generated::XdgToplevelId> {
        client
            .insert_object(
                state,
                crate::wayland_objects::xdg_toplevel::XdgToplevel::new(
                    crate::protocol_generated::XdgToplevelId::from(60),
                    Resolved::new(
                        client.client_id(),
                        crate::protocol_generated::XdgSurfaceId::from(1),
                        crate::state::ObjectId::from(1usize),
                    ),
                    7,
                ),
            )
            .0
    }

    #[test]
    fn mapping_a_toplevel_publishes_its_identity_and_unmap_stops_capture() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let toplevel = insert_toplevel(&mut client, &mut state);
        ExtForeignToplevelListV1::attach(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelListV1Id::from(50),
        );
        map_xdg_toplevel(&mut state, &mut dispatcher, toplevel, "Terminal", "app");
        test_deliver(&mut client, &mut state, &mut dispatcher);
        let key = state.capture.by_xdg[&toplevel];
        let handle = state.capture.handles_for(key)[0];
        let handle_id: u32 = handle.wayland_id().into();
        assert_eq!(handle.rref(&state).title(), "Terminal");
        assert_eq!(handle.rref(&state).identifier(), key.identifier());
        assert!(test_sent(&dispatcher, handle_id, 4));
        assert!(test_sent(&dispatcher, handle_id, 1));

        let mut event = test_request(60, 2, 1);
        event.string("Next");
        test_dispatch(&mut client, &mut state, &mut dispatcher, toplevel.wayland_id(), event);
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert_eq!(handle.rref(&state).title(), "Next");
        retitle_xdg_toplevel(&mut state, &mut dispatcher, toplevel, "Next".to_string());
        assert!(state.capture.pending.get(&client.client_id()).is_none());

        client.insert_object(
            &mut state,
            ExtForeignToplevelImageCaptureSourceManagerV1::new(
                ExtForeignToplevelImageCaptureSourceManagerV1Id::from(51),
            ),
        );
        let mut event = test_request(51, 0, 2);
        event.id(ExtImageCaptureSourceV1Id::from(52));
        event.id(handle.wayland_id());
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelImageCaptureSourceManagerV1Id::from(51),
            event,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelImageCaptureSourceManagerV1Id::from(51),
            test_request(51, 1, 0),
        );
        assert!(client
            .resolve(ExtImageCaptureSourceV1Id::from(52), &state)
            .is_some());
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(53)),
        );
        let mut event = test_request(53, 0, 3);
        event.id(ExtImageCopyCaptureSessionV1Id::from(54));
        event.id(ExtImageCaptureSourceV1Id::from(52));
        event.uint(0);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(53),
            event,
        );
        let first_identifier = handle.rref(&state).identifier().to_string();
        unmap_xdg_toplevel(&mut state, &mut dispatcher, toplevel);
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(handle.rref(&state).is_closed());
        assert!(test_sent(&dispatcher, 54, 5));

        map_xdg_toplevel(&mut state, &mut dispatcher, toplevel, "Next", "app");
        test_deliver(&mut client, &mut state, &mut dispatcher);
        let remapped = state.capture.by_xdg[&toplevel];
        let again = state.capture.handles_for(remapped)[0];
        assert_ne!(again.rref(&state).identifier(), first_identifier);
        assert_ne!(remapped, key);
    }

    #[test]
    fn a_later_list_receives_mapped_toplevels_without_replacing_old_handles() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let toplevel = insert_toplevel(&mut client, &mut state);
        map_xdg_toplevel(&mut state, &mut dispatcher, toplevel, "Terminal", "app");
        let first = ExtForeignToplevelListV1::attach(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelListV1Id::from(50),
        );
        test_deliver(&mut client, &mut state, &mut dispatcher);
        let key = state.capture.by_xdg[&toplevel];
        assert_eq!(state.capture.handles_for(key).len(), 1);
        let original = state.capture.handles_for(key)[0];
        ExtForeignToplevelListV1::attach(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelListV1Id::from(61),
        );
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert_eq!(state.capture.handles_for(key).len(), 2);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            original.wayland_id(),
            test_request(original.wayland_id().into(), 0, 0),
        );
        assert_eq!(state.capture.handles_for(key).len(), 1);
        sync_mapped_toplevels(&mut state, &mut dispatcher, first);
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert_eq!(state.capture.handles_for(key).len(), 1);
        assert!(test_sent(&dispatcher, 50, 0));
        assert!(test_sent(&dispatcher, 61, 0));
    }

    #[test]
    fn destroying_an_output_source_manager_leaves_the_source() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtOutputImageCaptureSourceManagerV1Id::from(30),
            test_request(30, 1, 0),
        );
        assert!(client
            .resolve(ExtOutputImageCaptureSourceManagerV1Id::from(30), &state)
            .is_none());
        assert!(client
            .resolve(ExtImageCaptureSourceV1Id::from(31), &state)
            .is_some());
    }
}
