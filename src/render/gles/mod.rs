mod context;
mod import;
mod loader;
mod offscreen;
mod pipeline;
mod readback;
mod renderer;
mod resources;
mod sync;

pub use {loader::init, renderer::GlesRenderer};

pub use crate::render::{RenderCompletion, TextureMask as RenderItemMask};

#[cfg(test)]
mod smoke;

pub type RenderItem = crate::render::TextureElement;
