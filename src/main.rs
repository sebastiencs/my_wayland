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

use std::{os::fd::AsRawFd, time::Duration};

use {
    io_uring::model::IoUringAction,
    nix::sys::timerfd::{self, TimerFd, TimerFlags, TimerSetTimeFlags},
};

use crate::{
    clients::{Clients, ClientsAction, ClientsInitParams},
    io_uring::{model::IoUringState, ring::PollCompleted},
    state::{AnyAction, Dispatcher, Model, Redispatcher, Runner},
    udev::Udev,
};

mod clients;
mod dbus;
mod dbus2;
mod egl;
mod input;
mod io_uring;
mod logger;
mod object;
mod protocol;
mod protocol_generated;
mod queue;
pub mod state;
mod udev;
mod wayland_objects;
mod xkb;

fn make_timer(duration: Duration) -> TimerFd {
    let timer = timerfd::TimerFd::new(
        timerfd::ClockId::CLOCK_MONOTONIC,
        TimerFlags::TFD_CLOEXEC | TimerFlags::TFD_NONBLOCK,
    )
    .unwrap();
    timer
        .set(timerfd::Expiration::OneShot(duration.into()), TimerSetTimeFlags::empty())
        .unwrap();
    timer
}

fn main() {
    // use std::any::Any;

    // dbus2::init().unwrap();

    // return;

    logger::install();

    // egl::init();

    // let xkb = xkb::Xkb::new();

    // log::info!("hello");

    let dbus = dbus2::Dbus::new().unwrap();

    let udev = Udev::new().unwrap();
    let libinput = input::Libinput::new();

    // protocol::gen().unwrap();

    let uring = IoUringState::init(());
    let clients = Clients::init(ClientsInitParams { libinput });

    let mut runner = Runner::default();
    runner.add_model(uring);
    runner.add_model(dbus);
    runner.add_model(udev);
    runner.add_model(clients);

    let mut dispatcher = Dispatcher::default();
    dispatcher.dispatch(ClientsAction::Start);

    fn on_timeout(_res: std::io::Result<PollCompleted>) -> AnyAction {
        eprintln!("ON_TIMEOUT");
        std::process::abort();
    }

    let timer = make_timer(Duration::from_secs(2));
    dispatcher.dispatch(IoUringAction::Poll {
        fd: timer.as_raw_fd(),
        dispatch_to: Redispatcher::new(on_timeout),
    });

    runner.run(dispatcher);

    // println!("Hello, world! {:?}", std::mem::size_of::<Box<dyn Any>>());
    // println!("Hello, world! {:?}", std::mem::size_of::<Box<()>>());
}
