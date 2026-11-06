use std::{
    any::Any,
    collections::HashMap,
    os::fd::{AsFd, OwnedFd},
    rc::{Rc, Weak},
};

use khronos_types::gl;

use crate::{
    dmabuf::{DmaBuf, DmaBufId},
    gbm::GbmDevice,
    region::RectI,
    render::{
        FormatSupport, FrameElement, GraphicsApi, OffscreenTarget, RenderCompletion, RenderFrame,
        RenderTarget, Renderer, RendererCapabilities, RendererError, ShmImage, Texture,
    },
    sync::AcquireSync,
    transform::OutputPresentationTransform,
};

use super::{
    context::EglContext, import::GlTexture, offscreen::OffscreenColorTarget, pipeline::GlesDraw,
    resources::EglFrameBuffer,
};

/// GLES 2 renderer for one DRM device.
#[derive(Debug)]
pub struct GlesRenderer {
    context: EglContext,
    capabilities: RendererCapabilities,
    /// Live imports of client dma-bufs. Weak entries die with the last scene item or snapshot.
    dmabuf_textures: HashMap<DmaBufId, Weak<dyn Texture>>,
}

impl GlesRenderer {
    pub fn new(context: EglContext) -> Self {
        let capabilities = capabilities_from(&context);
        Self { context, capabilities, dmabuf_textures: HashMap::new() }
    }

    pub fn from_gbm(gbm: &GbmDevice) -> Result<Self, String> {
        let egl = super::loader::initialized_egl()?;
        Ok(Self::new(EglContext::make(&egl, gbm)?))
    }

    pub(super) fn context(&self) -> &EglContext {
        &self.context
    }

    fn draw(&self, width: i32, height: i32) -> GlesDraw<'_> {
        GlesDraw {
            programs: &self.context.programs,
            device_id: self.context.device_id(),
            width,
            height,
        }
    }
}

fn capabilities_from(context: &EglContext) -> RendererCapabilities {
    let mut fourccs: Vec<_> = context.opengl_formats.keys().copied().collect();
    fourccs.sort_by_key(|fourcc| format!("{fourcc:?}"));
    let sample_formats = fourccs
        .iter()
        .map(|fourcc| FormatSupport {
            fourcc: *fourcc,
            modifiers: context.opengl_formats[fourcc]
                .read_modifiers
                .iter()
                .map(|modifier| modifier.0)
                .collect(),
        })
        .collect();
    let render_formats = fourccs
        .into_iter()
        .map(|fourcc| FormatSupport {
            fourcc,
            modifiers: context.opengl_formats[&fourcc]
                .write_modifiers
                .iter()
                .map(|modifier| modifier.0)
                .collect(),
        })
        .collect();
    RendererCapabilities {
        sample_formats,
        render_formats,
        explicit_sync: context.supports_native_render_fence(),
    }
}

impl RenderTarget for EglFrameBuffer {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn width(&self) -> i32 {
        self.width
    }

    fn height(&self) -> i32 {
        self.height
    }
}

impl RenderTarget for OffscreenColorTarget {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn width(&self) -> i32 {
        OffscreenColorTarget::width(self)
    }

    fn height(&self) -> i32 {
        OffscreenColorTarget::height(self)
    }
}

impl OffscreenTarget for OffscreenColorTarget {}

impl Renderer for GlesRenderer {
    fn api(&self) -> GraphicsApi {
        GraphicsApi::OpenGl
    }

    fn capabilities(&self) -> &RendererCapabilities {
        &self.capabilities
    }

    fn import_dmabuf(&mut self, source: &DmaBuf) -> Result<Rc<dyn Texture>, RendererError> {
        self.dmabuf_textures
            .retain(|_, texture| texture.strong_count() > 0);
        if let Some(texture) = self.dmabuf_textures.get(&source.id).and_then(Weak::upgrade) {
            return Ok(texture);
        }
        self.context.bind();
        let image = self.context.import_dmabuf(source);
        let texture = image
            .into_texture(self.context.device_id())
            .map(|texture| Rc::new(texture) as Rc<dyn Texture>)
            .map_err(|_| RendererError::ImportFailed)?;
        self.dmabuf_textures
            .insert(source.id, Rc::downgrade(&texture));
        Ok(texture)
    }

    fn import_shm(&mut self, image: ShmImage<'_>) -> Result<Rc<dyn Texture>, RendererError> {
        self.context.bind();
        GlTexture::import_shm_bytes(
            image.bytes,
            image.width,
            image.height,
            image.stride,
            image.fourcc,
            self.context.device_id(),
        )
        .map(|texture| Rc::new(texture) as Rc<dyn Texture>)
        .map_err(|_| RendererError::ImportFailed)
    }

    fn upload_rgba(
        &mut self,
        pixels: &[u8],
        width: i32,
        height: i32,
    ) -> Result<Rc<dyn Texture>, RendererError> {
        self.context.bind();
        GlTexture::from_rgba(pixels, width, height, self.context.device_id())
            .map(|texture| Rc::new(texture) as Rc<dyn Texture>)
            .map_err(|_| RendererError::UploadFailed)
    }

    fn import_output_target(
        &mut self,
        source: &DmaBuf,
    ) -> Result<Box<dyn RenderTarget>, RendererError> {
        let gpu_diagnostics = std::env::var_os("TEMA_GPU_DIAGNOSTICS").is_some();
        log::debug!(
            "GLES output import begin: id={:?} size={}x{} format={} modifier={:?} planes={:?}",
            source.id,
            source.width,
            source.height,
            source.format.name,
            source.modifier,
            source
                .planes
                .iter()
                .map(|plane| (plane.offset, plane.stride))
                .collect::<Vec<_>>()
        );
        self.context.bind();
        let image = self.context.import_dmabuf(source);
        log::debug!("GLES output import created EGL image: id={:?}", source.id);
        let mut framebuffer = self.context.make_fb_from_image(&image, source);
        log::debug!("GLES output import created framebuffer: id={:?}", source.id);
        // The renderbuffer already refers to this image. Holding the image keeps
        // that storage alive without also binding it as a texture.
        framebuffer.imported_image = Some(image);
        log::debug!("GLES output import clearing framebuffer: id={:?}", source.id);
        framebuffer.clear();
        if gpu_diagnostics {
            unsafe { gl::Finish() };
            let error = unsafe { gl::GetError() };
            log::debug!(
                "GLES output import blocking clear completed: id={:?} gl_error=0x{:x}",
                source.id,
                error
            );
        } else {
            log::debug!("GLES output import queued clear: id={:?}", source.id);
        }
        Ok(Box::new(framebuffer))
    }

    fn create_offscreen_target(
        &mut self,
        width: i32,
        height: i32,
    ) -> Result<Box<dyn OffscreenTarget>, RendererError> {
        self.context.bind();
        OffscreenColorTarget::new(width, height)
            .map(|target| Box::new(target) as Box<dyn OffscreenTarget>)
            .ok_or(RendererError::TargetFailed)
    }

    fn render(
        &mut self,
        target: &mut dyn RenderTarget,
        frame: RenderFrame<'_>,
    ) -> Result<RenderCompletion, RendererError> {
        self.context.bind();
        if let Some(framebuffer) = target.as_any_mut().downcast_mut::<EglFrameBuffer>() {
            let projection = crate::transform::output_projection(
                framebuffer.width as f32,
                framebuffer.height as f32,
                frame.presentation_transform,
            );
            return Ok(framebuffer.render(
                frame.recipe,
                projection,
                frame.sync_mode,
                frame.damage,
                frame.preserve_contents,
                frame.presentation_transform,
            ));
        }
        let Some(offscreen) = target.as_any().downcast_ref::<OffscreenColorTarget>() else {
            return Err(RendererError::RenderFailed);
        };
        let (reference_width, reference_height) = frame
            .blur_reference
            .unwrap_or((offscreen.width(), offscreen.height()));
        let draw = self.draw(reference_width, reference_height);
        offscreen.render_elements(&draw, &frame.recipe.elements, frame.presentation_transform);
        Ok(RenderCompletion::blocking_fallback())
    }

    fn readback(
        &mut self,
        target: &dyn RenderTarget,
        rect: RectI,
        stride: usize,
        dst: &mut [u8],
    ) -> Result<(), RendererError> {
        self.context.bind();
        let read = if let Some(framebuffer) = target.as_any().downcast_ref::<EglFrameBuffer>() {
            framebuffer.read_top_left(rect.x, rect.y, rect.width, rect.height, stride, dst)
        } else if let Some(offscreen) = target.as_any().downcast_ref::<OffscreenColorTarget>() {
            offscreen.read_rect(rect.x, rect.y, rect.width, rect.height, stride, dst)
        } else {
            false
        };
        if read {
            Ok(())
        } else {
            Err(RendererError::ReadbackFailed)
        }
    }

    fn capture_transparent(
        &mut self,
        elements: &[FrameElement],
        width: i32,
        height: i32,
        blur_reference: (i32, i32),
        presentation_transform: OutputPresentationTransform,
    ) -> Result<Rc<dyn Texture>, RendererError> {
        self.context.bind();
        self.draw(blur_reference.0, blur_reference.1)
            .capture_transparent(elements, width, height, presentation_transform)
            .map(|texture| Rc::new(texture) as Rc<dyn Texture>)
            .ok_or(RendererError::RenderFailed)
    }

    fn make_current(&self) {
        self.context.bind();
    }

    fn wait_fence(&mut self, fence: OwnedFd) -> Result<(), RendererError> {
        self.context.bind();
        self.context
            .wait_on_native_fence(fence)
            .map_err(|_| RendererError::FenceWaitFailed)
    }

    fn wait_acquire(&mut self, acquire: &AcquireSync) -> Result<(), RendererError> {
        let Ok(Some(sync_file)) = acquire.prepare_wait_fd() else {
            return Ok(());
        };
        self.context.bind();
        if let Err(error) = self.context.wait_on_native_fence(sync_file) {
            log::warn!(
                "renderer explicit acquire wait failed, falling back to CPU wait: {:?}",
                error
            );
            if let Ok(Some(sync_file)) = acquire.prepare_wait_fd() {
                if let Err(wait_error) = crate::sync::wait_sync_file_cpu(sync_file.as_fd()) {
                    log::warn!("renderer explicit acquire CPU wait also failed: {:?}", wait_error);
                }
            }
        }
        Ok(())
    }
}
