use std::{os::fd::AsRawFd, time::Duration};

use rustbus::{
    connection::Timeout,
    message_builder::MarshalledMessage,
    wire::{unmarshal::traits::Variant, ObjectPath},
    CallBuilder, MessageBuilder, MessageType, RpcConn, Unmarshal,
};

use crate::{
    state::{Dispatcher, Model},
    udev::Device,
};

trait IntoMessage {
    const MEMBER: &'static str;
    const INTERFACE: &'static str;
    const DESTINATION: &'static str;

    fn into_message(self) -> MarshalledMessage;

    fn make_call_builder() -> CallBuilder {
        MessageBuilder::new()
            .call(Self::MEMBER)
            .with_interface(Self::INTERFACE)
            .at(Self::DESTINATION)
    }
}

#[derive(Debug)]
struct Login1ManagerGetSession(String);

impl IntoMessage for Login1ManagerGetSession {
    const MEMBER: &'static str = "GetSession";
    const INTERFACE: &'static str = "org.freedesktop.login1.Manager";
    const DESTINATION: &'static str = "org.freedesktop.login1";

    fn into_message(self) -> MarshalledMessage {
        const PATH: &str = "/org/freedesktop/login1";

        let mut msg = Self::make_call_builder().on(PATH).build();
        msg.body.push_param(self.0).unwrap();
        msg
    }
}

fn send_call<T>(socket: &mut RpcConn, method: T) -> u32
where
    T: IntoMessage,
{
    let mut msg = method.into_message();
    // dbg!(&msg);
    socket.send_message(&mut msg).unwrap().write_all().unwrap()
}

#[derive(Debug)]
struct Login1ManagerGetSessionResponse(String);

trait MessageResponse {
    type MarshalType: for<'a> Unmarshal<'a, 'a>;
    fn from_marshal(msg: Self::MarshalType) -> Self;
}

impl MessageResponse for Login1ManagerGetSessionResponse {
    type MarshalType = ObjectPath<String>;

    fn from_marshal(msg: Self::MarshalType) -> Self {
        Self(msg.as_ref().to_string())
    }
}

fn get_reply<T>(socket: &mut RpcConn, serial: u32) -> Result<T, String>
where
    T: MessageResponse,
{
    let response = socket.wait_response(serial, Timeout::Infinite).unwrap();
    // dbg!(&response);

    match response.typ {
        MessageType::Reply => {
            let v = response.body.parser().get::<T::MarshalType>().unwrap();
            Ok(T::from_marshal(v))
        }
        MessageType::Error | MessageType::Invalid => Err(response.dynheader.error_name.unwrap()),
        MessageType::Signal | MessageType::Call => todo!(),
    }
}

pub fn init() -> String {
    let mut dbus_socket = rustbus::RpcConn::system_conn(Timeout::Infinite).unwrap();

    let session_id = std::env::var("XDG_SESSION_ID").expect("XDG_SESSION_ID not set");

    let serial = send_call(&mut dbus_socket, Login1ManagerGetSession(session_id));
    let response = get_reply::<Login1ManagerGetSessionResponse>(&mut dbus_socket, serial);
    dbg!(&response);

    let session_path = response.unwrap().0;

    struct Seat {
        session_path: String,
    }

    impl IntoMessage for Seat {
        const MEMBER: &'static str = "Get";
        const INTERFACE: &'static str = "org.freedesktop.DBus.Properties";
        const DESTINATION: &'static str = "org.freedesktop.login1";

        fn into_message(self) -> MarshalledMessage {
            let mut msg = Self::make_call_builder().on(self.session_path).build();
            msg.body
                .push_param2("org.freedesktop.login1.Session", "Seat")
                .unwrap();
            msg
        }
    }

    let serial = send_call(&mut dbus_socket, Seat { session_path: session_path.clone() });

    let response = dbus_socket
        .wait_response(serial, Timeout::Infinite)
        .unwrap();

    let seat = match response.typ {
        MessageType::Reply => {
            let v = response.body.parser().get::<Variant>().unwrap();
            let v = v.get::<(String, ObjectPath<String>)>().unwrap();
            dbg!(&v);
            v.0
        }
        MessageType::Error | MessageType::Invalid => todo!(),
        MessageType::Signal | MessageType::Call => todo!(),
    };

    if std::env::var("DISPLAY").is_ok() {
        // Don't take control on X11
        return seat;
    }

    struct TakeControl {
        session_path: String,
    }

    impl IntoMessage for TakeControl {
        const MEMBER: &'static str = "TakeControl";
        const INTERFACE: &'static str = "org.freedesktop.login1.Session";
        const DESTINATION: &'static str = "org.freedesktop.login1";

        fn into_message(self) -> MarshalledMessage {
            let mut msg = Self::make_call_builder().on(self.session_path).build();
            msg.body.push_param(false).unwrap();
            msg
        }
    }

    let serial = send_call(&mut dbus_socket, TakeControl { session_path });
    let response = dbus_socket
        .wait_response(serial, Timeout::Infinite)
        .unwrap();

    if !matches!(response.typ, MessageType::Reply) {
        log::error!(
            "Failed to take control of the session: {:?}",
            response.dynheader.error_name.as_ref()
        );
    }

    seat
}

pub struct Dbus {
    // dbus: rustbus::RpcConn,
}

pub enum DbusAction {
    Start,
    TakeDevice(Device),
}

impl Model for Dbus {
    type InitParams = ();
    type Action = DbusAction;

    fn init(params: Self::InitParams) -> Box<Self> {
        todo!()
    }

    fn process(&mut self, action: Self::Action, dispatcher: &mut Dispatcher) -> Result<(), ()> {
        use DbusAction::*;

        match action {
            Start => self.start(),
            TakeDevice(device) => self.take_device(device, dispatcher),
        };
        Ok(())
    }
}

impl Dbus {
    fn start(&self) {
        // let fd = self.dbus.conn().as_raw_fd();
    }

    fn take_device(&self, device: Device, dispatcher: &mut Dispatcher) {}
}

enum ThreadMsg {
    TakeDevice { device: Device },
}

/// Run in another thread, because the crate `rustbus` doesn't easily allow
/// to read/write messages with `io_uring`.
fn thread_loop(
    mut dbus: rustbus::RpcConn,
    session_path: String,
    recv: std::sync::mpsc::Receiver<ThreadMsg>,
    sender: std::sync::mpsc::Sender<ThreadMsg>,
) {
    const TIMEOUT: Timeout = Timeout::Duration(Duration::from_secs(5));

    while let Ok(msg) = recv.recv() {
        match msg {
            ThreadMsg::TakeDevice { device } => {
                let session_path = session_path.clone();
                let major = 0;
                let minor = 0;
                let serial = send_call(&mut dbus, TakeDevice { session_path, major, minor });
                let response = dbus.wait_response(serial, TIMEOUT).unwrap();
                // let a: TakeDeviceResponse = TakeDeviceResponse::unmarshal(response);
            }
        }
    }
}

struct TakeDevice {
    session_path: String,
    major: u32,
    minor: u32,
}

impl IntoMessage for TakeDevice {
    const MEMBER: &'static str = "TakeDevice";
    const INTERFACE: &'static str = "org.freedesktop.login1.Session";
    const DESTINATION: &'static str = "org.freedesktop.login1";

    fn into_message(self) -> MarshalledMessage {
        let mut msg = Self::make_call_builder().on(self.session_path).build();
        msg.body.push_param2(self.major, self.minor).unwrap();
        msg
    }
}

#[derive(Unmarshal, rustbus::Signature)]
struct TakeDeviceResponse {
    fd: rustbus::wire::UnixFd,
    inactive: bool,
}
