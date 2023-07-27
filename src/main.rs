#![feature(array_chunks, c_variadic)]
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

macro_rules! index_map_id {
    ($name:ident) => {
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name(u32);
        impl From<usize> for $name {
            fn from(value: usize) -> Self {
                Self(value.try_into().unwrap())
            }
        }
        impl From<$name> for usize {
            fn from(value: $name) -> usize {
                value.0 as usize
            }
        }
    };
}

use std::{os::fd::AsRawFd, time::Duration};

use {
    io_uring::model::IoUringAction,
    nix::sys::timerfd::{self, TimerFd, TimerFlags, TimerSetTimeFlags},
};

use state::State;

use crate::{
    clients::{Clients, ClientsAction},
    io_uring::{model::IoUringState, ring::PollCompleted},
    runner::{AnyAction, Dispatcher, Redispatcher, Runner},
    udev::Udev,
};

mod clients;
mod dbus;
mod drm;
mod egl;
mod input;
mod io_uring;
mod logger;
mod object;
mod protocol;
mod protocol_generated;
mod queue;
pub mod runner;
mod state;
mod udev;
mod utils;
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

    let state = State::default();

    let dbus = dbus::Dbus::new().unwrap();

    let udev = Udev::new().unwrap();
    let libinput = input::Libinput::new();

    // protocol::gen().unwrap();

    let uring = Box::<IoUringState>::default();
    let clients = Clients::new();

    let mut runner = Runner::with_state(state);
    runner.add_model(uring);
    runner.add_model(dbus);
    runner.add_model(udev);
    runner.add_model(libinput);
    runner.add_model(clients);

    let mut dispatcher = Dispatcher::default();
    dispatcher.dispatch(ClientsAction::Start);

    let timer = make_timer(Duration::from_secs(2));

    if !utils::is_x11() {
        fn on_timeout(_res: std::io::Result<PollCompleted>) -> AnyAction {
            eprintln!("ON_TIMEOUT");
            std::process::abort();
        }
        dispatcher.dispatch(IoUringAction::Poll {
            fd: timer.as_raw_fd(),
            dispatch_to: Redispatcher::new(on_timeout),
        });
    }

    runner.run(dispatcher);

    // println!("Hello, world! {:?}", std::mem::size_of::<Box<dyn Any>>());
    // println!("Hello, world! {:?}", std::mem::size_of::<Box<()>>());
}
