mod allocator;
mod command;
mod descriptor;
mod device;
mod draw;
mod format;
mod instance;
mod memory;
mod pipeline;
mod readback;
mod resources;
mod sync;
mod texture;

use std::{
    collections::HashMap,
    fmt,
    os::fd::AsFd,
    rc::{Rc, Weak},
};

use crate::{
    dmabuf::{DmaBuf, DmaBufId},
    region::RectI,
    render::{
        FrameElement, GraphicsApi, OffscreenTarget, RenderCompletion, RenderFrame, RenderTarget,
        Renderer, RendererCapabilities, RendererError, ShmImage, Texture,
    },
    sync::AcquireSync,
    transform::OutputPresentationTransform,
};

use {
    device::VulkanDevice,
    pipeline::DrawState,
    resources::{OffscreenImage, OutputTarget},
    texture::VulkanTexture,
};

pub(super) mod compiled_shaders {
    include!(concat!(env!("OUT_DIR"), "/vulkan_shaders.rs"));
}

/// Vulkan renderer for one DRM device.
pub struct VulkanRenderer {
    device: Rc<VulkanDevice>,
    draw: DrawState,
    dmabuf_textures: HashMap<DmaBufId, Weak<dyn Texture>>,
}

impl VulkanRenderer {
    pub fn from_drm(drm: &impl AsFd) -> Result<Self, String> {
        let device = VulkanDevice::open(drm)?;
        let draw = DrawState::new(&device)?;
        Ok(Self { device, draw, dmabuf_textures: HashMap::new() })
    }
}

impl fmt::Debug for VulkanRenderer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VulkanRenderer")
            .field("api", &GraphicsApi::Vulkan)
            .field("capabilities", &self.device.formats.capabilities)
            .finish()
    }
}

impl Renderer for VulkanRenderer {
    fn api(&self) -> GraphicsApi {
        GraphicsApi::Vulkan
    }

    fn capabilities(&self) -> &RendererCapabilities {
        &self.device.formats.capabilities
    }

    fn import_dmabuf(&mut self, source: &DmaBuf) -> Result<Rc<dyn Texture>, RendererError> {
        self.dmabuf_textures
            .retain(|_, texture| texture.strong_count() > 0);
        if let Some(texture) = self
            .dmabuf_textures
            .get(&source.id)
            .and_then(|texture| texture.upgrade())
        {
            return Ok(texture);
        }
        let texture = VulkanTexture::import_dmabuf(&self.device, source)?;
        let texture: Rc<dyn Texture> = Rc::new(texture);
        self.dmabuf_textures
            .insert(source.id, Rc::downgrade(&texture));
        Ok(texture)
    }

    fn import_shm(&mut self, image: ShmImage<'_>) -> Result<Rc<dyn Texture>, RendererError> {
        VulkanTexture::import_shm(&self.device, image)
            .map(|texture| Rc::new(texture) as Rc<dyn Texture>)
    }

    fn upload_rgba(
        &mut self,
        pixels: &[u8],
        width: i32,
        height: i32,
    ) -> Result<Rc<dyn Texture>, RendererError> {
        VulkanTexture::upload_rgba(&self.device, pixels, width, height)
            .map(|texture| Rc::new(texture) as Rc<dyn Texture>)
    }

    fn import_output_target(
        &mut self,
        source: &DmaBuf,
    ) -> Result<Box<dyn RenderTarget>, RendererError> {
        OutputTarget::import(&self.device, source)
            .map(|target| Box::new(target) as Box<dyn RenderTarget>)
    }

    fn create_offscreen_target(
        &mut self,
        width: i32,
        height: i32,
    ) -> Result<Box<dyn OffscreenTarget>, RendererError> {
        OffscreenImage::new(&self.device, width, height)
            .map(|target| Box::new(target) as Box<dyn OffscreenTarget>)
    }

    fn render(
        &mut self,
        target: &mut dyn RenderTarget,
        frame: RenderFrame<'_>,
    ) -> Result<RenderCompletion, RendererError> {
        if let Some(target) = target.as_any().downcast_ref::<OutputTarget>() {
            return draw::paint(&self.device, &mut self.draw, target, frame);
        }
        if let Some(target) = target.as_any().downcast_ref::<OffscreenImage>() {
            return draw::paint(&self.device, &mut self.draw, target, frame);
        }
        log::error!("vulkan renderer received a target from a different backend");
        Err(RendererError::RenderFailed)
    }

    fn readback(
        &mut self,
        target: &dyn RenderTarget,
        rect: RectI,
        stride: usize,
        dst: &mut [u8],
    ) -> Result<(), RendererError> {
        if let Some(target) = target.as_any().downcast_ref::<OffscreenImage>() {
            return readback::read_color(&self.device, target, rect, stride, dst);
        }
        if let Some(target) = target.as_any().downcast_ref::<OutputTarget>() {
            return readback::read_color(&self.device, target, rect, stride, dst);
        }
        Err(RendererError::ReadbackFailed)
    }

    fn capture_transparent(
        &mut self,
        _elements: &[FrameElement],
        _width: i32,
        _height: i32,
        _blur_reference: (i32, i32),
        _presentation_transform: OutputPresentationTransform,
    ) -> Result<Rc<dyn Texture>, RendererError> {
        Err(RendererError::NotImplemented("vulkan transparent capture"))
    }

    fn wait_acquire(&mut self, _acquire: &AcquireSync) -> Result<(), RendererError> {
        Err(RendererError::NotImplemented("vulkan acquire wait"))
    }

    fn make_current(&self) {}

    fn wait_fence(&mut self, _fence: std::os::fd::OwnedFd) -> Result<(), RendererError> {
        Err(RendererError::NotImplemented("vulkan fence wait"))
    }
}

#[cfg(test)]
mod compare;

#[cfg(test)]
mod tests {
    use std::{fs::OpenOptions, os::fd::OwnedFd, path::PathBuf, rc::Rc};

    use drm::buffer::DrmFourcc;

    use crate::{
        config::RendererSelection,
        drm::DrmFd,
        gbm::{GbmDevice, UseFlags},
        region::{DamageRegion, RectI},
        render::{
            create_renderer, OutputFrameRecipe, RenderFrame, RenderSyncMode, Renderer,
            RendererError,
        },
        transform::OutputPresentationTransform,
    };

    fn open_render_node() -> Option<OwnedFd> {
        let entries = std::fs::read_dir("/dev/dri").ok()?;
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("renderD") || name.starts_with("card"))
            })
            .collect();
        paths.sort_by_key(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            (usize::from(!name.starts_with("renderD")), name.to_string())
        });
        paths.into_iter().find_map(|path| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .ok()
                .map(OwnedFd::from)
        })
    }

    struct Setup {
        renderer: Box<dyn Renderer>,
        gbm: GbmDevice,
    }

    fn setup() -> Option<Setup> {
        let fd = open_render_node()?;
        let drm = DrmFd::from_owned(fd);
        let gbm = GbmDevice::new(&drm).ok()?;
        let renderer = create_renderer(RendererSelection::Vulkan, &gbm, &gbm).ok()?;
        Some(Setup { renderer, gbm })
    }

    #[test]
    fn vulkan_imports_a_scanout_buffer_without_drawing() {
        let Some(mut setup) = setup() else {
            eprintln!("skipping vulkan import: no matching DRM node");
            return;
        };
        assert_eq!(setup.renderer.api(), crate::render::GraphicsApi::Vulkan);
        assert!(setup.renderer.capabilities().explicit_sync);
        assert!(setup
            .renderer
            .capabilities()
            .render_formats
            .iter()
            .any(|format| format.fourcc == DrmFourcc::Xrgb8888));
        let render_modifiers = setup
            .renderer
            .capabilities()
            .render_formats
            .iter()
            .find(|format| format.fourcc == DrmFourcc::Xrgb8888)
            .map(|format| format.modifiers.clone())
            .unwrap_or_default();
        let bo = render_modifiers.iter().find_map(|modifier| {
            setup.gbm.try_create_bo_with_modifiers(
                64,
                64,
                DrmFourcc::Xrgb8888,
                UseFlags::RENDERING | UseFlags::SCANOUT,
                &[crate::drm::Modifier(*modifier)],
            )
        });
        let Some(bo) = bo else {
            eprintln!("skipping vulkan import: GBM allocated no scanout buffer");
            return;
        };
        let dmabuf = bo.dmabuf().expect("scanout dma-buf");
        let sample_modifiers = setup
            .renderer
            .capabilities()
            .sample_formats
            .iter()
            .find(|format| format.fourcc == DrmFourcc::Xrgb8888)
            .map(|format| format.modifiers.clone())
            .unwrap_or_default();
        if let Some(sample) = sample_modifiers.iter().find_map(|modifier| {
            setup.gbm.try_create_bo_with_modifiers(
                64,
                64,
                DrmFourcc::Xrgb8888,
                UseFlags::RENDERING,
                &[crate::drm::Modifier(*modifier)],
            )
        }) {
            let source = sample.dmabuf().expect("sampled dma-buf");
            let first = setup
                .renderer
                .import_dmabuf(source)
                .expect("import a sampled dma-buf");
            let second = setup
                .renderer
                .import_dmabuf(source)
                .expect("reuse the sampled dma-buf");
            assert!(Rc::ptr_eq(&first, &second));
        }
        assert!(
            render_modifiers.contains(&dmabuf.modifier.0),
            "GBM modifier {:#x} planes {} is not a vulkan render modifier",
            dmabuf.modifier.0,
            dmabuf.planes.len()
        );
        let mut target = setup
            .renderer
            .import_output_target(dmabuf)
            .expect("import vulkan output target");
        assert_eq!(target.width(), dmabuf.width);
        assert_eq!(target.height(), dmabuf.height);
        let recipe = OutputFrameRecipe { elements: Vec::new() };
        let damage = DamageRegion::new();
        let completion = setup
            .renderer
            .render(
                &mut *target,
                RenderFrame {
                    recipe: &recipe,
                    damage: &damage,
                    preserve_contents: false,
                    presentation_transform: OutputPresentationTransform::FlipY,
                    sync_mode: RenderSyncMode::BlockingFinishFallback,
                    blur_reference: None,
                },
            )
            .expect("clear the scanout target");
        assert!(completion.used_blocking_fallback);
        assert!(completion.in_fence_fd.is_none());
        let mut pixels = [0_u8; 4];
        let readback = setup.renderer.readback(
            &*target,
            RectI::new(0, 0, 1, 1).expect("rect"),
            4,
            &mut pixels,
        );
        assert!(matches!(readback, Err(RendererError::ReadbackFailed)));
        drop(target);
    }
}
