use std::{
    os::fd::{AsRawFd, RawFd},
    thread::JoinHandle,
    time::Duration,
};

use {
    nix::sys::eventfd::{eventfd, EfdFlags},
    rustbus::{
        connection::Timeout,
        message_builder::MarshalledMessage,
        wire::{unmarshal::traits::Variant, ObjectPath},
        CallBuilder, MessageBuilder, MessageType, RpcConn, Unmarshal,
    },
};

use crate::{
    io_uring::{model::IoUringAction, ring::PollCompleted},
    queue::{self, Consumer, Producer},
    state::{AnyAction, Dispatcher, Model, Redispatcher},
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

fn init() -> (String, String, RpcConn) {
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
        return (session_path, seat, dbus_socket);
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

    let serial = send_call(&mut dbus_socket, TakeControl { session_path: session_path.clone() });
    let response = dbus_socket
        .wait_response(serial, Timeout::Infinite)
        .unwrap();

    if !matches!(response.typ, MessageType::Reply) {
        log::error!(
            "Failed to take control of the session: {:?}",
            response.dynheader.error_name.as_ref()
        );
    }

    (session_path, seat, dbus_socket)
}

pub struct Dbus {
    session_path: String,
    seat: String,
    // dbus: rustbus::RpcConn,
    thread: JoinHandle<()>,
    consumer: Consumer<ThreadEvent>,
    sender: std::sync::mpsc::SyncSender<ThreadMsg>,
    eventfd: RawFd,
    // recv: std::sync::mpsc::Receiver<ThreadMsg>,
    // sender: std::sync::mpsc::SyncSender<ThreadMsg>,
    // recv:
}

pub enum DbusAction {
    Start,
    TakeDevice(Device),
    EventFromThread,
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
            Start => self.start(dispatcher),
            TakeDevice(device) => self.take_device(device, dispatcher),
            EventFromThread => {
                while let Ok(event) = self.consumer.pop() {
                    log::error!("NEW EVENT HERE {:?}", event);
                }
            }
        };
        Ok(())
    }
}

impl From<std::io::Result<PollCompleted>> for DbusAction {
    fn from(_: std::io::Result<PollCompleted>) -> Self {
        DbusAction::EventFromThread
    }
}

impl Dbus {
    pub fn new() -> Box<Self> {
        let (session_path, seat, dbus) = init();

        let eventfd = eventfd(0, EfdFlags::EFD_CLOEXEC | EfdFlags::EFD_NONBLOCK).unwrap();
        let (sender, recv) = std::sync::mpsc::sync_channel(1024);
        let (producer, consumer) = queue::bounded(1024);

        let session_path_thread = session_path.clone();
        let thread = std::thread::spawn(move || {
            thread_loop(eventfd, dbus, session_path_thread, recv, producer);
        });

        Box::new(Self { session_path, seat, thread, consumer, sender, eventfd })
    }

    fn start(&mut self, dispatcher: &mut Dispatcher) {
        dispatcher.dispatch(IoUringAction::Poll {
            fd: self.eventfd,
            dispatch_to: Redispatcher::into::<DbusAction>(),
        });
    }

    fn take_device(&self, device: Device, dispatcher: &mut Dispatcher) {
        self.sender.send(ThreadMsg::TakeDevice { device }).unwrap();
    }
}

#[derive(Debug)]
enum ThreadMsg {
    TakeDevice { device: Device },
    Stop,
}

#[derive(Debug)]
enum ThreadEvent {
    DeviceTaken { fd: RawFd },
}

/// Run in another thread, because the crate `rustbus` doesn't easily allow
/// to read/write messages with `io_uring`.
fn thread_loop(
    eventfd: RawFd,
    mut dbus: rustbus::RpcConn,
    session_path: String,
    recv: std::sync::mpsc::Receiver<ThreadMsg>,
    mut producer: Producer<ThreadEvent>,
) {
    macro_rules! info {
        ($($arg:tt)+) => (log::info!(target: "dbus-thread", $($arg)+))
    }

    const TIMEOUT: Timeout = Timeout::Duration(Duration::from_secs(5));
    let notify_fd = || {
        nix::unistd::write(eventfd, &0u64.to_ne_bytes()).unwrap();
    };

    while let Ok(msg) = recv.recv() {
        match msg {
            ThreadMsg::TakeDevice { device } => {
                let session_path = session_path.clone();
                let major = device.major();
                let minor = device.minor();

                // std::thread::sleep_ms(1000);
                let serial = send_call(&mut dbus, TakeDevice { session_path, major, minor });
                let response = dbus.wait_response(serial, TIMEOUT).unwrap();
                info!("take_device {:?}", device);
                // info!("response {:?}", response);
                // log::info!(target: "dbus-thread", "response {:?}", response);

                use MessageType::*;
                match response.typ {
                    Reply => {
                        let parsed = response.body.parser().get2::<rustbus::wire::UnixFd, bool>();
                        info!("parsed {:?}", parsed);
                    }
                    Invalid | Error => {
                        let err = response.body.parser().get::<&str>();
                        info!("err name={:?} {:?}", response.dynheader.error_name.as_ref(), err);
                    }
                    Signal | Call => unimplemented!(),
                }

                producer.push(ThreadEvent::DeviceTaken { fd: 1 }).unwrap();
                notify_fd();

                // let a: TakeDeviceResponse = TakeDeviceResponse::unmarshal(response);
            }
            ThreadMsg::Stop => return,
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

#[derive(Debug, Unmarshal, rustbus::Signature)]
struct TakeDeviceResponse {
    fd: rustbus::wire::UnixFd,
    inactive: bool,
}
