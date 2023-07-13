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
    clients::{Clients, ClientsAction, ClientsInitParams},
    io_uring::model::IoUringState,
    state::{Dispatcher, Model, Runner},
    udev::Udev,
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

    let seat_id = dbus::init();

    dbg!(seat_id.as_str());

    let udev = Udev::new().unwrap();
    let libinput = input::Libinput::new(seat_id);

    // protocol::gen().unwrap();

    let uring = IoUringState::init(());
    let clients = Clients::init(ClientsInitParams { libinput });

    let mut runner = Runner::default();
    runner.add_model(uring);
    runner.add_model(udev);
    runner.add_model(clients);

    let mut dispatcher = Dispatcher::default();
    dispatcher.dispatch(ClientsAction::Start);

    runner.run(dispatcher);

    // println!("Hello, world! {:?}", std::mem::size_of::<Box<dyn Any>>());
    // println!("Hello, world! {:?}", std::mem::size_of::<Box<()>>());
}
