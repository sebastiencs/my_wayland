use std::os::fd::{AsFd, AsRawFd};

use crate::{
    config::RendererSelection,
    gbm::GbmDevice,
    render::{gles::GlesRenderer, vulkan::VulkanRenderer, Renderer},
};

/// Build the renderer for one DRM device.
///
/// `drm` is the device node. `gbm` is the allocator created from that node.
/// `auto` tries OpenGL and then Vulkan. An explicit choice that cannot be
/// created fails this device.
pub fn create_renderer(
    selection: RendererSelection,
    drm: &impl AsFd,
    gbm: &GbmDevice,
) -> Result<Box<dyn Renderer>, String> {
    match selection {
        RendererSelection::OpenGl => open_gl(selection, drm, gbm),
        RendererSelection::Vulkan => vulkan(selection, drm),
        RendererSelection::Auto => match open_gl(selection, drm, gbm) {
            Ok(renderer) => Ok(renderer),
            Err(opengl_error) => {
                log::info!("opengl renderer failed ({opengl_error}); trying vulkan");
                vulkan(selection, drm)
                    .map_err(|vulkan_error| format!("{opengl_error}; {vulkan_error}"))
            }
        },
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

fn vulkan(selection: RendererSelection, drm: &impl AsFd) -> Result<Box<dyn Renderer>, String> {
    let renderer = VulkanRenderer::from_drm(drm)?;
    log::info!(
        "using vulkan renderer for selection {selection} on drm fd {}",
        drm.as_fd().as_raw_fd()
    );
    Ok(Box::new(renderer))
}
