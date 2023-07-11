#![feature(array_chunks)]
#![allow(dead_code)]

macro_rules! wayland_id {
    ($($name:ident,)+) => ($(
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, derive_more::From, derive_more::Into)]
        pub struct $name(pub(super) u32);
        impl From<WaylandId> for $name {
            fn from(value: WaylandId) -> Self {
                Self(value.0)
            }
        }
        impl From<$name> for WaylandId {
            fn from(value: $name) -> Self {
                Self(value.0)
            }
        }
    )+)
}

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
mod object;
mod protocol;
mod protocol_generated;
pub mod state;
mod udev;
mod wayland_objects;
mod xkb;

fn main() {
    // use std::any::Any;

    logger::install();

    // egl::init();

    // let xkb = xkb::Xkb::new();

    // log::info!("hello");

    // let seat_id = dbus::init();

    // udev::init().unwrap();
    // input::init(seat_id);

    // protocol::gen().unwrap();

    let uring = IoUringState::init(());
    let clients = Clients::init(());

    let mut runner = Runner::new();
    runner.add_model(uring);
    runner.add_model(clients);
    runner.start(ClientsAction::StartListen);

    // println!("Hello, world! {:?}", std::mem::size_of::<Box<dyn Any>>());
    // println!("Hello, world! {:?}", std::mem::size_of::<Box<()>>());
}
