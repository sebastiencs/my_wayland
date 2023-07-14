#[allow(clippy::module_inception)]
mod queue;
mod sync;

pub use queue::{bounded, Consumer, Producer};
