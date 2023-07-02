use rustbus::{connection::Timeout, wire::ObjectPath, Marshal, RpcConn, Unmarshal};

trait Message {
    const MEMBER: &'static str;
    const INTERFACE: &'static str;
    const DESTINATION: &'static str;
    const PATH: &'static str;

    type MarshalType: Marshal;

    fn into_marshal(self) -> Self::MarshalType;
}

trait MessageResponse {
    type MarshalType: for<'a> Unmarshal<'a, 'a>;
    fn from_marshal(msg: Self::MarshalType) -> Self;
}

#[derive(Debug)]
struct Login1ManagerGetSession(String);

#[derive(Debug)]
struct Login1ManagerGetSessionResponse(String);

impl Message for Login1ManagerGetSession {
    const MEMBER: &'static str = "GetSession";
    const INTERFACE: &'static str = "org.freedesktop.login1.Manager";
    const DESTINATION: &'static str = "org.freedesktop.login1";
    const PATH: &'static str = "/org/freedesktop/login1";

    type MarshalType = String;
    fn into_marshal(self) -> Self::MarshalType {
        self.0
    }
}

impl MessageResponse for Login1ManagerGetSessionResponse {
    type MarshalType = ObjectPath<String>;

    fn from_marshal(msg: Self::MarshalType) -> Self {
        Self(msg.as_ref().to_string())
    }
}

fn send_call<T>(socket: &mut RpcConn, method: T) -> u32
where
    T: Message,
{
    let mut msg = rustbus::MessageBuilder::new()
        .call(T::MEMBER)
        .on(T::PATH)
        .with_interface(T::INTERFACE)
        .at(T::DESTINATION)
        .build();

    msg.body.push_param(method.into_marshal()).unwrap();

    socket.send_message(&mut msg).unwrap().write_all().unwrap()
}

fn get_reply<T>(socket: &mut RpcConn, serial: u32) -> Result<T, String>
where
    T: MessageResponse,
{
    use rustbus::MessageType;

    let response = socket.wait_response(serial, Timeout::Infinite).unwrap();

    match response.typ {
        MessageType::Reply => {
            let v = response.body.parser().get::<T::MarshalType>().unwrap();
            Ok(T::from_marshal(v))
        }
        MessageType::Error | MessageType::Invalid => Err(response.dynheader.error_name.unwrap()),
        MessageType::Signal | MessageType::Call => todo!(),
    }
}

pub fn init() {
    let mut dbus_socket = rustbus::RpcConn::system_conn(Timeout::Infinite).unwrap();

    let session_id = std::env::var("XDG_SESSION_ID").unwrap_or("4".to_string());

    let serial = send_call(&mut dbus_socket, Login1ManagerGetSession(session_id));
    let response = get_reply::<Login1ManagerGetSessionResponse>(&mut dbus_socket, serial);

    dbg!(&response);
}
