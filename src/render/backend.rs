//! Renderer boundary shared by the OpenGL and Vulkan backends.
//!
//! This module defines the contract only. The GLES call sites still draw through
//! [`crate::egl::EglFrameBuffer`] until a later phase implements [`Renderer`].
//! [`crate::render::TextureElement`] still carries a GL texture name; the recipe
//! becomes handle-neutral in the next phase.
//!
//! An acquire fence is waited on separately from dma-buf import so direct scanout
//! can hand that fence to KMS without importing the buffer.

use std::{fmt::Debug, os::fd::OwnedFd, rc::Rc};

use drm::buffer::DrmFourcc;

use crate::{
    dmabuf::DmaBuf,
    region::{DamageRegion, RectI},
    render::{FrameElement, OutputFrameRecipe},
    sync::AcquireSync,
    transform::OutputPresentationTransform,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphicsApi {
    OpenGl,
    Vulkan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderSyncMode {
    ExplicitInFence,
    BlockingFinishFallback,
}

#[derive(Debug)]
pub struct RenderCompletion {
    pub in_fence_fd: Option<OwnedFd>,
    pub used_blocking_fallback: bool,
}

impl RenderCompletion {
    pub(crate) fn explicit(in_fence_fd: OwnedFd) -> Self {
        Self {
            in_fence_fd: Some(in_fence_fd),
            used_blocking_fallback: false,
        }
    }

    pub(crate) fn blocking_fallback() -> Self {
        Self { in_fence_fd: None, used_blocking_fallback: true }
    }
}

/// One DRM format and the modifiers a backend can sample or render.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormatSupport {
    pub fourcc: DrmFourcc,
    pub modifiers: Vec<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RendererCapabilities {
    /// Formats that can be sampled from a client buffer.
    pub sample_formats: Vec<FormatSupport>,
    /// Formats that can be rendered into an output buffer.
    pub render_formats: Vec<FormatSupport>,
    pub explicit_sync: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RendererError {
    #[error("format or modifier is not supported")]
    UnsupportedFormat,
    #[error("buffer import failed")]
    ImportFailed,
    #[error("pixel upload failed")]
    UploadFailed,
    #[error("render target creation failed")]
    TargetFailed,
    #[error("render failed")]
    RenderFailed,
    #[error("readback failed")]
    ReadbackFailed,
    #[error("acquire wait failed")]
    AcquireWaitFailed,
}

/// Backend-owned image. Sampling stays inside the renderer that created it.
pub trait Texture: Debug {}

/// Color target for an output or an offscreen image.
pub trait RenderTarget: Debug {
    fn width(&self) -> i32;
    fn height(&self) -> i32;
}

/// Offscreen color target. It is not a KMS buffer.
pub trait OffscreenTarget: RenderTarget {}

/// Shared-memory image copied into a renderer texture.
///
/// `bytes` uses the layout described by `fourcc` and `stride`. It is not
/// necessarily tightly packed or DRM `ARGB8888`.
pub struct ShmImage<'a> {
    pub bytes: &'a [u8],
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub fourcc: DrmFourcc,
}

/// One composited draw into an existing target.
///
/// The backend builds its projection from the target size and
/// `presentation_transform`. Damage is in top-left target coordinates.
pub struct RenderFrame<'a> {
    pub recipe: &'a OutputFrameRecipe,
    pub damage: &'a DamageRegion,
    pub preserve_contents: bool,
    pub presentation_transform: OutputPresentationTransform,
    pub sync_mode: RenderSyncMode,
}

/// One renderer for one DRM device.
pub trait Renderer: Debug {
    fn api(&self) -> GraphicsApi;

    fn capabilities(&self) -> &RendererCapabilities;

    fn import_dmabuf(&mut self, source: &DmaBuf) -> Result<Rc<dyn Texture>, RendererError>;

    fn import_shm(&mut self, image: ShmImage<'_>) -> Result<Rc<dyn Texture>, RendererError>;

    /// Upload tightly packed RGBA bytes, four bytes per pixel.
    fn upload_rgba(
        &mut self,
        pixels: &[u8],
        width: i32,
        height: i32,
    ) -> Result<Rc<dyn Texture>, RendererError>;

    fn import_output_target(
        &mut self,
        source: &DmaBuf,
    ) -> Result<Box<dyn RenderTarget>, RendererError>;

    fn create_offscreen_target(
        &mut self,
        width: i32,
        height: i32,
    ) -> Result<Box<dyn OffscreenTarget>, RendererError>;

    fn render(
        &mut self,
        target: &mut dyn RenderTarget,
        frame: RenderFrame<'_>,
    ) -> Result<RenderCompletion, RendererError>;

    /// Read top-left `ARGB8888` bytes (`B, G, R, A` in memory) into `dst`.
    fn readback(
        &mut self,
        target: &dyn RenderTarget,
        rect: RectI,
        stride: usize,
        dst: &mut [u8],
    ) -> Result<(), RendererError>;

    /// Draw `elements` into a new transparent texture of `width` by `height`.
    fn capture_transparent(
        &mut self,
        elements: &[FrameElement],
        width: i32,
        height: i32,
        presentation_transform: OutputPresentationTransform,
    ) -> Result<Rc<dyn Texture>, RendererError>;

    /// Wait until `acquire` is readable by this renderer.
    ///
    /// Import does not wait. Direct scanout skips this call and passes the
    /// fence to KMS instead.
    fn wait_acquire(&mut self, acquire: &AcquireSync) -> Result<(), RendererError>;
}

#[cfg(test)]
mod tests {
    use std::{fs::File, os::fd::OwnedFd};

    use super::{RenderCompletion, RenderSyncMode, Renderer, Texture};

    #[test]
    fn renderer_traits_are_object_safe() {
        let _renderer = std::any::TypeId::of::<Box<dyn Renderer>>();
        let _texture = std::any::TypeId::of::<Box<dyn Texture>>();
        let _target = std::any::TypeId::of::<Box<dyn super::RenderTarget>>();
        let _offscreen = std::any::TypeId::of::<Box<dyn super::OffscreenTarget>>();
    }

    #[test]
    fn explicit_completion_owns_the_fence_without_a_blocking_fallback() {
        let completion =
            RenderCompletion::explicit(OwnedFd::from(File::open("/dev/null").unwrap()));

        assert!(completion.in_fence_fd.is_some());
        assert!(!completion.used_blocking_fallback);
        assert_ne!(RenderSyncMode::ExplicitInFence, RenderSyncMode::BlockingFinishFallback);
    }

    #[test]
    fn blocking_completion_has_no_fence() {
        let completion = RenderCompletion::blocking_fallback();

        assert!(completion.in_fence_fd.is_none());
        assert!(completion.used_blocking_fallback);
    }
}
