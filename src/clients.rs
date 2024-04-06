use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    os::{
        fd::{AsRawFd, RawFd},
        unix::net::UnixListener,
    },
    path::PathBuf,
};

use libc::getsockopt;

use crate::{
    dbus::Dbus,
    io_uring::{
        model::IoUringAction,
        ring::{AcceptCompleted, RecvmsgCompleted},
    },
    object::{Object, ParserError},
    protocol_generated::WaylandId,
    runner::{AnyAction, Dispatcher, Model, Redispatcher},
    state::{ObjectId, State},
    udev::Udev,
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
    NewClient { new_client: RawFd },
    LibinputEvent,
    UdevEvent,
    Close { id: ClientId },
}

impl Clients {
    pub fn new() -> Box<Self> {
        let filename = PathBuf::from("/var/run/user/1000/wayland-1");
        if filename.exists() {
            std::fs::remove_file(&filename).unwrap();
        }
        let fd = UnixListener::bind(filename).unwrap();
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
        let AcceptCompleted { fd: _, new_client } = result.unwrap();
        AnyAction::from(ClientsAction::NewClient { new_client })
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
    command: String,
    cred: libc::ucred,
}

pub struct Client {
    id: ClientId,
    fd: RawFd,
    reader_id: Option<ClientReaderId>,
    objects: BTreeMap<WaylandId, ObjectId>,
    info: Option<ProcessInfo>,
}

pub enum ClientAction {
    Start,
    MsgReceived { request: RawRequest },
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
        }
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
        let command = command.trim().to_string();

        self.info = Some(ProcessInfo { command, cred })
    }

    fn insert_object<O: Object>(&mut self, state: &mut State, object: O) -> WaylandId {
        let (wayland_id, object_id) = state.insert_object(object);
        self.objects.insert(wayland_id, object_id);
        wayland_id
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
}

impl<'a> ClientDispatcher<'a> {
    fn make(client: &Client, dispatcher: &'a mut Dispatcher) -> Self {
        Self { dispatcher, client_fd: client.fd }
    }

    pub fn send_event(&mut self, event: RawEvent) {
        let action = IoUringAction::Write {
            fd: self.client_fd,
            offset: 0,
            buffer: event.into_bytes(),
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
        use {crate::wayland_objects::wl_display::WlDisplay, ClientAction::*};

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
            MsgReceived { request } => {
                let obj_id = self.objects.get(&request.object_id()).unwrap();

                let mut obj = state.objects.grab(*obj_id);
                obj.on(self, state, request, &mut dispatcher);
                state.objects.release(obj);
            }
            Close => {
                log::info!("client_id={:?} closed", self.id);
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
    ReadMsg { buffer: Box<[u8]> },
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

    fn dispatch_read(&self, dispatcher: &mut Dispatcher) {
        dispatcher.dispatch(IoUringAction::Recvmsg {
            fd: self.fd,
            dispatch_to: Redispatcher::id(self.id, Self::on_recvmsg),
        });
    }

    fn on_recvmsg(result: std::io::Result<RecvmsgCompleted>) -> AnyAction {
        match result {
            Ok(RecvmsgCompleted { fd: _, buffer, fds: _ }) => {
                AnyAction::from(ClientReaderAction::ReadMsg { buffer })
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
                self.dispatch_read(dispatcher);
            }
            ReadMsg { buffer } => {
                if self.is_eof(&buffer) {
                    self.dispatch_close(dispatcher);
                    return Ok(());
                }
                self.read_buffer.append(&buffer);
                loop {
                    let msg: &[u8] = match self.read_buffer.get_whole_message() {
                        Some(msg) => msg,
                        None => {
                            self.dispatch_read(dispatcher);
                            return Ok(());
                        }
                    };
                    let request: RawRequest = match msg.try_into() {
                        Ok(request) => request,
                        Err(_) => todo!(),
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
}

pub trait EventWriter {
    type Event: 'static;
    fn to_raw_event(&self, event: Self::Event) -> RawEvent;
    fn send_event(&self, event: Self::Event, dispatcher: &mut ClientDispatcher) {
        dispatcher.send_event(self.to_raw_event(event));
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

    pub fn string<S: AsRef<str>>(&mut self, s: S) {
        let s = s.as_ref();
        let len: u32 = s.len().try_into().unwrap();
        self.push(len);

        for (chunk, chunk_len) in s.as_bytes().chunks(4).map(|chunk| (chunk, chunk.len())) {
            let uint = if chunk_len == 4 {
                u32::from_ne_bytes(chunk.try_into().unwrap())
            } else {
                let mut bytes = [0u8; 4];
                bytes[..chunk_len].copy_from_slice(chunk);
                u32::from_ne_bytes(bytes)
            };
            self.push(uint);
        }
    }

    pub fn fixed(&mut self, _fixed: f32) {
        todo!()
    }

    pub fn array(&mut self, _array: crate::protocol_generated::Array) {
        todo!()
    }

    pub fn fd(&mut self, _fd: RawFd) {
        todo!()
    }

    pub fn into_bytes(self) -> Box<[u8]> {
        let mut words = self.arguments.args;
        let length = words.len() + 2;

        let length: u16 = (length * 4).try_into().unwrap();
        let word = (length as u32) << 16 | self.request_id as u32;

        words.push_front(word);
        words.push_front(self.object_id);

        words.into_iter().flat_map(u32::to_ne_bytes).collect()
    }
}

#[derive(Clone, Debug, derive_more::Deref, derive_more::DerefMut)]
pub struct RawRequest {
    pub object_id: u32,
    pub request_id: u16,
    #[deref]
    #[deref_mut]
    pub arguments: Arguments,
}

impl RawRequest {
    pub fn object_id(&self) -> WaylandId {
        WaylandId::from(self.object_id)
    }
}

impl TryFrom<&[u8]> for RawRequest {
    type Error = ();

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let object_id = u32::from_ne_bytes(value[0..4].try_into().unwrap());
        let word = u32::from_ne_bytes(value[4..8].try_into().unwrap());

        let len = (word >> 16) as usize;
        let request_id = (word & 0xFFFF) as u16;

        if len % 4 != 0 {
            return Err(()); // Invalid
        }

        let args = value[8..]
            .array_chunks::<4>()
            .copied()
            .map(u32::from_ne_bytes)
            .collect();

        Ok(Self { object_id, request_id, arguments: Arguments { args } })
    }
}

#[derive(Clone, Debug)]
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

    pub fn string(&mut self) -> Result<String, ParserError> {
        let len = self.uint()? as usize;
        let nwords = len / 4;

        let mut bytes = Vec::with_capacity(len);
        for _ in 0..nwords {
            let word = self.pop()?;
            bytes.extend(u32::to_ne_bytes(word));
        }
        assert!(bytes.len() >= len);
        bytes.truncate(len);

        Ok(String::from_utf8(bytes).unwrap())
    }

    pub fn fixed(&mut self) -> Result<f32, ParserError> {
        todo!()
    }

    pub fn fd(&mut self) -> Result<RawFd, ParserError> {
        todo!()
    }
}
