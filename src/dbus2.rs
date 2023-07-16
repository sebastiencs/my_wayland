use std::{
    borrow::Cow,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
};

use dbus_message_parser::encode::EncodeError;

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

pub struct Dbus {
    socket: UnixStream,
    session_name: String,
    seat: String
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

fn get_session_name(buffered_socket: &mut BufReader<UnixStream>) -> Result<String, DbusError> {
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

    let Some(Value::ObjectPath(obj_session_name)) = msg.get_body().first() else {
        let body_str = format!("{:?}", msg.get_body());
        return Err(DbusError::SessionNameNotFound(body_str));
    };

    let session_name: String = obj_session_name.clone().into();

    Ok(session_name)
}

fn get_seat(
    buffered_socket: &mut BufReader<UnixStream>,
    session_name: &str,
) -> Result<String, DbusError>  {
    let make_err = |msg: &Message| {
        Err::<String, _>(DbusError::SeatNotFound(format!("{:?}", msg)))
    };

    let mut msg = make_method_call(MethodCallParams {
        destination: "org.freedesktop.login1".into(),
        object_path: session_name.into(),
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
    session_name: &str,
) -> Result<(), DbusError> {
    let mut msg = make_method_call(MethodCallParams {
        destination: "org.freedesktop.login1".into(),
        object_path: session_name.into(),
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

    Ok(())
}

pub fn init() -> Result<Dbus, DbusError> {
    const SYSTEM_PATH: &str = "/run/dbus/system_bus_socket";
    let socket = UnixStream::connect(SYSTEM_PATH)?;
    let mut socket = BufReader::new(socket);

    auth(&mut socket)?;
    negociate_unix_fds(&mut socket)?;
    send_begin(&mut socket)?;
    say_hello(&mut socket)?;

    let session_name = get_session_name(&mut socket)?;
    let seat = get_seat(&mut socket, &session_name)?;
    take_session_control(&mut socket, &session_name)?;

    let socket = socket.into_inner();
    Ok(Dbus { socket, session_name, seat })
}

impl Dbus {
    pub fn new() -> Result<Self, DbusError> {
        init()
    }
}
