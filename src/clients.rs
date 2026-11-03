use std::{
    any::TypeId,
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    os::{
        fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd},
        unix::net::UnixListener,
    },
    path::PathBuf,
    rc::Rc,
};

use libc::getsockopt;

use crate::{
    dbus::Dbus,
    drm::{kick_visual_animation_pump, request_active_output, request_output},
    io_uring::{
        model::IoUringAction,
        ring::{AcceptCompleted, RecvmsgCompleted, RingId},
    },
    object::{AbortRequest, IdToObject, Object, ObjectWithId, Parser, ParserError, PostAction},
    protocol_generated::{
        wl_data_device_manager::DndAction, wl_data_offer, wl_display, wl_pointer, Array, Constants,
        ExtDataControlSourceV1Id, ExtWorkspaceManagerV1Id, HyprlandCtmControlManagerV1Id,
        WaylandId, WlCallbackId, WlDataOfferId, WlDataSourceId, WlOutputId, WlRegistryId,
        WlSurfaceId, WpPresentationFeedbackId, XdgPopupId, XdgToplevelId,
        ZwlrDataControlSourceV1Id, ZwlrGammaControlV1Id, ZwlrLayerSurfaceV1Id,
        ZwpPrimarySelectionSourceV1Id, ZxdgOutputV1Id,
    },
    region::RectI,
    runner::{Dispatcher, Model, ModelInstanceId, Redispatcher},
    scene::visibility::root_has_protocol_visibility,
    seat::SeatClientEvent,
    state::{DataOfferRole, DrmId, GrabbedObject, ObjectId, OutputId, SelectionSourceState, State},
    sync::AcquireReadyWaiterId,
    udev::Udev,
    wayland_objects::{
        ext_data_control_offer_v1::{
            create_ext_data_control_offer, create_ext_data_control_primary_offer,
            inert_ext_data_control_offer,
        },
        ext_workspace_manager_v1::publish_workspace_state_changed,
        hyprland_ctm_control_manager_v1::HyprlandCtmControlManagerV1,
        wl_data_offer::WlDataOffer,
        wl_display::WlDisplay,
        wl_output::WlOutput,
        wp_presentation_feedback::WpPresentationFeedback,
        zwlr_data_control_offer_v1::{
            create_data_control_offer, create_data_control_primary_offer, inert_data_control_offer,
        },
        zwlr_gamma_control_v1::ZwlrGammaControlV1,
        zwlr_layer_surface_v1::relayout_client_layer_surfaces,
        zwp_primary_selection_offer_v1::{
            create_primary_selection_offer, inert_primary_selection_offer,
        },
        zxdg_output_v1::ZxdgOutputV1,
    },
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, derive_more::Into)]
pub struct ClientId(u64);

#[cfg(test)]
impl ClientId {
    pub(crate) fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Resolved<K> {
    client_id: ClientId,
    wayland_id: K,
    object_id: ObjectId,
}

impl<K> Resolved<K> {
    pub fn new(client_id: ClientId, wayland_id: K, object_id: ObjectId) -> Self {
        Self { client_id, wayland_id, object_id }
    }

    pub fn client_id(self) -> ClientId {
        self.client_id
    }

    pub fn object_id(self) -> ObjectId {
        self.object_id
    }

    pub fn wayland_id(&self) -> K
    where
        K: Copy,
    {
        self.wayland_id
    }
}

impl<K> Resolved<K>
where
    K: IdToObject,
{
    pub fn try_ref<'a>(self, state: &'a State) -> Option<&'a K::Object> {
        state.objects.try_ref_object::<K::Object>(self.object_id)
    }
    pub fn rref<'a>(self, state: &'a State) -> &'a K::Object {
        state
            .objects
            .try_ref_object::<K::Object>(self.object_id)
            .unwrap()
    }

    pub fn try_ref_mut<'a>(self, state: &'a mut State) -> Option<&'a mut K::Object> {
        state
            .objects
            .try_ref_object_mut::<K::Object>(self.object_id)
    }

    pub fn ref_mut<'a>(self, state: &'a mut State) -> &'a mut K::Object {
        state
            .objects
            .try_ref_object_mut::<K::Object>(self.object_id)
            .unwrap()
    }

    pub fn try_grab(self, state: &mut State) -> Option<GrabbedObject<K::Object>> {
        state.objects.try_grab_object::<K::Object>(self.object_id)
    }

    pub fn grab(self, state: &mut State) -> GrabbedObject<K::Object> {
        state
            .objects
            .try_grab_object::<K::Object>(self.object_id)
            .unwrap()
    }

    pub fn exists(&self, state: &State) -> bool {
        state.objects.contains(self.object_id)
    }
}

pub const WAYLAND_DISPLAY_NAME: &str = "my-wayland-0";

pub struct ClientManager {
    current_id: u64,
    fd: UnixListener,
    path: PathBuf,
    set: BTreeSet<ClientId>,
    accept_op_id: Option<RingId>,
    accepting: bool,
    unlinked: bool,
}

#[derive(Debug)]
pub struct SocketBindError {
    path: PathBuf,
    source: io::Error,
}

impl std::fmt::Display for SocketBindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "bind Wayland socket {}: {}", self.path.display(), self.source)
    }
}

impl std::error::Error for SocketBindError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Debug)]
pub enum ClientManagerAction {
    Start,
    RearmAccept,
    NewClient { new_client: RawFd },
    LibinputEvent,
    UdevEvent,
    RegistryGlobalAdded { name: u32, interface: String, version: u32 },
    RegistryGlobalRemoved { name: u32 },
    OutputStateChanged { output_id: OutputId },
    OutputRemoved { output_id: OutputId },
    GammaControlFailed { output_id: OutputId },
    DiscardSurfacePresentationFeedbacksForBackend { drm_id: DrmId },
    PublishWorkspaceStateChanged { affected_outputs: Vec<OutputId> },
    WorkspaceStateChanged,
    LayerShellLayoutChanged,
    Close { id: ClientId },
    Stop,
}

impl ClientManager {
    pub fn new() -> Result<Box<Self>, SocketBindError> {
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let path = PathBuf::from(runtime).join(WAYLAND_DISPLAY_NAME);
        Self::bind(path).map(Box::new)
    }

    pub(crate) fn bind(path: impl Into<PathBuf>) -> Result<Self, SocketBindError> {
        let path = path.into();
        if path.exists() {
            if let Err(error) = std::fs::remove_file(&path) {
                log::warn!("could not remove stale Wayland socket {}: {error}", path.display());
            }
        }
        let fd = UnixListener::bind(&path)
            .map_err(|source| SocketBindError { path: path.clone(), source })?;
        log::info!("Listening on Wayland socket {}", path.display());
        Ok(Self {
            current_id: 0,
            fd,
            path,
            set: BTreeSet::default(),
            accept_op_id: None,
            accepting: true,
            unlinked: false,
        })
    }

    fn next_id(&mut self) -> ClientId {
        let id = self.current_id;
        self.current_id += 1;
        ClientId(id)
    }

    fn dispatch_accept(&mut self, dispatcher: &mut Dispatcher) {
        if !self.accepting || self.accept_op_id.is_some() {
            return;
        }
        let accept_op_id = RingId::fresh();
        self.accept_op_id = Some(accept_op_id);
        dispatcher.dispatch(IoUringAction::Accept {
            id: Some(accept_op_id),
            fd: self.fd.as_raw_fd(),
            dispatch_to: Redispatcher::to::<Self>(|result| match result {
                Ok(AcceptCompleted { fd: _, new_client }) => {
                    ClientManagerAction::NewClient { new_client }
                }
                Err(e) => {
                    log::error!("accept failed: {:?}", e);
                    ClientManagerAction::RearmAccept
                }
            }),
        });
    }

    fn close_client(&mut self, dispatcher: &mut Dispatcher, id: ClientId) {
        log::trace!("client {:?} closed", id);
        if self.set.remove(&id) {
            dispatcher.stop_model(id);
        }
    }

    fn unlink_socket(&mut self) {
        if self.unlinked {
            return;
        }
        self.unlinked = true;
        if let Err(error) = std::fs::remove_file(&self.path) {
            if error.kind() != io::ErrorKind::NotFound {
                log::warn!("could not unlink Wayland socket {}: {error}", self.path.display());
            }
        }
    }

    fn stop(&mut self, dispatcher: &mut Dispatcher) {
        self.accepting = false;
        if let Some(accept_op_id) = self.accept_op_id.take() {
            dispatcher.dispatch(IoUringAction::Cancel { target: accept_op_id });
        }
        for id in self.set.iter().copied().collect::<Vec<_>>() {
            self.close_client(dispatcher, id);
        }
        self.unlink_socket();
    }
}

impl Drop for ClientManager {
    fn drop(&mut self) {
        self.unlink_socket();
    }
}

impl Model for ClientManager {
    type Action = ClientManagerAction;

    const START: Option<Self::Action> = Some(ClientManagerAction::Start);
    const STOP: Option<Self::Action> = Some(ClientManagerAction::Stop);

    fn process(
        &mut self,
        state: &mut State,
        action: ClientManagerAction,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        use ClientManagerAction::*;

        match action {
            Start => {
                self.dispatch_accept(dispatcher);
                dispatcher.start::<Dbus>();
                dispatcher.start::<Udev>();
            }
            RearmAccept => {
                self.accept_op_id = None;
                self.dispatch_accept(dispatcher);
            }
            NewClient { new_client } => {
                self.accept_op_id = None;
                if !self.accepting {
                    drop(unsafe { OwnedFd::from_raw_fd(new_client) });
                    return Ok(());
                }
                self.dispatch_accept(dispatcher);

                let client_id = self.next_id();
                self.set.insert(client_id);
                dispatcher.start_model(client_id, Client::new(client_id, new_client));

                log::trace!("new_client id={:?} fd={:?}", client_id, new_client);
            }
            LibinputEvent => {
                // self.libinput.dispatch();
                // while let Some(event) = self.libinput.next_event() {
                //     log::info!("libinput event={:?}", event);
                // }
            }
            UdevEvent => {
                eprintln!("UDEV EVENT");
                // for e in self.udev.udev_monitor.iter() {
                //     let prop = e.property_value("ID_VENDOR_FROM_DATABASE");
                //     dbg!(prop);
                //     // match e.event_type() {
                //     //     udev::EventType::Add => todo!(),
                //     //     udev::EventType::Change => todo!(),
                //     //     udev::EventType::Remove => todo!(),
                //     //     udev::EventType::Bind => todo!(),
                //     //     udev::EventType::Unbind => todo!(),
                //     //     udev::EventType::Unknown => todo!(),
                //     // }
                //     // eprintln!("EVENT={:?}", e);
                // }
            }
            RegistryGlobalAdded { name, interface, version } => {
                for client_id in self.set.iter().copied() {
                    dispatcher.dispatch_to(
                        client_id,
                        ClientAction::RegistryGlobalAdded {
                            name,
                            interface: interface.clone(),
                            version,
                        },
                    );
                }
            }
            RegistryGlobalRemoved { name } => {
                for client_id in self.set.iter().copied() {
                    dispatcher.dispatch_to(client_id, ClientAction::RegistryGlobalRemoved { name });
                }
            }
            OutputStateChanged { output_id } => {
                crate::wayland_objects::ext_session_lock_v1::on_output_state_changed(
                    state, dispatcher, output_id,
                );
                for client_id in self.set.iter().copied() {
                    dispatcher
                        .dispatch_to(client_id, ClientAction::OutputStateChanged { output_id });
                }
            }
            OutputRemoved { output_id } => {
                for client_id in self.set.iter().copied() {
                    dispatcher.dispatch_to(client_id, ClientAction::OutputRemoved { output_id });
                }
            }
            GammaControlFailed { output_id } => {
                for client_id in self.set.iter().copied() {
                    dispatcher
                        .dispatch_to(client_id, ClientAction::GammaControlFailed { output_id });
                }
            }
            DiscardSurfacePresentationFeedbacksForBackend { drm_id } => {
                for client_id in self.set.iter().copied() {
                    dispatcher.dispatch_to(
                        client_id,
                        ClientAction::DiscardSurfacePresentationFeedbacksForBackend { drm_id },
                    );
                }
            }
            PublishWorkspaceStateChanged { affected_outputs } => {
                publish_workspace_state_changed(state, dispatcher, affected_outputs);
            }
            WorkspaceStateChanged => {
                crate::idle_inhibit::refresh(state, dispatcher, std::time::Instant::now());
                for client_id in self.set.iter().copied() {
                    dispatcher.dispatch_to(client_id, ClientAction::WorkspaceStateChanged);
                }
            }
            LayerShellLayoutChanged => {
                crate::idle_inhibit::refresh(state, dispatcher, std::time::Instant::now());
                for client_id in self.set.iter().copied() {
                    dispatcher.dispatch_to(client_id, ClientAction::RefreshLayerSurfaces);
                }
            }
            Close { id } => self.close_client(dispatcher, id),
            Stop => self.stop(dispatcher),
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct ProcessInfo {
    command: Rc<str>,
    cred: libc::ucred,
}

impl ProcessInfo {
    pub(crate) fn command(&self) -> &str {
        &self.command
    }
}

const MIN_SERVER_ID: u32 = 0xff00_0000;

#[derive(Clone, Copy, Debug)]
struct ClientObjectEntry {
    type_id: TypeId,
    object_id: ObjectId,
}

pub struct Client {
    id: ClientId,
    fd: OwnedFd,
    reader_id: Option<ClientReaderId>,
    /// the key `WaylandId` is the id we share with the client
    /// the value `ObjectId` is an index in our `State::objects`
    /// So this `objects` is just used to lookup into `State::objects`, from the client `WaylandId`
    // TODO: Replace `BTreeMap` with a `IndexMap` here ?
    objects: BTreeMap<WaylandId, ClientObjectEntry>,
    info: Option<Rc<ProcessInfo>>,
    pending_fds: VecDeque<OwnedFd>,
    next_server_id: u32,
}

impl ModelInstanceId for ClientId {
    type Model = Client;

    fn into_raw(self) -> u64 {
        self.0
    }
}

pub enum ClientAction {
    Start,
    MsgReceived {
        request: RawRequest<'static>,
    },
    FdsReceived {
        fds: Vec<OwnedFd>,
    },
    SeatEvent {
        event: SeatClientEvent,
    },
    RequestToplevelClose {
        toplevel: Resolved<crate::protocol_generated::XdgToplevelId>,
    },
    RefreshDataDeviceSelection,
    RefreshDataControlSelection,
    RefreshDataControlPrimarySelection,
    RefreshExtDataControlSelection,
    RefreshExtDataControlPrimarySelection,
    RefreshPrimarySelection,
    DataDeviceDragEnter {
        serial: u32,
        surface: Resolved<WlSurfaceId>,
        x: f32,
        y: f32,
        source: Option<Resolved<WlDataSourceId>>,
    },
    DataDeviceDragLeave,
    DataDeviceDragMotion {
        time: u32,
        x: f32,
        y: f32,
    },
    DataDeviceDragDrop,
    DataControlSourceEvent {
        source: Resolved<ZwlrDataControlSourceV1Id>,
        event: DataControlSourceEvent,
    },
    ExtDataControlSourceEvent {
        source: Resolved<ExtDataControlSourceV1Id>,
        event: ExtDataControlSourceEvent,
    },
    PrimarySelectionSourceEvent {
        source: Resolved<ZwpPrimarySelectionSourceV1Id>,
        event: PrimarySelectionSourceEvent,
    },
    DataSourceEvent {
        source: Resolved<WlDataSourceId>,
        event: DataSourceClientEvent,
    },
    RegistryGlobalAdded {
        name: u32,
        interface: String,
        version: u32,
    },
    RegistryGlobalRemoved {
        name: u32,
    },
    OutputStateChanged {
        output_id: OutputId,
    },
    OutputRemoved {
        output_id: OutputId,
    },
    GammaControlFailed {
        output_id: OutputId,
    },
    WorkspaceStateChanged,
    RefreshLayerSurfaces,
    RefreshSurfaceOutputs,
    ReleaseBuffer {
        buffer_id: crate::protocol_generated::WlBufferId,
    },
    CompleteBufferReleaseCallbacks {
        callbacks: Vec<Resolved<WlCallbackId>>,
    },
    CompleteSurfaceFrameCallbacks {
        surfaces: Vec<Resolved<WlSurfaceId>>,
        time_msec: u32,
    },
    SendSessionLockLocked,
    ReconfigureLockOutput {
        output_id: OutputId,
    },
    PresentationFeedbackPresented {
        feedbacks: Vec<Resolved<WpPresentationFeedbackId>>,
        output_id: Option<OutputId>,
        tv_sec_hi: u32,
        tv_sec_lo: u32,
        tv_nsec: u32,
        refresh: u32,
        seq_hi: u32,
        seq_lo: u32,
        flags: u32,
    },
    PresentationFeedbackDiscarded {
        feedbacks: Vec<Resolved<WpPresentationFeedbackId>>,
    },
    DiscardSurfacePresentationFeedbacksForBackend {
        drm_id: DrmId,
    },
    CancelBlockedSurface {
        surface: Resolved<WlSurfaceId>,
        waiter_id: AcquireReadyWaiterId,
    },
    DismissPopupGrab {
        popups: Vec<Resolved<XdgPopupId>>,
    },
    InteractiveResizeConfigure {
        xdg_toplevel: Resolved<XdgToplevelId>,
        width: i32,
        height: i32,
        resizing: bool,
    },
    Close,
    /// Send capture `stopped`, `failed`, `closed`, or foreign-toplevel announcements queued for
    /// this client.
    DeliverCaptureEvents,
    IdleNotification {
        id: crate::protocol_generated::ExtIdleNotificationV1Id,
        notice: crate::wayland_objects::ext_idle_notification_v1::IdleNotice,
    },
}

#[derive(Debug)]
pub enum DataControlSourceEvent {
    Send { mime_type: String, fd: RawFd },
    Cancelled,
}

#[derive(Debug)]
pub enum ExtDataControlSourceEvent {
    Send { mime_type: String, fd: RawFd },
    Cancelled,
}

#[derive(Debug)]
pub enum PrimarySelectionSourceEvent {
    Send { mime_type: String, fd: RawFd },
    Cancelled,
}

pub enum DataSourceClientEvent {
    Target(Option<String>),
    Send { mime_type: String, fd: RawFd },
    Cancelled,
    DndDropPerformed,
    DndFinished,
    Action { dnd_action: u32 },
}

impl Client {
    fn new(client_id: ClientId, fd: RawFd) -> Self {
        Self {
            id: client_id,
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
            reader_id: None,
            objects: BTreeMap::default(),
            info: None,
            pending_fds: VecDeque::new(),
            next_server_id: MIN_SERVER_ID,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_tests(client_id: ClientId) -> Self {
        let fd = std::fs::File::open("/dev/null").unwrap().into_raw_fd();
        Self::new(client_id, fd)
    }

    pub fn client_id(&self) -> ClientId {
        self.id
    }

    /// Send `wl_display.error` for `object_id` with a protocol error `code`, then disconnect the client.
    pub fn protocol_violation_disconnect(
        &mut self,
        dispatcher: &mut ClientDispatcher,
        object_id: WaylandId,
        code: u32,
        message: impl AsRef<str>,
    ) {
        dispatcher.send_wl_display_error(object_id, code, message.as_ref());
        dispatcher.disconnect_client(self.id);
    }

    /// Send `wl_display.error` without disconnecting (rare; prefer [`Self::protocol_violation_disconnect`]).
    pub fn send_protocol_error(
        &self,
        dispatcher: &mut ClientDispatcher,
        object_id: WaylandId,
        code: u32,
        message: impl AsRef<str>,
    ) {
        dispatcher.send_wl_display_error(object_id, code, message.as_ref());
    }

    fn read_process_info(&mut self) {
        let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
        let mut len = std::mem::size_of::<libc::ucred>() as u32;
        let result = unsafe {
            getsockopt(
                self.fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut _ as *mut _,
                &mut len as *mut _,
            )
        };
        if result == -1 {
            let e = std::io::Error::last_os_error();
            log::error!("Unable to `getsockopt` of new client: {:?}", e);
            return;
        }
        let command = match std::fs::read(format!("/proc/{}/cmdline", cred.pid)) {
            Ok(cmdline) if !cmdline.is_empty() => {
                let argv = cmdline
                    .split(|byte| *byte == 0)
                    .filter(|arg| !arg.is_empty())
                    .map(String::from_utf8_lossy)
                    .map(|arg| arg.into_owned())
                    .collect::<Vec<_>>();
                if argv.is_empty() {
                    None
                } else {
                    Some(argv.join(" "))
                }
            }
            Ok(_) | Err(_) => match std::fs::read(format!("/proc/{}/comm", cred.pid)) {
                Ok(comm) => Some(String::from_utf8_lossy(&comm).trim().to_string()),
                Err(e) => {
                    log::error!("Unable to read process name of new client: {:?}", e);
                    None
                }
            },
        };
        let Some(command) = command else {
            return;
        };
        let command = Rc::from(command);

        self.info = Some(Rc::new(ProcessInfo { command, cred }))
    }

    pub fn ref_object_mut<'a, K>(&self, id: K, state: &'a mut State) -> &'a mut K::Object
    where
        K: IdToObject,
    {
        let id: WaylandId = id.into();
        let obj_id = self.objects[&id].object_id;
        state.objects[obj_id].downcast_mut().unwrap()
    }

    pub fn ref_object<'a, K>(&self, id: K, state: &'a State) -> &'a K::Object
    where
        K: IdToObject,
    {
        let id: WaylandId = id.into();
        let obj_id = self.objects[&id].object_id;
        state.objects[obj_id].downcast_ref().unwrap()
    }

    pub fn with_ungrabbed<O: ObjectWithId, R>(
        &self,
        object: GrabbedObject<O>,
        state: &mut State,
        fun: impl FnOnce(&mut State) -> R,
    ) -> (GrabbedObject<O>, R) {
        let object_id = self.resolve_this(&object, state);
        drop(object);
        let result = fun(state);
        let object = object_id.grab(state);
        (object, result)
    }

    pub fn with_ungrabbed_mut<O: ObjectWithId, R>(
        &mut self,
        object: GrabbedObject<O>,
        state: &mut State,
        fun: impl FnOnce(&mut State, &mut Client) -> R,
    ) -> (GrabbedObject<O>, R) {
        let object_id = self.resolve_this(&object, state);
        drop(object);
        let result = fun(state, self);
        let object = object_id.grab(state);
        (object, result)
    }

    pub fn resolve_this<O: ObjectWithId>(
        &self,
        this: &GrabbedObject<O>,
        state: &State,
    ) -> Resolved<O::Id> {
        self.resolve::<O::Id>(O::Id::from(this.id()), state)
            .unwrap()
    }

    /// Resolve a client-visible Wayland id to a live compositor object of the
    /// expected interface type.
    pub fn resolve<K>(&self, id: K, state: &State) -> Option<Resolved<K>>
    where
        K: IdToObject,
    {
        let wayland_id: WaylandId = id.into();
        let entry = self.objects.get(&wayland_id)?;
        (entry.type_id == TypeId::of::<K::Object>() && state.objects.contains(entry.object_id))
            .then_some(Resolved {
                client_id: self.id,
                wayland_id: id,
                object_id: entry.object_id,
            })
    }

    pub fn resolve_or_disconnect<K>(
        &mut self,
        id: K,
        state: &State,
        dispatcher: &mut ClientDispatcher,
    ) -> Result<Resolved<K>, AbortRequest>
    where
        K: IdToObject,
        K::Object: Constants,
    {
        let Some(resolved) = self.resolve(id, state) else {
            let requester_object_id = dispatcher
                .requester_object_id
                .expect("missing requester object id");
            self.protocol_violation_disconnect(
                dispatcher,
                requester_object_id,
                wl_display::Error::InvalidObject as u32,
                format!("requires a live {}", K::Object::INTERFACE_NAME),
            );
            return Err(AbortRequest::InvalidObject);
        };
        Ok(resolved)
    }

    /// Resolve a client-visible Wayland id to our internal `ObjectId` if the id
    /// is still live in both the per-client object map and `State::objects`.
    ///
    /// This is useful for request validation paths where we must distinguish
    /// "the client sent an unknown/deleted object id" from "the id exists but is
    /// the wrong interface", without panicking on a typed downcast.
    pub fn try_object_id<K>(&self, id: K, state: &State) -> Option<ObjectId>
    where
        K: Into<WaylandId>,
    {
        let id: WaylandId = id.into();
        let object_id = self.objects.get(&id)?.object_id;
        state.objects.contains(object_id).then_some(object_id)
    }

    /// Deprecated: Use `iter_resolved_of()` instead.
    fn iter_object_ids_of<'a, T: Object>(
        &'a self,
        state: &'a State,
    ) -> impl Iterator<Item = ObjectId> + 'a {
        let type_id = TypeId::of::<T>();
        self.objects.values().filter_map(move |entry| {
            (entry.type_id == type_id && state.objects.contains(entry.object_id))
                .then_some(entry.object_id)
        })
    }

    fn iter_resolved_of<'a, K>(&'a self, state: &'a State) -> impl Iterator<Item = Resolved<K>> + 'a
    where
        K: IdToObject,
    {
        let type_id = TypeId::of::<K::Object>();
        self.objects.iter().filter_map(move |(wayland_id, entry)| {
            (entry.type_id == type_id && state.objects.contains(entry.object_id))
                .then_some(Resolved::new(self.id, K::from(*wayland_id), entry.object_id))
        })
    }

    /// Return whether a client-visible Wayland id currently refers to a live
    /// object of the expected interface type `K::Object`.
    ///
    /// This is intentionally stricter than `contains_object()`: it checks both
    /// liveness and the concrete object type, which lets protocol handlers reject
    /// malformed ids with a protocol error instead of panicking in `get_object()`.
    pub fn object_is<K>(&self, id: K, state: &State) -> bool
    where
        K: IdToObject,
    {
        self.resolve(id, state).is_some()
    }

    pub fn grab_object<K>(&self, id: K, state: &mut State) -> GrabbedObject<K::Object>
    where
        K: IdToObject,
    {
        let id: WaylandId = id.into();
        let obj_id = self.objects[&id].object_id;
        state.objects.grab_object(obj_id)
    }

    pub fn object_id<K>(&self, id: K) -> ObjectId
    where
        K: Into<WaylandId>,
    {
        let id: WaylandId = id.into();
        self.objects[&id].object_id
    }

    pub fn object_id_for_wayland<K: Into<WaylandId>>(&self, id: K) -> Option<ObjectId> {
        self.objects.get(&id.into()).map(|entry| entry.object_id)
    }

    pub fn remove_wayland_object<K: Into<WaylandId>>(&mut self, id: K) -> Option<ObjectId> {
        self.objects.remove(&id.into()).map(|entry| entry.object_id)
    }

    pub fn allocate_server_object_id<I>(&mut self) -> I
    where
        I: From<u32>,
    {
        loop {
            let next = self.next_server_id;
            self.next_server_id = self
                .next_server_id
                .checked_add(1)
                .expect("per-client server object id space exhausted");

            let wayland_id = WaylandId::from(next);
            if self.objects.contains_key(&wayland_id) {
                continue;
            }

            return I::from(next);
        }
    }

    pub fn insert_object<'a, O>(
        &mut self,
        state: &'a mut State,
        object: O,
    ) -> (Resolved<O::Id>, &'a mut O)
    where
        O: ObjectWithId,
    {
        let (wayland_id, object_id) = state.insert_object(object);
        self.objects
            .insert(wayland_id, ClientObjectEntry { type_id: TypeId::of::<O>(), object_id });
        let object: &mut O = state.objects[object_id].downcast_mut::<O>().unwrap();
        let id = O::Id::from(wayland_id);
        (Resolved::new(self.client_id(), id, object_id), object)
    }

    pub fn contains_object<K>(&self, id: K, state: &State) -> bool
    where
        K: IdToObject,
    {
        let id: WaylandId = id.into();
        let Some(obj_id) = self.objects.get(&id).map(|entry| entry.object_id) else {
            return false;
        };
        state.objects.contains(obj_id)
    }

    pub fn delete_id(
        &mut self,
        wl_display: &WlDisplay,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        id: WaylandId,
    ) {
        let object_id = self.objects.remove(&id).unwrap().object_id;
        state.remove_object(object_id);
        wl_display.send_event(wl_display::Event::DeleteId { id: id.into() }, dispatcher);
    }

    pub(crate) fn dismiss_popup_ids(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        popups: Vec<Resolved<XdgPopupId>>,
        detached_parent_layer_surface_id: Option<ZwlrLayerSurfaceV1Id>,
    ) {
        for popup in popups.into_iter().rev() {
            if !popup.exists(state) {
                continue;
            }

            crate::seat::remove_popup_grab(state, dispatcher, popup, None);

            let (xdg_surface_id, parent_surface_id, parent_layer_surface) = {
                let popup = popup.rref(state);
                (popup.xdg_surface(), popup.parent_surface(), popup.parent_layer_surface())
            };

            if let Some(parent_surface_id) = parent_surface_id {
                if parent_surface_id.exists(state) {
                    let mut parent_surface = parent_surface_id.grab(state);
                    parent_surface.remove_popup_child(popup);
                }
            }
            if let Some(parent_layer_surface) = parent_layer_surface {
                if Some(parent_layer_surface.wayland_id()) != detached_parent_layer_surface_id
                    && parent_layer_surface.exists(state)
                {
                    let mut parent_surface = parent_layer_surface.grab(state);
                    parent_surface.remove_popup_child(popup);
                }
            }

            if xdg_surface_id.exists(state) {
                let wl_surface_id = {
                    let xdg_surface = xdg_surface_id.rref(state);
                    xdg_surface.wl_surface().wayland_id()
                };
                if self.contains_object(wl_surface_id, state) {
                    let mut wl_surface = self.grab_object(wl_surface_id, state);
                    wl_surface.force_unmap_for_role_dismissal(self, state, dispatcher);
                }
            }

            {
                let mut popup = popup.grab(state);
                popup.dismiss(dispatcher);
            }
        }
    }

    fn handle_seat_event(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        event: SeatClientEvent,
    ) {
        if crate::logger::ENABLE_NOISY_LOG {
            log::debug!("client {:?} seat event {:?}", self.id, event);
        }
        match event {
            SeatClientEvent::PointerEnter { serial, surface, x, y } => {
                for pointer in state.seat.pointer_resources(self.id) {
                    if !pointer.exists(state) {
                        continue;
                    }
                    state.seat.record_client_serial(self.id, serial);
                    pointer
                        .ref_mut(state)
                        .send_enter(serial, surface, x, y, dispatcher);
                }
            }
            SeatClientEvent::PointerLeave { serial, surface } => {
                for pointer in state.seat.pointer_resources(self.id) {
                    if !pointer.exists(state) {
                        continue;
                    }
                    state.seat.record_client_serial(self.id, serial);
                    pointer
                        .ref_mut(state)
                        .send_leave(serial, surface, dispatcher);
                }
            }
            SeatClientEvent::PointerMotion { time, x, y } => {
                for pointer in state.seat.pointer_resources(self.id) {
                    if !pointer.exists(state) {
                        continue;
                    }
                    pointer.ref_mut(state).send_motion(time, x, y, dispatcher);
                }
            }
            SeatClientEvent::PointerWarp { time, x, y } => {
                for pointer in state.seat.pointer_resources(self.id) {
                    if !pointer.exists(state) {
                        continue;
                    }
                    let pointer = pointer.ref_mut(state);
                    if pointer.version() >= wl_pointer::WARP_SINCE {
                        pointer.send_warp(x, y, dispatcher);
                    } else {
                        pointer.send_motion(time, x, y, dispatcher);
                    }
                }
            }
            SeatClientEvent::PointerButton { serial, time, button, state: button_state } => {
                for pointer in state.seat.pointer_resources(self.id) {
                    if !pointer.exists(state) {
                        continue;
                    }
                    state.seat.record_client_serial(self.id, serial);
                    pointer.ref_mut(state).send_button(
                        serial,
                        time,
                        button,
                        button_state,
                        dispatcher,
                    );
                }
            }
            SeatClientEvent::PointerLockLocked { lock } => {
                if let Some(lock) = lock.try_ref(state) {
                    lock.send_locked(dispatcher);
                }
            }
            SeatClientEvent::PointerLockUnlocked { lock } => {
                if let Some(lock) = lock.try_ref(state) {
                    lock.send_unlocked(dispatcher);
                }
            }
            SeatClientEvent::PointerConfineConfined { confine } => {
                if let Some(confine) = confine.try_ref(state) {
                    confine.send_confined(dispatcher);
                }
            }
            SeatClientEvent::PointerConfineUnconfined { confine } => {
                if let Some(confine) = confine.try_ref(state) {
                    confine.send_unconfined(dispatcher);
                }
            }
            SeatClientEvent::PointerFrame => {
                for pointer in state.seat.pointer_resources(self.id) {
                    if !pointer.exists(state) {
                        continue;
                    }
                    pointer.ref_mut(state).send_frame(dispatcher);
                }
            }
            SeatClientEvent::RelativePointerMotion {
                utime_hi,
                utime_lo,
                dx,
                dy,
                dx_unaccel,
                dy_unaccel,
            } => {
                for relative in state.seat.relative_pointers_for_client(self.id) {
                    if !relative.exists(state) {
                        continue;
                    }
                    relative.ref_mut(state).send_relative_motion(
                        utime_hi, utime_lo, dx, dy, dx_unaccel, dy_unaccel, dispatcher,
                    );
                }
            }
            SeatClientEvent::PointerScroll(frame) => {
                for pointer in state.seat.pointer_resources(self.id) {
                    if !pointer.exists(state) {
                        continue;
                    }
                    pointer.ref_mut(state).send_scroll(&frame, dispatcher);
                }
            }
            SeatClientEvent::KeyboardEnter { serial, surface, keys } => {
                for keyboard in state.seat.keyboard_resources(self.id) {
                    if !keyboard.exists(state) {
                        continue;
                    }
                    state.seat.record_client_serial(self.id, serial);
                    keyboard
                        .ref_mut(state)
                        .send_enter(serial, surface, keys.clone(), dispatcher);
                }
            }
            SeatClientEvent::KeyboardLeave { serial, surface } => {
                for keyboard in state.seat.keyboard_resources(self.id) {
                    if !keyboard.exists(state) {
                        continue;
                    }
                    state.seat.record_client_serial(self.id, serial);
                    keyboard
                        .ref_mut(state)
                        .send_leave(serial, surface, dispatcher);
                }
            }
            SeatClientEvent::KeyboardKey { serial, time, key, state: key_state } => {
                for keyboard in state.seat.keyboard_resources(self.id) {
                    if !keyboard.exists(state) {
                        continue;
                    }
                    state.seat.record_client_serial(self.id, serial);
                    keyboard
                        .ref_mut(state)
                        .send_key(serial, time, key, key_state, dispatcher);
                }
            }
            SeatClientEvent::KeyboardModifiers {
                serial,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                for keyboard in state.seat.keyboard_resources(self.id) {
                    if !keyboard.exists(state) {
                        continue;
                    }
                    state.seat.record_client_serial(self.id, serial);
                    keyboard.ref_mut(state).send_modifiers(
                        serial,
                        mods_depressed,
                        mods_latched,
                        mods_locked,
                        group,
                        dispatcher,
                    );
                }
            }
        }
    }

    pub(crate) fn destroy_data_offer(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        offer: Resolved<WlDataOfferId>,
    ) {
        if offer.client_id() != self.id {
            return;
        }
        if !offer.exists(state) {
            return;
        }
        if let Some(mut offer_obj) = offer.try_grab(state) {
            if let Some(source) = offer_obj.source_object_id() {
                state.unregister_data_offer_for_source(source, offer);
            }
            offer_obj.before_destroy(self.id, offer, state, dispatcher);
        }
        self.delete_id(&WlDisplay::default(), state, dispatcher, offer.wayland_id().into());
    }

    fn create_data_offer_for_device(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        device_id: crate::protocol_generated::WlDataDeviceId,
        source: Resolved<WlDataSourceId>,
        role: DataOfferRole,
    ) -> Option<Resolved<WlDataOfferId>> {
        if !self.contains_object(device_id, state) {
            return None;
        }
        let (device_version, previous_chosen_action) = {
            let device = self.ref_object(device_id, state);
            (
                device.version(),
                state
                    .seat
                    .active_drag()
                    .map(|drag| drag.negotiation.chosen_action)
                    .unwrap_or(0),
            )
        };
        let mime_types = SelectionSourceState::WlData(source).mime_types(state)?;
        let source_actions = source.try_ref(state)?.effective_dnd_actions();
        let chosen_action = if role == DataOfferRole::Drag
            && device_version < wl_data_offer::SOURCE_ACTIONS_SINCE
        {
            if previous_chosen_action != 0 {
                previous_chosen_action
            } else if source_actions & DndAction::Copy as u32 != 0 {
                DndAction::Copy as u32
            } else {
                0
            }
        } else {
            previous_chosen_action
        };

        let offer_id = self.allocate_server_object_id::<WlDataOfferId>();
        let offer = WlDataOffer::new(offer_id, device_version, role, Some(source));
        let (resolved_offer, _) = self.insert_object(state, offer);
        state.register_data_offer_for_source(source.object_id(), resolved_offer);
        {
            let device = self.ref_object(device_id, state);
            device.send_data_offer(resolved_offer, dispatcher);
        }
        for mime_type in mime_types {
            let offer = self.ref_object(offer_id, state);
            offer.send_offer(mime_type, dispatcher);
        }
        if role == DataOfferRole::Drag && device_version >= wl_data_offer::SOURCE_ACTIONS_SINCE {
            let offer = self.ref_object_mut(offer_id, state);
            offer.send_source_actions(source_actions, dispatcher);
            if chosen_action != 0 {
                offer.send_action(chosen_action, dispatcher);
            }
        }
        Some(resolved_offer)
    }

    fn refresh_data_device_selection(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let has_keyboard_focus = state
            .seat
            .keyboard_focus()
            .is_some_and(|focus| focus.client_id() == self.id);
        let selection_source = has_keyboard_focus
            .then(|| state.seat.selection_source())
            .flatten();

        for data_device in state.seat.data_device_resources(self.id) {
            if !data_device.exists(state) {
                continue;
            }

            let previous_offer = {
                let data_device = data_device.ref_mut(state);
                data_device.take_selection_offer()
            };
            if let Some(previous_offer) = previous_offer {
                WlDataOffer::make_selection_offer_inert(previous_offer, state);
            }

            let next_offer = selection_source.and_then(|selection| {
                self.create_selection_data_offer(
                    state,
                    dispatcher,
                    data_device.wayland_id(),
                    selection,
                )
            });

            let data_device = data_device.ref_mut(state);
            data_device.set_selection_offer(next_offer);
            data_device.send_selection(next_offer, dispatcher);
        }
    }

    fn create_selection_data_offer(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        device_id: crate::protocol_generated::WlDataDeviceId,
        selection: SelectionSourceState,
    ) -> Option<Resolved<WlDataOfferId>> {
        if !self.contains_object(device_id, state) {
            return None;
        }
        let device_version = self.ref_object(device_id, state).version();
        let mime_types = selection.mime_types(state)?;
        let offer_id = self.allocate_server_object_id::<WlDataOfferId>();
        let offer = WlDataOffer::from_selection(offer_id, device_version, selection);
        let (resolved_offer, _) = self.insert_object(state, offer);
        state.register_data_offer_for_source(selection.object_id(), resolved_offer);
        {
            let device = self.ref_object(device_id, state);
            device.send_data_offer(resolved_offer, dispatcher);
        }
        for mime_type in mime_types {
            self.ref_object(offer_id, state)
                .send_offer(mime_type, dispatcher);
        }
        Some(resolved_offer)
    }

    fn refresh_data_control_selection(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let selection = state.seat.selection_source();
        for device in state.seat.data_control_devices(self.id) {
            if !device.exists(state) || device.rref(state).is_inert() {
                continue;
            }
            let previous_offer = device.ref_mut(state).take_selection_offer();
            if let Some(previous_offer) = previous_offer {
                inert_data_control_offer(previous_offer, state);
            }
            let next_offer = selection.and_then(|selection| {
                create_data_control_offer(self, state, dispatcher, device, selection)
            });
            let device = device.ref_mut(state);
            device.set_selection_offer(next_offer);
            device.send_selection(next_offer, dispatcher);
        }
    }

    fn refresh_data_control_primary_selection(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let selection = state.seat.primary_selection_source();
        for device in state.seat.data_control_devices(self.id) {
            if !device.exists(state) || device.rref(state).is_inert() {
                continue;
            }
            if device.rref(state).version()
                < crate::protocol_generated::zwlr_data_control_device_v1::PRIMARY_SELECTION_SINCE
            {
                continue;
            }
            let previous_offer = device.ref_mut(state).take_primary_offer();
            if let Some(previous_offer) = previous_offer {
                inert_data_control_offer(previous_offer, state);
            }
            let next_offer = selection.and_then(|selection| {
                create_data_control_primary_offer(self, state, dispatcher, device, selection)
            });
            let device = device.ref_mut(state);
            device.set_primary_offer(next_offer);
            device.send_primary_selection(next_offer, dispatcher);
        }
    }

    fn refresh_ext_data_control_selection(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let selection = state.seat.selection_source();
        for device in state.seat.ext_data_control_devices(self.id) {
            if !device.exists(state) || device.rref(state).is_inert() {
                continue;
            }
            let previous_offer = device.ref_mut(state).take_selection_offer();
            if let Some(previous_offer) = previous_offer {
                inert_ext_data_control_offer(previous_offer, state);
            }
            let next_offer = selection.and_then(|selection| {
                create_ext_data_control_offer(self, state, dispatcher, device, selection)
            });
            let device = device.ref_mut(state);
            device.set_selection_offer(next_offer);
            device.send_selection(next_offer, dispatcher);
        }
    }

    fn refresh_ext_data_control_primary_selection(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let selection = state.seat.primary_selection_source();
        for device in state.seat.ext_data_control_devices(self.id) {
            if !device.exists(state) || device.rref(state).is_inert() {
                continue;
            }
            let previous_offer = device.ref_mut(state).take_primary_offer();
            if let Some(previous_offer) = previous_offer {
                inert_ext_data_control_offer(previous_offer, state);
            }
            let next_offer = selection.and_then(|selection| {
                create_ext_data_control_primary_offer(self, state, dispatcher, device, selection)
            });
            let device = device.ref_mut(state);
            device.set_primary_offer(next_offer);
            device.send_primary_selection(next_offer, dispatcher);
        }
    }

    fn refresh_primary_selection(&mut self, state: &mut State, dispatcher: &mut ClientDispatcher) {
        let has_keyboard_focus = state
            .seat
            .keyboard_focus()
            .is_some_and(|focus| focus.client_id() == self.id);
        let selection = has_keyboard_focus
            .then(|| state.seat.primary_selection_source())
            .flatten();

        for device in state.seat.primary_selection_devices(self.id) {
            if !device.exists(state) {
                continue;
            }
            let previous_offer = device.ref_mut(state).take_selection_offer();
            if let Some(previous_offer) = previous_offer {
                inert_primary_selection_offer(previous_offer, state);
            }
            let next_offer = selection.and_then(|selection| {
                create_primary_selection_offer(self, state, dispatcher, device, selection)
            });
            let device = device.ref_mut(state);
            device.set_selection_offer(next_offer);
            device.send_selection(next_offer, dispatcher);
        }
    }

    fn handle_data_device_drag_enter(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        serial: u32,
        surface: Resolved<WlSurfaceId>,
        x: f32,
        y: f32,
        source: Option<Resolved<WlDataSourceId>>,
    ) {
        let has_legacy_device = state
            .seat
            .data_device_resources(self.id)
            .into_iter()
            .filter(|data_device| data_device.exists(state))
            .any(|data_device| {
                data_device.rref(state).version() < wl_data_offer::SOURCE_ACTIONS_SINCE
            });
        if has_legacy_device
            && state
                .seat
                .active_drag()
                .is_some_and(|drag| drag.negotiation.chosen_action == 0)
        {
            let source_actions = source
                .and_then(|source| {
                    source
                        .try_ref(state)
                        .map(|source| source.effective_dnd_actions())
                })
                .unwrap_or(0);
            if source_actions & DndAction::Copy as u32 != 0 {
                let _ = state
                    .seat
                    .set_active_drag_chosen_action(self.id, DndAction::Copy as u32);
            }
        }
        for data_device in state.seat.data_device_resources(self.id) {
            if !data_device.exists(state) {
                continue;
            }
            let device_version = data_device.rref(state).version();

            let previous_offer = {
                let data_device = data_device.ref_mut(state);
                data_device.take_drag_offer()
            };
            if let Some(previous_offer) = previous_offer {
                self.destroy_data_offer(state, dispatcher, previous_offer);
            }

            let offer = source.and_then(|source| {
                self.create_data_offer_for_device(
                    state,
                    dispatcher,
                    data_device.wayland_id(),
                    source,
                    DataOfferRole::Drag,
                )
            });
            if let Some(offer) = offer {
                let _ = state.seat.register_active_drag_offer(
                    self.id,
                    offer,
                    device_version >= wl_data_offer::ACTION_SINCE,
                );
            }

            let data_device = data_device.ref_mut(state);
            data_device.set_drag_offer(offer);
            data_device.send_enter(serial, surface, x, y, offer, dispatcher);
        }
    }

    fn handle_data_device_drag_leave(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        for data_device in state.seat.data_device_resources(self.id) {
            if !data_device.exists(state) {
                continue;
            }
            let previous_offer = {
                let data_device = data_device.ref_mut(state);
                data_device.send_leave(dispatcher);
                data_device.take_drag_offer()
            };
            if let Some(previous_offer) = previous_offer {
                self.destroy_data_offer(state, dispatcher, previous_offer);
            }
        }
    }

    fn handle_data_device_drag_motion(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        time: u32,
        x: f32,
        y: f32,
    ) {
        for data_device in state.seat.data_device_resources(self.id) {
            if !data_device.exists(state) {
                continue;
            }
            data_device.rref(state).send_motion(time, x, y, dispatcher);
        }
    }

    fn handle_data_device_drag_drop(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        for data_device in state.seat.data_device_resources(self.id) {
            if !data_device.exists(state) {
                continue;
            }
            data_device.rref(state).send_drop(dispatcher);
        }

        if let Some(drag) = state.seat.active_drag() {
            if drag
                .target
                .is_some_and(|target| target.client_id() == self.id)
            {
                if let Some(source) = drag.source.resolved() {
                    if drag.negotiation.chosen_action == 0 {
                        return;
                    }
                    dispatcher.dispatch_to(
                        source.client_id(),
                        ClientAction::DataSourceEvent {
                            source,
                            event: DataSourceClientEvent::Action {
                                dnd_action: drag.negotiation.chosen_action,
                            },
                        },
                    );
                }
            }
        }
    }

    fn handle_data_control_source_event(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        source: Resolved<ZwlrDataControlSourceV1Id>,
        event: DataControlSourceEvent,
    ) {
        if !source.exists(state) {
            if let DataControlSourceEvent::Send { fd, .. } = event {
                let _ = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
            }
            return;
        }
        let source = source.ref_mut(state);
        match event {
            DataControlSourceEvent::Send { mime_type, fd } => {
                source.send_send(mime_type, fd, dispatcher);
            }
            DataControlSourceEvent::Cancelled => source.send_cancelled(dispatcher),
        }
    }

    fn handle_ext_data_control_source_event(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        source: Resolved<ExtDataControlSourceV1Id>,
        event: ExtDataControlSourceEvent,
    ) {
        if !source.exists(state) {
            if let ExtDataControlSourceEvent::Send { fd, .. } = event {
                let _ = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
            }
            return;
        }
        let source = source.ref_mut(state);
        match event {
            ExtDataControlSourceEvent::Send { mime_type, fd } => {
                source.send_send(mime_type, fd, dispatcher);
            }
            ExtDataControlSourceEvent::Cancelled => source.send_cancelled(dispatcher),
        }
    }

    fn handle_primary_selection_source_event(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        source: Resolved<ZwpPrimarySelectionSourceV1Id>,
        event: PrimarySelectionSourceEvent,
    ) {
        if !source.exists(state) {
            if let PrimarySelectionSourceEvent::Send { fd, .. } = event {
                let _ = unsafe { OwnedFd::from_raw_fd(fd) };
            }
            return;
        }
        let source = source.ref_mut(state);
        match event {
            PrimarySelectionSourceEvent::Send { mime_type, fd } => {
                source.send_send(mime_type, fd, dispatcher);
            }
            PrimarySelectionSourceEvent::Cancelled => source.send_cancelled(dispatcher),
        }
    }

    fn handle_data_source_event(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        source: Resolved<WlDataSourceId>,
        event: DataSourceClientEvent,
    ) {
        if !source.exists(state) {
            if let DataSourceClientEvent::Send { fd, .. } = event {
                let _ = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
            }
            return;
        };
        let source = source.ref_mut(state);
        match event {
            DataSourceClientEvent::Target(mime_type) => {
                source.send_target(mime_type, dispatcher);
            }
            DataSourceClientEvent::Send { mime_type, fd } => {
                source.send_send(mime_type, fd, dispatcher);
            }
            DataSourceClientEvent::Cancelled => {
                source.send_cancelled(dispatcher);
            }
            DataSourceClientEvent::DndDropPerformed => {
                source.send_dnd_drop_performed(dispatcher);
            }
            DataSourceClientEvent::DndFinished => {
                source.send_dnd_finished(dispatcher);
            }
            DataSourceClientEvent::Action { dnd_action } => {
                source.send_action(dnd_action, dispatcher);
            }
        }
    }

    fn complete_surface_frame_callbacks(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        surfaces: Vec<Resolved<WlSurfaceId>>,
        time_msec: u32,
    ) {
        let display = WlDisplay::default();
        for surface in surfaces {
            if !surface.exists(state) {
                continue;
            }
            let callbacks = {
                let Some(mut surface) = surface.try_grab(state) else {
                    continue;
                };
                surface.take_frame_callbacks()
            };
            for callback_id in callbacks {
                if !callback_id.exists(state) {
                    continue;
                }
                {
                    let callback = callback_id.rref(state);
                    callback.done(time_msec, dispatcher);
                }
                self.delete_id(&display, state, dispatcher, callback_id.wayland_id().into());
            }
        }
    }

    fn output_resources_for_feedback(&self, state: &State, output_id: OutputId) -> Vec<WaylandId> {
        let mut resources = Vec::new();
        for wl_output in self.iter_resolved_of::<WlOutputId>(state) {
            let wl_output = wl_output.rref(state);
            if wl_output.matches_output(output_id) {
                resources.push(wl_output.wl_output_id().into());
            }
        }
        resources
    }

    fn present_feedback(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        feedbacks: Vec<Resolved<WpPresentationFeedbackId>>,
        output_id: Option<OutputId>,
        tv_sec_hi: u32,
        tv_sec_lo: u32,
        tv_nsec: u32,
        refresh: u32,
        seq_hi: u32,
        seq_lo: u32,
        flags: u32,
    ) {
        let display = WlDisplay::default();
        let sync_outputs =
            output_id.map(|output_id| self.output_resources_for_feedback(state, output_id));
        for feedback in feedbacks {
            if !feedback.exists(state) {
                continue;
            }
            {
                let feedback: &mut WpPresentationFeedback = feedback.ref_mut(state);
                if let Some(sync_outputs) = sync_outputs.as_ref() {
                    for output in sync_outputs {
                        feedback.send_sync_output((*output).into(), dispatcher);
                    }
                }
                feedback.send_presented(
                    tv_sec_hi, tv_sec_lo, tv_nsec, refresh, seq_hi, seq_lo, flags, dispatcher,
                );
            }
            self.delete_id(&display, state, dispatcher, feedback.wayland_id().into());
        }
    }

    fn discard_presentation_feedback(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        feedbacks: Vec<Resolved<WpPresentationFeedbackId>>,
    ) {
        let display = WlDisplay::default();
        for feedback in feedbacks {
            if !feedback.exists(state) {
                continue;
            }
            {
                let feedback = feedback.rref(state);
                feedback.send_discarded(dispatcher);
            }
            self.delete_id(&display, state, dispatcher, feedback.wayland_id().into());
        }
    }

    fn discard_surface_presentation_feedbacks_for_backend(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        drm_id: DrmId,
    ) {
        let backend = crate::state::BackendOutputRef::new(drm_id);
        let surface_ids = self
            .iter_resolved_of::<WlSurfaceId>(state)
            .collect::<Vec<_>>();
        for surface_id in surface_ids {
            if !surface_id.exists(state) {
                continue;
            }
            let mut surface = surface_id.grab(state);
            surface.discard_presentation_feedbacks_for_backend(self, state, dispatcher, backend);
        }
    }

    fn log_request<R: Parser>(&self, object_id: u32, result: Result<&R::Request, &ParserError>)
    where
        R::Request: std::fmt::Debug,
    {
        let interface_name = R::INTERFACE_NAME;
        let (proc, pid) = match self.info.as_ref() {
            Some(info) => (&*info.command, info.cred.pid),
            None => ("UNKNOWN", 0),
        };
        let id = self.id;
        let s = format!("in {:?}-{}-{}", proc, pid, id.0);
        match result {
            Ok(req) => {
                log::trace!(target: &s, "[{}] {}::{:?}", object_id, interface_name, req)
            }
            Err(err) => {
                log::error!(target: &s, "[{}] {}: {:?}", object_id, interface_name, err)
            }
        }
    }

    fn log_event<E: EventWriter + Constants>(
        writer: &E,
        event: &E::Event,
        dispatcher: &mut ClientDispatcher,
    ) {
        if log::Level::Trace > log::max_level() {
            return;
        }
        let (proc, pid) = match dispatcher.command.as_ref() {
            Some(info) => (&*info.command, info.cred.pid),
            None => ("UNKNOWN", 0),
        };
        let id = dispatcher.client_id;
        let s = format!("out {:?}-{}-{}", proc, pid, id.0);
        let skip = (!crate::logger::ENABLE_NOISY_LOG)
            .then(|| ["wl_pointer"].contains(&E::INTERFACE_NAME))
            .unwrap_or(false);
        if skip {
            return;
        }
        log::trace!(target: &s, "[{}] {}::{:?}", u32::from(writer.object_id()), E::INTERFACE_NAME, event);
    }

    /// Helper that just log the request
    pub fn parse_and_log<R: Parser>(&self, req: RawRequest) -> Result<R::Request, ParserError>
    where
        R::Request: std::fmt::Debug,
    {
        let object_id = req.object_id;
        let result = R::parse_request(req);
        if log::Level::Trace <= log::max_level() {
            self.log_request::<R>(object_id, result.as_ref());
        }
        result
    }

    /// Decode a request. On failure, post `wl_display.invalid_method` and disconnect this client.
    pub fn parse<R: Parser>(
        &mut self,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> Result<R::Request, AbortRequest>
    where
        R::Request: std::fmt::Debug,
    {
        let object_id = req.object_id();
        match self.parse_and_log::<R>(req) {
            Ok(request) => Ok(request),
            Err(error) => {
                log::warn!(
                    "malformed {} request on object {object_id:?}: {error:?}",
                    R::INTERFACE_NAME
                );
                self.protocol_violation_disconnect(
                    dispatcher,
                    object_id,
                    wl_display::Error::InvalidMethod as u32,
                    format!("malformed request: {error:?}"),
                );
                Err(AbortRequest::Malformed)
            }
        }
    }

    fn handle_registry_global_added(
        &self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        name: u32,
        interface: String,
        version: u32,
    ) {
        let registries = self
            .iter_resolved_of::<WlRegistryId>(state)
            .collect::<Vec<_>>();
        for registry in registries {
            registry
                .rref(state)
                .send_global(dispatcher, name, interface.clone(), version);
            state
                .globals
                .note_offer(crate::globals::GlobalName::from_raw(name), registry);
        }
    }

    fn handle_registry_global_removed(
        &self,
        state: &State,
        dispatcher: &mut ClientDispatcher,
        name: u32,
    ) {
        let global_name = crate::globals::GlobalName::from_raw(name);
        for registry in self.iter_resolved_of::<WlRegistryId>(state) {
            if !state.globals.was_offered(global_name, registry) {
                continue;
            }
            let registry = registry.rref(state);
            registry.send_global_remove(dispatcher, name);
        }
    }

    fn handle_output_state_changed(
        &mut self,
        state: &State,
        dispatcher: &mut ClientDispatcher,
        output_id: OutputId,
    ) {
        let xdg_output_ids = self
            .iter_resolved_of::<ZxdgOutputV1Id>(state)
            .collect::<Vec<_>>();
        let wl_output_ids = self
            .iter_resolved_of::<WlOutputId>(state)
            .collect::<Vec<_>>();

        for xdg_output in xdg_output_ids {
            let xdg_output = xdg_output.rref(state);
            if xdg_output.matches_output(output_id) {
                xdg_output.send_state(state, dispatcher);
            }
        }

        for wl_output in wl_output_ids {
            let wl_output = wl_output.rref(state);
            if wl_output.matches_output(output_id) {
                wl_output.send_state(state, dispatcher);
            }
        }
    }

    pub(crate) fn output_object_ids_for_output(
        &self,
        state: &State,
        output_id: OutputId,
    ) -> Vec<Resolved<WlOutputId>> {
        let mut output_object_ids = Vec::new();
        let object_ids = self
            .iter_resolved_of::<WlOutputId>(state)
            .collect::<Vec<_>>();
        for wl_output in object_ids {
            if wl_output.rref(state).matches_output(output_id) {
                output_object_ids.push(wl_output);
            }
        }
        output_object_ids
    }

    fn send_surface_output_membership_event(
        &self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        surface: Resolved<WlSurfaceId>,
        output: Resolved<WlOutputId>,
        entered: bool,
    ) {
        log::debug!(
            "surface output event surface={:?} output={:?} entered={} wl_output_resources={:?}",
            surface.object_id(),
            output.wayland_id(),
            entered,
            [output.wayland_id()]
        );
        let Some(surface) = surface.try_ref(state) else {
            return;
        };
        if entered {
            surface.send_output_enter(output, dispatcher);
        } else {
            surface.send_output_leave(output, dispatcher);
        }
    }

    pub(crate) fn refresh_workspace_protocols(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let manager_object_ids = self
            .iter_resolved_of::<ExtWorkspaceManagerV1Id>(state)
            .collect::<Vec<_>>();
        for manager in manager_object_ids {
            let Some(mut manager) = manager.try_grab(state) else {
                continue;
            };
            manager.sync(self, state, dispatcher, true);
        }
    }

    pub(crate) fn replay_surface_output_for_output(
        &self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        output_id: OutputId,
    ) {
        let object_ids = self
            .iter_resolved_of::<WlSurfaceId>(state)
            .collect::<Vec<_>>();
        let mut surfaces = Vec::new();
        for surface in object_ids {
            let surface_ref = surface.rref(state);
            if surface_ref.contains_output(output_id) {
                surfaces.push(surface);
            }
        }
        let output_objects = self.output_object_ids_for_output(state, output_id);
        for surface in surfaces {
            for output in &output_objects {
                self.send_surface_output_membership_event(
                    state, dispatcher, surface, *output, true,
                );
            }
        }
    }

    pub(crate) fn refresh_surface_outputs(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let root_surface_ids = state.root_surfaces();
        let active_cursor_surface = active_cursor_surface_refresh_params(state, self.id);
        let active_dnd_icon_surface = active_dnd_icon_surface_refresh_params(state, self.id);
        log::debug!("refresh_surface_outputs client={:?} roots={:?}", self.id, root_surface_ids);
        for root_surface in root_surface_ids {
            let Some(surface) = root_surface.try_ref(state) else {
                continue;
            };
            log::debug!(
                "refresh_surface_outputs root candidate object={:?} client={:?} is_root={}",
                surface.object_id(),
                surface.client_id(),
                surface.is_root()
            );
            if surface.client_id() != self.id || !surface.is_root() {
                continue;
            }
            if state.is_popup_root_surface(root_surface) {
                continue;
            }
            if surface.has_cursor_role() {
                if let Some((active_surface, base_x, base_y)) = active_cursor_surface {
                    if active_surface == root_surface {
                        refresh_surface_tree_outputs(
                            self,
                            state,
                            dispatcher,
                            root_surface,
                            base_x,
                            base_y,
                        );
                    } else {
                        clear_surface_tree_outputs(self, state, dispatcher, root_surface);
                    }
                } else {
                    clear_surface_tree_outputs(self, state, dispatcher, root_surface);
                }
                continue;
            }
            if surface.has_dnd_icon_role() {
                if let Some((active_surface, base_x, base_y)) = active_dnd_icon_surface {
                    if active_surface == root_surface {
                        refresh_surface_tree_outputs(
                            self,
                            state,
                            dispatcher,
                            root_surface,
                            base_x,
                            base_y,
                        );
                    } else {
                        clear_surface_tree_outputs(self, state, dispatcher, root_surface);
                    }
                } else {
                    clear_surface_tree_outputs(self, state, dispatcher, root_surface);
                }
                continue;
            }
            if !root_has_protocol_visibility(state, root_surface) {
                clear_surface_tree_outputs(self, state, dispatcher, root_surface);
                continue;
            }
            refresh_surface_tree_outputs(self, state, dispatcher, root_surface, 0, 0);
        }
    }

    fn reconfigure_mapped_toplevels(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let opened = state.relayout_dirty_workspaces();
        if !opened.is_empty() {
            kick_visual_animation_pump(dispatcher);
        }
        let tiled_resize_surfaces = crate::seat::tiled_resize_active_root_surfaces(state);
        let toplevels = self
            .iter_resolved_of::<XdgToplevelId>(state)
            .collect::<Vec<_>>();
        for toplevel in toplevels {
            let toplevel_ref = toplevel.rref(state);
            let xdg_surface = toplevel_ref.xdg_surface();
            let has_decoration_object = toplevel_ref.has_decoration_object();
            let requested_decoration_mode = toplevel_ref.requested_decoration_mode(state);
            let xdg_surface_ref = xdg_surface.rref(state);
            let wl_surface = xdg_surface_ref.wl_surface();
            let Some(root_surface) = state.root_surface_for_surface(wl_surface) else {
                continue;
            };
            if !root_has_protocol_visibility(state, root_surface) {
                continue;
            }
            let tiled_geometry = state.tiled_root_surface_geometry(root_surface);
            let resizing = tiled_resize_surfaces.contains(&root_surface);
            let surface_configure_bounds = state.configure_bounds_for_surface(wl_surface, None);
            let scheduled_floating_restore =
                if tiled_geometry.is_none() && !state.root_surface_is_fullscreen(root_surface) {
                    state.take_scheduled_floating_restore_state(root_surface)
                } else {
                    None
                };
            let desired_decoration_mode = Some(state.xdg_decoration_mode_for_surface(
                Some(root_surface),
                has_decoration_object,
                requested_decoration_mode,
            ));

            {
                let mut xdg_surface = xdg_surface.grab(state);
                let mut toplevel = toplevel.grab(state);
                if let Some(mode) = desired_decoration_mode {
                    toplevel.schedule_decoration_mode(mode);
                }
                let tiled_configure_geometry = tiled_geometry.zip(desired_decoration_mode).map(
                    |(geometry, decoration_mode)| {
                        crate::root_geometry::project_configure_geometry(
                            geometry,
                            decoration_mode,
                            state.configured_decoration_insets(),
                        )
                    },
                );
                let configure_bounds = tiled_configure_geometry
                    .map(|geometry| geometry.size)
                    .or(surface_configure_bounds);
                if let Some((geometry, configure_geometry)) =
                    tiled_geometry.zip(tiled_configure_geometry)
                {
                    toplevel.send_tiled_layout_configure(
                        self,
                        state,
                        &mut xdg_surface,
                        geometry,
                        configure_geometry,
                        resizing,
                        dispatcher,
                    );
                } else if let Some(remembered_state) = scheduled_floating_restore {
                    toplevel.send_interactive_resize_configure(
                        self,
                        state,
                        configure_bounds,
                        &mut xdg_surface,
                        remembered_state.configure_size.0,
                        remembered_state.configure_size.1,
                        false,
                        dispatcher,
                    );
                } else {
                    toplevel.schedule_configure(
                        self,
                        state,
                        configure_bounds,
                        &mut xdg_surface,
                        dispatcher,
                    );
                }
            }
            if let Some((maximized, fullscreen)) = toplevel
                .try_ref(state)
                .map(|toplevel| toplevel.published_shell_state())
            {
                crate::capture::note_xdg_shell_state(
                    state, dispatcher, toplevel, maximized, fullscreen,
                );
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn test_reconfigure_mapped_toplevels(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.reconfigure_mapped_toplevels(state, dispatcher);
    }

    fn handle_output_removed(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        output_id: OutputId,
    ) {
        self.invalidate_gamma_control_for_output(state, dispatcher, output_id);
        self.forget_ctm_state_for_output(state, output_id);
        self.refresh_surface_outputs(state, dispatcher);
        let wl_output_ids = self
            .iter_object_ids_of::<WlOutput>(state)
            .collect::<Vec<_>>();
        let xdg_output_ids = self
            .iter_object_ids_of::<ZxdgOutputV1>(state)
            .collect::<Vec<_>>();

        for object_id in wl_output_ids {
            let Some(object) = state.objects.try_get_mut(object_id) else {
                continue;
            };
            let Some(wl_output) = object.downcast_mut::<WlOutput>() else {
                continue;
            };
            if wl_output.matches_output(output_id) {
                wl_output.mark_removed();
            }
        }
        for object_id in xdg_output_ids {
            let Some(object) = state.objects.try_get_mut(object_id) else {
                continue;
            };
            let Some(xdg_output) = object.downcast_mut::<ZxdgOutputV1>() else {
                continue;
            };
            if xdg_output.matches_output(output_id) {
                xdg_output.mark_removed();
            }
        }
        self.reconfigure_mapped_toplevels(state, dispatcher);
        relayout_client_layer_surfaces(self, state, dispatcher);
    }

    fn invalidate_gamma_control_for_output(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        output_id: OutputId,
    ) -> bool {
        let gamma_control_ids = self
            .objects
            .iter()
            .filter_map(|(wayland_id, entry)| {
                (entry.type_id == TypeId::of::<ZwlrGammaControlV1>())
                    .then_some(ZwlrGammaControlV1Id::from(u32::from(*wayland_id)))
            })
            .collect::<Vec<_>>();
        let mut invalidated = false;
        for gamma_control_id in gamma_control_ids {
            if !self.contains_object(gamma_control_id, state) {
                continue;
            }
            let gamma_control_object_id = self.object_id(gamma_control_id);
            let matches_output = state
                .objects
                .try_ref_object::<ZwlrGammaControlV1>(gamma_control_object_id)
                .is_some_and(|gamma_control| gamma_control.controls_output(output_id));
            if !matches_output {
                continue;
            }
            let mut gamma_control = self.grab_object(gamma_control_id, state);
            gamma_control.invalidate(state, dispatcher);
            invalidated = true;
        }
        invalidated
    }

    fn forget_ctm_state_for_output(&mut self, state: &mut State, output_id: OutputId) {
        let ctm_manager_ids = self
            .objects
            .iter()
            .filter_map(|(wayland_id, entry)| {
                (entry.type_id == TypeId::of::<HyprlandCtmControlManagerV1>())
                    .then_some(HyprlandCtmControlManagerV1Id::from(u32::from(*wayland_id)))
            })
            .collect::<Vec<_>>();
        for ctm_manager_id in ctm_manager_ids {
            if !self.contains_object(ctm_manager_id, state) {
                continue;
            }
            let mut ctm_manager = self.grab_object(ctm_manager_id, state);
            ctm_manager.forget_output(output_id);
        }
    }
}

fn surface_intersecting_outputs(
    state: &State,
    render_state: &crate::wayland_objects::wl_surface::SurfaceRenderState,
) -> Vec<OutputId> {
    let Some(surface_bounds) =
        RectI::new(render_state.x, render_state.y, render_state.width, render_state.height)
    else {
        return Vec::new();
    };
    let mut outputs = state.outputs_intersecting_rect(surface_bounds);
    if let Some(primary_output_id) = state.primary_output_for_rect(surface_bounds, None) {
        outputs.sort_by_key(|output_id| (*output_id != primary_output_id, output_id.as_u64()));
    }
    outputs
}

fn refresh_surface_tree_outputs(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    surface: Resolved<WlSurfaceId>,
    base_x: i32,
    base_y: i32,
) {
    let (stack, child_base, delta) = {
        let Some(mut surface_obj) = surface.try_grab(state) else {
            return;
        };
        let render_state = surface_obj.render_state_at(base_x, base_y);
        let surface_origin = render_state
            .as_ref()
            .map(|render_state| (render_state.x, render_state.y))
            .unwrap_or_else(|| {
                let (x, y) = surface_obj.root_position();
                (base_x + x, base_y + y)
            });
        let current_position = surface_obj.current_subsurface_position();
        let next_outputs = render_state
            .as_ref()
            .map(|render_state| surface_intersecting_outputs(state, render_state))
            .unwrap_or_default();
        let child_base =
            (surface_origin.0 + current_position.0, surface_origin.1 + current_position.1);
        log::debug!(
            "surface output refresh surface={:?} render={:?} child_base={:?} next_outputs={:?}",
            surface.object_id(),
            render_state.as_ref().map(|r| (r.x, r.y, r.width, r.height)),
            child_base,
            next_outputs
        );
        let delta = surface_obj.update_output_membership(next_outputs);
        log::debug!(
            "surface output delta surface={:?} entered={:?} left={:?} active_changed={}",
            surface.object_id(),
            delta.entered,
            delta.left,
            delta.active_output_changed
        );
        (surface_obj.current_stack().to_vec(), child_base, delta)
    };
    crate::capture::sync_toplevel_outputs(state, dispatcher, surface);

    for output_id in delta.left {
        for output in client.output_object_ids_for_output(state, output_id) {
            client.send_surface_output_membership_event(state, dispatcher, surface, output, false);
        }
    }
    for output_id in delta.entered {
        for output in client.output_object_ids_for_output(state, output_id) {
            client.send_surface_output_membership_event(state, dispatcher, surface, output, true);
        }
    }

    let preferred_output = surface
        .try_ref(state)
        .and_then(|surface_ref| surface_ref.preferred_output_id_for_object(state, surface))
        .and_then(|output_id| {
            state
                .outputs
                .try_get(output_id)
                .map(|output| (output.scale, output.legacy_scale().max(1), output.transform as u32))
        });
    if let Some((fractional_scale, scale, transform)) = preferred_output {
        let Some(mut surface_obj) = surface.try_grab(state) else {
            return;
        };
        surface_obj.send_preferred_fractional_scale_for_output(fractional_scale, state, dispatcher);
        surface_obj.send_preferred_buffer_state_for_output(scale, transform, dispatcher);
    }

    for entry in stack {
        if let crate::wayland_objects::wl_surface::StackEntry::Child(child_surface) = entry {
            refresh_surface_tree_outputs(
                client,
                state,
                dispatcher,
                child_surface,
                child_base.0,
                child_base.1,
            );
        }
    }
    for popup_surface in state.popup_child_surfaces(surface) {
        let Some((popup_x, popup_y)) = state.surface_scene_origin(popup_surface) else {
            continue;
        };
        refresh_surface_tree_outputs(client, state, dispatcher, popup_surface, popup_x, popup_y);
    }
}

fn clear_surface_tree_outputs(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    surface: Resolved<WlSurfaceId>,
) {
    let (stack, delta) = {
        let Some(mut surface_obj) = surface.try_grab(state) else {
            return;
        };
        let delta = surface_obj.update_output_membership(Vec::new());
        (surface_obj.current_stack().to_vec(), delta)
    };
    crate::capture::sync_toplevel_outputs(state, dispatcher, surface);

    for output_id in delta.left {
        for output in client.output_object_ids_for_output(state, output_id) {
            client.send_surface_output_membership_event(state, dispatcher, surface, output, false);
        }
    }

    for entry in stack {
        if let crate::wayland_objects::wl_surface::StackEntry::Child(child_surface) = entry {
            clear_surface_tree_outputs(client, state, dispatcher, child_surface);
        }
    }
    for popup_surface in state.popup_child_surfaces(surface) {
        clear_surface_tree_outputs(client, state, dispatcher, popup_surface);
    }
}

fn active_cursor_surface_refresh_params(
    state: &State,
    client_id: ClientId,
) -> Option<(Resolved<WlSurfaceId>, i32, i32)> {
    let (pointer_x, pointer_y) = state.seat.pointer_position();
    let crate::cursor::CursorSourceRef::Surface { cursor } = state.seat.visible_cursor_source()?
    else {
        return None;
    };
    if cursor.surface.client_id() != client_id {
        return None;
    }
    let surface = cursor.surface.try_ref(state)?;
    let (root_x, root_y) = surface.root_position();
    let base_x = pointer_x.round() as i32 - cursor.hotspot_x - root_x;
    let base_y = pointer_y.round() as i32 - cursor.hotspot_y - root_y;
    Some((cursor.surface, base_x, base_y))
}

fn active_dnd_icon_surface_refresh_params(
    state: &State,
    client_id: ClientId,
) -> Option<(Resolved<WlSurfaceId>, i32, i32)> {
    let drag = state.seat.active_drag()?;
    if drag.source.initiator_client_id() != client_id {
        return None;
    }
    let icon_surface = drag.icon_surface?;
    let surface = icon_surface.try_ref(state)?;
    let (pointer_x, pointer_y) = state.seat.pointer_position();
    let (root_x, root_y) = surface.root_position();
    Some((icon_surface, pointer_x.round() as i32 - root_x, pointer_y.round() as i32 - root_y))
}

/// A specialized `Dispatcher`
///
/// Difference is that this one contains the client fd, so we can send events
#[derive(derive_more::Deref, derive_more::DerefMut)]
pub struct ClientDispatcher<'a> {
    #[deref]
    #[deref_mut]
    dispatcher: &'a mut Dispatcher,
    client_fd: RawFd,
    client_id: ClientId,
    requester_object_id: Option<WaylandId>,
    command: Option<Rc<ProcessInfo>>,
    /// Deferred work to run after the current `Object::on` handler returns and its grabbed object
    /// is released (see `MsgReceived` in `Client::process`).
    post_actions: VecDeque<Box<dyn PostAction>>,
}

impl<'a> ClientDispatcher<'a> {
    fn make(client: &Client, dispatcher: &'a mut Dispatcher) -> Self {
        let command =
            if log::Level::Trace <= log::max_level() { client.info.clone() } else { None };
        Self {
            dispatcher,
            client_fd: client.fd.as_raw_fd(),
            client_id: client.id,
            requester_object_id: None,
            command,
            post_actions: VecDeque::new(),
        }
    }

    #[cfg(test)]
    pub fn set_requester_object_id_for_tests(&mut self, id: WaylandId) {
        self.requester_object_id = Some(id);
    }

    /// Enqueue work to run after the current `Object::on` handler returns and its grabbed object
    /// is released.
    pub fn dispatch_post_action<P: PostAction>(&mut self, action: P) {
        self.post_actions.push_back(Box::new(action));
    }

    /// Run all queued post-actions until the queue is empty (later actions may enqueue more).
    #[cfg(test)]
    pub fn drain_post_actions(&mut self, client: &mut Client, state: &mut State) {
        while let Some(action) = self.post_actions.pop_front() {
            action.execute(client, state, self);
        }
    }

    /// `wl_display` singleton id is always `1` in this compositor.
    pub fn send_wl_display_error(
        &mut self,
        object_id: WaylandId,
        code: u32,
        message: impl AsRef<str>,
    ) {
        // The raw event skips `EventWriter::send_event`, so the error would otherwise
        // never show up in the client trace.
        let (proc, pid) = match self.command.as_ref() {
            Some(info) => (&*info.command, info.cred.pid),
            None => ("UNKNOWN", 0),
        };
        let target = format!("out {:?}-{}-{}", proc, pid, self.client_id.0);
        log::warn!(
            target: &target,
            "[1] wl_display::Error {{ object_id: WaylandId({}), code: {}, message: {:?} }}",
            u32::from(object_id),
            code,
            message.as_ref()
        );
        let mut raw = RawEvent::new(WaylandId::from(1u32), 0, 3);
        raw.id(object_id);
        raw.uint(code);
        raw.string(message.as_ref().to_string());
        self.send_event(raw);
    }

    pub fn disconnect_client(&mut self, client_id: ClientId) {
        self.dispatch(ClientManagerAction::Close { id: client_id });
    }

    pub fn send_event(&mut self, mut event: RawEvent) {
        let fds = event.take_fds();
        let action = IoUringAction::Sendmsg {
            fd: self.client_fd,
            buffer: event.into_bytes(),
            fds,
            dispatch_to: Redispatcher::None,
        };
        self.dispatch(action);
    }
}

#[cfg(test)]
impl Client {
    pub(crate) fn test_dispatcher<'a>(
        &self,
        dispatcher: &'a mut Dispatcher,
    ) -> ClientDispatcher<'a> {
        ClientDispatcher::make(self, dispatcher)
    }
}

impl Model for Client {
    type Action = ClientAction;
    const START: Option<Self::Action> = Some(Self::Action::Start);
    const STOP: Option<Self::Action> = Some(Self::Action::Close);

    fn process(
        &mut self,
        state: &mut State,
        action: ClientAction,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        use ClientAction::*;

        let mut dispatcher = ClientDispatcher::make(self, dispatcher);

        match action {
            Start => {
                self.read_process_info();
                state.set_client_process_info(self.id, self.info.clone());
                log::info!(
                    "new client_id={:?} command={:?}",
                    self.id,
                    self.info.as_ref().map(|p| &p.command)
                );

                let display: WlDisplay = WlDisplay::default();
                let _ = self.insert_object(state, display);

                let (client_reader_id, client_reader) =
                    ClientReader::new(self.fd.as_raw_fd(), self.id);
                self.reader_id = Some(client_reader_id);
                dispatcher.start_model(client_reader_id, client_reader);
            }
            FdsReceived { fds } => {
                self.pending_fds.extend(fds);
            }
            SeatEvent { event } => {
                self.handle_seat_event(state, &mut dispatcher, event);
            }
            RequestToplevelClose { toplevel } => {
                if let Some(toplevel) = toplevel.try_ref(state) {
                    toplevel.send_close(&mut dispatcher);
                }
            }
            RefreshDataDeviceSelection => {
                self.refresh_data_device_selection(state, &mut dispatcher);
            }
            RefreshDataControlSelection => {
                self.refresh_data_control_selection(state, &mut dispatcher);
            }
            RefreshDataControlPrimarySelection => {
                self.refresh_data_control_primary_selection(state, &mut dispatcher);
            }
            RefreshExtDataControlSelection => {
                self.refresh_ext_data_control_selection(state, &mut dispatcher);
            }
            RefreshExtDataControlPrimarySelection => {
                self.refresh_ext_data_control_primary_selection(state, &mut dispatcher);
            }
            DataControlSourceEvent { source, event } => {
                self.handle_data_control_source_event(state, &mut dispatcher, source, event);
            }
            ExtDataControlSourceEvent { source, event } => {
                self.handle_ext_data_control_source_event(state, &mut dispatcher, source, event);
            }
            RefreshPrimarySelection => {
                self.refresh_primary_selection(state, &mut dispatcher);
            }
            PrimarySelectionSourceEvent { source, event } => {
                self.handle_primary_selection_source_event(state, &mut dispatcher, source, event);
            }
            DataDeviceDragEnter { serial, surface, x, y, source } => {
                self.handle_data_device_drag_enter(
                    state,
                    &mut dispatcher,
                    serial,
                    surface,
                    x,
                    y,
                    source,
                );
            }
            DataDeviceDragLeave => {
                self.handle_data_device_drag_leave(state, &mut dispatcher);
            }
            DataDeviceDragMotion { time, x, y } => {
                self.handle_data_device_drag_motion(state, &mut dispatcher, time, x, y);
            }
            DataDeviceDragDrop => {
                self.handle_data_device_drag_drop(state, &mut dispatcher);
            }
            DataSourceEvent { source, event } => {
                self.handle_data_source_event(state, &mut dispatcher, source, event);
            }
            RegistryGlobalAdded { name, interface, version } => {
                self.handle_registry_global_added(state, &mut dispatcher, name, interface, version);
            }
            RegistryGlobalRemoved { name } => {
                self.handle_registry_global_removed(state, &mut dispatcher, name);
            }
            OutputStateChanged { output_id } => {
                self.handle_output_state_changed(state, &mut dispatcher, output_id);
                self.refresh_surface_outputs(state, &mut dispatcher);
                self.refresh_workspace_protocols(state, &mut dispatcher);
                self.reconfigure_mapped_toplevels(state, &mut dispatcher);
                relayout_client_layer_surfaces(self, state, &mut dispatcher);
            }
            OutputRemoved { output_id } => {
                self.handle_output_removed(state, &mut dispatcher, output_id);
                self.refresh_workspace_protocols(state, &mut dispatcher);
            }
            GammaControlFailed { output_id } => {
                if self.invalidate_gamma_control_for_output(state, &mut dispatcher, output_id)
                    && state.outputs.try_get(output_id).is_some()
                {
                    request_output(state, output_id, &mut dispatcher);
                }
            }
            RefreshLayerSurfaces => {
                relayout_client_layer_surfaces(self, state, &mut dispatcher);
                self.reconfigure_mapped_toplevels(state, &mut dispatcher);
            }
            WorkspaceStateChanged => {
                self.reconfigure_mapped_toplevels(state, &mut dispatcher);
                self.refresh_surface_outputs(state, &mut dispatcher);
                self.refresh_workspace_protocols(state, &mut dispatcher);
            }
            RefreshSurfaceOutputs => {
                self.refresh_surface_outputs(state, &mut dispatcher);
            }
            ReleaseBuffer { buffer_id } => {
                if self.contains_object(buffer_id, state) {
                    let buffer = self.ref_object_mut(buffer_id, state);
                    buffer.send_release(&mut dispatcher);
                }
            }
            CompleteBufferReleaseCallbacks { callbacks } => {
                use crate::wayland_objects::wl_display::WlDisplay;
                let display = WlDisplay::default();
                for callback in callbacks {
                    if !self.contains_object(callback.wayland_id(), state) {
                        continue;
                    }
                    callback.rref(state).done(0, &mut dispatcher);
                    self.delete_id(&display, state, &mut dispatcher, callback.wayland_id().into());
                }
            }
            CompleteSurfaceFrameCallbacks { surfaces, time_msec } => {
                self.complete_surface_frame_callbacks(state, &mut dispatcher, surfaces, time_msec);
            }
            SendSessionLockLocked => {
                if let Some(lock) = state.session_lock.lock() {
                    if lock.client_id() == self.id {
                        if let Some(lock) = lock.try_ref_mut(state) {
                            lock.send_locked(&mut dispatcher);
                        }
                    }
                }
            }
            ReconfigureLockOutput { output_id } => {
                let Some(surface_id) = state.session_lock.surface_for_output(output_id) else {
                    return Ok(());
                };
                if surface_id.client_id() != self.id {
                    return Ok(());
                }
                let Some((width, height)) = state
                    .outputs
                    .try_get(output_id)
                    .map(|output| output.logical_size())
                else {
                    return Ok(());
                };
                let width = u32::try_from(width).unwrap_or(0);
                let height = u32::try_from(height).unwrap_or(0);
                let serial = state.seat.next_serial();
                let Some(surface) = surface_id.try_ref_mut(state) else {
                    return Ok(());
                };
                if surface.is_inert() || surface.configured_size() == Some((width, height)) {
                    return Ok(());
                }
                surface.configure(serial, width, height, &mut dispatcher);
            }
            PresentationFeedbackPresented {
                feedbacks,
                output_id,
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                flags,
            } => {
                self.present_feedback(
                    state,
                    &mut dispatcher,
                    feedbacks,
                    output_id,
                    tv_sec_hi,
                    tv_sec_lo,
                    tv_nsec,
                    refresh,
                    seq_hi,
                    seq_lo,
                    flags,
                );
            }
            PresentationFeedbackDiscarded { feedbacks } => {
                self.discard_presentation_feedback(state, &mut dispatcher, feedbacks);
            }
            DiscardSurfacePresentationFeedbacksForBackend { drm_id } => {
                self.discard_surface_presentation_feedbacks_for_backend(
                    state,
                    &mut dispatcher,
                    drm_id,
                );
            }
            CancelBlockedSurface { surface, waiter_id } => {
                let Some(mut surface) = surface.try_grab(state) else {
                    return Ok(());
                };
                surface.cancel_blocked_if_waiter_matches(waiter_id, self, state, &mut dispatcher);
            }
            DismissPopupGrab { popups } => {
                self.dismiss_popup_ids(state, &mut dispatcher, popups, None);
            }
            InteractiveResizeConfigure { xdg_toplevel, width, height, resizing } => {
                let Some(xdg_surface) = xdg_toplevel
                    .try_ref(state)
                    .map(|toplevel| toplevel.xdg_surface())
                else {
                    return Ok(());
                };
                if !xdg_surface.exists(state) {
                    return Ok(());
                }
                let configure_bounds = {
                    xdg_surface.try_ref(state).and_then(|xdg_surface| {
                        let wl_surface = xdg_surface.wl_surface();
                        state.configure_bounds_for_surface(wl_surface, None)
                    })
                };
                {
                    let mut xdg_surface = xdg_surface.grab(state);
                    let mut toplevel = xdg_toplevel.grab(state);
                    toplevel.send_interactive_resize_configure(
                        self,
                        state,
                        configure_bounds,
                        &mut xdg_surface,
                        width,
                        height,
                        resizing,
                        &mut dispatcher,
                    );
                }
                if let Some((maximized, fullscreen)) = xdg_toplevel
                    .try_ref(state)
                    .map(|toplevel| toplevel.published_shell_state())
                {
                    crate::capture::note_xdg_shell_state(
                        state,
                        &mut dispatcher,
                        xdg_toplevel,
                        maximized,
                        fullscreen,
                    );
                }
            }
            MsgReceived { mut request } => {
                let requester_object_id = request.object_id();
                let Some(obj_id) = self
                    .objects
                    .get(&request.object_id())
                    .map(|entry| entry.object_id)
                else {
                    log::warn!(
                        "unknown Wayland object id {:?} from client {:?} (request opcode {})",
                        request.object_id(),
                        self.id,
                        request.request_id
                    );
                    self.protocol_violation_disconnect(
                        &mut dispatcher,
                        request.object_id(),
                        wl_display::Error::InvalidObject as u32,
                        "invalid object id",
                    );
                    return Ok(());
                };

                assert!(dispatcher.requester_object_id.is_none());
                let mut pending_fds = std::mem::take(&mut self.pending_fds);
                request.fds = Some(&mut pending_fds);
                dispatcher.requester_object_id = Some(requester_object_id);

                let _ = {
                    let obj = state.objects.grab(obj_id);
                    obj.on(self, state, request, &mut dispatcher)
                };
                while let Some(action) = dispatcher.post_actions.pop_front() {
                    action.execute(self, state, &mut dispatcher);
                }

                dispatcher.requester_object_id = None;
                self.pending_fds = pending_fds;
            }
            IdleNotification { id, notice } => {
                if let Some(notification) = self.resolve(id, state) {
                    notification
                        .rref(state)
                        .send_notice(notice, &mut dispatcher);
                }
            }
            DeliverCaptureEvents => {
                crate::capture::deliver(self, state, &mut dispatcher);
            }
            Close => {
                log::info!("client_id={:?} closed", self.id);
                state.capture.drop_client(self.id);
                if let Some(id) = self.reader_id.take() {
                    dispatcher.stop_model(id);
                };
                state.clear_client_process_info(self.id);
                state.activation_tokens.remove_client(self.id);
                let client_objects: Vec<(WaylandId, ObjectId)> = self
                    .objects
                    .iter()
                    .map(|(wid, entry)| (*wid, entry.object_id))
                    .collect();
                let had_client_objects = !client_objects.is_empty();
                for (_, object_id) in &client_objects {
                    if let Some(mut object) = state.objects.try_grab(*object_id) {
                        object.on_client_disconnect(self, state, &mut dispatcher);
                    }
                }
                for (wid, object_id) in client_objects {
                    self.objects.remove(&wid);
                    let _ = state.remove_object(object_id);
                }
                let seat_removal = state.seat.remove_client(self.id);
                if seat_removal.interactive_grab_cleared {
                    crate::seat::recover_pointer_after_interactive_grab_loss(
                        state,
                        &mut dispatcher,
                        0,
                    );
                }
                if state
                    .seat
                    .active_drag()
                    .is_some_and(|drag| drag.phase == crate::state::DragPhase::Cancelled)
                {
                    crate::seat::cancel_active_drag(state, &mut dispatcher, None);
                }
                if seat_removal.keyboard_focus_cleared {
                    crate::seat::focus_after_client_disconnect(state, &mut dispatcher);
                }
                crate::idle_inhibit::refresh(state, &mut dispatcher, std::time::Instant::now());
                self.pending_fds.clear();
                if seat_removal.cursor_changed
                    || seat_removal.interactive_grab_cleared
                    || had_client_objects
                {
                    request_active_output(state, &mut dispatcher);
                }
                if had_client_objects {
                    dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
                }
            }
        };

        Ok(())
    }
}

/// Store and parse clients messages
struct Buffer {
    data: Vec<u8>,
    pos: usize,
}

impl Buffer {
    fn new() -> Self {
        Self { data: vec![0; 32 * 1024], pos: 0 }
    }

    fn append(&mut self, buffer: &[u8]) {
        let pos = self.pos;
        let buffer_len = buffer.len();
        self.data[pos..pos + buffer_len].copy_from_slice(buffer);
        self.pos += buffer_len;
    }

    fn get_header(&self) -> Option<[u32; 2]> {
        if self.pos < 8 {
            return None;
        }

        let object_id = u32::from_ne_bytes(self.data[0..4].try_into().unwrap());
        let word = u32::from_ne_bytes(self.data[4..8].try_into().unwrap());

        // word is 2 u16: [len; request]
        Some([object_id, word])
    }

    fn get_whole_message(&self) -> Option<&[u8]> {
        let [_obj_id, word] = self.get_header()?;
        let len = (word >> 16) as usize;
        // let req = (word & 0xFFFF) as usize;
        // dbg!(obj_id, len, req);
        self.data.get(..len)
    }

    fn len(&self) -> usize {
        self.pos
    }

    fn consume_msg(&mut self) {
        let [_obj_id, word] = self.get_header().unwrap();
        let len = (word >> 16) as usize;

        if self.pos == len {
            self.pos = 0;
        } else {
            assert!(self.pos > len);
            self.data.copy_within(len..self.pos, 0);
            self.pos -= len;
        }
    }
}

model_instance_id!(ClientReaderId => ClientReader);

/// Responsible for reading client's fd, parsing the message, and
/// notify `Client`.
struct ClientReader {
    fd: RawFd,
    id: ClientReaderId,
    client_id: ClientId,
    read_buffer: Buffer,
    neof: usize,
    recvmsg_op_id: Option<RingId>,
}

enum ClientReaderAction {
    Start,
    ReadMsg { buffer: Box<[u8]>, fds: Vec<OwnedFd> },
    ErrorOnSocket { error: std::io::Error },
    Stop,
}

impl ClientReader {
    fn new(fd: RawFd, client_id: ClientId) -> (ClientReaderId, Self) {
        let id = ClientReader::next_id();
        let read_buffer = Buffer::new();
        let neof = 0;
        let this = Self { fd, id, client_id, read_buffer, neof, recvmsg_op_id: None };
        (id, this)
    }

    fn next_id() -> ClientReaderId {
        use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
        pub static CLIENT_READER_ID: AtomicU64 = AtomicU64::new(0);
        let id = CLIENT_READER_ID.fetch_add(1, Relaxed);
        ClientReaderId(id)
    }

    fn is_eof(&mut self, buffer: &[u8]) -> bool {
        if !buffer.is_empty() {
            self.neof = 0;
        } else {
            self.neof += 1;
            // Number of EOF (len == 0) received
            // Consider EOF reached after 2
            // https://github.com/facebook/rocksdb/pull/6441#issuecomment-589843435
            if self.neof == 2 {
                return true;
            }
        }
        false
    }

    fn dispatch_recvmsg(&mut self, dispatcher: &mut Dispatcher) {
        let recvmsg_op_id = RingId::fresh();
        self.recvmsg_op_id = Some(recvmsg_op_id);
        dispatcher.dispatch(IoUringAction::Recvmsg {
            id: Some(recvmsg_op_id),
            fd: self.fd,
            dispatch_to: Redispatcher::to_instance(self.id, |result| match result {
                Ok(RecvmsgCompleted { fd: _, buffer, fds }) => {
                    ClientReaderAction::ReadMsg { buffer, fds }
                }
                Err(error) => ClientReaderAction::ErrorOnSocket { error },
            }),
        });
    }

    fn dispatch_close(&self, dispatcher: &mut Dispatcher) {
        dispatcher.dispatch(ClientManagerAction::Close { id: self.client_id });
    }
}

impl Model for ClientReader {
    type Action = ClientReaderAction;
    const START: Option<Self::Action> = Some(Self::Action::Start);
    const STOP: Option<Self::Action> = Some(Self::Action::Stop);

    fn process(
        &mut self,
        _state: &mut State,
        action: Self::Action,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        use ClientReaderAction::*;

        match action {
            Start => {
                self.dispatch_recvmsg(dispatcher);
            }
            ReadMsg { buffer, fds } => {
                self.recvmsg_op_id = None;
                if self.is_eof(&buffer) {
                    self.dispatch_close(dispatcher);
                    return Ok(());
                }
                if !fds.is_empty() {
                    dispatcher.dispatch_to(self.client_id, ClientAction::FdsReceived { fds });
                }
                self.read_buffer.append(&buffer);
                loop {
                    let msg: &[u8] = match self.read_buffer.get_whole_message() {
                        Some(msg) => msg,
                        None => {
                            self.dispatch_recvmsg(dispatcher);
                            return Ok(());
                        }
                    };
                    let request: RawRequest = match msg.try_into() {
                        Ok(request) => request,
                        Err(()) => {
                            log::warn!(
                                "malformed Wayland message from client {:?}, disconnecting",
                                self.client_id
                            );
                            self.dispatch_close(dispatcher);
                            return Ok(());
                        }
                    };
                    dispatcher.dispatch_to(self.client_id, ClientAction::MsgReceived { request });
                    self.read_buffer.consume_msg();
                }
            }
            ErrorOnSocket { error } => {
                self.recvmsg_op_id = None;
                log::error!("An error occured on the socket of {:?}: {:?}", self.client_id, error);
                // Dispatch an action stop our Model
                self.dispatch_close(dispatcher);
            }
            Stop => {
                if let Some(recvmsg_op_id) = self.recvmsg_op_id.take() {
                    dispatcher.dispatch(IoUringAction::Cancel { target: recvmsg_op_id });
                }
            }
        };

        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct RawEvent {
    pub object_id: u32,
    pub request_id: u16,
    pub arguments: Arguments,
    fds: Vec<RawFd>,
}

pub trait EventWriter: Constants + Sized {
    type Event: 'static + std::fmt::Debug;
    /// Helper method, for loggings
    fn object_id(&self) -> WaylandId;
    fn to_raw_event(&self, event: Self::Event) -> RawEvent;
    fn send_event(&self, event: Self::Event, dispatcher: &mut ClientDispatcher) {
        Client::log_event::<Self>(self, &event, dispatcher);
        dispatcher.send_event(self.to_raw_event(event));
    }
    /// Send `wl_display.error` with this object's id as the source of the error.
    fn send_error(&self, dispatcher: &mut ClientDispatcher, code: u32, message: impl AsRef<str>) {
        dispatcher.send_wl_display_error(self.object_id(), code, message.as_ref());
    }
}

impl RawEvent {
    pub fn new(id: WaylandId, req_id: u16, nargs: usize) -> Self {
        Self {
            object_id: id.into(),
            request_id: req_id,
            arguments: Arguments {
                // + 2 for object_id + (length | req_id) on serialization
                args: VecDeque::with_capacity(nargs + 2),
            },
            fds: Vec::default(),
        }
    }

    fn push(&mut self, value: u32) {
        self.arguments.args.push_back(value);
    }

    pub fn uint(&mut self, uint: u32) {
        self.push(uint);
    }

    pub fn int(&mut self, int: i32) {
        self.push(int as u32);
    }

    pub fn id<I>(&mut self, id: I)
    where
        I: Into<u32>,
    {
        self.push(id.into());
    }

    pub fn new_id_with_interface<I>(&mut self, id: I) -> Result<I, ParserError>
    where
        I: Into<WaylandId> + Copy,
    {
        let w: WaylandId = id.into();
        self.id(w);
        Ok(id)
    }

    pub fn string<S: AsRef<str>>(&mut self, s: S) {
        let s = s.as_ref();
        let len: u32 = s.len().try_into().unwrap();
        self.push(len + 1); // +1 for terminating null byte

        let null_byte = std::iter::once(0);
        let mut iter = s
            .as_bytes()
            .iter()
            .copied()
            .chain(null_byte)
            .array_chunks::<4>();
        for bytes in &mut iter {
            self.push(u32::from_ne_bytes(bytes));
        }
        let remainder = iter.into_remainder();
        let remainder = remainder.as_slice();
        if !remainder.is_empty() {
            let mut bytes = [0u8; 4];
            bytes[..remainder.len()].copy_from_slice(remainder);
            self.push(u32::from_ne_bytes(bytes));
        }
    }

    /// Wayland `fixed` is a signed 24.8 value in the wire format.
    pub fn fixed(&mut self, fixed: f32) {
        let v = (fixed * 256.0).round() as i32;
        self.int(v);
    }

    pub fn array(&mut self, array: Array) {
        let len: u32 = array.len().try_into().unwrap();
        self.push(len * 4); // len in number of bytes
        for word in array.iter() {
            self.push(*word);
        }
    }

    pub fn fd(&mut self, fd: RawFd) {
        self.fds.push(fd)
    }

    pub fn into_bytes(self) -> Box<[u8]> {
        // let cloned = self.clone();

        let mut words = self.arguments.args;
        let length = words.len() + 2;

        let length: u16 = (length * 4).try_into().unwrap();
        let word = (length as u32) << 16 | self.request_id as u32;

        words.push_front(word);
        words.push_front(self.object_id);

        let bytes: Box<[u8]> = words.into_iter().flat_map(u32::to_ne_bytes).collect();

        // let req: RawRequest = (&*bytes).try_into().unwrap();
        // eprintln!("req parsed =     {:?}", req);
        // assert_eq!(req.arguments, cloned.arguments);
        // assert_eq!(req.object_id, cloned.object_id);
        // assert_eq!(req.request_id, cloned.request_id);

        bytes
    }

    fn take_fds(&mut self) -> Box<[RawFd]> {
        std::mem::take(&mut self.fds).into_boxed_slice()
    }
}

#[derive(Debug, derive_more::Deref, derive_more::DerefMut)]
pub struct RawRequest<'a> {
    pub object_id: u32,
    pub request_id: u16,
    #[deref]
    #[deref_mut]
    pub arguments: Arguments,
    pub fds: Option<&'a mut VecDeque<OwnedFd>>,
}

impl RawRequest<'_> {
    pub fn object_id(&self) -> WaylandId {
        WaylandId::from(self.object_id)
    }

    pub fn fd(&mut self) -> Result<RawFd, ParserError> {
        self.fds
            .as_mut()
            .unwrap()
            .pop_front()
            .map(IntoRawFd::into_raw_fd)
            .ok_or(ParserError::MissingFileDescriptor)
    }
}

impl TryFrom<&[u8]> for RawRequest<'static> {
    type Error = ();

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() < 8 {
            return Err(());
        }

        let object_id = u32::from_ne_bytes(value[0..4].try_into().unwrap());
        let word = u32::from_ne_bytes(value[4..8].try_into().unwrap());

        let len = (word >> 16) as usize;
        let request_id = (word & 0xFFFF) as u16;

        if len % 4 != 0 {
            return Err(()); // Invalid
        }

        let args = value[8..]
            .iter()
            .copied()
            .array_chunks::<4>()
            .map(u32::from_ne_bytes)
            .collect();

        Ok(Self {
            object_id,
            request_id,
            arguments: Arguments { args },
            fds: None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arguments {
    args: VecDeque<u32>,
}

#[cfg(test)]
impl Arguments {
    pub(crate) fn from_words(args: VecDeque<u32>) -> Self {
        Self { args }
    }
}

impl Arguments {
    fn pop(&mut self) -> Result<u32, ParserError> {
        self.args.pop_front().ok_or(ParserError::Malformed)
    }

    pub fn uint(&mut self) -> Result<u32, ParserError> {
        self.pop()
    }

    pub fn int(&mut self) -> Result<i32, ParserError> {
        self.uint().map(|uint| uint as i32)
    }

    pub fn id<I>(&mut self) -> Result<I, ParserError>
    where
        I: From<u32>,
    {
        self.pop().map(Into::into)
    }

    /// Parses Wayland `new_id` marshalling: interface name, client-requested version, object id.
    pub fn new_id_with_interface<I>(&mut self) -> Result<(I, u32), ParserError>
    where
        I: From<u32>,
    {
        let _interface_name = self.string()?;
        let requested_version = self.uint()?;
        let id = self.id()?;
        Ok((id, requested_version))
    }

    pub fn string(&mut self) -> Result<String, ParserError> {
        let len = self.uint()? as usize;
        // let nwords = len / 4;
        let nwords = len.next_multiple_of(4) / 4;

        let mut bytes = Vec::with_capacity(len);
        for _ in 0..nwords {
            let word = self.pop()?;
            bytes.extend(u32::to_ne_bytes(word));
        }
        assert!(bytes.len() >= len);
        bytes.truncate(len - 1);

        String::from_utf8(bytes).map_err(|_| ParserError::InvalidString)
    }

    pub fn fixed(&mut self) -> Result<f32, ParserError> {
        let v = self.int()?;
        Ok(v as f32 / 256.0)
    }

    /// Wayland array: a byte length, then the contents padded to a 32-bit boundary.
    pub fn array(&mut self) -> Result<Array, ParserError> {
        let len = self.uint()? as usize;
        let nwords = len.next_multiple_of(4) / 4;
        let mut words = Vec::with_capacity(nwords);
        for _ in 0..nwords {
            words.push(self.pop()?);
        }
        words.truncate(len / 4);
        Ok(Array(words))
    }

    pub fn fd(&mut self) -> Result<RawFd, ParserError> {
        // Implemented by `RawRequest`
        Err(ParserError::Malformed)
    }
}

#[cfg(test)]
mod clients_wire_tests {
    use std::{
        collections::VecDeque,
        os::fd::{FromRawFd, IntoRawFd, OwnedFd},
    };

    use super::{Arguments, ParserError, RawRequest};

    #[test]
    fn arguments_array_reads_padded_words() {
        let mut args = VecDeque::new();
        args.push_back(8);
        args.push_back(0x1111);
        args.push_back(0x2222);
        let mut args = Arguments::from_words(args);
        assert_eq!(args.array().unwrap().0, vec![0x1111, 0x2222]);

        let mut partial = VecDeque::new();
        partial.push_back(2);
        partial.push_back(u32::from_ne_bytes([1, 2, 0, 0]));
        let mut partial = Arguments::from_words(partial);
        assert!(partial.array().unwrap().0.is_empty());
        assert!(partial.uint().is_err());
    }

    fn arguments_string_rejects_invalid_utf8() {
        let mut args = VecDeque::new();
        args.push_back(2);
        args.push_back(u32::from_ne_bytes([0xFFu8, 0xFF, 0, 0]));
        let mut a = Arguments { args };
        assert_eq!(a.string(), Err(ParserError::InvalidString));
    }

    #[test]
    fn arguments_fixed_roundtrip() {
        let v = (1.5f32 * 256.0).round() as i32 as u32;
        let mut a = Arguments { args: VecDeque::from([v]) };
        assert!((a.fixed().unwrap() - 1.5f32).abs() < 0.002);
    }

    #[test]
    fn raw_request_try_from_rejects_short_buffer() {
        assert!(RawRequest::try_from([1u8, 0, 0, 0].as_slice()).is_err());
    }

    #[test]
    fn malformed_opcodes_fail_for_core_shell_buffer_and_extension() {
        use crate::{
            object::Parser,
            wayland_objects::{
                wl_buffer::WlBuffer, wl_registry::WlRegistry, wp_viewporter::WpViewporter,
                xdg_surface::XdgSurface,
            },
        };

        let request = |opcode| RawRequest {
            object_id: 1,
            request_id: opcode,
            arguments: Arguments { args: VecDeque::new() },
            fds: None,
        };
        assert!(WlRegistry::parse_request(request(50)).is_err());
        assert!(XdgSurface::parse_request(request(50)).is_err());
        assert!(WlBuffer::parse_request(request(50)).is_err());
        assert!(WpViewporter::parse_request(request(50)).is_err());
    }

    #[test]
    fn a_malformed_request_disconnects_only_that_client() {
        use crate::{runner::Dispatcher, wayland_objects::wl_display::WlDisplay};

        use super::{Client, ClientId, ClientManagerAction};

        let mut client = Client::new_for_tests(ClientId::from_raw(7));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let request = RawRequest {
            object_id: 5,
            request_id: 50,
            arguments: Arguments { args: VecDeque::new() },
            fds: None,
        };
        assert!(matches!(
            client.parse::<WlDisplay>(request, &mut client_dispatcher),
            Err(crate::object::AbortRequest::Malformed)
        ));
        assert!(dispatcher.test_has_queued_action(|action: &ClientManagerAction| {
            matches!(action, ClientManagerAction::Close { id } if *id == ClientId::from_raw(7))
        }));
    }

    #[test]
    fn raw_request_fd_consumes_owned_fd_queue() {
        let raw = std::fs::File::open("/dev/null").unwrap().into_raw_fd();
        let mut fds = VecDeque::from([unsafe { OwnedFd::from_raw_fd(raw) }]);
        let mut request = RawRequest {
            object_id: 1,
            request_id: 2,
            arguments: Arguments { args: VecDeque::new() },
            fds: Some(&mut fds),
        };

        let returned = request.fd().unwrap();
        let _returned = unsafe { OwnedFd::from_raw_fd(returned) };

        assert!(fds.is_empty());
    }
}

#[cfg(test)]
mod surface_output_refresh_tests {
    use crate::{
        clients::{Client, ClientId, Resolved},
        io_uring::model::IoUringAction,
        runner::Dispatcher,
        state::{BackendOutputRef, DrmId, FractionalScale, ObjectId},
        test_support::{state_for_tests, test_output_drm1},
        wayland_objects::{wl_surface::WlSurface, wp_fractional_scale_v1::WpFractionalScaleV1},
    };

    #[test]
    fn refresh_surface_outputs_resends_fractional_scale_when_output_scale_changes() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(9));
        let output_id = state.insert_output(test_output_drm1(0, 0, 200, 120));
        let surface_id = crate::protocol_generated::WlSurfaceId::from(10u32);
        let fractional_id = crate::protocol_generated::WpFractionalScaleV1Id::from(11u32);
        let (surface_resolved, _) =
            client.insert_object(&mut state, WlSurface::new(surface_id, client.client_id(), 6));
        let surface_object_id = surface_resolved.object_id();
        state.register_root_surface(surface_resolved);

        let fractional_object_id = state
            .insert_object(WpFractionalScaleV1::new(
                fractional_id,
                Resolved::new(client.client_id(), surface_id, surface_object_id),
            ))
            .1;
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            surface
                .set_fractional_scale_object(Resolved::new(
                    client.client_id(),
                    fractional_id,
                    fractional_object_id,
                ))
                .unwrap();
            surface.test_map(ObjectId::from(999usize), 0, 0, 200, 120);
        }

        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            client.refresh_surface_outputs(&mut state, &mut client_dispatcher);
        }
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));

        state.outputs.try_get_mut(output_id).unwrap().scale = FractionalScale::ONE_POINT_FIVE;
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            client.refresh_surface_outputs(&mut state, &mut client_dispatcher);
        }
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));

        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            client.refresh_surface_outputs(&mut state, &mut client_dispatcher);
        }
        assert!(!dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));
    }

    #[test]
    fn protocol_visibility_skips_disabled_outputs_without_layout_bounds() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let enabled = state.insert_output(crate::test_support::test_output_with_crtc_name(
            backend, 1, 0, 0, 100, 100,
        ));
        let disabled = state.insert_output(crate::test_support::test_output_with_crtc_name(
            backend, 2, 100, 0, 100, 100,
        ));
        state.outputs.try_get_mut(disabled).unwrap().enabled = false;

        let surface_id = crate::protocol_generated::WlSurfaceId::from(12u32);
        let (surface_resolved, _) = Client::new_for_tests(ClientId::from_raw(9))
            .insert_object(&mut state, WlSurface::new(surface_id, ClientId::from_raw(9), 6));
        state.register_root_surface(surface_resolved);
        state
            .objects
            .get_mut(surface_resolved.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(998usize), 10, 10, 20, 20);

        assert_eq!(state.output_bounds(enabled).is_some(), true);
        assert!(crate::scene::visibility::root_has_protocol_visibility(&state, surface_resolved));
    }
}

#[cfg(test)]
mod socket_shutdown_tests {
    use std::{io::Read, os::unix::net::UnixStream};

    use {
        super::*,
        crate::{
            runner::{Runner, ShutdownReason},
            test_support::state_for_tests,
        },
    };

    fn temp_socket() -> (mktemp::Temp, PathBuf) {
        let dir = mktemp::Temp::new_dir().unwrap();
        let path = dir.to_path_buf().join("wayland-test");
        (dir, path)
    }

    fn socket_pair() -> (RawFd, UnixStream) {
        let (server, client) = UnixStream::pair().unwrap();
        (server.into_raw_fd(), client)
    }

    #[test]
    fn stop_closes_every_client_and_cancels_accept() {
        let (_dir, path) = temp_socket();
        let mut manager = ClientManager::bind(&path).unwrap();
        let mut state = state_for_tests();
        let mut dispatcher = Dispatcher::default();
        manager
            .process(&mut state, ClientManagerAction::Start, &mut dispatcher)
            .unwrap();
        let (first, _first_peer) = socket_pair();
        let (second, _second_peer) = socket_pair();
        manager
            .process(
                &mut state,
                ClientManagerAction::NewClient { new_client: first },
                &mut dispatcher,
            )
            .unwrap();
        manager
            .process(
                &mut state,
                ClientManagerAction::NewClient { new_client: second },
                &mut dispatcher,
            )
            .unwrap();
        manager
            .process(&mut state, ClientManagerAction::Stop, &mut dispatcher)
            .unwrap();

        assert!(dispatcher.test_stop_queued(ClientId(0)));
        assert!(dispatcher.test_stop_queued(ClientId(1)));
        assert_eq!(
            dispatcher.test_count_queued_action(|action: &IoUringAction| {
                matches!(action, IoUringAction::Cancel { .. })
            }),
            1
        );
        assert!(!path.exists());
    }

    #[test]
    fn repeated_stop_cancels_accept_once() {
        let (_dir, path) = temp_socket();
        let mut manager = ClientManager::bind(&path).unwrap();
        let mut state = state_for_tests();
        let mut dispatcher = Dispatcher::default();
        manager
            .process(&mut state, ClientManagerAction::Start, &mut dispatcher)
            .unwrap();
        manager
            .process(&mut state, ClientManagerAction::Stop, &mut dispatcher)
            .unwrap();
        manager
            .process(&mut state, ClientManagerAction::Stop, &mut dispatcher)
            .unwrap();
        assert_eq!(
            dispatcher.test_count_queued_action(|action: &IoUringAction| {
                matches!(action, IoUringAction::Cancel { .. })
            }),
            1
        );
    }

    #[test]
    fn shutdown_does_not_accept_a_new_client() {
        let (_dir, path) = temp_socket();
        let mut manager = ClientManager::bind(&path).unwrap();
        let mut state = state_for_tests();
        let mut dispatcher = Dispatcher::default();
        manager
            .process(&mut state, ClientManagerAction::Start, &mut dispatcher)
            .unwrap();
        manager
            .process(&mut state, ClientManagerAction::Stop, &mut dispatcher)
            .unwrap();
        let accepts = dispatcher.test_count_queued_action(|action: &IoUringAction| {
            matches!(action, IoUringAction::Accept { .. })
        });

        let (server, mut peer) = socket_pair();
        peer.set_nonblocking(true).unwrap();
        manager
            .process(
                &mut state,
                ClientManagerAction::NewClient { new_client: server },
                &mut dispatcher,
            )
            .unwrap();
        manager
            .process(&mut state, ClientManagerAction::RearmAccept, &mut dispatcher)
            .unwrap();

        let mut buffer = [0];
        assert_eq!(peer.read(&mut buffer).unwrap(), 0);
        assert!(!dispatcher.test_stop_queued(ClientId(0)));
        assert_eq!(
            dispatcher.test_count_queued_action(|action: &IoUringAction| {
                matches!(action, IoUringAction::Accept { .. })
            }),
            accepts
        );
    }

    #[test]
    fn stale_socket_is_replaced_and_drop_unlinks_it() {
        let (_dir, path) = temp_socket();
        std::fs::write(&path, b"stale").unwrap();
        let manager = ClientManager::bind(&path).unwrap();
        UnixStream::connect(&path).unwrap();
        drop(manager);
        assert!(!path.exists());

        std::fs::write(&path, b"stale").unwrap();
        let manager = ClientManager::bind(&path).unwrap();
        UnixStream::connect(&path).unwrap();
        drop(manager);
        assert!(!path.exists());
    }

    #[test]
    fn shutdown_closes_clients_and_unlinks_before_returning() {
        let (_dir, path) = temp_socket();
        let manager = ClientManager::bind(&path).unwrap();
        let mut runner = Runner::with_state(state_for_tests());
        runner.add_model(Box::new(manager));
        let (server, _peer) = socket_pair();
        let mut dispatcher = Dispatcher::default();
        dispatcher.dispatch(ClientManagerAction::NewClient { new_client: server });
        dispatcher.request_shutdown(ShutdownReason::Quit);
        let outcome = runner.run(dispatcher);

        assert_eq!(outcome.code, 0);
        assert!(!path.exists());
        assert!(!runner.test_has_model::<ClientManager>());
        assert!(!runner.test_has_instance(ClientId(0)));
    }

    #[test]
    fn quit_unlinks_the_socket_and_the_next_session_can_bind_it() {
        let (_dir, path) = temp_socket();
        let manager = ClientManager::bind(&path).unwrap();
        let state = state_for_tests();
        let watcher = crate::config::ConfigWatcher::try_new(&state.config).unwrap();
        let mut runner = Runner::with_state(state);
        runner.add_model(Box::new(manager));
        runner.add_model(Box::new(watcher));
        runner.add_model(Box::new(crate::drm::VisualAnimationPump::new()));
        let mut dispatcher = Dispatcher::default();
        dispatcher.request_shutdown(ShutdownReason::Quit);
        dispatcher
            .request_shutdown(ShutdownReason::Fatal { source: "late", message: "ignored".into() });
        let outcome = runner.run(dispatcher);

        assert_eq!(outcome.code, 0);
        assert_eq!(outcome.reason, ShutdownReason::Quit);
        assert_eq!(outcome.diagnostics, vec!["late: ignored".to_string()]);
        assert!(!path.exists());
        assert!(!runner.test_has_model::<ClientManager>());
        assert!(!runner.test_has_model::<crate::config::ConfigWatcher>());
        assert!(!runner.test_has_model::<crate::drm::VisualAnimationPump>());

        let manager = ClientManager::bind(&path).unwrap();
        UnixStream::connect(&path).unwrap();
        drop(manager);
        assert!(!path.exists());
    }

    #[test]
    fn a_fatal_model_still_unlinks_the_socket() {
        let (_dir, path) = temp_socket();
        let manager = ClientManager::bind(&path).unwrap();
        let mut runner = Runner::with_state(state_for_tests());
        runner.add_model(Box::new(manager));
        runner.add_model(Box::new(Boom));
        let mut dispatcher = Dispatcher::default();
        dispatcher.dispatch(BoomAction::Fail);
        let outcome = runner.run(dispatcher);

        assert_eq!(outcome.code, 1);
        assert!(!path.exists());
        assert!(!runner.test_has_model::<ClientManager>());
    }

    #[test]
    fn an_unsupported_request_does_not_fail_the_session() {
        use std::collections::VecDeque;

        let mut client = Client::new_for_tests(ClientId::from_raw(7));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let request = RawRequest {
            object_id: 1,
            request_id: 50,
            arguments: Arguments { args: VecDeque::new() },
            fds: None,
        };
        assert!(matches!(
            client.parse::<WlDisplay>(request, &mut client_dispatcher),
            Err(AbortRequest::Malformed)
        ));
        assert!(!dispatcher.shutdown_requested());
        assert!(dispatcher.test_has_queued_action(|action: &ClientManagerAction| {
            matches!(action, ClientManagerAction::Close { id } if *id == ClientId::from_raw(7))
        }));
        dispatcher.request_shutdown(ShutdownReason::Quit);
        let outcome = Runner::with_state(state_for_tests()).run(dispatcher);
        assert_eq!(outcome.code, 0);
    }

    #[test]
    fn bind_failure_is_an_error() {
        let (_dir, path) = temp_socket();
        let missing = path.join("nested").join("sock");
        match ClientManager::bind(&missing) {
            Err(error) => assert!(error.to_string().contains("bind Wayland socket")),
            Ok(_manager) => panic!("bind succeeded"),
        }
    }
}

#[cfg(test)]
struct Boom;

#[cfg(test)]
enum BoomAction {
    Fail,
    Stop,
}

#[cfg(test)]
impl Model for Boom {
    type Action = BoomAction;
    const STOP: Option<Self::Action> = Some(BoomAction::Stop);

    fn process(
        &mut self,
        _state: &mut State,
        action: Self::Action,
        _dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        match action {
            BoomAction::Fail => {
                Err(crate::runner::ModelFailure::Shutdown(crate::runner::ShutdownReason::Fatal {
                    source: "test",
                    message: "injected".into(),
                }))
            }
            BoomAction::Stop => Ok(()),
        }
    }
}
