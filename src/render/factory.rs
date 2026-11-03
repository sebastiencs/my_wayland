use std::os::fd::{AsFd, AsRawFd};

use crate::{
    config::RendererSelection,
    gbm::GbmDevice,
    render::{gles::GlesRenderer, Renderer},
};

/// Build the renderer for one DRM device.
///
/// `drm` is the device node. `gbm` is the allocator created from that node.
/// `auto` tries OpenGL. Vulkan is not implemented, so an explicit Vulkan choice
/// fails this device and an OpenGL failure has no second backend to try.
pub fn create_renderer(
    selection: RendererSelection,
    drm: &impl AsFd,
    gbm: &GbmDevice,
) -> Result<Box<dyn Renderer>, String> {
    match selection {
        RendererSelection::Vulkan => Err(vulkan_unavailable()),
        RendererSelection::OpenGl => open_gl(selection, drm, gbm),
        RendererSelection::Auto => open_gl(selection, drm, gbm)
            .map_err(|error| format!("{error}; vulkan fallback is not implemented")),
    }
}

fn open_gl(
    selection: RendererSelection,
    drm: &impl AsFd,
    gbm: &GbmDevice,
) -> Result<Box<dyn Renderer>, String> {
    let renderer = GlesRenderer::from_gbm(gbm)?;
    log::info!(
        "using opengl renderer for selection {selection} on drm fd {}",
        drm.as_fd().as_raw_fd()
    );
    Ok(Box::new(renderer))
}

fn vulkan_unavailable() -> String {
    "vulkan renderer is not implemented".to_string()
}
