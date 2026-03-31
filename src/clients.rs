use std::{
    any::TypeId,
    collections::{BTreeMap, BTreeSet, VecDeque},
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
    drm::{schedule_active_output, schedule_output},
    io_uring::{
        model::IoUringAction,
        ring::{AcceptCompleted, RecvmsgCompleted, RingId},
    },
    object::{IdToObject, Object, Parser, ParserError},
    protocol_generated::{
        wl_display, Array, Constants, HyprlandCtmControlManagerV1Id, WaylandId, WlDataOfferId,
        WlDataSourceId, WlOutputId, WlSurfaceId, WpPresentationFeedbackId, XdgPopupId,
        XdgToplevelId, ZwlrGammaControlV1Id, ZwlrLayerSurfaceV1Id,
    },
    region::RectI,
    runner::{Dispatcher, Model, ModelInstanceId, Redispatcher},
    seat::SeatClientEvent,
    state::{DataOfferRole, DrmId, GrabbedObject, ObjectId, OutputId, State},
    sync::AcquireReadyWaiterId,
    udev::Udev,
    wayland_objects::{
        ext_workspace_manager_v1::{publish_workspace_state_changed, ExtWorkspaceManagerV1},
        hyprland_ctm_control_manager_v1::HyprlandCtmControlManagerV1,
        wl_data_offer::WlDataOffer,
        wl_data_source::WlDataSource,
        wl_display::WlDisplay,
        wl_keyboard::WlKeyboard,
        wl_output::WlOutput,
        wl_pointer::WlPointer,
        wl_registry::WlRegistry,
        wl_surface::WlSurface,
        wp_presentation_feedback::WpPresentationFeedback,
        xdg_popup::XdgPopup,
        xdg_surface::{xdg_toplevel::XdgToplevel, XdgSurface},
        zwlr_gamma_control_v1::ZwlrGammaControlV1,
        zwlr_layer_surface_v1::relayout_client_layer_surfaces,
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

pub struct ClientManager {
    current_id: u64,
    fd: UnixListener,
    set: BTreeSet<ClientId>,
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
    PublishWorkspaceStateChanged { affected_outputs: Vec<OutputId> },
    WorkspaceStateChanged,
    LayerShellLayoutChanged,
    Close { id: ClientId },
}

impl ClientManager {
    pub fn new() -> Box<Self> {
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let filename = PathBuf::from(runtime).join("my-wayland-0");
        if filename.exists() {
            let _ = std::fs::remove_file(&filename);
        }
        let fd = UnixListener::bind(&filename).unwrap_or_else(|e| {
            panic!("bind Wayland socket {:?}: {}", filename, e);
        });
        log::info!("Listening on Wayland socket {:?}", filename);
        let current_id = 0;
        let set = BTreeSet::default();

        Box::new(Self { current_id, fd, set })
    }

    fn next_id(&mut self) -> ClientId {
        let id = self.current_id;
        self.current_id += 1;
        ClientId(id)
    }

    fn dispatch_accept(&self, dispatcher: &mut Dispatcher) {
        dispatcher.dispatch(IoUringAction::Accept {
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
}

impl Model for ClientManager {
    type Action = ClientManagerAction;

    const START: Option<Self::Action> = Some(ClientManagerAction::Start);

    fn process(
        &mut self,
        state: &mut State,
        action: ClientManagerAction,
        dispatcher: &mut Dispatcher,
    ) -> Result<(), ()> {
        use ClientManagerAction::*;

        match action {
            Start => {
                self.dispatch_accept(dispatcher);
                dispatcher.start::<Dbus>();
                dispatcher.start::<Udev>();
            }
            RearmAccept => {
                self.dispatch_accept(dispatcher);
            }
            NewClient { new_client } => {
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
            PublishWorkspaceStateChanged { affected_outputs } => {
                publish_workspace_state_changed(state, dispatcher, affected_outputs);
            }
            WorkspaceStateChanged => {
                for client_id in self.set.iter().copied() {
                    dispatcher.dispatch_to(client_id, ClientAction::WorkspaceStateChanged);
                }
            }
            LayerShellLayoutChanged => {
                for client_id in self.set.iter().copied() {
                    dispatcher.dispatch_to(client_id, ClientAction::RefreshLayerSurfaces);
                }
            }
            Close { id } => {
                log::trace!("client {:?} closed", id);
                if self.set.remove(&id) {
                    dispatcher.stop_model(id);
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ProcessInfo {
    command: Rc<str>,
    cred: libc::ucred,
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
    RefreshDataDeviceSelection,
    DataDeviceDragEnter {
        serial: u32,
        surface: WlSurfaceId,
        x: f32,
        y: f32,
        source_object_id: Option<ObjectId>,
    },
    DataDeviceDragLeave,
    DataDeviceDragMotion {
        time: u32,
        x: f32,
        y: f32,
    },
    DataDeviceDragDrop,
    DataSourceEvent {
        source_id: WlDataSourceId,
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
    CompleteSurfaceFrameCallbacks {
        surface_ids: Vec<WlSurfaceId>,
        time_msec: u32,
    },
    PresentationFeedbackPresented {
        feedback_ids: Vec<WpPresentationFeedbackId>,
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
        feedback_ids: Vec<WpPresentationFeedbackId>,
    },
    DiscardSurfacePresentationFeedbacksForBackend {
        drm_id: DrmId,
    },
    CancelBlockedSurface {
        surface_object_id: ObjectId,
        waiter_id: AcquireReadyWaiterId,
    },
    DismissPopupGrab {
        popup_ids: Vec<XdgPopupId>,
    },
    InteractiveResizeConfigure {
        xdg_toplevel_id: XdgToplevelId,
        width: i32,
        height: i32,
        resizing: bool,
    },
    Close,
}

#[derive(Debug)]
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
        let comm = match std::fs::read(format!("/proc/{}/comm", cred.pid)) {
            Ok(comm) => comm,
            Err(e) => {
                log::error!("Unable to read `comm` of new client: {:?}", e);
                return;
            }
        };
        let command = String::from_utf8_lossy(&comm);
        let command = Rc::from(command.trim());

        self.info = Some(Rc::new(ProcessInfo { command, cred }))
    }

    pub fn ref_object_mut<'a, K>(&self, id: K, state: &'a mut State) -> &'a mut K::Object
    where
        K: Into<WaylandId> + IdToObject,
        K::Object: Object,
    {
        let id: WaylandId = id.into();
        let obj_id = self.objects[&id].object_id;
        state.objects[obj_id].downcast_mut().unwrap()
    }

    pub fn ref_object<'a, K>(&self, id: K, state: &'a State) -> &'a K::Object
    where
        K: Into<WaylandId> + IdToObject,
        K::Object: Object,
    {
        let id: WaylandId = id.into();
        let obj_id = self.objects[&id].object_id;
        state.objects[obj_id].downcast_ref().unwrap()
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

    /// Return whether a client-visible Wayland id currently refers to a live
    /// object of the expected interface type `K::Object`.
    ///
    /// This is intentionally stricter than `contains_object()`: it checks both
    /// liveness and the concrete object type, which lets protocol handlers reject
    /// malformed ids with a protocol error instead of panicking in `get_object()`.
    pub fn object_is<K>(&self, id: K, state: &State) -> bool
    where
        K: Into<WaylandId> + IdToObject,
        K::Object: Object,
    {
        let id: WaylandId = id.into();
        let Some(entry) = self.objects.get(&id) else {
            return false;
        };
        entry.type_id == TypeId::of::<K::Object>() && state.objects.contains(entry.object_id)
    }

    pub fn grab_object<K>(&self, id: K, state: &mut State) -> GrabbedObject<K::Object>
    where
        K: Into<WaylandId> + IdToObject,
        K::Object: Object,
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

    pub fn insert_object<'a, O: Object>(
        &mut self,
        state: &'a mut State,
        object: O,
    ) -> ((WaylandId, ObjectId), &'a mut O) {
        let (wayland_id, object_id) = state.insert_object(object);
        self.objects
            .insert(wayland_id, ClientObjectEntry { type_id: TypeId::of::<O>(), object_id });
        let object: &mut O = state.objects[object_id].downcast_mut::<O>().unwrap();
        ((wayland_id, object_id), object)
    }

    pub fn contains_object<K>(&self, id: K, state: &State) -> bool
    where
        K: Into<WaylandId> + IdToObject,
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
        popup_ids: Vec<XdgPopupId>,
        detached_parent_layer_surface_id: Option<ZwlrLayerSurfaceV1Id>,
    ) {
        for popup_id in popup_ids.into_iter().rev() {
            if !self.contains_object(popup_id, state) {
                continue;
            }

            crate::seat::remove_popup_grab(state, dispatcher, popup_id, None);

            let (xdg_surface_id, parent_surface_id, parent_layer_surface_id) = {
                let popup_object_id = self.object_id(popup_id);
                let Some(popup) = state.objects.try_ref_object::<XdgPopup>(popup_object_id) else {
                    continue;
                };
                (popup.xdg_surface_id(), popup.parent_surface_id(), popup.parent_layer_surface_id())
            };

            if let Some(parent_surface_id) = parent_surface_id {
                if self.contains_object(parent_surface_id, state) {
                    let mut parent_surface = self.grab_object(parent_surface_id, state);
                    parent_surface.remove_popup_child(popup_id);
                }
            }
            if let Some(parent_layer_surface_id) = parent_layer_surface_id {
                if Some(parent_layer_surface_id) != detached_parent_layer_surface_id
                    && self.contains_object(parent_layer_surface_id, state)
                {
                    let mut parent_surface = self.grab_object(parent_layer_surface_id, state);
                    parent_surface.remove_popup_child(popup_id);
                }
            }

            if self.contains_object(xdg_surface_id, state) {
                let wl_surface_id = {
                    let xdg_surface_object_id = self.object_id(xdg_surface_id);
                    let Some(xdg_surface) = state
                        .objects
                        .try_ref_object::<XdgSurface>(xdg_surface_object_id)
                    else {
                        continue;
                    };
                    xdg_surface.wl_surface_id()
                };
                if self.contains_object(wl_surface_id, state) {
                    let mut wl_surface = self.grab_object(wl_surface_id, state);
                    wl_surface.force_unmap_for_role_dismissal(self, state, dispatcher);
                }
            }

            {
                let mut popup = self.grab_object(popup_id, state);
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
        log::debug!("client {:?} seat event {:?}", self.id, event);
        match event {
            SeatClientEvent::PointerEnter { serial, surface, x, y } => {
                let pointers = state.seat.pointer_resources(self.id);
                for pointer_id in pointers {
                    if !self.contains_object(pointer_id, state) {
                        continue;
                    }
                    state.seat.record_client_serial(self.id, serial);
                    let pointer: &mut WlPointer = self.ref_object_mut(pointer_id, state);
                    pointer.send_enter(serial, surface, x, y, dispatcher);
                }
            }
            SeatClientEvent::PointerLeave { serial, surface } => {
                let pointers = state.seat.pointer_resources(self.id);
                for pointer_id in pointers {
                    if !self.contains_object(pointer_id, state) {
                        continue;
                    }
                    state.seat.record_client_serial(self.id, serial);
                    let pointer: &mut WlPointer = self.ref_object_mut(pointer_id, state);
                    pointer.send_leave(serial, surface, dispatcher);
                }
            }
            SeatClientEvent::PointerMotion { time, x, y } => {
                let pointers = state.seat.pointer_resources(self.id);
                for pointer_id in pointers {
                    if !self.contains_object(pointer_id, state) {
                        continue;
                    }
                    let pointer: &mut WlPointer = self.ref_object_mut(pointer_id, state);
                    pointer.send_motion(time, x, y, dispatcher);
                }
            }
            SeatClientEvent::PointerButton { serial, time, button, state: button_state } => {
                let pointers = state.seat.pointer_resources(self.id);
                for pointer_id in pointers {
                    if !self.contains_object(pointer_id, state) {
                        continue;
                    }
                    state.seat.record_client_serial(self.id, serial);
                    let pointer: &mut WlPointer = self.ref_object_mut(pointer_id, state);
                    pointer.send_button(serial, time, button, button_state, dispatcher);
                }
            }
            SeatClientEvent::PointerFrame => {
                let pointers = state.seat.pointer_resources(self.id);
                for pointer_id in pointers {
                    if !self.contains_object(pointer_id, state) {
                        continue;
                    }
                    let pointer: &mut WlPointer = self.ref_object_mut(pointer_id, state);
                    pointer.send_frame(dispatcher);
                }
            }
            SeatClientEvent::KeyboardEnter { serial, surface, keys } => {
                let keyboards = state.seat.keyboard_resources(self.id);
                for keyboard_id in keyboards {
                    if !self.contains_object(keyboard_id, state) {
                        continue;
                    }
                    state.seat.record_client_serial(self.id, serial);
                    let keyboard: &mut WlKeyboard = self.ref_object_mut(keyboard_id, state);
                    keyboard.send_enter(serial, surface, keys.clone(), dispatcher);
                }
            }
            SeatClientEvent::KeyboardLeave { serial, surface } => {
                let keyboards = state.seat.keyboard_resources(self.id);
                for keyboard_id in keyboards {
                    if !self.contains_object(keyboard_id, state) {
                        continue;
                    }
                    state.seat.record_client_serial(self.id, serial);
                    let keyboard: &mut WlKeyboard = self.ref_object_mut(keyboard_id, state);
                    keyboard.send_leave(serial, surface, dispatcher);
                }
            }
            SeatClientEvent::KeyboardKey { serial, time, key, state: key_state } => {
                let keyboards = state.seat.keyboard_resources(self.id);
                for keyboard_id in keyboards {
                    if !self.contains_object(keyboard_id, state) {
                        continue;
                    }
                    state.seat.record_client_serial(self.id, serial);
                    let keyboard: &mut WlKeyboard = self.ref_object_mut(keyboard_id, state);
                    keyboard.send_key(serial, time, key, key_state, dispatcher);
                }
            }
            SeatClientEvent::KeyboardModifiers {
                serial,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                let keyboards = state.seat.keyboard_resources(self.id);
                for keyboard_id in keyboards {
                    if !self.contains_object(keyboard_id, state) {
                        continue;
                    }
                    state.seat.record_client_serial(self.id, serial);
                    let keyboard: &mut WlKeyboard = self.ref_object_mut(keyboard_id, state);
                    keyboard.send_modifiers(
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
        offer_id: WlDataOfferId,
    ) {
        if !self.contains_object(offer_id, state) {
            return;
        }
        let Some(object_id) = self.object_id_for_wayland(offer_id) else {
            return;
        };
        if let Some(mut offer) = state.objects.try_grab_object::<WlDataOffer>(object_id) {
            offer.before_destroy(self.id, state, dispatcher);
        }
        self.delete_id(&WlDisplay::default(), state, dispatcher, offer_id.into());
    }

    fn create_data_offer_for_device(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        device_id: crate::protocol_generated::WlDataDeviceId,
        source_object_id: ObjectId,
        role: DataOfferRole,
    ) -> Option<WlDataOfferId> {
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
        let (source_id, source_client_id, mime_types, source_actions) = {
            let source = state
                .objects
                .try_ref_object::<WlDataSource>(source_object_id)?;
            (
                source.wl_data_source_id(),
                source.owner_client_id(),
                source.mime_types().to_vec(),
                source.effective_dnd_actions(),
            )
        };
        let chosen_action = if role == DataOfferRole::Drag
            && device_version < crate::protocol_generated::wl_data_offer::SOURCE_ACTIONS_SINCE
        {
            if previous_chosen_action != 0 {
                previous_chosen_action
            } else if source_actions
                & crate::protocol_generated::wl_data_device_manager::DndAction::Copy as u32
                != 0
            {
                crate::protocol_generated::wl_data_device_manager::DndAction::Copy as u32
            } else {
                0
            }
        } else {
            previous_chosen_action
        };

        let offer_id = self.allocate_server_object_id::<WlDataOfferId>();
        let offer = WlDataOffer::new(
            offer_id,
            device_version,
            role,
            Some(source_object_id),
            Some(source_id),
            Some(source_client_id),
        );
        let _ = self.insert_object(state, offer);
        {
            let device = self.ref_object(device_id, state);
            device.send_data_offer(offer_id, dispatcher);
        }
        for mime_type in mime_types {
            let offer = self.ref_object(offer_id, state);
            offer.send_offer(mime_type, dispatcher);
        }
        if role == DataOfferRole::Drag
            && device_version >= crate::protocol_generated::wl_data_offer::SOURCE_ACTIONS_SINCE
        {
            let offer = self.ref_object_mut(offer_id, state);
            offer.send_source_actions(source_actions, dispatcher);
            if chosen_action != 0 {
                offer.send_action(chosen_action, dispatcher);
            }
        }
        Some(offer_id)
    }

    fn refresh_data_device_selection(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let has_keyboard_focus = state
            .seat
            .keyboard_focus()
            .is_some_and(|focus| focus.client_id == self.id);
        let selection_source = has_keyboard_focus
            .then(|| state.seat.selection_source())
            .flatten();

        for data_device_id in state.seat.data_device_resources(self.id) {
            if !self.contains_object(data_device_id, state) {
                continue;
            }

            let previous_offer = {
                let data_device = self.ref_object_mut(data_device_id, state);
                data_device.take_selection_offer()
            };
            if let Some(previous_offer) = previous_offer {
                self.destroy_data_offer(state, dispatcher, previous_offer);
            }

            let next_offer = selection_source.and_then(|selection| {
                self.create_data_offer_for_device(
                    state,
                    dispatcher,
                    data_device_id,
                    selection.source_object_id,
                    DataOfferRole::Selection,
                )
            });

            let data_device = self.ref_object_mut(data_device_id, state);
            data_device.set_selection_offer(next_offer);
            data_device.send_selection(
                next_offer.unwrap_or_else(|| WlDataOfferId::from(0u32)),
                dispatcher,
            );
        }
    }

    fn handle_data_device_drag_enter(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        serial: u32,
        surface: WlSurfaceId,
        x: f32,
        y: f32,
        source_object_id: Option<ObjectId>,
    ) {
        let has_legacy_device = state
            .seat
            .data_device_resources(self.id)
            .into_iter()
            .filter(|data_device_id| self.contains_object(*data_device_id, state))
            .any(|data_device_id| {
                self.ref_object(data_device_id, state).version()
                    < crate::protocol_generated::wl_data_offer::SOURCE_ACTIONS_SINCE
            });
        if has_legacy_device
            && state
                .seat
                .active_drag()
                .is_some_and(|drag| drag.negotiation.chosen_action == 0)
        {
            let source_actions = source_object_id
                .and_then(|source_object_id| {
                    state
                        .objects
                        .try_ref_object::<WlDataSource>(source_object_id)
                        .map(|source| source.effective_dnd_actions())
                })
                .unwrap_or(0);
            if source_actions
                & crate::protocol_generated::wl_data_device_manager::DndAction::Copy as u32
                != 0
            {
                let _ = state.seat.set_active_drag_chosen_action(
                    self.id,
                    crate::protocol_generated::wl_data_device_manager::DndAction::Copy as u32,
                );
            }
        }
        for data_device_id in state.seat.data_device_resources(self.id) {
            if !self.contains_object(data_device_id, state) {
                continue;
            }
            let device_version = self.ref_object(data_device_id, state).version();

            let previous_offer = {
                let data_device = self.ref_object_mut(data_device_id, state);
                data_device.take_drag_offer()
            };
            if let Some(previous_offer) = previous_offer {
                self.destroy_data_offer(state, dispatcher, previous_offer);
            }

            let offer_id = source_object_id.and_then(|source_object_id| {
                self.create_data_offer_for_device(
                    state,
                    dispatcher,
                    data_device_id,
                    source_object_id,
                    DataOfferRole::Drag,
                )
            });
            if let Some(offer_id) = offer_id {
                let _ = state.seat.register_active_drag_offer(
                    self.id,
                    offer_id,
                    device_version >= crate::protocol_generated::wl_data_offer::ACTION_SINCE,
                );
            }

            let data_device = self.ref_object_mut(data_device_id, state);
            data_device.set_drag_offer(offer_id);
            data_device.send_enter(
                serial,
                surface,
                x,
                y,
                offer_id.unwrap_or_else(|| WlDataOfferId::from(0u32)),
                dispatcher,
            );
        }
    }

    fn handle_data_device_drag_leave(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        for data_device_id in state.seat.data_device_resources(self.id) {
            if !self.contains_object(data_device_id, state) {
                continue;
            }
            let previous_offer = {
                let data_device = self.ref_object_mut(data_device_id, state);
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
        for data_device_id in state.seat.data_device_resources(self.id) {
            if !self.contains_object(data_device_id, state) {
                continue;
            }
            let data_device = self.ref_object(data_device_id, state);
            data_device.send_motion(time, x, y, dispatcher);
        }
    }

    fn handle_data_device_drag_drop(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        for data_device_id in state.seat.data_device_resources(self.id) {
            if !self.contains_object(data_device_id, state) {
                continue;
            }
            let data_device = self.ref_object(data_device_id, state);
            data_device.send_drop(dispatcher);
        }

        if let Some(drag) = state.seat.active_drag() {
            if drag
                .target
                .is_some_and(|target| target.client_id == self.id)
            {
                if let Some(source_id) = drag.source_id {
                    if drag.negotiation.chosen_action == 0 {
                        return;
                    }
                    dispatcher.dispatch_to(
                        drag.source_client_id,
                        ClientAction::DataSourceEvent {
                            source_id,
                            event: DataSourceClientEvent::Action {
                                dnd_action: drag.negotiation.chosen_action,
                            },
                        },
                    );
                }
            }
        }
    }

    fn handle_data_source_event(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        source_id: WlDataSourceId,
        event: DataSourceClientEvent,
    ) {
        if !self.contains_object(source_id, state) {
            if let DataSourceClientEvent::Send { fd, .. } = event {
                let _ = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
            }
            return;
        }
        let source = self.ref_object_mut(source_id, state);
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
        surface_ids: Vec<WlSurfaceId>,
        time_msec: u32,
    ) {
        let display = WlDisplay::default();
        for surface_id in surface_ids {
            let Some(object_id) = self.object_id_for_wayland(surface_id) else {
                continue;
            };
            let callbacks = {
                let Some(mut surface) = state.objects.try_grab_object::<WlSurface>(object_id)
                else {
                    continue;
                };
                surface.take_frame_callbacks()
            };
            for callback_id in callbacks {
                if !self.contains_object(callback_id, state) {
                    continue;
                }
                {
                    let callback = self.ref_object(callback_id, state);
                    callback.done(time_msec, dispatcher);
                }
                self.delete_id(&display, state, dispatcher, callback_id.into());
            }
        }
    }

    fn output_resources_for_feedback(&self, state: &State, output_id: OutputId) -> Vec<WaylandId> {
        let mut resources = Vec::new();
        for object_id in self.iter_object_ids_of::<WlOutput>(state) {
            let Some(wl_output) = state.objects.try_ref_object::<WlOutput>(object_id) else {
                continue;
            };
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
        feedback_ids: Vec<WpPresentationFeedbackId>,
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
        for feedback_id in feedback_ids {
            if !self.contains_object(feedback_id, state) {
                continue;
            }
            {
                let feedback: &mut WpPresentationFeedback = self.ref_object_mut(feedback_id, state);
                if let Some(sync_outputs) = sync_outputs.as_ref() {
                    for output in sync_outputs {
                        feedback.send_sync_output((*output).into(), dispatcher);
                    }
                }
                feedback.send_presented(
                    tv_sec_hi, tv_sec_lo, tv_nsec, refresh, seq_hi, seq_lo, flags, dispatcher,
                );
            }
            self.delete_id(&display, state, dispatcher, feedback_id.into());
        }
    }

    fn discard_presentation_feedback(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        feedback_ids: Vec<WpPresentationFeedbackId>,
    ) {
        let display = WlDisplay::default();
        for feedback_id in feedback_ids {
            if !self.contains_object(feedback_id, state) {
                continue;
            }
            {
                let feedback = self.ref_object(feedback_id, state);
                feedback.send_discarded(dispatcher);
            }
            self.delete_id(&display, state, dispatcher, feedback_id.into());
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
            .objects
            .iter()
            .filter_map(|(wayland_id, entry)| {
                let surface = state.objects.try_ref_object::<WlSurface>(entry.object_id)?;
                Some(
                    (surface.client_id() == self.id)
                        .then_some(WlSurfaceId::from(u32::from(*wayland_id)))?,
                )
            })
            .collect::<Vec<_>>();
        for surface_id in surface_ids {
            if !self.contains_object(surface_id, state) {
                continue;
            }
            let mut surface = self.grab_object(surface_id, state);
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
        log::trace!(target: &s, "[{}] {}::{:?}", u32::from(writer.object_id()), E::INTERFACE_NAME, event);
    }

    /// Helper that just log the request
    pub fn parse<R: Parser>(&self, req: RawRequest) -> Result<R::Request, ParserError>
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

    fn handle_registry_global_added(
        &self,
        state: &State,
        dispatcher: &mut ClientDispatcher,
        name: u32,
        interface: String,
        version: u32,
    ) {
        for object_id in self.iter_object_ids_of::<WlRegistry>(state) {
            let Some(registry) = state.objects.try_ref_object::<WlRegistry>(object_id) else {
                continue;
            };
            registry.send_global(dispatcher, name, interface.clone(), version);
        }
    }

    fn handle_registry_global_removed(
        &self,
        state: &State,
        dispatcher: &mut ClientDispatcher,
        name: u32,
    ) {
        for object_id in self.iter_object_ids_of::<WlRegistry>(state) {
            let Some(registry) = state.objects.try_ref_object::<WlRegistry>(object_id) else {
                continue;
            };
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
            .iter_object_ids_of::<ZxdgOutputV1>(state)
            .collect::<Vec<_>>();
        let wl_output_ids = self
            .iter_object_ids_of::<WlOutput>(state)
            .collect::<Vec<_>>();

        for object_id in xdg_output_ids {
            let Some(xdg_output) = state.objects.try_ref_object::<ZxdgOutputV1>(object_id) else {
                continue;
            };
            if xdg_output.matches_output(output_id) {
                xdg_output.send_state(state, dispatcher);
            }
        }

        for object_id in wl_output_ids {
            let Some(wl_output) = state.objects.try_ref_object::<WlOutput>(object_id) else {
                continue;
            };
            if wl_output.matches_output(output_id) {
                wl_output.send_state(state, dispatcher);
            }
        }
    }

    pub(crate) fn output_object_ids_for_output(
        &self,
        state: &State,
        output_id: OutputId,
    ) -> Vec<WlOutputId> {
        let mut output_object_ids = Vec::new();
        let object_ids = self
            .iter_object_ids_of::<WlOutput>(state)
            .collect::<Vec<_>>();
        for object_id in object_ids {
            let Some(wl_output) = state.objects.try_ref_object::<WlOutput>(object_id) else {
                continue;
            };
            if wl_output.matches_output(output_id) {
                output_object_ids.push(wl_output.wl_output_id());
            }
        }
        output_object_ids
    }

    fn send_surface_output_membership_event(
        &self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        surface_object_id: ObjectId,
        output_id: OutputId,
        entered: bool,
    ) {
        let output_object_ids = self.output_object_ids_for_output(state, output_id);
        log::debug!(
            "surface output event surface={:?} output={:?} entered={} wl_output_resources={:?}",
            surface_object_id,
            output_id,
            entered,
            output_object_ids
        );
        if output_object_ids.is_empty() {
            return;
        }
        let Some(surface) = state.objects.try_ref_object::<WlSurface>(surface_object_id) else {
            return;
        };
        for output_object_id in output_object_ids {
            if entered {
                surface.send_output_enter(output_object_id, dispatcher);
            } else {
                surface.send_output_leave(output_object_id, dispatcher);
            }
        }
    }

    pub(crate) fn refresh_workspace_protocols(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let manager_object_ids = self
            .iter_object_ids_of::<ExtWorkspaceManagerV1>(state)
            .collect::<Vec<_>>();
        for object_id in manager_object_ids {
            let Some(mut manager) = state
                .objects
                .try_grab_object::<ExtWorkspaceManagerV1>(object_id)
            else {
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
            .iter_object_ids_of::<WlSurface>(state)
            .collect::<Vec<_>>();
        let mut surface_object_ids = Vec::new();
        for object_id in object_ids {
            let Some(surface) = state.objects.try_ref_object::<WlSurface>(object_id) else {
                continue;
            };
            if surface.contains_output(output_id) {
                surface_object_ids.push(object_id);
            }
        }
        for surface_object_id in surface_object_ids {
            self.send_surface_output_membership_event(
                state,
                dispatcher,
                surface_object_id,
                output_id,
                true,
            );
        }
    }

    pub(crate) fn refresh_surface_outputs(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let root_surface_ids = state.root_surface_ids();
        let active_cursor_surface = active_cursor_surface_refresh_params(state, self.id)
            .map(|(object_id, base_x, base_y)| (object_id, base_x, base_y));
        let active_dnd_icon_surface = active_dnd_icon_surface_refresh_params(state, self.id)
            .map(|(object_id, base_x, base_y)| (object_id, base_x, base_y));
        log::debug!("refresh_surface_outputs client={:?} roots={:?}", self.id, root_surface_ids);
        for surface_object_id in root_surface_ids {
            let Some(surface) = state.objects.try_ref_object::<WlSurface>(surface_object_id) else {
                continue;
            };
            log::debug!(
                "refresh_surface_outputs root candidate object={:?} client={:?} is_root={}",
                surface_object_id,
                surface.client_id(),
                surface.is_root()
            );
            if surface.client_id() != self.id || !surface.is_root() {
                continue;
            }
            if surface.has_cursor_role() {
                if let Some((active_surface_object_id, base_x, base_y)) = active_cursor_surface {
                    if active_surface_object_id == surface_object_id {
                        refresh_surface_tree_outputs(
                            self,
                            state,
                            dispatcher,
                            surface_object_id,
                            base_x,
                            base_y,
                        );
                    } else {
                        clear_surface_tree_outputs(self, state, dispatcher, surface_object_id);
                    }
                } else {
                    clear_surface_tree_outputs(self, state, dispatcher, surface_object_id);
                }
                continue;
            }
            if surface.has_dnd_icon_role() {
                if let Some((active_surface_object_id, base_x, base_y)) = active_dnd_icon_surface {
                    if active_surface_object_id == surface_object_id {
                        refresh_surface_tree_outputs(
                            self,
                            state,
                            dispatcher,
                            surface_object_id,
                            base_x,
                            base_y,
                        );
                    } else {
                        clear_surface_tree_outputs(self, state, dispatcher, surface_object_id);
                    }
                } else {
                    clear_surface_tree_outputs(self, state, dispatcher, surface_object_id);
                }
                continue;
            }
            if !state.root_surface_is_visible(surface_object_id) {
                clear_surface_tree_outputs(self, state, dispatcher, surface_object_id);
                continue;
            }
            refresh_surface_tree_outputs(self, state, dispatcher, surface_object_id, 0, 0);
        }
    }

    fn reconfigure_mapped_toplevels(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        state.relayout_dirty_workspaces();
        let tiled_resize_surfaces = crate::seat::tiled_resize_active_root_surfaces(state);
        let object_ids = self
            .iter_object_ids_of::<XdgToplevel>(state)
            .collect::<Vec<_>>();
        for object_id in object_ids {
            let Some(toplevel_ref) = state.objects.try_ref_object::<XdgToplevel>(object_id) else {
                continue;
            };
            let xdg_surface_id = toplevel_ref.xdg_surface_id();
            let toplevel_id = toplevel_ref.toplevel_id();
            let Some(xdg_surface_object_id) = self.object_id_for_wayland(xdg_surface_id) else {
                continue;
            };
            let Some(xdg_surface_ref) = state
                .objects
                .try_ref_object::<XdgSurface>(xdg_surface_object_id)
            else {
                continue;
            };
            let wl_surface_id = xdg_surface_ref.wl_surface_id();
            let Some(wl_surface_object_id) = self.object_id_for_wayland(wl_surface_id) else {
                continue;
            };
            let Some(surface_ref) = state
                .objects
                .try_ref_object::<WlSurface>(wl_surface_object_id)
            else {
                continue;
            };
            if !state.root_surface_is_visible(wl_surface_object_id) {
                continue;
            }
            let tiled_rect = state.tiled_root_surface_frame_rect(wl_surface_object_id);
            let tiled_edges = state
                .tiled_root_surface_edges(wl_surface_object_id)
                .unwrap_or_default();
            let fullscreen = state.root_surface_is_fullscreen(wl_surface_object_id);
            let resizing = tiled_resize_surfaces.contains(&wl_surface_object_id);
            let configure_bounds = tiled_rect
                .map(|rect| (rect.width, rect.height))
                .or_else(|| surface_ref.configure_bounds_for_object(state, wl_surface_object_id));
            let pending_floating_restore = if tiled_rect.is_none() && !fullscreen {
                state.take_pending_floating_rect_restore(wl_surface_object_id)
            } else {
                None
            };
            let desired_decoration_mode = state
                .objects
                .try_ref_object::<XdgToplevel>(object_id)
                .map(|toplevel| {
                    state.xdg_decoration_mode_for_surface(
                        Some(wl_surface_object_id),
                        toplevel.has_decoration_object(),
                        toplevel.requested_decoration_mode(state),
                    )
                });

            let mut xdg_surface = self.grab_object(xdg_surface_id, state);
            let mut toplevel = self.grab_object(toplevel_id, state);
            if let Some(mode) = desired_decoration_mode {
                toplevel.schedule_decoration_mode(mode);
            }
            if let Some(rect) = tiled_rect {
                toplevel.send_layout_configure(
                    self,
                    state,
                    Some((rect.width, rect.height)),
                    &mut xdg_surface,
                    rect.width,
                    rect.height,
                    tiled_edges,
                    fullscreen,
                    resizing,
                    dispatcher,
                );
            } else if let Some(rect) = pending_floating_restore {
                toplevel.send_interactive_resize_configure(
                    self,
                    state,
                    configure_bounds,
                    &mut xdg_surface,
                    rect.width,
                    rect.height,
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
    surface_object_id: ObjectId,
    base_x: i32,
    base_y: i32,
) {
    let (stack, child_base, delta) = {
        let Some(mut surface) = state
            .objects
            .try_grab_object::<WlSurface>(surface_object_id)
        else {
            return;
        };
        let render_state = surface.render_state_at(base_x, base_y);
        let surface_origin = render_state
            .as_ref()
            .map(|render_state| (render_state.x, render_state.y))
            .unwrap_or_else(|| {
                let (x, y) = surface.root_position();
                (base_x + x, base_y + y)
            });
        let current_position = surface.current_subsurface_position();
        let next_outputs = render_state
            .as_ref()
            .map(|render_state| surface_intersecting_outputs(state, render_state))
            .unwrap_or_default();
        let child_base =
            (surface_origin.0 + current_position.0, surface_origin.1 + current_position.1);
        log::debug!(
            "surface output refresh surface={:?} render={:?} child_base={:?} next_outputs={:?}",
            surface_object_id,
            render_state.as_ref().map(|r| (r.x, r.y, r.width, r.height)),
            child_base,
            next_outputs
        );
        let delta = surface.update_output_membership(next_outputs);
        log::debug!(
            "surface output delta surface={:?} entered={:?} left={:?} active_changed={}",
            surface_object_id,
            delta.entered,
            delta.left,
            delta.active_output_changed
        );
        (surface.current_stack().to_vec(), child_base, delta)
    };

    for output_id in delta.left {
        client.send_surface_output_membership_event(
            state,
            dispatcher,
            surface_object_id,
            output_id,
            false,
        );
    }
    for output_id in delta.entered {
        client.send_surface_output_membership_event(
            state,
            dispatcher,
            surface_object_id,
            output_id,
            true,
        );
    }

    let preferred_output = state
        .objects
        .try_ref_object::<WlSurface>(surface_object_id)
        .and_then(|surface| surface.preferred_output_id_for_object(state, surface_object_id))
        .and_then(|output_id| {
            state
                .outputs
                .try_get(output_id)
                .map(|output| (output.scale, output.legacy_scale().max(1), output.transform as u32))
        });
    if let Some((fractional_scale, scale, transform)) = preferred_output {
        let Some(mut surface) = state
            .objects
            .try_grab_object::<WlSurface>(surface_object_id)
        else {
            return;
        };
        surface.send_preferred_fractional_scale_for_output(fractional_scale, state, dispatcher);
        surface.send_preferred_buffer_state_for_output(scale, transform, dispatcher);
    }

    for entry in stack {
        if let crate::wayland_objects::wl_surface::StackEntry::Child(child_surface_object_id) =
            entry
        {
            refresh_surface_tree_outputs(
                client,
                state,
                dispatcher,
                child_surface_object_id,
                child_base.0,
                child_base.1,
            );
        }
    }
}

fn clear_surface_tree_outputs(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    surface_object_id: ObjectId,
) {
    let (stack, delta) = {
        let Some(mut surface) = state
            .objects
            .try_grab_object::<WlSurface>(surface_object_id)
        else {
            return;
        };
        let delta = surface.update_output_membership(Vec::new());
        (surface.current_stack().to_vec(), delta)
    };

    for output_id in delta.left {
        client.send_surface_output_membership_event(
            state,
            dispatcher,
            surface_object_id,
            output_id,
            false,
        );
    }

    for entry in stack {
        if let crate::wayland_objects::wl_surface::StackEntry::Child(child_surface_object_id) =
            entry
        {
            clear_surface_tree_outputs(client, state, dispatcher, child_surface_object_id);
        }
    }
}

fn active_cursor_surface_refresh_params(
    state: &State,
    client_id: ClientId,
) -> Option<(ObjectId, i32, i32)> {
    let (pointer_x, pointer_y) = state.seat.pointer_position();
    let crate::cursor::CursorSourceRef::Surface { owner_client_id, cursor } =
        state.seat.visible_cursor_source()?
    else {
        return None;
    };
    if owner_client_id != client_id {
        return None;
    }
    let surface = state
        .objects
        .try_ref_object::<WlSurface>(cursor.object_id)?;
    let (root_x, root_y) = surface.root_position();
    let base_x = pointer_x.round() as i32 - cursor.hotspot_x - root_x;
    let base_y = pointer_y.round() as i32 - cursor.hotspot_y - root_y;
    Some((cursor.object_id, base_x, base_y))
}

fn active_dnd_icon_surface_refresh_params(
    state: &State,
    client_id: ClientId,
) -> Option<(ObjectId, i32, i32)> {
    let drag = state.seat.active_drag()?;
    if drag.source_client_id != client_id {
        return None;
    }
    let surface_object_id = drag.icon_surface_object_id?;
    let surface = state
        .objects
        .try_ref_object::<WlSurface>(surface_object_id)?;
    let (pointer_x, pointer_y) = state.seat.pointer_position();
    let (root_x, root_y) = surface.root_position();
    Some((surface_object_id, pointer_x.round() as i32 - root_x, pointer_y.round() as i32 - root_y))
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
    command: Option<Rc<ProcessInfo>>,
}

impl<'a> ClientDispatcher<'a> {
    fn make(client: &Client, dispatcher: &'a mut Dispatcher) -> Self {
        let command =
            if log::Level::Trace <= log::max_level() { client.info.clone() } else { None };
        Self {
            dispatcher,
            client_fd: client.fd.as_raw_fd(),
            client_id: client.id,
            command,
        }
    }

    /// `wl_display` singleton id is always `1` in this compositor.
    pub fn send_wl_display_error(
        &mut self,
        object_id: WaylandId,
        code: u32,
        message: impl AsRef<str>,
    ) {
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
    ) -> Result<(), ()> {
        use ClientAction::*;

        let mut dispatcher = ClientDispatcher::make(self, dispatcher);

        match action {
            Start => {
                self.read_process_info();
                log::info!(
                    "new client_id={:?} command={:?}",
                    self.id,
                    self.info.as_ref().map(|p| &p.command)
                );

                let display: WlDisplay = WlDisplay::default();
                self.insert_object(state, display);

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
            RefreshDataDeviceSelection => {
                self.refresh_data_device_selection(state, &mut dispatcher);
            }
            DataDeviceDragEnter { serial, surface, x, y, source_object_id } => {
                self.handle_data_device_drag_enter(
                    state,
                    &mut dispatcher,
                    serial,
                    surface,
                    x,
                    y,
                    source_object_id,
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
            DataSourceEvent { source_id, event } => {
                self.handle_data_source_event(state, &mut dispatcher, source_id, event);
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
                    schedule_output(state, output_id, &mut dispatcher);
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
            CompleteSurfaceFrameCallbacks { surface_ids, time_msec } => {
                self.complete_surface_frame_callbacks(
                    state,
                    &mut dispatcher,
                    surface_ids,
                    time_msec,
                );
            }
            PresentationFeedbackPresented {
                feedback_ids,
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
                    feedback_ids,
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
            PresentationFeedbackDiscarded { feedback_ids } => {
                self.discard_presentation_feedback(state, &mut dispatcher, feedback_ids);
            }
            DiscardSurfacePresentationFeedbacksForBackend { drm_id } => {
                self.discard_surface_presentation_feedbacks_for_backend(
                    state,
                    &mut dispatcher,
                    drm_id,
                );
            }
            CancelBlockedSurface { surface_object_id, waiter_id } => {
                let Some(mut surface) = state
                    .objects
                    .try_grab_object::<WlSurface>(surface_object_id)
                else {
                    return Ok(());
                };
                surface.cancel_blocked_if_waiter_matches(waiter_id, self, state, &mut dispatcher);
            }
            DismissPopupGrab { popup_ids } => {
                self.dismiss_popup_ids(state, &mut dispatcher, popup_ids, None);
            }
            InteractiveResizeConfigure { xdg_toplevel_id, width, height, resizing } => {
                if !self.contains_object(xdg_toplevel_id, state) {
                    return Ok(());
                }
                let xdg_surface_id = {
                    let toplevel_object_id = self.object_id(xdg_toplevel_id);
                    let Some(toplevel) = state
                        .objects
                        .try_ref_object::<XdgToplevel>(toplevel_object_id)
                    else {
                        return Ok(());
                    };
                    toplevel.xdg_surface_id()
                };
                if !self.contains_object(xdg_surface_id, state) {
                    return Ok(());
                }
                let configure_bounds = {
                    let xdg_surface_object_id = self.object_id(xdg_surface_id);
                    state
                        .objects
                        .try_ref_object::<XdgSurface>(xdg_surface_object_id)
                        .and_then(|xdg_surface| {
                            let wl_surface_object_id =
                                self.object_id_for_wayland(xdg_surface.wl_surface_id())?;
                            let surface = state
                                .objects
                                .try_ref_object::<WlSurface>(wl_surface_object_id)?;
                            surface.configure_bounds_for_object(state, wl_surface_object_id)
                        })
                };
                let mut xdg_surface = self.grab_object(xdg_surface_id, state);
                let mut toplevel = self.grab_object(xdg_toplevel_id, state);
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
            MsgReceived { mut request } => {
                let Some(obj_id) = self
                    .objects
                    .get(&request.object_id())
                    .map(|entry| entry.object_id)
                else {
                    log::warn!(
                        "unknown Wayland object id {:?} from client {:?}",
                        request.object_id(),
                        self.id
                    );
                    self.protocol_violation_disconnect(
                        &mut dispatcher,
                        request.object_id(),
                        wl_display::Error::InvalidObject as u32,
                        "invalid object id",
                    );
                    return Ok(());
                };

                let mut pending_fds = std::mem::take(&mut self.pending_fds);
                request.fds = Some(&mut pending_fds);

                let mut obj = state.objects.grab(obj_id);
                obj.on(self, state, request, &mut dispatcher);

                self.pending_fds = pending_fds;
            }
            Close => {
                log::info!("client_id={:?} closed", self.id);
                if let Some(id) = self.reader_id.take() {
                    dispatcher.stop_model(id);
                };
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
                self.pending_fds.clear();
                if seat_removal.cursor_changed
                    || seat_removal.interactive_grab_cleared
                    || had_client_objects
                {
                    schedule_active_output(state, &mut dispatcher);
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
    ) -> Result<(), ()> {
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
        clients::{Client, ClientId},
        egl::EGL,
        globals::Globals,
        io_uring::model::IoUringAction,
        runner::Dispatcher,
        state::{BackendOutputRef, DrmId, FractionalScale, ObjectId, Output, OutputMode, State},
        wayland_objects::{wl_surface::WlSurface, wp_fractional_scale_v1::WpFractionalScaleV1},
        xkb::Xkb,
    };

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

    fn make_output(x: i32, y: i32, width: i32, height: i32) -> Output {
        Output {
            backend: BackendOutputRef::new(DrmId::from(1usize)),
            drm_crtc_id: 1,
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
            name: "output-1".to_string(),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width, height, refresh_millihz: 60_000 },
            preferred_mode: None,
        }
    }

    #[test]
    fn refresh_surface_outputs_resends_fractional_scale_when_output_scale_changes() {
        let mut state = make_state();
        let mut client = Client::new_for_tests(ClientId::from_raw(9));
        let output_id = state.insert_output(make_output(0, 0, 200, 120));
        let surface_id = crate::protocol_generated::WlSurfaceId::from(10u32);
        let fractional_id = crate::protocol_generated::WpFractionalScaleV1Id::from(11u32);
        let ((_, surface_object_id), _) =
            client.insert_object(&mut state, WlSurface::new(surface_id, client.client_id(), 6));
        state.register_root_surface(surface_object_id);

        let fractional_object_id = state
            .insert_object(WpFractionalScaleV1::new(fractional_id, surface_object_id))
            .1;
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            surface
                .set_fractional_scale_object(fractional_object_id)
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
}
