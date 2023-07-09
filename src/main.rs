#![feature(array_chunks)]

#![allow(dead_code, unused_variables)]

use crate::{
    clients::{Clients, ClientsAction},
    io_uring::model::IoUringState,
    state::{Model, Runner},
};

mod clients;
mod dbus;
mod egl;
mod input;
mod io_uring;
mod logger;
mod protocol;
pub mod state;
mod udev;
mod xkb;

fn parse_request(bytes: &[u8]) -> std::io::Result<()> {
    if bytes.len() < 8 {
        eprintln!("ERROR len={}", bytes.len());
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    let sender_id = u32::from_ne_bytes(bytes[..4].try_into().unwrap());
    let word = u32::from_ne_bytes(bytes[4..8].try_into().unwrap());

    let msg_size = word >> 16; // upper 16-bits
    let request_opcode = word & 0xFFFF; // lower 16-bits

    dbg!(sender_id, msg_size, request_opcode);

    let payload = &bytes[8..];

    Ok(())
}

fn main() {
    use std::any::Any;

    logger::install();

    // egl::init();

    // let xkb = xkb::Xkb::new();

    // log::info!("hello");

    // let seat_id = dbus::init();

    // udev::init().unwrap();
    // input::init(seat_id);

    // protocol::gen().unwrap();

    // /var/run/user/1000/wayland-1

    // let unix = UnixListener::bind("/var/run/user/1000/wayland-1").unwrap();

    // // accept connections and process them, spawning a new thread for each one
    // for stream in listener.incoming() {
    //     match stream {
    //         Ok(mut stream) => {
    //             println!("connected {:?}", stream);
    //             loop {
    //                 let mut buf = vec![0; 128 * 1024];
    //                 let nread = stream.read(&mut buf).unwrap();

    //                 parse_request(&buf[..nread]);

    //                 break;
    //                 // if nread > 0 {
    //                 //     println!("read={:?}", &buf[..nread]);
    //                 // }
    //             }
    //             /* connection succeeded */
    //             // thread::spawn(|| handle_client(stream));
    //         }
    //         Err(err) => {
    //             /* connection failed */
    //             println!("failed {:?}", err);
    //             break;
    //         }
    //     }
    // }

    let uring = IoUringState::init(());
    let clients = Clients::init(());

    let mut runner = Runner::new();
    runner.add_model(uring);
    runner.add_model(clients);

    // fn on_accept(result: std::io::Result<AcceptCompleted>) -> AnyAction {
    //     eprintln!("on_accept={:?}", result);
    //     AnyAction::empty()
    // }

    // runner.dispatch(IoUringAction::Accept {
    //     fd: unix.as_raw_fd(),
    //     dispatch_to: on_accept,
    // });
    runner.dispatch(ClientsAction::StartListen);
    // runner.dispatch(IoUringAction::WaitForCompletion);
    runner.run();

    println!("Hello, world! {:?}", std::mem::size_of::<Box<dyn Any>>());
    println!("Hello, world! {:?}", std::mem::size_of::<Box<()>>());
}
