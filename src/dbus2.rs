use std::{
    borrow::Cow,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
};

use dbus_message_parser::message::Message;

struct Dbus {}

fn auth(buffered_socket: &mut BufReader<UnixStream>) {
    let uid: String = {
        let uid = nix::unistd::getuid().as_raw().to_string();
        uid.as_bytes().iter().map(|b| format!("{:x}", b)).collect()
    };

    buffered_socket
        .get_mut()
        .write_all(format!("\0AUTH EXTERNAL {}\r\n", uid).as_bytes())
        .unwrap();

    let mut s = String::with_capacity(32);
    buffered_socket.read_line(&mut s).unwrap();

    assert!(s.starts_with("OK"));
}

fn negociate_unix_fds(buffered_socket: &mut BufReader<UnixStream>) {
    buffered_socket
        .get_mut()
        .write_all("NEGOTIATE_UNIX_FD\r\n".as_bytes())
        .unwrap();

    let mut s = String::with_capacity(32);
    buffered_socket.read_line(&mut s).unwrap();

    assert!(s.starts_with("AGREE_UNIX_FD"));
}

fn send_begin(buffered_socket: &mut BufReader<UnixStream>) {
    buffered_socket
        .get_mut()
        .write_all("BEGIN\r\n".as_bytes())
        .unwrap();
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

pub fn init() {
    let path = "/run/dbus/system_bus_socket";

    let socket = UnixStream::connect(path).unwrap();
    let mut buffered_socket = BufReader::with_capacity(32, socket);

    auth(&mut buffered_socket);
    negociate_unix_fds(&mut buffered_socket);
    send_begin(&mut buffered_socket);

    let mut msg = make_method_call(MethodCallParams {
        destination: "org.freedesktop.DBus".into(),
        object_path: "/org/freedesktop/DBus".into(),
        interface: "org.freedesktop.DBus".into(),
        member: "Hello".into(),
    });
    msg.set_serial(1);

    // Message::method_call(destination, object_path, interface, member)

    println!("OK");

    // Message::method_call(destination, object_path, interface, member);
}

// fn make_standard_msg(name: &str) -> MarshalledMessage {
//     rustbus::MessageBuilder::new()
//         .call("Hello")
//         .on("/org/freedesktop/DBus")
//         .with_interface("org.freedesktop.DBus")
//         .at("org.freedesktop.DBus")
//         .build()
// }

// /// Sends the obligatory hello message and returns the unique id the daemon assigned this connection
// pub fn send_hello(&mut self, timeout: crate::connection::Timeout) -> super::Result<String> {
//     let start_time = time::Instant::now();

//     let hello = crate::standard_messages::hello();
//     let serial = self
//         .send
//         .send_message(&hello)?
//         .write(super::calc_timeout_left(&start_time, timeout)?)
//         .map_err(|(ctx, e)| {
//             ctx.force_finish();
//             e
//         })?;
//     let resp = self
//         .recv
//         .get_next_message(super::calc_timeout_left(&start_time, timeout)?)?;
//     if resp.dynheader.response_serial != Some(serial) {
//         return Err(super::Error::AuthFailed);
//     }
//     let unique_name = resp.body.parser().get::<String>()?;
//     Ok(unique_name)
// }

// connect(sock, &addr)?;
// let mut stream = unsafe { UnixStream::from_raw_fd(sock) };
// match auth::do_auth(&mut stream)? {
//     auth::AuthResult::Ok => {}
//     auth::AuthResult::Rejected => return Err(Error::AuthFailed),
// }

// if with_unix_fd {
//     match auth::negotiate_unix_fds(&mut stream)? {
//         auth::AuthResult::Ok => {}
//         auth::AuthResult::Rejected => return Err(Error::UnixFdNegotiationFailed),
//     }
// }

// auth::send_begin(&mut stream)?;

#[cfg(test)]
mod tests {
    use std::fmt::{Formatter, Write};

    use super::*;

    #[test]
    fn test_hex_uid() {
        let uid: String = {
            let uid = nix::unistd::getuid().as_raw().to_string();
            uid.as_bytes().iter().map(|b| format!("{:x}", b)).collect()
        };

        dbg!(uid);
    }
}
