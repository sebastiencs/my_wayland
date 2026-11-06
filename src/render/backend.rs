//! Renderer boundary shared by the OpenGL and Vulkan backends.
//!
//! GLES implements this contract on [`crate::render::gles::GlesRenderer`]. Callers
//! outside that module draw, import, and read back through [`Renderer`].
//! Recipe textures are [`Texture`] handles. A GLES texture records the DRM device
//! that created it, and that backend rejects a handle from another device.
//!
//! An acquire fence is waited on separately from dma-buf import. Direct scanout
//! passes that fence to KMS and does not wait in the renderer.

use std::{
    any::Any,
    fmt::Debug,
    os::fd::OwnedFd,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use drm::buffer::DrmFourcc;

use crate::{
    dmabuf::DmaBuf,
    region::{DamageRegion, RectI},
    render::{FrameElement, OutputFrameRecipe},
    sync::AcquireSync,
    transform::OutputPresentationTransform,
};

/// Identifies the DRM device whose renderer created a texture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderDeviceId(u64);

impl RenderDeviceId {
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

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
    #[error("fence wait failed")]
    FenceWaitFailed,
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),
}

/// Backend-owned image. Sampling stays inside the renderer that created it.
pub trait Texture: Debug + Any {
    /// Dropping the texture deletes a GPU object, so its device context must be current.
    fn requires_device_context_on_drop(&self) -> bool {
        false
    }
}

/// GPU-free texture for tests that build recipes without rendering.
#[derive(Debug)]
pub struct InertTexture;

impl Texture for InertTexture {}

pub fn inert_texture() -> Rc<dyn Texture> {
    Rc::new(InertTexture)
}

/// Color target for an output or an offscreen image.
pub trait RenderTarget: Debug {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
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
    /// Replaces the target's usual clear. Output frames leave this empty.
    pub clear_color: Option<[f32; 4]>,
    /// Blur padding uses this size instead of the target size.
    ///
    /// Output rendering leaves this empty and measures blur against the target.
    /// Window capture sets it to the output size.
    pub blur_reference: Option<(i32, i32)>,
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
    ///
    /// `blur_reference` is the output size blur padding is measured against.
    fn capture_transparent(
        &mut self,
        elements: &[FrameElement],
        width: i32,
        height: i32,
        blur_reference: (i32, i32),
        presentation_transform: OutputPresentationTransform,
    ) -> Result<Rc<dyn Texture>, RendererError>;

    /// Wait until `acquire` is readable by this renderer.
    ///
    /// Import does not wait. Direct scanout skips this call and passes the
    /// fence to KMS instead.
    fn wait_acquire(&mut self, acquire: &AcquireSync) -> Result<(), RendererError>;

    /// Make this device current so a later drop can delete its GPU resources.
    fn make_current(&self);

    /// Wait for a fence previously returned by [`Renderer::render`].
    fn wait_fence(&mut self, fence: OwnedFd) -> Result<(), RendererError>;
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
