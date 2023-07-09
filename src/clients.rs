use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    os::{
        fd::{AsRawFd, RawFd},
        unix::net::UnixListener,
    },
};

use libc::getsockopt;

use crate::{
    io_uring::{
        model::IoUringAction,
        ring::{AcceptCompleted, RecvmsgCompleted},
    },
    object::{Object, ParserError},
    protocol_generated::WaylandId,
    state::{AnyAction, Dispatcher, Model, Redispatcher},
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, derive_more::Into)]
struct ClientId(u64);

pub struct Clients {
    current_id: u64,
    fd: UnixListener,
    set: BTreeSet<ClientId>,
}

#[derive(Debug)]
pub enum ClientsAction {
    StartListen,
    NewClient { new_client: RawFd, fd: RawFd },
}

impl Clients {
    fn next_id(&mut self) -> ClientId {
        let id = self.current_id;
        self.current_id += 1;
        ClientId(id)
    }

    fn on_accept(result: std::io::Result<AcceptCompleted>) -> AnyAction {
        let AcceptCompleted { fd, new_client } = result.unwrap();
        AnyAction::from(ClientsAction::NewClient { fd, new_client })
    }

    fn dispatch_accept(&self, dispatcher: &mut Dispatcher) {
        dispatcher.dispatch(IoUringAction::Accept {
            fd: self.fd.as_raw_fd(),
            dispatch_to: Redispatcher::new(Self::on_accept),
        });
    }
}

impl Model for Clients {
    type InitParams = ();
    type Action = ClientsAction;

    fn init(params: Self::InitParams) -> Box<Self> {
        let unix = UnixListener::bind("/var/run/user/1000/wayland-1").unwrap();
        Box::new(Self {
            current_id: 0,
            fd: unix,
            set: BTreeSet::default(),
        })
    }

    #[allow(clippy::needless_late_init)]
    fn reducer(&mut self, action: &ClientsAction, dispatcher: &mut Dispatcher) -> Result<(), ()> {
        match action {
            ClientsAction::StartListen => {
                self.dispatch_accept(dispatcher);
            }
            ClientsAction::NewClient { fd, new_client } => {
                self.dispatch_accept(dispatcher);

                let client_id = self.next_id();
                log::trace!("new_client id={:?} fd={:?}", client_id, new_client);

                self.set.insert(client_id);

                dispatcher.start_model_with_id(client_id, Client::new(client_id, *new_client));
            }
        }
        Ok(())
    }
}

struct ProcessInfo {
    command: String,
    cred: libc::ucred,
}

pub struct Client {
    id: ClientId,
    fd: RawFd,
    objects: BTreeMap<WaylandId, Box<dyn Object>>,
    info: Option<ProcessInfo>,
}

pub enum ClientAction {
    Start,
    MsgReceived { request: RawRequest },
    Closed,
}

impl Client {
    fn new(client_id: ClientId, fd: RawFd) -> Self {
        Self {
            id: client_id,
            fd,
            objects: BTreeMap::default(),
            info: None,
        }
    }

    fn read_process_info(&mut self) {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
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
            return log::warn!("Unable to `getsockopt` of new client: {:?}", e);
        }
        let comm = match std::fs::read(format!("/proc/{}/comm", cred.pid)) {
            Ok(comm) => comm,
            Err(e) => return log::warn!("Unable to read `comm` of new client: {:?}", e),
        };
        let command = String::from_utf8_lossy(&comm);
        let command = command.trim().to_string();

        self.info = Some(ProcessInfo { command, cred })
    }
}

impl Model for Client {
    type InitParams = ();
    type Action = ClientAction;
    const START: Option<Self::Action> = Some(Self::Action::Start);

    fn init(params: Self::InitParams) -> Box<Self> {
        todo!()
    }

    fn reducer(&mut self, action: &ClientAction, dispatcher: &mut Dispatcher) -> Result<(), ()> {
        match action {
            ClientAction::Start => {
                self.read_process_info();

                let (client_reader_id, client_reader) = ClientReader::new(self.fd, self.id);
                dispatcher.start_model_with_id(client_reader_id, client_reader);
            }
            ClientAction::MsgReceived { request } => {
                let mut obj = crate::wayland_objects::wl_display::WlDisplay {
                    id: crate::protocol_generated::WlDisplayId::from(0),
                };
                obj.on(self, request.clone());
            }
            ClientAction::Closed => {
                eprintln!("client_id={:?} CLOSED", self.id);
            }
        };

        Ok(())
    }
}

struct Buffer {
    data: Vec<u8>,
    pos: usize,
}

impl Buffer {
    fn new() -> Self {
        Self {
            data: vec![0; 32 * 1024],
            pos: 0,
        }
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

        let fst = u32::from_ne_bytes(self.data[0..4].try_into().unwrap());
        let snd = u32::from_ne_bytes(self.data[4..8].try_into().unwrap());

        Some([fst, snd])
    }

    fn get_whole_message(&self) -> Option<&[u8]> {
        let [obj_id, word] = self.get_header()?;
        let len = (word >> 16) as usize;
        // let req = (word & 0xFFFF) as usize;
        // dbg!(obj_id, len, req);
        self.data.get(..len)
    }

    fn len(&self) -> usize {
        self.pos
    }

    fn consume_msg(&mut self) {
        let [obj_id, word] = self.get_header().unwrap();
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

struct ClientReader {
    fd: RawFd,
    id: ClientReaderId,
    client_id: ClientId,
    read_buffer: Buffer,
    neof: usize,
}

enum ClientReaderAction {
    Start,
    ReadMsg { fd: RawFd, buffer: Box<[u8]> },
}

impl ClientReader {
    fn new(fd: RawFd, client_id: ClientId) -> (ClientReaderId, Self) {
        let id = ClientReader::next_id();
        let read_buffer = Buffer::new();
        let neof = 0;
        let this = Self {
            fd,
            id,
            client_id,
            read_buffer,
            neof,
        };
        (id, this)
    }

    fn next_id() -> ClientReaderId {
        use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
        pub static CLIENT_READER_ID: AtomicU64 = AtomicU64::new(0);
        let id = CLIENT_READER_ID.fetch_add(1, Relaxed);
        ClientReaderId(id)
    }

    fn is_eof(&mut self, buffer: &[u8], dispatcher: &mut Dispatcher) -> bool {
        if !buffer.is_empty() {
            self.neof = 0;
        } else {
            self.neof += 1;
            // Number of EOF (len == 0) received
            // Consider EOF reached after 2
            // https://github.com/facebook/rocksdb/pull/6441#issuecomment-589843435
            if self.neof == 2 {
                dispatcher.dispatch_to(self.client_id, ClientAction::Closed);
                return true;
            }
        }
        false
    }

    fn read(&self, dispatcher: &mut Dispatcher) -> Result<(), ()> {
        dispatcher.dispatch(IoUringAction::Recvmsg {
            fd: self.fd,
            dispatch_to: Redispatcher::new_with_id(self.id, Self::on_recvmsg),
        });
        Ok(())
    }

    fn on_recvmsg(result: std::io::Result<RecvmsgCompleted>) -> AnyAction {
        let RecvmsgCompleted { fd, buffer } = result.unwrap();
        AnyAction::from(ClientReaderAction::ReadMsg { fd, buffer })
    }
}

impl Model for ClientReader {
    type InitParams = ();
    type Action = ClientReaderAction;
    const START: Option<Self::Action> = Some(Self::Action::Start);

    fn init(params: Self::InitParams) -> Box<Self> {
        todo!()
    }

    fn reducer(&mut self, action: &Self::Action, dispatcher: &mut Dispatcher) -> Result<(), ()> {
        match action {
            Self::Action::Start => self.read(dispatcher),
            Self::Action::ReadMsg { fd, buffer } => {
                if self.is_eof(buffer, dispatcher) {
                    return Ok(());
                }
                self.read_buffer.append(buffer);
                loop {
                    let msg: &[u8] = match self.read_buffer.get_whole_message() {
                        Some(msg) => msg,
                        None => return self.read(dispatcher),
                    };
                    let request: RawRequest = match msg.try_into() {
                        Ok(request) => request,
                        Err(_) => todo!(),
                    };
                    dispatcher.dispatch_to(self.client_id, ClientAction::MsgReceived { request });
                    self.read_buffer.consume_msg();
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct RawRequest {
    pub object_id: u32,
    pub request_id: u16,
    pub arguments: Arguments,
}

impl RawRequest {
    pub fn object_id(&self) -> WaylandId {
        WaylandId::from(self.object_id)
    }
}

impl std::ops::Deref for RawRequest {
    type Target = Arguments;

    fn deref(&self) -> &Self::Target {
        &self.arguments
    }
}

impl std::ops::DerefMut for RawRequest {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.arguments
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

        Ok(Self {
            object_id,
            request_id,
            arguments: Arguments { args },
        })
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

        let mut words = Vec::with_capacity(len);
        for _ in 0..nwords {
            let word = self.pop()?;
            words.extend(u32::to_ne_bytes(word));
        }
        words.truncate(len);

        Ok(String::from_utf8(words).unwrap())
    }

    pub fn fixed(&mut self) -> Result<f32, ParserError> {
        todo!()
    }

    pub fn fd(&mut self) -> Result<RawFd, ParserError> {
        todo!()
    }
}
