use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    os::{
        fd::{AsRawFd, RawFd},
        unix::net::UnixListener,
    },
    path::PathBuf,
    rc::Rc,
};

use libc::getsockopt;

use crate::{
    dbus::Dbus,
    io_uring::{
        model::IoUringAction,
        ring::{AcceptCompleted, RecvmsgCompleted},
    },
    object::{IdToObject, Object, Parser, ParserError},
    protocol_generated::{wl_display, Array, Constants, WaylandId},
    runner::{AnyAction, Dispatcher, Model, Redispatcher},
    state::{GrabbedObject, ObjectId, State},
    udev::Udev,
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, derive_more::Into)]
pub struct ClientId(u64);

pub struct Clients {
    current_id: u64,
    fd: UnixListener,
    set: BTreeSet<ClientId>,
}

#[derive(Debug)]
pub enum ClientsAction {
    Start,
    RearmAccept,
    NewClient { new_client: RawFd },
    LibinputEvent,
    UdevEvent,
    Close { id: ClientId },
}

impl Clients {
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

    fn on_accept(result: std::io::Result<AcceptCompleted>) -> AnyAction {
        match result {
            Ok(AcceptCompleted { fd: _, new_client }) => {
                AnyAction::from(ClientsAction::NewClient { new_client })
            }
            Err(e) => {
                log::error!("accept failed: {:?}", e);
                AnyAction::from(ClientsAction::RearmAccept)
            }
        }
    }

    fn dispatch_accept(&self, dispatcher: &mut Dispatcher) {
        dispatcher.dispatch(IoUringAction::Accept {
            fd: self.fd.as_raw_fd(),
            dispatch_to: Redispatcher::new::<Self>(Self::on_accept),
        });
    }
}

impl Model for Clients {
    type Action = ClientsAction;

    fn process(
        &mut self,
        _state: &mut State,
        action: ClientsAction,
        dispatcher: &mut Dispatcher,
    ) -> Result<(), ()> {
        use ClientsAction::*;

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
            Close { id } => {
                log::trace!("client {:?} closed", id);
                self.set.remove(&id);
                dispatcher.stop_model::<_, Client>(id);
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

pub struct Client {
    id: ClientId,
    fd: RawFd,
    reader_id: Option<ClientReaderId>,
    /// the key `WaylandId` is the id we share with the client
    /// the value `ObjectId` is an index in our `State::objects`
    /// So this `objects` is just used to lookup into `State::objects`, from the client `WaylandId`
    // TODO: Replace `BTreeMap` with a `IndexMap` here ?
    objects: BTreeMap<WaylandId, ObjectId>,
    info: Option<Rc<ProcessInfo>>,
    pending_fds: VecDeque<RawFd>,
}

pub enum ClientAction {
    Start,
    MsgReceived { request: RawRequest<'static> },
    FdsReceived { fds: Box<[RawFd]> },
    Close,
}

impl Client {
    fn new(client_id: ClientId, fd: RawFd) -> Self {
        Self {
            id: client_id,
            fd,
            reader_id: None,
            objects: BTreeMap::default(),
            info: None,
            pending_fds: VecDeque::new(),
        }
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
                self.fd,
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

    pub fn get_object<'a, K>(&self, id: K, state: &'a mut State) -> &'a mut K::Object
    where
        K: Into<WaylandId> + IdToObject,
        K::Object: Object,
    {
        let id: WaylandId = id.into();
        let obj_id = self.objects[&id];
        state.objects[obj_id].downcast_mut().unwrap()
    }

    pub fn grab_object<K>(&self, id: K, state: &mut State) -> GrabbedObject<K::Object>
    where
        K: Into<WaylandId> + IdToObject,
        K::Object: Object,
    {
        let id: WaylandId = id.into();
        let obj_id = self.objects[&id];
        let grabbed = state.objects.grab(obj_id);
        GrabbedObject::new(grabbed)
    }

    pub fn object_id<K>(&self, id: K) -> ObjectId
    where
        K: Into<WaylandId>,
    {
        let id: WaylandId = id.into();
        self.objects[&id]
    }

    pub fn object_id_for_wayland(&self, id: WaylandId) -> Option<ObjectId> {
        self.objects.get(&id).copied()
    }

    pub fn remove_wayland_object(&mut self, id: WaylandId) -> Option<ObjectId> {
        self.objects.remove(&id)
    }

    pub fn insert_object<'a, O: Object>(
        &mut self,
        state: &'a mut State,
        object: O,
    ) -> ((WaylandId, ObjectId), &'a mut O) {
        let (wayland_id, object_id) = state.insert_object(object);
        self.objects.insert(wayland_id, object_id);
        let object: &mut O = state.objects[object_id].downcast_mut::<O>().unwrap();
        ((wayland_id, object_id), object)
    }

    pub fn contains_object<K>(&self, id: K, state: &State) -> bool
    where
        K: Into<WaylandId> + IdToObject,
    {
        let id: WaylandId = id.into();
        let Some(obj_id) = self.objects.get(&id).cloned() else {
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
        let object_id = self.objects.remove(&id).unwrap();
        state.remove_object(object_id);
        wl_display.send_event(wl_display::Event::DeleteId { id: id.into() }, dispatcher);
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
            client_fd: client.fd,
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
        self.dispatch(ClientsAction::Close { id: client_id });
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

                let (client_reader_id, client_reader) = ClientReader::new(self.fd, self.id);
                self.reader_id = Some(client_reader_id);
                dispatcher.start_model(client_reader_id, client_reader);
            }
            FdsReceived { fds } => {
                self.pending_fds.extend(fds.iter());
            }
            MsgReceived { mut request } => {
                let Some(&obj_id) = self.objects.get(&request.object_id()) else {
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
                let wayland_ids: Vec<WaylandId> = self.objects.keys().copied().collect();
                for wid in wayland_ids {
                    if let Some(oid) = self.objects.remove(&wid) {
                        let _ = state.remove_object(oid);
                    }
                }
                if let Some(id) = self.reader_id {
                    dispatcher.stop_model::<_, ClientReader>(id)
                };
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

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, derive_more::Into)]
struct ClientReaderId(u64);

/// Responsible for reading client's fd, parsing the message, and
/// notify `Client`.
struct ClientReader {
    fd: RawFd,
    id: ClientReaderId,
    client_id: ClientId,
    read_buffer: Buffer,
    neof: usize,
}

enum ClientReaderAction {
    Start,
    ReadMsg { buffer: Box<[u8]>, fds: Box<[RawFd]> },
    ErrorOnSocket { error: std::io::Error },
    Stop,
}

impl ClientReader {
    fn new(fd: RawFd, client_id: ClientId) -> (ClientReaderId, Self) {
        let id = ClientReader::next_id();
        let read_buffer = Buffer::new();
        let neof = 0;
        let this = Self { fd, id, client_id, read_buffer, neof };
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

    fn dispatch_recvmsg(&self, dispatcher: &mut Dispatcher) {
        dispatcher.dispatch(IoUringAction::Recvmsg {
            fd: self.fd,
            dispatch_to: Redispatcher::id(self.id, Self::on_recvmsg),
        });
    }

    fn on_recvmsg(result: std::io::Result<RecvmsgCompleted>) -> AnyAction {
        match result {
            Ok(RecvmsgCompleted { fd: _, buffer, fds }) => {
                AnyAction::from(ClientReaderAction::ReadMsg { buffer, fds })
            }
            Err(error) => AnyAction::from(ClientReaderAction::ErrorOnSocket { error }),
        }
    }

    fn dispatch_close(&self, dispatcher: &mut Dispatcher) {
        dispatcher.dispatch(ClientsAction::Close { id: self.client_id });
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
                log::error!("An error occured on the socket of {:?}: {:?}", self.client_id, error);
                // Dispatch an action stop our Model
                self.dispatch_close(dispatcher);
            }
            Stop => {
                // TODO: Close file descriptor
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
    pub fds: Option<&'a mut VecDeque<RawFd>>,
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

    pub fn new_id_with_interface<I>(&mut self) -> Result<I, ParserError>
    where
        I: From<u32>,
    {
        let _interface_name = self.string()?;
        let _interface_version = self.uint()?;
        self.id()
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
    use std::collections::VecDeque;

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
}
