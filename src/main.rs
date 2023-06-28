#![allow(dead_code, unused_variables)]

mod egl;
mod io_uring;
mod logger;
pub mod state;
mod xkb;

fn main() {
    use std::any::Any;

    logger::install();

    egl::init();

    let xkb = xkb::Xkb::new();

    log::info!("hello");

    println!("Hello, world! {:?}", std::mem::size_of::<Box<dyn Any>>());
    println!("Hello, world! {:?}", std::mem::size_of::<Box<()>>());
}
