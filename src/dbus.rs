use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, BufReader, Read, Write},
    os::{
        fd::{AsRawFd, OwnedFd, RawFd},
        unix::net::UnixStream,
    },
};

use dbus_message_parser::encode::EncodeError;

use crate::{
    io_uring::{
        model::IoUringAction,
        ring::{RecvmsgCompleted, RingId},
    },
    runner::{Dispatcher, Model, Redispatcher, ShutdownReason},
    scheduler::{output_scheduler_id, OutputSchedulerAction},
    state::State,
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
    SubscribeSessionSignals,
    GetSessionActive,
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
    held_devices: BTreeSet<libc::dev_t>,
    took_control: bool,
    released_control: bool,
}

fn session_ownership_lost(error_name: Option<&str>) -> bool {
    matches!(
        error_name,
        Some(
            "org.freedesktop.login1.NoSession"
                | "org.freedesktop.DBus.Error.Disconnected"
                | "org.freedesktop.DBus.Error.NameHasNoOwner"
                | "org.freedesktop.DBus.Error.ServiceUnknown"
        )
    )
}

fn take_device_reply_is_usable(body: &[Value], fd_count: usize) -> bool {
    fd_count > 0 && matches!(body, [Value::UnixFD(_), Value::Boolean(_)])
}

fn session_method(session_path: &str, member: &str) -> Message {
    make_method_call(MethodCallParams {
        destination: "org.freedesktop.login1".into(),
        object_path: session_path.into(),
        interface: "org.freedesktop.login1.Session".into(),
        member: member.into(),
    })
}

#[cfg(test)]
fn session_release_members(held: &[libc::dev_t], release_control: bool) -> Vec<&'static str> {
    let mut members = Vec::with_capacity(held.len() + usize::from(release_control));
    members.extend(std::iter::repeat_n("ReleaseDevice", held.len()));
    if release_control {
        members.push("ReleaseControl");
    }
    members
}

#[cfg(test)]
fn deliver_session_release(
    members: &[&str],
    mut send: impl FnMut(&str) -> Result<(), ()>,
) -> usize {
    let mut attempts = 0;
    for member in members {
        attempts += 1;
        if let Err(()) = send(member) {
            log::warn!("session call {member} failed");
        }
    }
    attempts
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
) -> Result<(DbusSerial, bool), DbusError> {
    if crate::utils::is_x11() {
        // Don't take control on X11
        return Ok((DbusSerial(4), false));
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

    Ok((DbusSerial(5), true))
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
    let (current_serial, took_control) = take_session_control(&mut socket, &session_path)?;

    let socket = socket.into_inner();
    let pending = BTreeMap::default();
    Ok(Dbus {
        socket,
        session_path,
        seat,
        current_serial,
        pending,
        held_devices: BTreeSet::new(),
        took_control,
        released_control: false,
    })
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

    fn handle_message(
        &mut self,
        state: &mut State,
        msg: Message,
        mut fds: Vec<OwnedFd>,
        dispatcher: &mut Dispatcher,
    ) {
        use {dbus_message_parser::message::MessageType::*, Value::*};

        match msg.get_type() {
            MethodCall => self.reply_unknown_method(msg),
            MethodReturn => {
                let Some(reply_serial) = msg.get_reply_serial().map(DbusSerial) else {
                    log::warn!("dbus method return has no reply serial");
                    return;
                };
                let Some(pending) = self.pending.remove(&reply_serial) else {
                    return;
                };
                match pending {
                    Pending::TakeDevice { devnum } => {
                        self.accept_take_device(devnum, &msg, &mut fds, dispatcher);
                    }
                    Pending::SubscribeSessionSignals => {}
                    Pending::GetSessionActive => {
                        if let [Variant(value)] = msg.get_body() {
                            if let Boolean(active) = value.as_ref() {
                                self.dispatch_session_active(state, *active, dispatcher);
                            } else {
                                log::warn!("session Active property is not a boolean");
                            }
                        } else {
                            log::warn!("session Active reply is missing its value");
                        }
                    }
                }
            }
            Error => {
                let error_name = msg.get_error_name().map(|name| name.as_ref().to_string());
                let pending = msg
                    .get_reply_serial()
                    .map(DbusSerial)
                    .and_then(|serial| self.pending.remove(&serial));
                if session_ownership_lost(error_name.as_deref()) {
                    dispatcher.request_shutdown(ShutdownReason::Fatal {
                        source: "dbus",
                        message: format!(
                            "logind session is gone: {}",
                            error_name.as_deref().unwrap_or("unknown error")
                        ),
                    });
                } else if let Some(Pending::TakeDevice { devnum }) = pending {
                    log::warn!("TakeDevice failed for {devnum}: {error_name:?}");
                } else {
                    log::warn!("dbus error pending={pending:?} name={error_name:?}");
                }
            }
            Signal => self.handle_signal(state, msg, dispatcher),
        }
    }

    fn reply_unknown_method(&mut self, msg: Message) {
        let member = msg.get_member().map(|member| member.as_ref().to_string());
        log::warn!("unexpected dbus method call: {member:?}");
        let Some(reply) = msg.unknown_member() else {
            return;
        };
        let _ = self.write_session_message(reply);
    }

    fn accept_take_device(
        &mut self,
        devnum: libc::dev_t,
        msg: &Message,
        fds: &mut Vec<OwnedFd>,
        dispatcher: &mut Dispatcher,
    ) {
        if !take_device_reply_is_usable(msg.get_body(), fds.len()) {
            log::error!("TakeDevice reply for {devnum} is unusable");
            if !fds.is_empty() {
                // logind handed us a device fd we will not use. Remember the
                // devnum so shutdown still sends ReleaseDevice.
                self.held_devices.insert(devnum);
            }
            fds.clear();
            return;
        }
        let Value::Boolean(inactive) = msg.get_body()[1] else {
            return;
        };
        let fd = fds.remove(0);
        self.held_devices.insert(devnum);
        dispatcher.dispatch(UdevAction::DeviceTaken { fd, inactive, devnum });
    }

    fn handle_signal(&mut self, state: &mut State, msg: Message, dispatcher: &mut Dispatcher) {
        use Value::{Array, Boolean, DictEntry, String, Variant};

        let is_session_signal = msg
            .get_path()
            .is_some_and(|path| path.as_ref() == self.session_path)
            && msg
                .get_interface()
                .is_some_and(|interface| interface.as_ref() == "org.freedesktop.DBus.Properties")
            && msg
                .get_member()
                .is_some_and(|member| member.as_ref() == "PropertiesChanged");
        if !is_session_signal {
            return;
        }
        let [String(changed_interface), Array(changed_properties), _] = msg.get_body() else {
            return;
        };
        if changed_interface != "org.freedesktop.login1.Session" {
            return;
        }
        for property in changed_properties.as_ref() {
            let DictEntry(entry) = property else {
                continue;
            };
            let (String(name), Variant(value)) = entry.as_ref() else {
                continue;
            };
            if name == "Active" {
                if let Boolean(active) = value.as_ref() {
                    self.dispatch_session_active(state, *active, dispatcher);
                }
                return;
            }
        }
    }

    fn dispatch_session_active(
        &self,
        state: &mut State,
        active: bool,
        dispatcher: &mut Dispatcher,
    ) {
        state.session_active = active;
        crate::drm::kick_visual_animation_pump(dispatcher);
        for (output_id, _) in state.outputs.iter() {
            dispatcher.dispatch_to(
                output_scheduler_id(output_id),
                OutputSchedulerAction::SessionChanged { active },
            );
        }
    }

    fn send_message(&mut self, msg: Message, pending: Pending, dispatcher: &mut Dispatcher) {
        let serial = DbusSerial(msg.get_serial());
        let encoded = msg.encode().unwrap();
        dispatcher.dispatch(IoUringAction::Write {
            fd: self.socket.as_raw_fd(),
            offset: 0,
            buffer: Box::from(&encoded[..]),
            dispatch_to: Redispatcher::None,
        });
        self.pending.insert(serial, pending);
    }

    fn subscribe_session_signals(&mut self, dispatcher: &mut Dispatcher) {
        let mut msg = make_method_call(MethodCallParams {
            destination: "org.freedesktop.DBus".into(),
            object_path: "/org/freedesktop/DBus".into(),
            interface: "org.freedesktop.DBus".into(),
            member: "AddMatch".into(),
        });
        msg.set_serial(self.next_serial().as_u32());
        msg.add_value(Value::String(format!(
            "type='signal',sender='org.freedesktop.login1',path='{}',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged',arg0='org.freedesktop.login1.Session'",
            self.session_path
        )));
        self.send_message(msg, Pending::SubscribeSessionSignals, dispatcher);
    }

    fn request_session_active(&mut self, dispatcher: &mut Dispatcher) {
        let mut msg = make_get(GetParams {
            destination: "org.freedesktop.login1".into(),
            object_path: self.session_path.as_str().into(),
            interface: "org.freedesktop.login1.Session".into(),
            property: "Active",
        });
        msg.set_serial(self.next_serial().as_u32());
        self.send_message(msg, Pending::GetSessionActive, dispatcher);
    }

    fn release_device(&mut self, devnum: libc::dev_t) {
        if !self.held_devices.contains(&devnum) {
            return;
        }
        let mut message = session_method(self.session_path.as_str(), "ReleaseDevice");
        let major: u32 = nix::sys::stat::major(devnum).try_into().unwrap_or(0);
        let minor: u32 = nix::sys::stat::minor(devnum).try_into().unwrap_or(0);
        message.add_value(Value::Uint32(major));
        message.add_value(Value::Uint32(minor));
        if self.write_session_message(message).is_ok() {
            self.held_devices.remove(&devnum);
        }
    }

    fn release_control(&mut self) {
        for devnum in self.held_devices.iter().copied().collect::<Vec<_>>() {
            self.release_device(devnum);
        }
        if !self.took_control || self.released_control {
            return;
        }
        let message = session_method(self.session_path.as_str(), "ReleaseControl");
        if self.write_session_message(message).is_ok() {
            self.released_control = true;
        }
    }

    fn write_session_message(&mut self, mut message: Message) -> std::io::Result<()> {
        message.set_serial(self.next_serial().as_u32());
        let bytes = message.encode().map_err(|error| {
            std::io::Error::other(format!("could not encode session release: {error}"))
        })?;
        if let Err(error) = self.socket.write_all(&bytes) {
            log::warn!("session release write failed: {error}");
            return Err(error);
        }
        Ok(())
    }
}

pub enum DbusAction {
    Start,
    TakeDevice { devnum: libc::dev_t },
    ReleaseDevice { devnum: libc::dev_t },
    ReleaseControl,
    MsgReceived { msg: Message, fds: Vec<OwnedFd> },
}

impl Model for Dbus {
    type Action = DbusAction;
    const START: Option<Self::Action> = Some(Self::Action::Start);
    const STOP: Option<Self::Action> = Some(Self::Action::ReleaseControl);

    fn process(
        &mut self,
        state: &mut State,
        action: Self::Action,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        use DbusAction::*;

        match action {
            Start => {
                let fd = self.socket.as_raw_fd();
                dispatcher.start_model(DBUS_READER_ID, DbusReader::new(fd));
                self.subscribe_session_signals(dispatcher);
                self.request_session_active(dispatcher);
            }
            TakeDevice { devnum } => {
                let serial = self.take_device(devnum, dispatcher);
                self.pending.insert(serial, Pending::TakeDevice { devnum });
            }
            ReleaseDevice { devnum } => self.release_device(devnum),
            ReleaseControl => {
                self.release_control();
                dispatcher.stop_model(DBUS_READER_ID);
            }
            MsgReceived { msg, fds } => self.handle_message(state, msg, fds, dispatcher),
        };

        Ok(())
    }
}

/// Responsible for reading dbus socket, parsing messages
/// and sending them to `Dbus`
struct DbusReader {
    fd: RawFd,
    read_buffer: DbusBuffer,
    fds: Vec<OwnedFd>,
    neof: usize,
    recvmsg_op_id: Option<RingId>,
}

model_instance_id!(DbusReaderId => DbusReader);
const DBUS_READER_ID: DbusReaderId = DbusReaderId(0);

#[derive(Debug)]
enum DbusReaderAction {
    Start,
    ReadMsg { buffer: Box<[u8]>, fds: Vec<OwnedFd> },
    Stop,
}

impl Model for DbusReader {
    type Action = DbusReaderAction;
    const START: Option<Self::Action> = Some(Self::Action::Start);
    const STOP: Option<Self::Action> = Some(Self::Action::Stop);

    fn process(
        &mut self,
        _state: &mut State,
        action: Self::Action,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        use DbusReaderAction::*;

        match action {
            Start => self.dispatch_read(dispatcher),
            ReadMsg { buffer, fds } => {
                self.recvmsg_op_id = None;
                if self.is_eof(&buffer) {
                    dispatcher.request_shutdown(ShutdownReason::Fatal {
                        source: "dbus",
                        message: "system bus closed".into(),
                    });
                    self.dispatch_close(dispatcher);
                    return Ok(());
                }
                self.read_buffer.append(&buffer);
                self.fds.extend(fds);
                loop {
                    let msg: &[u8] = match self.read_buffer.get_whole_message() {
                        Some(msg) => msg,
                        None => {
                            self.dispatch_read(dispatcher);
                            return Ok(());
                        }
                    };
                    let raw_fds: Vec<RawFd> = self.fds.iter().map(AsRawFd::as_raw_fd).collect();
                    let (msg, _offset) =
                        match Message::decode_with_fds(msg.to_vec().into(), &raw_fds) {
                            Ok((msg, offset, _)) => (msg, offset),
                            Err(error) => {
                                log::error!("dbus message decode failed: {error:?}");
                                dispatcher.request_shutdown(ShutdownReason::Fatal {
                                    source: "dbus",
                                    message: format!(
                                        "system bus message could not be decoded: {error}"
                                    ),
                                });
                                self.dispatch_close(dispatcher);
                                return Ok(());
                            }
                        };
                    let message_fds = if let Some(nfds) = msg.get_unix_fds() {
                        self.fds.drain(0..nfds as usize).collect()
                    } else {
                        Vec::new()
                    };
                    dispatcher.dispatch(DbusAction::MsgReceived { msg, fds: message_fds });
                    self.read_buffer.consume_msg();
                }
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

/// Store and parse dbus messages
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
        Self { fd, read_buffer, neof, fds, recvmsg_op_id: None }
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

    fn dispatch_read(&mut self, dispatcher: &mut Dispatcher) {
        let recvmsg_op_id = RingId::fresh();
        self.recvmsg_op_id = Some(recvmsg_op_id);
        dispatcher.dispatch(IoUringAction::Recvmsg {
            id: Some(recvmsg_op_id),
            fd: self.fd,
            dispatch_to: Redispatcher::to_instance(DBUS_READER_ID, |result| match result {
                Ok(RecvmsgCompleted { fd: _, buffer, fds }) => {
                    DbusReaderAction::ReadMsg { buffer, fds }
                }
                Err(error) => {
                    log::warn!("dbus reader recvmsg failed: {:?}", error);
                    DbusReaderAction::Stop
                }
            }),
        });
    }

    fn dispatch_close(&self, dispatcher: &mut Dispatcher) {
        dispatcher.stop_model(DBUS_READER_ID);
    }
}

#[cfg(test)]
mod session_release_tests {
    use super::{deliver_session_release, session_release_members};

    #[test]
    fn release_devices_come_before_release_control() {
        assert_eq!(
            session_release_members(&[1, 2], true),
            ["ReleaseDevice", "ReleaseDevice", "ReleaseControl"]
        );
        assert_eq!(session_release_members(&[], false), Vec::<&str>::new());
    }

    #[test]
    fn a_failed_release_does_not_stop_the_remaining_calls() {
        let members = session_release_members(&[1, 2], true);
        let attempts = deliver_session_release(&members, |member| {
            if member == "ReleaseDevice" {
                Err(())
            } else {
                Ok(())
            }
        });
        assert_eq!(attempts, 3);
    }

    #[test]
    fn session_loss_errors_are_fatal_and_device_errors_are_not() {
        use super::session_ownership_lost;

        assert!(session_ownership_lost(Some("org.freedesktop.login1.NoSession")));
        assert!(session_ownership_lost(Some("org.freedesktop.DBus.Error.Disconnected")));
        assert!(session_ownership_lost(Some("org.freedesktop.DBus.Error.NameHasNoOwner")));
        assert!(session_ownership_lost(Some("org.freedesktop.DBus.Error.ServiceUnknown")));
        assert!(!session_ownership_lost(Some("org.freedesktop.DBus.Error.AccessDenied")));
        assert!(!session_ownership_lost(None));
    }

    #[test]
    fn take_device_reply_requires_a_file_descriptor_and_a_boolean() {
        use {super::take_device_reply_is_usable, dbus_message_parser::value::Value};

        assert!(take_device_reply_is_usable(&[Value::UnixFD(7), Value::Boolean(false)], 1));
        assert!(!take_device_reply_is_usable(&[Value::UnixFD(7), Value::Boolean(false)], 0));
        assert!(!take_device_reply_is_usable(&[Value::Boolean(false)], 1));
    }

    #[test]
    fn an_unexpected_method_call_gets_an_unknown_member_reply() {
        use dbus_message_parser::message::Message;

        let call = Message::method_call(
            "org.freedesktop.login1".try_into().unwrap(),
            "/org/freedesktop/login1/session/self".try_into().unwrap(),
            "org.freedesktop.login1.Session".try_into().unwrap(),
            "PauseDevice".try_into().unwrap(),
        );
        let reply = call.unknown_member().expect("method call has a member");
        assert_eq!(
            reply
                .get_error_name()
                .map(|name| name.as_ref().to_string())
                .as_deref(),
            Some("org.freedesktop.DBus.Error.UnknownMember")
        );
    }
}
