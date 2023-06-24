#![allow(
    dead_code,
    unused_variables
)]

mod state;

fn main() {
    use std::any::Any;

    println!("Hello, world! {:?}", std::mem::size_of::<Box<dyn Any>>());
    println!("Hello, world! {:?}", std::mem::size_of::<Box<()>>());
}
