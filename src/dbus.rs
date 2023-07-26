use std::{
    borrow::Cow,
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    os::{
        fd::{AsRawFd, RawFd},
        unix::net::UnixStream,
    },
};

use dbus_message_parser::encode::EncodeError;

use crate::{
    io_uring::{model::IoUringAction, ring::RecvmsgCompleted},
    state::{Dispatcher, Model, Redispatcher},
    udev::UdevAction,
};

use {
    dbus_message_parser::{
        decode::DecodeError,
        message::{Message, MessageType::Signal},
        value::Value,
    },
    thiserror::Error,
};

#[derive(Error, Debug)]
pub enum InitializationError {
    #[error("IO Error: `{0}`")]
    IO(#[from] std::io::Error),
    #[error("Authentication: `{0}`")]
    Auth(String),
    #[error("Negociate unix fd: `{0}`")]
    Negociate(String),
    #[error("Decoding: `{0}`")]
    Decoding(#[from] DecodeError),
    #[error("Encoding: `{0}`")]
    Encoding(#[from] EncodeError),
}

#[derive(Error, Debug)]
pub enum DbusError {
    #[error("Error during dbus initialization: `{0}`")]
    Intialization(#[from] InitializationError),
    #[error("Decoding: `{0}`")]
    Decoding(#[from] DecodeError),
    #[error("Encoding: `{0}`")]
    Encoding(#[from] EncodeError),
    #[error("IO Error: `{0}`")]
    IO(#[from] std::io::Error),
    #[error("Session name not found: `{0}`")]
    SessionNameNotFound(String),
    #[error("Seat not found: `{0}`")]
    SeatNotFound(String),
    #[error("Failed to take session control: `{0}`")]
    TakeSession(String),
}

#[derive(Debug)]
enum Pending {
    TakeDevice { devnum: libc::dev_t },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DbusSerial(u32);

impl DbusSerial {
    fn next(&self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    fn as_u32(&self) -> u32 {
        self.0
    }
}

pub struct Dbus {
    socket: UnixStream,
    session_path: String,
    seat: String,
    current_serial: DbusSerial,
    pending: BTreeMap<DbusSerial, Pending>,
}

fn auth(buffered_socket: &mut BufReader<UnixStream>) -> Result<(), InitializationError> {
    let uid: String = {
        let uid = nix::unistd::getuid().as_raw().to_string();
        uid.as_bytes().iter().map(|b| format!("{:x}", b)).collect()
    };

    buffered_socket
        .get_mut()
        .write_all(format!("\0AUTH EXTERNAL {}\r\n", uid).as_bytes())?;

    let mut s = String::with_capacity(32);
    buffered_socket.read_line(&mut s)?;

    if !s.starts_with("OK") {
        return Err(InitializationError::Auth(s));
    }

    Ok(())
}

fn negociate_unix_fds(
    buffered_socket: &mut BufReader<UnixStream>,
) -> Result<(), InitializationError> {
    buffered_socket
        .get_mut()
        .write_all("NEGOTIATE_UNIX_FD\r\n".as_bytes())?;

    let mut s = String::with_capacity(32);
    buffered_socket.read_line(&mut s)?;

    if !s.starts_with("AGREE_UNIX_FD") {
        return Err(InitializationError::Negociate(s));
    }

    Ok(())
}

fn send_begin(buffered_socket: &mut BufReader<UnixStream>) -> Result<(), InitializationError> {
    buffered_socket
        .get_mut()
        .write_all("BEGIN\r\n".as_bytes())?;
    Ok(())
}

struct MethodCallParams<'a> {
    destination: Cow<'a, str>,
    object_path: Cow<'a, str>,
    interface: Cow<'a, str>,
    member: Cow<'a, str>,
}

fn make_method_call(params: MethodCallParams) -> Message {
    let MethodCallParams { destination, object_path, interface, member } = params;
    Message::method_call(
        destination.as_ref().try_into().unwrap(),
        object_path.as_ref().try_into().unwrap(),
        interface.as_ref().try_into().unwrap(),
        member.as_ref().try_into().unwrap(),
    )
}

struct GetParams<'a> {
    destination: Cow<'a, str>,
    object_path: Cow<'a, str>,
    interface: Cow<'a, str>,
    property: &'a str,
}

fn make_get(params: GetParams) -> Message {
    let GetParams { destination, object_path, interface, property } = params;
    Message::property_get(
        destination.as_ref().try_into().unwrap(),
        object_path.as_ref().try_into().unwrap(),
        interface.as_ref().try_into().unwrap(),
        property,
    )
}

fn read_u32(slice: &[u8]) -> u32 {
    u32::from_ne_bytes(slice.try_into().unwrap())
}

fn read_sync(
    buffered_socket: &mut BufReader<UnixStream>,
    buffer: &mut Vec<u8>,
) -> Result<Message, InitializationError> {
    const HEADER_LENGTH: usize = 16;

    buffered_socket.read_exact(&mut buffer[..HEADER_LENGTH])?;
    let body_length = read_u32(&buffer[4..8]);
    let headers_length = read_u32(&buffer[12..16]);

    let dyn_header_len = headers_length + (headers_length.wrapping_neg() & 7);
    let remaining = (dyn_header_len + body_length) as usize;
    let total = HEADER_LENGTH + remaining;

    if buffer.len() < total {
        buffer.resize(total, 0);
    }

    buffered_socket.read_exact(&mut buffer[HEADER_LENGTH..total])?;

    let (msg, _offset) = Message::decode(buffer.clone().into())?;
    Ok(msg)
}

/// Say hello and read acquired name
fn say_hello(
    buffered_socket: &mut BufReader<UnixStream>,
) -> Result<Option<String>, InitializationError> {
    let mut msg = make_method_call(MethodCallParams {
        destination: "org.freedesktop.DBus".into(),
        object_path: "/org/freedesktop/DBus".into(),
        interface: "org.freedesktop.DBus".into(),
        member: "Hello".into(),
    });
    msg.set_serial(1);

    buffered_socket.get_mut().write_all(&msg.encode()?)?;

    let mut buffer = vec![0u8; 32 * 1024];
    let mut acquired_name = None;
    loop {
        let msg = read_sync(buffered_socket, &mut buffer)?;
        if let Signal = msg.get_type() {
            if msg.get_member().map(|m| m.as_ref()) == Some("NameAcquired") {
                if let Some(Value::String(s)) = msg.get_body().first() {
                    acquired_name = Some(s.clone());
                }
            }
        };
        if buffered_socket.buffer().is_empty() {
            break;
        }
    }

    Ok(acquired_name)
}

fn get_session_path(buffered_socket: &mut BufReader<UnixStream>) -> Result<String, DbusError> {
    let session_id = std::env::var("XDG_SESSION_ID").expect("XDG_SESSION_ID not set");

    let mut msg = make_method_call(MethodCallParams {
        destination: "org.freedesktop.login1".into(),
        object_path: "/org/freedesktop/login1".into(),
        interface: "org.freedesktop.login1.Manager".into(),
        member: "GetSession".into(),
    });
    msg.set_serial(2);
    msg.add_value(Value::String(session_id));

    buffered_socket.get_mut().write_all(&msg.encode()?)?;

    let mut buffer = vec![0u8; 32 * 1024];
    let msg = read_sync(buffered_socket, &mut buffer)?;

    let Some(Value::ObjectPath(obj_session_path)) = msg.get_body().first() else {
        return Err(DbusError::SessionNameNotFound(format!("{:?}", msg)));
    };

    let session_path: String = obj_session_path.clone().into();

    Ok(session_path)
}

fn get_seat(
    buffered_socket: &mut BufReader<UnixStream>,
    session_path: &str,
) -> Result<String, DbusError> {
    let make_err = |msg: &Message| Err::<String, _>(DbusError::SeatNotFound(format!("{:?}", msg)));

    let mut msg = make_method_call(MethodCallParams {
        destination: "org.freedesktop.login1".into(),
        object_path: session_path.into(),
        interface: "org.freedesktop.DBus.Properties".into(),
        member: "Get".into(),
    });
    msg.set_serial(3);
    msg.add_value(Value::String("org.freedesktop.login1.Session".into()));
    msg.add_value(Value::String("Seat".into()));

    buffered_socket.get_mut().write_all(&msg.encode()?)?;

    let mut buffer = vec![0u8; 32 * 1024];
    let msg = read_sync(buffered_socket, &mut buffer)?;

    let Some(Value::Variant(variant)) = msg.get_body().first() else {
        return make_err(&msg);
    };
    let Value::Struct(values) = &**variant else {
        return make_err(&msg);
    };
    let Some(Value::String(seat)) = values.as_ref().first() else {
        return make_err(&msg);
    };
    Ok(seat.clone())
}

fn take_session_control(
    buffered_socket: &mut BufReader<UnixStream>,
    session_path: &str,
) -> Result<DbusSerial, DbusError> {
    if crate::utils::is_x11() {
        // Don't take control on X11
        return Ok(DbusSerial(4));
    }

    log::info!("Taking session control: {}", session_path);

    let mut msg = make_method_call(MethodCallParams {
        destination: "org.freedesktop.login1".into(),
        object_path: session_path.into(),
        interface: "org.freedesktop.login1.Session".into(),
        member: "TakeControl".into(),
    });
    msg.set_serial(4);
    msg.add_value(Value::Boolean(false)); // Force

    buffered_socket.get_mut().write_all(&msg.encode()?)?;

    let mut buffer = vec![0u8; 32 * 1024];
    let msg = read_sync(buffered_socket, &mut buffer)?;

    if let Some(error_name) = msg.get_error_name() {
        return Err(DbusError::TakeSession(error_name.as_ref().to_string()));
    }

    Ok(DbusSerial(5))
}

pub fn init() -> Result<Dbus, DbusError> {
    const SYSTEM_PATH: &str = "/run/dbus/system_bus_socket";
    let socket = UnixStream::connect(SYSTEM_PATH)?;
    let mut socket = BufReader::new(socket);

    auth(&mut socket)?;
    negociate_unix_fds(&mut socket)?;
    send_begin(&mut socket)?;
    say_hello(&mut socket)?;

    let session_path = get_session_path(&mut socket)?;
    let seat = get_seat(&mut socket, &session_path)?;
    let current_serial = take_session_control(&mut socket, &session_path)?;

    let socket = socket.into_inner();
    let pending = BTreeMap::default();
    Ok(Dbus { socket, session_path, seat, current_serial, pending })
}

impl Dbus {
    pub fn new() -> Result<Box<Self>, DbusError> {
        init().map(Box::new)
    }

    fn next_serial(&mut self) -> DbusSerial {
        let serial = self.current_serial;
        self.current_serial = serial.next();
        serial
    }

    fn take_device(&mut self, devnum: libc::dev_t, dispatcher: &mut Dispatcher) -> DbusSerial {
        let major: u32 = nix::sys::stat::major(devnum).try_into().unwrap();
        let minor: u32 = nix::sys::stat::minor(devnum).try_into().unwrap();

        let msg_serial = self.next_serial();
        let mut msg = make_method_call(MethodCallParams {
            destination: "org.freedesktop.login1".into(),
            object_path: self.session_path.as_str().into(),
            interface: "org.freedesktop.login1.Session".into(),
            member: "TakeDevice".into(),
        });
        msg.set_serial(msg_serial.as_u32());
        msg.add_value(Value::Uint32(major));
        msg.add_value(Value::Uint32(minor));

        let msg = msg.encode().unwrap();
        let action = IoUringAction::Write {
            fd: self.socket.as_raw_fd(),
            offset: 0,
            buffer: Box::from(&msg[..]),
            dispatch_to: Redispatcher::None,
        };
        dispatcher.dispatch(action);

        msg_serial
    }

    fn handle_message(&mut self, msg: Message, dispatcher: &mut Dispatcher) {
        use {dbus_message_parser::message::MessageType::*, Value::*};

        let reply_serial = msg.get_reply_serial().map(DbusSerial);
        let Some(reply_serial) = reply_serial else {
            return;
        };
        let pending = self.pending.remove(&reply_serial).unwrap();

        match msg.get_type() {
            MethodCall => todo!(),
            MethodReturn => match pending {
                Pending::TakeDevice { devnum } => {
                    let [UnixFD(fd), Boolean(inactive)] = msg.get_body() else {
                        panic!();
                    };
                    let (fd, inactive) = (*fd, *inactive);
                    dispatcher.dispatch(UdevAction::DeviceTaken { fd, inactive, devnum });
                }
            },
            Error => {
                log::error!("pending={:?} error={:?}", pending, msg);
            }
            Signal => todo!(),
        }
    }
}

pub enum DbusAction {
    Start,
    TakeDevice { devnum: libc::dev_t },
    MsgReceived { msg: Message },
}

impl Model for Dbus {
    type Action = DbusAction;
    const START: Option<Self::Action> = Some(Self::Action::Start);

    fn process(&mut self, action: Self::Action, dispatcher: &mut Dispatcher) -> Result<(), ()> {
        use DbusAction::*;

        match action {
            Start => {
                let fd = self.socket.as_raw_fd();
                dispatcher.start_model2(DbusReader::new(fd));
            }
            TakeDevice { devnum } => {
                let serial = self.take_device(devnum, dispatcher);
                self.pending.insert(serial, Pending::TakeDevice { devnum });
            }
            MsgReceived { msg } => {
                self.handle_message(msg, dispatcher);
            }
        };

        Ok(())
    }
}

struct DbusReader {
    fd: RawFd,
    read_buffer: DbusBuffer,
    fds: Vec<RawFd>,
    neof: usize,
}

#[derive(Debug)]
enum DbusReaderAction {
    Start,
    ReadMsg { buffer: Box<[u8]>, fds: Box<[RawFd]> },
    Stop,
}

impl Model for DbusReader {
    type Action = DbusReaderAction;
    const START: Option<Self::Action> = Some(Self::Action::Start);
    const STOP: Option<Self::Action> = Some(Self::Action::Stop);

    fn process(&mut self, action: Self::Action, dispatcher: &mut Dispatcher) -> Result<(), ()> {
        use DbusReaderAction::*;

        match action {
            Start => self.read(dispatcher),
            ReadMsg { buffer, fds } => {
                if self.is_eof(&buffer) {
                    return self.close(dispatcher);
                }
                self.read_buffer.append(&buffer);
                self.fds.extend(fds.iter());
                loop {
                    let msg: &[u8] = match self.read_buffer.get_whole_message() {
                        Some(msg) => msg,
                        None => return self.read(dispatcher),
                    };
                    let (msg, _offset) =
                        match Message::decode_with_fds(msg.to_vec().into(), &self.fds) {
                            Ok((msg, offset, _)) => (msg, offset),
                            Err(e) => panic!("{:?}", e),
                        };
                    if let Some(nfds) = msg.get_unix_fds() {
                        self.fds.drain(0..nfds as usize);
                    }
                    dispatcher.dispatch(DbusAction::MsgReceived { msg });
                    self.read_buffer.consume_msg();
                }
            }
            Stop => Ok(()),
        }
    }
}

struct DbusBuffer {
    data: Vec<u8>,
    pos: usize,
}

impl DbusBuffer {
    fn new() -> Self {
        Self { data: vec![0; 128 * 1024], pos: 0 }
    }

    fn append(&mut self, buffer: &[u8]) {
        let pos = self.pos;
        let buffer_len = buffer.len();

        if self.data.len() < pos + buffer_len {
            self.data.resize(pos + buffer_len, 0);
        }

        self.data[pos..pos + buffer_len].copy_from_slice(buffer);
        self.pos += buffer_len;
    }

    fn get_whole_message(&self) -> Option<&[u8]> {
        const HEADER_LENGTH: usize = 16;
        let data = &self.data[..self.pos];

        let header = data.get(..HEADER_LENGTH)?;
        let body_length = read_u32(&header[4..8]);
        let headers_length = read_u32(&header[12..16]);

        let dyn_header_len = headers_length + (headers_length.wrapping_neg() & 7);
        let remaining = (dyn_header_len + body_length) as usize;
        let total = HEADER_LENGTH + remaining;

        data.get(..total)
    }

    fn len(&self) -> usize {
        self.pos
    }

    fn consume_msg(&mut self) {
        let msg = self.get_whole_message().unwrap();
        let len = msg.len();

        if self.pos == len {
            self.pos = 0;
        } else {
            assert!(self.pos > len);
            self.data.copy_within(len..self.pos, 0);
            self.pos -= len;
        }
    }
}

impl From<std::io::Result<RecvmsgCompleted>> for DbusReaderAction {
    fn from(value: std::io::Result<RecvmsgCompleted>) -> Self {
        let RecvmsgCompleted { fd: _, buffer, fds } = value.unwrap();
        Self::ReadMsg { buffer, fds }
    }
}

impl DbusReader {
    fn new(fd: RawFd) -> Self {
        let read_buffer = DbusBuffer::new();
        let neof = 0;
        let fds = Vec::with_capacity(16);
        Self { fd, read_buffer, neof, fds }
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

    fn read(&self, dispatcher: &mut Dispatcher) -> Result<(), ()> {
        dispatcher.dispatch(IoUringAction::Recvmsg {
            fd: self.fd,
            dispatch_to: Redispatcher::into::<DbusReaderAction>(),
        });
        Ok(())
    }

    fn close(&self, _dispatcher: &mut Dispatcher) -> Result<(), ()> {
        todo!()
    }
}
