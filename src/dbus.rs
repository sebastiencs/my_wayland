use std::{
    borrow::Cow,
    collections::BTreeMap,
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
        ring::{ReadWriteCompleted, RecvmsgCompleted, RingId},
    },
    runner::{
        isolate_session_fault, session_shutdown, Dispatcher, Model, Redispatcher, SessionFault,
        ShutdownReason,
    },
    scheduler::{output_scheduler_id, OutputSchedulerAction},
    state::State,
    udev::{DevicePauseKind, UdevAction},
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
    devices: BTreeMap<libc::dev_t, DeviceLease>,
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

fn device_number(major: u32, minor: u32) -> libc::dev_t {
    nix::sys::stat::makedev(u64::from(major), u64::from(minor))
}

/// Lifecycle of one device logind has handed to this session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DevicePhase {
    Active,
    Pausing(DevicePauseKind),
    Paused,
    Resuming,
    Gone,
}

struct DeviceLease {
    phase: DevicePhase,
    /// Resume fd that arrived while a pause was still being confirmed.
    pending_resume: Option<OwnedFd>,
}

impl DeviceLease {
    fn active() -> Self {
        Self { phase: DevicePhase::Active, pending_resume: None }
    }
}

fn pause_device_request(body: &[Value]) -> Option<(libc::dev_t, DevicePauseKind)> {
    let [Value::Uint32(major), Value::Uint32(minor), Value::String(kind)] = body else {
        isolate_session_fault(SessionFault::MalformedSignal, "PauseDevice body is malformed");
        return None;
    };
    let kind = match kind.as_str() {
        "pause" => DevicePauseKind::Pause,
        "force" => DevicePauseKind::Force,
        "gone" => DevicePauseKind::Gone,
        other => {
            isolate_session_fault(
                SessionFault::MalformedSignal,
                format!("PauseDevice type {other} is not pause, force, or gone"),
            );
            return None;
        }
    };
    Some((device_number(*major, *minor), kind))
}

fn resume_device_request(body: &[Value], fds: &mut Vec<OwnedFd>) -> Option<(libc::dev_t, OwnedFd)> {
    let (major, minor) = match body {
        [Value::Uint32(major), Value::Uint32(minor), Value::UnixFD(_)]
        | [Value::Uint32(major), Value::Uint32(minor)] => (*major, *minor),
        _ => {
            isolate_session_fault(SessionFault::MalformedSignal, "ResumeDevice body is malformed");
            return None;
        }
    };
    if fds.is_empty() {
        isolate_session_fault(
            SessionFault::MissingResumeFd,
            format!("ResumeDevice for {major}:{minor} has no file descriptor"),
        );
        return None;
    }
    Some((device_number(major, minor), fds.remove(0)))
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
        devices: BTreeMap::new(),
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
                    dispatcher.request_shutdown(session_shutdown(
                        SessionFault::SessionGone,
                        format!(
                            "logind session is gone: {}",
                            error_name.as_deref().unwrap_or("unknown error")
                        ),
                    ));
                } else if let Some(Pending::TakeDevice { devnum }) = pending {
                    log::warn!("TakeDevice failed for {devnum}: {error_name:?}");
                    dispatcher.dispatch(crate::udev::UdevAction::DeviceTakeFailed { devnum });
                } else {
                    log::warn!("dbus error pending={pending:?} name={error_name:?}");
                }
            }
            Signal => self.handle_signal(state, msg, fds, dispatcher),
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
                self.devices.insert(devnum, DeviceLease::active());
            }
            fds.clear();
            return;
        }
        let Value::Boolean(inactive) = msg.get_body()[1] else {
            return;
        };
        let fd = fds.remove(0);
        self.devices.insert(devnum, DeviceLease::active());
        dispatcher.dispatch(UdevAction::DeviceTaken { fd, inactive, devnum });
    }

    fn handle_signal(
        &mut self,
        state: &mut State,
        msg: Message,
        fds: Vec<OwnedFd>,
        dispatcher: &mut Dispatcher,
    ) {
        let member = msg.get_member().map(|member| member.as_ref().to_string());
        match member.as_deref() {
            Some("PropertiesChanged") => self.handle_properties_changed(state, msg, dispatcher),
            Some("PauseDevice") if self.is_login_session_signal(&msg) => {
                self.handle_pause_device(&msg, dispatcher);
            }
            Some("ResumeDevice") if self.is_login_session_signal(&msg) => {
                self.handle_resume_device(&msg, fds, dispatcher);
            }
            _ => {}
        }
    }

    fn is_login_session_signal(&self, msg: &Message) -> bool {
        msg.get_path()
            .is_some_and(|path| path.as_ref() == self.session_path)
            && msg
                .get_interface()
                .is_some_and(|interface| interface.as_ref() == "org.freedesktop.login1.Session")
    }

    fn handle_properties_changed(
        &mut self,
        state: &mut State,
        msg: Message,
        dispatcher: &mut Dispatcher,
    ) {
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
        if dispatcher.shutdown_requested() {
            return;
        }
        state.session_active = active;
        if !active {
            crate::seat::quiesce_for_session_pause(state, dispatcher);
        } else {
            dispatcher.dispatch(UdevAction::SessionActive);
            dispatcher.dispatch(crate::libinput::LibinputAction::SessionActive);
        }
        crate::drm::kick_visual_animation_pump(dispatcher);
        for (output_id, _) in state.outputs.iter() {
            dispatcher.dispatch_to(
                output_scheduler_id(output_id),
                OutputSchedulerAction::SessionChanged { active },
            );
        }
    }

    fn handle_pause_device(&mut self, msg: &Message, dispatcher: &mut Dispatcher) {
        let Some((devnum, kind)) = pause_device_request(msg.get_body()) else {
            return;
        };
        let route = {
            let Some(lease) = self.devices.get_mut(&devnum) else {
                log::warn!("PauseDevice for an untracked device {devnum}");
                return;
            };
            if kind == DevicePauseKind::Gone {
                if lease.phase == DevicePhase::Gone {
                    false
                } else {
                    lease.pending_resume = None;
                    lease.phase = DevicePhase::Gone;
                    true
                }
            } else {
                match lease.phase {
                    DevicePhase::Active | DevicePhase::Resuming => {
                        lease.pending_resume = None;
                        lease.phase = DevicePhase::Pausing(kind);
                        true
                    }
                    DevicePhase::Pausing(_) | DevicePhase::Paused | DevicePhase::Gone => {
                        log::debug!("ignoring duplicate PauseDevice for {devnum}");
                        false
                    }
                }
            }
        };
        if route {
            dispatcher.dispatch(UdevAction::PauseDevice { devnum, kind });
        }
    }

    fn handle_resume_device(
        &mut self,
        msg: &Message,
        mut fds: Vec<OwnedFd>,
        dispatcher: &mut Dispatcher,
    ) {
        let Some((devnum, fd)) = resume_device_request(msg.get_body(), &mut fds) else {
            return;
        };
        if dispatcher.shutdown_requested() {
            log::info!("dropping ResumeDevice for {devnum} because shutdown already started");
            drop(fd);
            return;
        }
        let mut fd = Some(fd);
        let forward = {
            let Some(lease) = self.devices.get_mut(&devnum) else {
                log::warn!("ResumeDevice for an untracked device {devnum}");
                return;
            };
            match lease.phase {
                DevicePhase::Pausing(_) => {
                    lease.pending_resume = fd.take();
                    false
                }
                DevicePhase::Paused | DevicePhase::Resuming => {
                    lease.phase = DevicePhase::Resuming;
                    true
                }
                DevicePhase::Active | DevicePhase::Gone => {
                    log::warn!("ResumeDevice ignored for device {devnum}");
                    false
                }
            }
        };
        if forward {
            if let Some(fd) = fd.take() {
                dispatcher.dispatch(UdevAction::ResumeDevice { devnum, fd });
            }
        }
    }

    fn finish_device_pause(&mut self, devnum: libc::dev_t, dispatcher: &mut Dispatcher) {
        if self.released_control {
            return;
        }
        let confirmed = {
            let Some(lease) = self.devices.get_mut(&devnum) else {
                return;
            };
            let DevicePhase::Pausing(kind) = lease.phase else {
                return;
            };
            let resume = lease.pending_resume.take();
            lease.phase = DevicePhase::Paused;
            (kind == DevicePauseKind::Pause, resume)
        };
        let (acknowledge, pending_resume) = confirmed;
        if acknowledge {
            self.queue_pause_device_complete(devnum, dispatcher);
        }
        if dispatcher.shutdown_requested() {
            log::info!("dropping a stored resume fd for {devnum} because shutdown already started");
            drop(pending_resume);
            return;
        }
        if let Some(fd) = pending_resume {
            if let Some(lease) = self.devices.get_mut(&devnum) {
                lease.phase = DevicePhase::Resuming;
            }
            dispatcher.dispatch(UdevAction::ResumeDevice { devnum, fd });
        }
    }

    fn queue_pause_device_complete(&mut self, devnum: libc::dev_t, dispatcher: &mut Dispatcher) {
        let major = nix::sys::stat::major(devnum);
        let minor = nix::sys::stat::minor(devnum);
        let (Ok(major), Ok(minor)) = (u32::try_from(major), u32::try_from(minor)) else {
            log::warn!("PauseDeviceComplete for {devnum} does not fit logind's major/minor");
            return;
        };
        let mut message = session_method(self.session_path.as_str(), "PauseDeviceComplete");
        message.add_value(Value::Uint32(major));
        message.add_value(Value::Uint32(minor));
        message.set_serial(self.next_serial().as_u32());
        let encoded = match message.encode() {
            Ok(encoded) => encoded,
            Err(error) => {
                log::warn!("could not encode PauseDeviceComplete: {error}");
                return;
            }
        };
        dispatcher.dispatch(IoUringAction::Write {
            fd: self.socket.as_raw_fd(),
            offset: 0,
            buffer: Box::from(&encoded[..]),
            dispatch_to: Redispatcher::to::<Self>(
                move |result: std::io::Result<ReadWriteCompleted>| match result {
                    Ok(_) => DbusAction::PauseAckFinished { devnum, error: None },
                    Err(error) => {
                        DbusAction::PauseAckFinished { devnum, error: Some(error.to_string()) }
                    }
                },
            ),
        });
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

        let mut devices = make_method_call(MethodCallParams {
            destination: "org.freedesktop.DBus".into(),
            object_path: "/org/freedesktop/DBus".into(),
            interface: "org.freedesktop.DBus".into(),
            member: "AddMatch".into(),
        });
        devices.set_serial(self.next_serial().as_u32());
        devices.add_value(Value::String(format!(
            "type='signal',sender='org.freedesktop.login1',path='{}',interface='org.freedesktop.login1.Session'",
            self.session_path
        )));
        self.send_message(devices, Pending::SubscribeSessionSignals, dispatcher);
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
        if !matches!(
            self.devices.get(&devnum).map(|lease| lease.phase),
            Some(
                DevicePhase::Active
                    | DevicePhase::Pausing(_)
                    | DevicePhase::Paused
                    | DevicePhase::Resuming
            )
        ) {
            return;
        }
        let mut message = session_method(self.session_path.as_str(), "ReleaseDevice");
        let major: u32 = nix::sys::stat::major(devnum).try_into().unwrap_or(0);
        let minor: u32 = nix::sys::stat::minor(devnum).try_into().unwrap_or(0);
        message.add_value(Value::Uint32(major));
        message.add_value(Value::Uint32(minor));
        if self.write_session_message(message).is_ok() {
            self.devices.remove(&devnum);
        }
    }

    fn release_control(&mut self) {
        let held: Vec<_> = self
            .devices
            .iter()
            .filter(|(_, lease)| lease.phase != DevicePhase::Gone)
            .map(|(devnum, _)| *devnum)
            .collect();
        for devnum in held {
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
    TakeDevice {
        devnum: libc::dev_t,
    },
    ReleaseDevice {
        devnum: libc::dev_t,
    },
    ReleaseControl,
    MsgReceived {
        msg: Message,
        fds: Vec<OwnedFd>,
    },
    /// The backend has stopped using a device that logind asked us to pause.
    DevicePauseQuiesced {
        devnum: libc::dev_t,
    },
    /// `PauseDeviceComplete` was written, or the write failed and the session continues.
    PauseAckFinished {
        devnum: libc::dev_t,
        error: Option<String>,
    },
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
            DevicePauseQuiesced { devnum } => self.finish_device_pause(devnum, dispatcher),
            PauseAckFinished { devnum, error } => {
                if let Some(error) = error {
                    isolate_session_fault(
                        SessionFault::PauseAckWrite,
                        format!("PauseDeviceComplete for {devnum} failed: {error}"),
                    );
                }
            }
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
                    dispatcher.request_shutdown(session_shutdown(
                        SessionFault::BusClosed,
                        "system bus closed",
                    ));
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
    use {
        super::{deliver_session_release, session_release_members},
        crate::runner::Model,
    };

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
            "Lock".try_into().unwrap(),
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

    fn dbus_pair() -> (super::Dbus, std::os::unix::net::UnixStream) {
        use std::os::unix::net::UnixStream;

        let (socket, peer) = UnixStream::pair().unwrap();
        (
            super::Dbus {
                socket,
                session_path: "/org/freedesktop/login1/session/self".into(),
                seat: "seat0".into(),
                current_serial: super::DbusSerial(1),
                pending: Default::default(),
                devices: Default::default(),
                took_control: true,
                released_control: false,
            },
            peer,
        )
    }

    fn dbus_for_tests() -> super::Dbus {
        dbus_pair().0
    }

    fn hold_device(session: &mut super::Dbus) -> libc::dev_t {
        let devnum = super::device_number(8, 3);
        session.devices.insert(devnum, super::DeviceLease::active());
        devnum
    }

    fn session_signal(member: &str) -> dbus_message_parser::message::Message {
        dbus_message_parser::message::Message::signal(
            "/org/freedesktop/login1/session/self".try_into().unwrap(),
            "org.freedesktop.login1.Session".try_into().unwrap(),
            member.try_into().unwrap(),
        )
    }

    fn pause_signal(kind: &str) -> dbus_message_parser::message::Message {
        use dbus_message_parser::value::Value;

        let mut message = session_signal("PauseDevice");
        message.add_value(Value::Uint32(8));
        message.add_value(Value::Uint32(3));
        message.add_value(Value::String(kind.to_string()));
        message
    }

    fn resume_signal() -> dbus_message_parser::message::Message {
        use dbus_message_parser::value::Value;

        let mut message = session_signal("ResumeDevice");
        message.add_value(Value::Uint32(8));
        message.add_value(Value::Uint32(3));
        message
    }

    fn null_fd() -> std::os::fd::OwnedFd {
        std::fs::File::open("/dev/null").unwrap().into()
    }

    fn pause_routes(dispatcher: &crate::runner::Dispatcher) -> usize {
        use crate::udev::UdevAction;

        dispatcher.test_count_queued_action(|action: &UdevAction| {
            matches!(action, UdevAction::PauseDevice { .. })
        })
    }

    fn pause_complete_bytes(serial: u32, major: u32, minor: u32) -> Vec<u8> {
        use dbus_message_parser::value::Value;

        let mut message =
            super::session_method("/org/freedesktop/login1/session/self", "PauseDeviceComplete");
        message.add_value(Value::Uint32(major));
        message.add_value(Value::Uint32(minor));
        message.set_serial(serial);
        message.encode().unwrap().to_vec()
    }

    fn pause_complete_count(dispatcher: &crate::runner::Dispatcher, serial: u32) -> usize {
        use crate::io_uring::model::IoUringAction;

        let expected = pause_complete_bytes(serial, 8, 3);
        dispatcher.test_count_queued_action(|action: &IoUringAction| {
            let IoUringAction::Write { buffer, .. } = action else {
                return false;
            };
            buffer.as_ref() == expected.as_slice()
        })
    }

    fn deliver_pause(
        session: &mut super::Dbus,
        kind: &str,
        dispatcher: &mut crate::runner::Dispatcher,
    ) {
        use crate::test_support::state_for_tests;

        let mut state = state_for_tests();
        session.handle_message(&mut state, pause_signal(kind), Vec::new(), dispatcher);
    }

    #[test]
    fn pause_is_acknowledged_only_after_the_backend_confirms() {
        use crate::udev::UdevAction;

        let mut session = dbus_for_tests();
        let devnum = hold_device(&mut session);
        let mut state = crate::test_support::state_for_tests();
        let mut dispatcher = crate::runner::Dispatcher::default();
        deliver_pause(&mut session, "pause", &mut dispatcher);
        assert_eq!(pause_routes(&dispatcher), 1);
        assert_eq!(pause_complete_count(&dispatcher, 1), 0);
        assert!(dispatcher.test_has_queued_action(|action: &UdevAction| {
            matches!(
                action,
                UdevAction::PauseDevice { kind: crate::udev::DevicePauseKind::Pause, .. }
            )
        }));

        session
            .process(&mut state, super::DbusAction::DevicePauseQuiesced { devnum }, &mut dispatcher)
            .expect("pause confirmation");
        assert_eq!(pause_complete_count(&dispatcher, 1), 1);
        assert_eq!(session.devices.get(&devnum).unwrap().phase, super::DevicePhase::Paused);

        session
            .process(&mut state, super::DbusAction::DevicePauseQuiesced { devnum }, &mut dispatcher)
            .expect("duplicate confirmation");
        assert_eq!(pause_complete_count(&dispatcher, 1), 1);
    }

    #[test]
    fn a_duplicate_pause_signal_is_not_routed_again() {
        let mut session = dbus_for_tests();
        hold_device(&mut session);
        let mut dispatcher = crate::runner::Dispatcher::default();
        deliver_pause(&mut session, "pause", &mut dispatcher);
        deliver_pause(&mut session, "pause", &mut dispatcher);
        assert_eq!(pause_routes(&dispatcher), 1);
        assert_eq!(pause_complete_count(&dispatcher, 1), 0);
    }

    #[test]
    fn force_and_gone_do_not_send_pause_device_complete() {
        use {crate::udev::UdevAction, std::io::Read};

        let mut session = dbus_for_tests();
        let devnum = hold_device(&mut session);
        let mut state = crate::test_support::state_for_tests();
        let mut dispatcher = crate::runner::Dispatcher::default();
        deliver_pause(&mut session, "force", &mut dispatcher);
        session
            .process(&mut state, super::DbusAction::DevicePauseQuiesced { devnum }, &mut dispatcher)
            .expect("force confirmation");
        assert_eq!(pause_complete_count(&dispatcher, 1), 0);
        assert_eq!(session.devices.get(&devnum).unwrap().phase, super::DevicePhase::Paused);

        let (mut session, mut peer) = dbus_pair();
        peer.set_nonblocking(true).unwrap();
        hold_device(&mut session);
        let mut dispatcher = crate::runner::Dispatcher::default();
        deliver_pause(&mut session, "gone", &mut dispatcher);
        assert!(dispatcher.test_has_queued_action(|action: &UdevAction| {
            matches!(
                action,
                UdevAction::PauseDevice { kind: crate::udev::DevicePauseKind::Gone, .. }
            )
        }));
        assert_eq!(pause_complete_count(&dispatcher, 1), 0);
        session.release_control();
        let mut bytes = [0u8; 4096];
        let read = peer.read(&mut bytes).unwrap();
        let text = String::from_utf8_lossy(&bytes[..read]);
        assert!(!text.contains("ReleaseDevice"));
        assert!(text.contains("ReleaseControl"));
    }

    #[test]
    fn shutdown_still_releases_a_paused_device() {
        use std::io::Read;

        let (mut session, mut peer) = dbus_pair();
        peer.set_nonblocking(true).unwrap();
        let devnum = hold_device(&mut session);
        session.devices.get_mut(&devnum).unwrap().phase = super::DevicePhase::Paused;
        session.release_control();
        let mut bytes = [0u8; 4096];
        let read = peer.read(&mut bytes).unwrap();
        let text = String::from_utf8_lossy(&bytes[..read]);
        assert!(text.contains("ReleaseDevice"));
        assert!(text.contains("ReleaseControl"));
    }

    #[test]
    fn malformed_and_untracked_pause_signals_do_not_change_the_session() {
        let mut session = dbus_for_tests();
        let devnum = hold_device(&mut session);
        let mut state = crate::test_support::state_for_tests();
        let mut dispatcher = crate::runner::Dispatcher::default();
        session.handle_message(
            &mut state,
            session_signal("PauseDevice"),
            Vec::new(),
            &mut dispatcher,
        );
        session.handle_message(&mut state, pause_signal("sleep"), Vec::new(), &mut dispatcher);
        let foreign = dbus_message_parser::message::Message::signal(
            "/org/freedesktop/login1/session/other".try_into().unwrap(),
            "org.freedesktop.login1.Session".try_into().unwrap(),
            "PauseDevice".try_into().unwrap(),
        );
        session.handle_message(&mut state, foreign, Vec::new(), &mut dispatcher);
        assert_eq!(pause_routes(&dispatcher), 0);
        assert!(!dispatcher.shutdown_requested());
        assert_eq!(session.devices.get(&devnum).unwrap().phase, super::DevicePhase::Active);

        let mut untracked = dbus_for_tests();
        let mut dispatcher = crate::runner::Dispatcher::default();
        deliver_pause(&mut untracked, "pause", &mut dispatcher);
        assert_eq!(pause_routes(&dispatcher), 0);
        assert!(untracked.devices.is_empty());
    }

    #[test]
    fn resume_owns_one_fd_and_waits_for_an_outstanding_pause() {
        use {crate::udev::UdevAction, std::os::fd::AsRawFd};

        let mut session = dbus_for_tests();
        let devnum = hold_device(&mut session);
        let mut state = crate::test_support::state_for_tests();
        let mut dispatcher = crate::runner::Dispatcher::default();
        session.handle_message(&mut state, resume_signal(), Vec::new(), &mut dispatcher);
        session.handle_message(&mut state, resume_signal(), vec![null_fd()], &mut dispatcher);
        assert!(!dispatcher.test_has_queued_action(|action: &UdevAction| {
            matches!(action, UdevAction::ResumeDevice { .. })
        }));
        assert_eq!(session.devices.get(&devnum).unwrap().phase, super::DevicePhase::Active);

        deliver_pause(&mut session, "pause", &mut dispatcher);
        let first = null_fd();
        let second = null_fd();
        let second_raw = second.as_raw_fd();
        session.handle_message(&mut state, resume_signal(), vec![first], &mut dispatcher);
        session.handle_message(&mut state, resume_signal(), vec![second], &mut dispatcher);
        assert!(!dispatcher.test_has_queued_action(|action: &UdevAction| {
            matches!(action, UdevAction::ResumeDevice { .. })
        }));

        session
            .process(&mut state, super::DbusAction::DevicePauseQuiesced { devnum }, &mut dispatcher)
            .expect("pause confirmation");
        assert_eq!(pause_complete_count(&dispatcher, 1), 1);
        assert!(dispatcher.test_has_queued_action(|action: &UdevAction| {
            matches!(action, UdevAction::ResumeDevice { fd, .. } if fd.as_raw_fd() == second_raw)
        }));
        assert_eq!(
            dispatcher.test_count_queued_action(|action: &UdevAction| {
                matches!(action, UdevAction::ResumeDevice { .. })
            }),
            1
        );
        assert_eq!(session.devices.get(&devnum).unwrap().phase, super::DevicePhase::Resuming);
    }

    #[test]
    fn startup_subscribes_to_session_device_signals() {
        use crate::io_uring::model::IoUringAction;

        let mut session = dbus_for_tests();
        let mut state = crate::test_support::state_for_tests();
        let mut dispatcher = crate::runner::Dispatcher::default();
        session
            .process(&mut state, super::DbusAction::Start, &mut dispatcher)
            .expect("subscribe");
        let contains = |needle: &[u8]| {
            dispatcher.test_count_queued_action(|action: &IoUringAction| {
                let IoUringAction::Write { buffer, .. } = action else {
                    return false;
                };
                buffer.windows(needle.len()).any(|window| window == needle)
            })
        };
        assert_eq!(contains(b"member='PropertiesChanged'"), 1);
        assert_eq!(contains(b"interface='org.freedesktop.login1.Session'"), 1);
    }

    fn error_reply(serial: u32, name: &str) -> dbus_message_parser::message::Message {
        let mut call = dbus_message_parser::message::Message::method_call(
            "org.freedesktop.login1".try_into().unwrap(),
            "/org/freedesktop/login1/session/self".try_into().unwrap(),
            "org.freedesktop.login1.Session".try_into().unwrap(),
            "TakeDevice".try_into().unwrap(),
        );
        call.set_serial(serial);
        let error_name: dbus_message_parser::value::Error = name.to_string().try_into().unwrap();
        call.error(error_name, "injected".into())
    }

    #[test]
    fn a_lost_session_shuts_down_and_a_device_error_does_not() {
        use crate::{runner::Dispatcher, test_support::state_for_tests, udev::UdevAction};

        use super::{DbusSerial, Pending};

        let mut session = dbus_for_tests();
        session
            .pending
            .insert(DbusSerial(4), Pending::TakeDevice { devnum: 9 });
        let mut state = state_for_tests();
        let mut dispatcher = Dispatcher::default();
        session.handle_message(
            &mut state,
            error_reply(4, "org.freedesktop.login1.NoSession"),
            Vec::new(),
            &mut dispatcher,
        );
        assert!(dispatcher.shutdown_requested());

        let mut session = dbus_for_tests();
        session
            .pending
            .insert(DbusSerial(4), Pending::TakeDevice { devnum: 9 });
        let mut dispatcher = Dispatcher::default();
        session.handle_message(
            &mut state,
            error_reply(4, "org.freedesktop.DBus.Error.AccessDenied"),
            Vec::new(),
            &mut dispatcher,
        );
        assert!(!dispatcher.shutdown_requested());
        assert!(dispatcher.test_has_queued_action(|action: &UdevAction| {
            matches!(action, UdevAction::DeviceTakeFailed { devnum: 9 })
        }));
    }

    #[test]
    fn a_failed_pause_ack_keeps_the_device_paused() {
        let mut session = dbus_for_tests();
        let devnum = hold_device(&mut session);
        session.devices.get_mut(&devnum).unwrap().phase = super::DevicePhase::Paused;
        let mut state = crate::test_support::state_for_tests();
        let mut dispatcher = crate::runner::Dispatcher::default();
        session
            .process(
                &mut state,
                super::DbusAction::PauseAckFinished { devnum, error: Some("broken pipe".into()) },
                &mut dispatcher,
            )
            .expect("ack failure is recoverable");
        assert!(!dispatcher.shutdown_requested());
        assert_eq!(session.devices.get(&devnum).unwrap().phase, super::DevicePhase::Paused);
    }

    #[test]
    fn shutdown_cancels_a_resume_and_still_releases_the_device() {
        use {
            crate::{runner::ShutdownReason, udev::UdevAction},
            std::io::Read,
        };

        let (mut session, mut peer) = dbus_pair();
        peer.set_nonblocking(true).unwrap();
        let devnum = hold_device(&mut session);
        session.devices.get_mut(&devnum).unwrap().phase = super::DevicePhase::Paused;
        let mut state = crate::test_support::state_for_tests();
        let mut dispatcher = crate::runner::Dispatcher::default();
        dispatcher.request_shutdown(ShutdownReason::Signal(libc::SIGTERM));
        session.handle_message(&mut state, resume_signal(), vec![null_fd()], &mut dispatcher);
        assert!(!dispatcher.test_has_queued_action(|action: &UdevAction| {
            matches!(action, UdevAction::ResumeDevice { .. })
        }));
        assert_eq!(session.devices.get(&devnum).unwrap().phase, super::DevicePhase::Paused);

        session.devices.get_mut(&devnum).unwrap().phase =
            super::DevicePhase::Pausing(crate::udev::DevicePauseKind::Pause);
        session.devices.get_mut(&devnum).unwrap().pending_resume = Some(null_fd());
        session
            .process(&mut state, super::DbusAction::DevicePauseQuiesced { devnum }, &mut dispatcher)
            .expect("quiesce during shutdown");
        assert!(!dispatcher.test_has_queued_action(|action: &UdevAction| {
            matches!(action, UdevAction::ResumeDevice { .. })
        }));
        assert_eq!(session.devices.get(&devnum).unwrap().phase, super::DevicePhase::Paused);
        assert!(session
            .devices
            .get(&devnum)
            .unwrap()
            .pending_resume
            .is_none());

        session.devices.get_mut(&devnum).unwrap().phase = super::DevicePhase::Resuming;
        session.release_control();
        let mut bytes = [0u8; 4096];
        let read = peer.read(&mut bytes).unwrap();
        let text = String::from_utf8_lossy(&bytes[..read]);
        let release = text.find("ReleaseDevice").expect("held device is released");
        let control = text
            .find("ReleaseControl")
            .expect("session control is released");
        assert!(release < control);
        assert!(session.devices.is_empty());
    }

    #[test]
    fn shutdown_releases_every_held_phase_before_release_control() {
        use {super::DevicePhase, std::io::Read};

        for phase in [
            DevicePhase::Active,
            DevicePhase::Pausing(crate::udev::DevicePauseKind::Pause),
            DevicePhase::Pausing(crate::udev::DevicePauseKind::Force),
            DevicePhase::Paused,
            DevicePhase::Resuming,
        ] {
            let (mut session, mut peer) = dbus_pair();
            peer.set_nonblocking(true).unwrap();
            let devnum = hold_device(&mut session);
            session.devices.get_mut(&devnum).unwrap().phase = phase;
            session.release_control();
            let mut bytes = [0u8; 4096];
            let read = peer.read(&mut bytes).unwrap();
            let text = String::from_utf8_lossy(&bytes[..read]);
            let release = text.find("ReleaseDevice").expect("held device is released");
            let control = text
                .find("ReleaseControl")
                .expect("session control is released");
            assert!(release < control);
            assert!(session.devices.is_empty());
        }

        let (mut session, mut peer) = dbus_pair();
        peer.set_nonblocking(true).unwrap();
        let devnum = hold_device(&mut session);
        session.devices.get_mut(&devnum).unwrap().phase = DevicePhase::Gone;
        session.release_control();
        let mut bytes = [0u8; 4096];
        let read = peer.read(&mut bytes).unwrap();
        let text = String::from_utf8_lossy(&bytes[..read]);
        assert!(!text.contains("ReleaseDevice"));
        assert!(text.contains("ReleaseControl"));
        assert!(session.devices.contains_key(&devnum));
    }

    #[test]
    fn a_pause_and_resume_keeps_the_output_and_requests_one_frame() {
        use crate::{
            drm::DrmAction,
            libinput::{Libinput, LibinputAction},
            runner::Model,
            scheduler::{
                frame_clock::{ClockDisposition, FrameClockMode},
                OutputScheduler, OutputSchedulerAction,
            },
            state::{BackendOutputRef, DrmId},
            test_support::test_output,
            udev::{DevicePauseKind, Udev, UdevAction},
        };

        let mut session = dbus_for_tests();
        let devnum = hold_device(&mut session);
        let mut udev = Udev::new().expect("udev monitor");
        udev.test_retain_input(devnum, "/dev/input/event0");
        let mut input = Libinput::new();
        let mut state = crate::test_support::state_for_tests();
        let output_id = state.insert_output(test_output(
            BackendOutputRef::new(DrmId::from(0usize)),
            1,
            0,
            0,
            1920,
            1080,
            "lifecycle",
        ));
        let mut scheduler = OutputScheduler::new(DrmId::from(0usize), output_id);
        let mut dispatcher = crate::runner::Dispatcher::default();
        state.keyboard.update_key(30, true);

        deliver_pause(&mut session, "pause", &mut dispatcher);
        assert_eq!(pause_routes(&dispatcher), 1);
        assert_eq!(pause_complete_count(&dispatcher, 1), 0);
        udev.process(
            &mut state,
            UdevAction::PauseDevice { devnum, kind: DevicePauseKind::Pause },
            &mut dispatcher,
        )
        .expect("pause");
        input
            .process(&mut state, LibinputAction::Quiesce { devnum: devnum as u64 }, &mut dispatcher)
            .expect("quiesce input");
        assert!(state.keyboard.pressed_keys().0.is_empty());
        assert!(udev.test_has_retained_input(devnum));
        session
            .process(&mut state, super::DbusAction::DevicePauseQuiesced { devnum }, &mut dispatcher)
            .expect("ack");
        assert_eq!(pause_complete_count(&dispatcher, 1), 1);
        assert_eq!(session.devices.get(&devnum).unwrap().phase, super::DevicePhase::Paused);
        assert_eq!(state.outputs.try_get(output_id).unwrap().name, "lifecycle");

        session.dispatch_session_active(&mut state, false, &mut dispatcher);
        scheduler
            .process(
                &mut state,
                OutputSchedulerAction::SessionChanged { active: false },
                &mut dispatcher,
            )
            .unwrap();
        let paused = state
            .frame_scheduler
            .get(&output_id)
            .expect("scheduler snapshot");
        assert!(!paused.session_active);
        assert_eq!(paused.clock.mode, FrameClockMode::Passive);

        session.handle_message(&mut state, resume_signal(), vec![null_fd()], &mut dispatcher);
        udev.process(
            &mut state,
            UdevAction::ResumeDevice { devnum, fd: null_fd() },
            &mut dispatcher,
        )
        .expect("store resume");
        assert!(udev.test_has_retained_input(devnum));
        assert!(!dispatcher.test_has_queued_action(|action: &LibinputAction| {
            matches!(action, LibinputAction::OpenInput { .. })
        }));

        session.dispatch_session_active(&mut state, true, &mut dispatcher);
        udev.process(&mut state, UdevAction::SessionActive, &mut dispatcher)
            .expect("resume input");
        scheduler
            .process(
                &mut state,
                OutputSchedulerAction::SessionChanged { active: true },
                &mut dispatcher,
            )
            .unwrap();
        assert_eq!(render_frames(&dispatcher), 0);
        scheduler
            .process(&mut state, OutputSchedulerAction::Restored, &mut dispatcher)
            .unwrap();
        assert_eq!(render_frames(&dispatcher), 1);
        assert!(dispatcher.test_has_queued_action(|action: &DrmAction| {
            matches!(
                action,
                DrmAction::RenderFrame { decision }
                    if decision.mode == FrameClockMode::Fixed
                        && decision.disposition == ClockDisposition::RenderNow
            )
        }));
        assert_eq!(
            dispatcher.test_count_queued_action(|action: &LibinputAction| {
                matches!(action, LibinputAction::OpenInput { .. })
            }),
            1
        );
        assert_eq!(state.outputs.try_get(output_id).unwrap().name, "lifecycle");

        session.dispatch_session_active(&mut state, true, &mut dispatcher);
        udev.process(&mut state, UdevAction::SessionActive, &mut dispatcher)
            .expect("duplicate active");
        scheduler
            .process(
                &mut state,
                OutputSchedulerAction::SessionChanged { active: true },
                &mut dispatcher,
            )
            .unwrap();
        scheduler
            .process(&mut state, OutputSchedulerAction::Restored, &mut dispatcher)
            .unwrap();
        assert_eq!(render_frames(&dispatcher), 1);
        assert_eq!(
            dispatcher.test_count_queued_action(|action: &LibinputAction| {
                matches!(action, LibinputAction::OpenInput { .. })
            }),
            1
        );
    }

    #[test]
    fn a_failed_input_restore_releases_only_that_device() {
        use crate::{
            libinput::{Libinput, LibinputAction},
            runner::Model,
            state::{BackendOutputRef, DrmId},
            test_support::test_output,
            udev::{Udev, UdevAction},
        };

        let mut udev = Udev::new().expect("udev monitor");
        let devnum = super::device_number(8, 3);
        udev.test_retain_input(devnum, "/dev/null");
        let mut input = Libinput::new();
        let mut state = crate::test_support::state_for_tests();
        let output_id = state.insert_output(test_output(
            BackendOutputRef::new(DrmId::from(0usize)),
            1,
            0,
            0,
            800,
            600,
            "kept",
        ));
        let mut dispatcher = crate::runner::Dispatcher::default();
        udev.process(
            &mut state,
            UdevAction::ResumeDevice { devnum, fd: null_fd() },
            &mut dispatcher,
        )
        .expect("queue the failed open");
        input
            .process(&mut state, LibinputAction::Quiesce { devnum: devnum as u64 }, &mut dispatcher)
            .expect("quiesce before the failed open");
        input
            .process(
                &mut state,
                LibinputAction::OpenInput {
                    devnode: "/dev/null".into(),
                    fd: null_fd(),
                    devnum: devnum as u64,
                },
                &mut dispatcher,
            )
            .expect("isolated input failure");
        assert!(!dispatcher.shutdown_requested());
        assert!(dispatcher.test_has_queued_action(|action: &super::DbusAction| {
            matches!(action, super::DbusAction::ReleaseDevice { .. })
        }));
        assert!(dispatcher.test_has_queued_action(|action: &UdevAction| {
            matches!(action, UdevAction::InputLost { .. })
        }));
        udev.process(&mut state, UdevAction::InputLost { devnum }, &mut dispatcher)
            .expect("drop the failed input");
        assert!(!udev.test_has_retained_input(devnum));
        assert_eq!(state.outputs.try_get(output_id).unwrap().name, "kept");
    }

    fn render_frames(dispatcher: &crate::runner::Dispatcher) -> usize {
        use crate::drm::DrmAction;

        dispatcher.test_count_queued_action(|action: &DrmAction| {
            matches!(action, DrmAction::RenderFrame { .. })
        })
    }
}
