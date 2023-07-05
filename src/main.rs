#![allow(dead_code, unused_variables)]

mod dbus;
mod egl;
mod input;
mod io_uring;
mod logger;
mod protocol;
pub mod state;
mod udev;
mod xkb;

fn main() {
    use std::any::Any;

    logger::install();

    egl::init();

    let xkb = xkb::Xkb::new();

    log::info!("hello");

    let seat_id = dbus::init();

    udev::init().unwrap();
    input::init(seat_id);

    protocol::gen().unwrap();

    println!("Hello, world! {:?}", std::mem::size_of::<Box<dyn Any>>());
    println!("Hello, world! {:?}", std::mem::size_of::<Box<()>>());
}
