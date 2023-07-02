#![allow(dead_code, unused_variables)]

mod dbus;
mod egl;
mod io_uring;
mod logger;
pub mod state;
mod udev;
mod xkb;

fn main() {
    use std::any::Any;

    logger::install();

    egl::init();

    let xkb = xkb::Xkb::new();

    log::info!("hello");

    dbus::init();

    udev::init();

    println!("Hello, world! {:?}", std::mem::size_of::<Box<dyn Any>>());
    println!("Hello, world! {:?}", std::mem::size_of::<Box<()>>());
}
