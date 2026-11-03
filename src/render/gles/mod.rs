mod context;
mod import;
mod loader;
mod offscreen;
mod pipeline;
mod readback;
mod renderer;
mod resources;
mod sync;

#[allow(unused_imports)]
pub use renderer::GlesRenderer;

pub use {
    context::EglContext,
    import::GlTexture,
    loader::{init, EGL},
    resources::EglFrameBuffer,
};

pub(crate) use offscreen::OffscreenColorTarget;

pub use crate::render::{RenderCompletion, TextureMask as RenderItemMask};

pub type RenderItem = crate::render::TextureElement;
