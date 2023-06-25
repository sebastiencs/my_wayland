#![allow(dead_code, unused_variables)]

mod io_uring;
mod logger;
pub mod state;

fn main() {
    use std::any::Any;

    logger::install();

    log::info!("hello");

    println!("Hello, world! {:?}", std::mem::size_of::<Box<dyn Any>>());
    println!("Hello, world! {:?}", std::mem::size_of::<Box<()>>());
}
