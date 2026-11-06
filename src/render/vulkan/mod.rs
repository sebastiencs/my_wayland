mod allocator;
mod command;
mod descriptor;
mod device;
mod draw;
mod effects;
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

use drm::control::Device;

use crate::{
    dmabuf::{DmaBuf, DmaBufId},
    region::{DamageRegion, RectI},
    render::{
        FrameElement, GraphicsApi, OffscreenTarget, OutputFrameRecipe, RenderCompletion,
        RenderFrame, RenderSyncMode, RenderTarget, Renderer, RendererCapabilities, RendererError,
        ShmImage, Texture,
    },
    sync::AcquireSync,
    transform::OutputPresentationTransform,
};

use {
    device::{PendingFrame, VulkanDevice},
    pipeline::DrawState,
    resources::{OffscreenImage, OutputTarget},
    sync::StashedSemaphore,
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
    pending_acquires: Vec<StashedSemaphore>,
    pending_frames: Vec<PendingFrame>,
}

impl Drop for VulkanRenderer {
    fn drop(&mut self) {
        if self.pending_frames.is_empty() {
            return;
        }
        log::warn!("vulkan renderer dropped with {} in-flight frame(s)", self.pending_frames.len());
        unsafe {
            if let Err(error) = self.device.device.device_wait_idle() {
                log::error!("vulkan wait before dropping in-flight frames failed: {error}");
            }
        }
    }
}

impl VulkanRenderer {
    pub fn from_drm(drm: &impl AsFd) -> Result<Self, String> {
        let device = VulkanDevice::open(drm)?;
        let draw = DrawState::new(&device)?;
        log::info!("vulkan device exports sync-file semaphores for explicit KMS fences");
        Ok(Self {
            device,
            draw,
            dmabuf_textures: HashMap::new(),
            pending_acquires: Vec::new(),
            pending_frames: Vec::new(),
        })
    }

    fn reap(&mut self) {
        let mut pending = std::mem::take(&mut self.pending_frames);
        pending.retain(|frame| !frame.signaled());
        self.pending_frames = pending;
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
        self.reap();
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
        self.reap();
        VulkanTexture::import_shm(&self.device, image)
            .map(|texture| Rc::new(texture) as Rc<dyn Texture>)
    }

    fn upload_rgba(
        &mut self,
        pixels: &[u8],
        width: i32,
        height: i32,
    ) -> Result<Rc<dyn Texture>, RendererError> {
        self.reap();
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
        self.reap();
        let acquires = std::mem::take(&mut self.pending_acquires);
        if let Some(target) = target.as_any().downcast_ref::<OutputTarget>() {
            return draw::paint(
                &self.device,
                &mut self.draw,
                target,
                frame,
                &mut self.pending_frames,
                acquires,
            );
        }
        if let Some(target) = target.as_any().downcast_ref::<OffscreenImage>() {
            return draw::paint(
                &self.device,
                &mut self.draw,
                target,
                frame,
                &mut self.pending_frames,
                acquires,
            );
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
        self.reap();
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
        elements: &[FrameElement],
        width: i32,
        height: i32,
        blur_reference: (i32, i32),
        presentation_transform: OutputPresentationTransform,
    ) -> Result<Rc<dyn Texture>, RendererError> {
        if width <= 0 || height <= 0 {
            log::error!("vulkan transparent capture has no positive size");
            return Err(RendererError::RenderFailed);
        }
        self.reap();
        let acquires = std::mem::take(&mut self.pending_acquires);
        let bounds = RectI::new(0, 0, width, height).ok_or(RendererError::RenderFailed)?;
        let target = OffscreenImage::new(&self.device, width, height)?;
        let recipe = OutputFrameRecipe { elements: elements.to_vec() };
        let damage = DamageRegion::from_rect(bounds);
        draw::paint(
            &self.device,
            &mut self.draw,
            &target,
            RenderFrame {
                recipe: &recipe,
                damage: &damage,
                preserve_contents: false,
                presentation_transform,
                sync_mode: RenderSyncMode::BlockingFinishFallback,
                blur_reference: Some(blur_reference),
                clear_color: None,
            },
            &mut self.pending_frames,
            acquires,
        )?;
        VulkanTexture::copy_from(&self.device, &target)
            .map(|texture| Rc::new(texture) as Rc<dyn Texture>)
    }

    fn wait_acquire(&mut self, acquire: &AcquireSync) -> Result<(), RendererError> {
        let sync_file = match acquire.prepare_wait_fd() {
            Ok(None) => return Ok(()),
            Ok(Some(sync_file)) => sync_file,
            Err(error) => {
                log::warn!("vulkan acquire sync-file export failed: {error}");
                return wait_acquire_on_cpu(acquire);
            }
        };
        match sync::import_sync_file(&self.device, sync_file) {
            Ok(semaphore) => {
                self.pending_acquires.push(semaphore);
                Ok(())
            }
            Err(error) => {
                log::warn!("vulkan acquire sync-file import failed: {error}");
                // Import closes the sync file. Export another and wait on the CPU.
                match acquire.prepare_wait_fd() {
                    Ok(Some(sync_file)) => crate::sync::wait_sync_file_cpu(sync_file.as_fd())
                        .map_err(|error| {
                            log::warn!("vulkan acquire CPU wait failed: {error}");
                            RendererError::FenceWaitFailed
                        }),
                    Ok(None) => Ok(()),
                    Err(error) => {
                        log::warn!("vulkan acquire sync-file export failed: {error}");
                        wait_acquire_on_cpu(acquire)
                    }
                }
            }
        }
    }

    fn make_current(&self) {}

    fn wait_fence(&mut self, fence: std::os::fd::OwnedFd) -> Result<(), RendererError> {
        crate::sync::wait_sync_file_cpu(fence.as_fd()).map_err(|error| {
            log::warn!("vulkan fence wait failed: {error}");
            RendererError::FenceWaitFailed
        })
    }
}

/// CPU fallback when a Vulkan semaphore cannot represent the acquire.
///
/// A timeline point is waited with `syncobj_timeline_wait`. That ioctl takes an
/// absolute `CLOCK_MONOTONIC` deadline. One second matches the dma-buf poll
/// used when implicit sync is unavailable.
fn wait_acquire_on_cpu(acquire: &AcquireSync) -> Result<(), RendererError> {
    match acquire {
        AcquireSync::None => Ok(()),
        AcquireSync::SyncFile(fd) => crate::sync::wait_sync_file_cpu(fd.as_fd()).map_err(|error| {
            log::warn!("vulkan acquire CPU wait failed: {error}");
            RendererError::FenceWaitFailed
        }),
        AcquireSync::TimelinePoint { timeline, point } => {
            let deadline = monotonic_deadline_nsec(std::time::Duration::from_secs(1))?;
            timeline
                .drm()
                .syncobj_timeline_wait(&[timeline.handle()], &[*point], deadline, true, true, false)
                .map(|_| ())
                .map_err(|error| {
                    log::warn!("vulkan acquire timeline wait failed: {error}");
                    RendererError::FenceWaitFailed
                })
        }
    }
}

fn monotonic_deadline_nsec(bound: std::time::Duration) -> Result<i64, RendererError> {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) } != 0 {
        log::warn!("vulkan acquire clock read failed: {}", std::io::Error::last_os_error());
        return Err(RendererError::FenceWaitFailed);
    }
    let now_nsec = (now.tv_sec as i64)
        .saturating_mul(1_000_000_000)
        .saturating_add(now.tv_nsec as i64);
    let bound_nsec = i64::try_from(bound.as_nanos()).unwrap_or(i64::MAX);
    Ok(now_nsec.saturating_add(bound_nsec))
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
        render::{create_renderer, OutputFrameRecipe, RenderFrame, RenderSyncMode, Renderer},
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
        let renderer = create_renderer(RendererSelection::Vulkan, &gbm).ok()?;
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
                    clear_color: None,
                },
            )
            .expect("clear the scanout target");
        assert!(completion.used_blocking_fallback);
        assert!(completion.in_fence_fd.is_none());
        let mut pixels = [0_u8; 4];
        setup
            .renderer
            .readback(&*target, RectI::new(0, 0, 1, 1).expect("rect"), 4, &mut pixels)
            .expect("read the cleared scanout target");
        assert_eq!(pixels, [40, 40, 40, 255]);
        drop(target);
    }

    use std::os::fd::{AsFd, AsRawFd, FromRawFd};

    #[repr(C)]
    struct DrmSyncobjHandle {
        handle: u32,
        flags: u32,
        fd: i32,
        pad: u32,
    }

    // drm-rs imports a sync file by passing handle 0, but this kernel installs
    // the fence into an existing syncobj and returns ENOENT for handle 0.
    nix::ioctl_readwrite!(drm_syncobj_fd_to_handle, b'd', 0xC2, DrmSyncobjHandle);

    use {
        super::{device::BlockedSyncFile, VulkanRenderer},
        drm::control::Device,
    };

    use crate::{
        render::{
            FrameElement, FramePhase, OffscreenTarget, ShmImage, TextureElement, TextureMask,
            TextureSource,
        },
        sync::{AcquireSync, SyncTimeline},
        transform::Mat3,
    };

    fn vulkan_device() -> Option<(DrmFd, VulkanRenderer)> {
        let fd = open_render_node()?;
        let drm = DrmFd::from_owned(fd);
        let renderer = VulkanRenderer::from_drm(&drm).ok()?;
        Some((drm, renderer))
    }

    fn sync_file_ready(fd: &OwnedFd) -> bool {
        let mut polled = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        unsafe { libc::poll(&mut polled, 1, 0) > 0 && (polled.revents & libc::POLLIN) != 0 }
    }

    fn render_offscreen(
        renderer: &mut VulkanRenderer,
        elements: Vec<FrameElement>,
        mode: RenderSyncMode,
    ) -> (Box<dyn OffscreenTarget>, crate::render::RenderCompletion) {
        let mut target = renderer
            .create_offscreen_target(1, 1)
            .expect("offscreen target");
        let recipe = OutputFrameRecipe { elements };
        let damage = DamageRegion::from_rect(RectI::new(0, 0, 1, 1).expect("damage"));
        let completion = renderer
            .render(
                &mut *target,
                RenderFrame {
                    recipe: &recipe,
                    damage: &damage,
                    preserve_contents: false,
                    presentation_transform: OutputPresentationTransform::FlipY,
                    sync_mode: mode,
                    blur_reference: None,
                    clear_color: None,
                },
            )
            .expect("render");
        (target, completion)
    }

    fn read_pixel(renderer: &mut VulkanRenderer, target: &dyn OffscreenTarget) -> [u8; 4] {
        let mut pixels = [0_u8; 4];
        renderer
            .readback(target, RectI::new(0, 0, 1, 1).expect("rect"), 4, &mut pixels)
            .expect("readback");
        pixels
    }

    fn green_quad(renderer: &mut VulkanRenderer) -> FrameElement {
        let texture = renderer
            .import_shm(ShmImage {
                bytes: &[0, 255, 0, 255],
                width: 1,
                height: 1,
                stride: 4,
                fourcc: DrmFourcc::Argb8888,
            })
            .expect("green texture");
        let rect = RectI::new(0, 0, 1, 1).expect("rect");
        FrameElement::Texture {
            phase: FramePhase::Content,
            element: TextureElement {
                source: TextureSource::Texture,
                texture,
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                tex_transform: Mat3::identity(),
                damage: DamageRegion::from_rect(rect),
                blend: false,
                alpha: 255,
                mask: TextureMask::None,
            },
        }
    }

    #[test]
    fn vulkan_explicit_sync_exports_a_fence_and_blocking_does_not() {
        let Some((_drm, mut renderer)) = vulkan_device() else {
            eprintln!("skipping vulkan explicit sync: no matching DRM node");
            return;
        };
        let (_blocked_target, blocked) =
            render_offscreen(&mut renderer, Vec::new(), RenderSyncMode::BlockingFinishFallback);
        assert!(blocked.used_blocking_fallback);
        assert!(blocked.in_fence_fd.is_none());
        let (_explicit_target, explicit) =
            render_offscreen(&mut renderer, Vec::new(), RenderSyncMode::ExplicitInFence);
        assert!(!explicit.used_blocking_fallback);
        let fence = explicit.in_fence_fd.expect("explicit sync file");
        renderer
            .wait_fence(fence)
            .expect("wait for the exported fence");
    }

    fn import_sync_file(
        drm: &DrmFd,
        handle: drm::control::syncobj::Handle,
        file: &OwnedFd,
    ) -> Result<(), String> {
        let mut args = DrmSyncobjHandle {
            handle: u32::from(handle),
            flags: 1,
            fd: file.as_raw_fd(),
            pad: 0,
        };
        unsafe { drm_syncobj_fd_to_handle(drm.as_fd().as_raw_fd(), &mut args) }
            .map(|_| ())
            .map_err(|error| format!("import sync file into syncobj: {error}"))
    }

    fn dup_fd(fd: &OwnedFd) -> OwnedFd {
        let raw = nix::unistd::dup(fd.as_raw_fd()).expect("dup sync file");
        unsafe { OwnedFd::from_raw_fd(raw) }
    }

    fn held_green_frame(renderer: &mut VulkanRenderer, quad: FrameElement, release: impl FnOnce()) {
        let (target, completion) =
            render_offscreen(renderer, vec![quad], RenderSyncMode::ExplicitInFence);
        let (other, _other_completion) =
            render_offscreen(renderer, Vec::new(), RenderSyncMode::ExplicitInFence);
        let fence = completion.in_fence_fd.expect("explicit fence");
        assert!(!sync_file_ready(&fence), "the render fence signaled before the acquire");
        release();
        renderer.wait_fence(fence).expect("wait for the acquire");
        assert_eq!(read_pixel(renderer, target.as_ref()), [0, 255, 0, 255]);
        drop(other);
    }

    #[test]
    fn vulkan_submit_waits_for_a_sync_file_and_a_timeline_point() {
        let Some((drm, mut renderer)) = vulkan_device() else {
            eprintln!("skipping vulkan acquire wait: no matching DRM node");
            return;
        };
        let quad = green_quad(&mut renderer);
        let blocked = BlockedSyncFile::open(&renderer.device).expect("unsignaled sync file");
        renderer
            .wait_acquire(&AcquireSync::from_sync_file(dup_fd(blocked.fd())))
            .expect("stash sync file");
        held_green_frame(&mut renderer, quad, || {
            blocked.release().expect("release the sync file");
        });
        drop(blocked);

        let quad = green_quad(&mut renderer);
        let blocked = BlockedSyncFile::open(&renderer.device).expect("timeline sync file");
        let imported = drm.create_syncobj(false).expect("binary syncobj");
        import_sync_file(&drm, imported, blocked.fd()).expect("import the sync file");
        let timeline_handle = drm.create_syncobj(false).expect("timeline syncobj");
        drm.syncobj_timeline_transfer(imported, timeline_handle, 0, 1)
            .expect("move the fence onto a timeline point");
        drm.destroy_syncobj(imported).ok();
        let timeline = SyncTimeline::new(drm.clone(), timeline_handle);
        renderer
            .wait_acquire(&AcquireSync::TimelinePoint { timeline: timeline.clone(), point: 1 })
            .expect("stash timeline point");
        held_green_frame(&mut renderer, quad, || {
            blocked.release().expect("release the timeline point");
        });
    }

    #[test]
    fn vulkan_implicit_sync_tracks_a_sampled_dma_buf() {
        let Some((drm, mut renderer)) = vulkan_device() else {
            eprintln!("skipping vulkan implicit sync: no matching DRM node");
            return;
        };
        let Ok(gbm) = GbmDevice::new(&drm) else {
            eprintln!("skipping vulkan implicit sync: no gbm device");
            return;
        };
        let caps = renderer.capabilities();
        let render = caps
            .render_formats
            .iter()
            .find(|format| format.fourcc == DrmFourcc::Xrgb8888);
        let sample = caps
            .sample_formats
            .iter()
            .find(|format| format.fourcc == DrmFourcc::Xrgb8888);
        let modifiers: Vec<u64> = match (render, sample) {
            (Some(render), Some(sample)) => render
                .modifiers
                .iter()
                .copied()
                .filter(|modifier| sample.modifiers.contains(modifier))
                .collect(),
            _ => Vec::new(),
        };
        let Some(bo) = modifiers.iter().find_map(|modifier| {
            gbm.try_create_bo_with_modifiers(
                8,
                4,
                DrmFourcc::Xrgb8888,
                UseFlags::RENDERING | UseFlags::SCANOUT,
                &[crate::drm::Modifier(*modifier)],
            )
        }) else {
            eprintln!("skipping vulkan implicit sync: GBM allocated no shared buffer");
            return;
        };
        let Some(dmabuf) = bo.dmabuf() else {
            eprintln!("skipping vulkan implicit sync: buffer exported no dma-buf");
            return;
        };
        let fresh =
            crate::dmabuf::export_sync_file(&dmabuf.planes[0].fd, crate::dmabuf::DMA_BUF_SYNC_READ)
                .expect("export a fresh dma-buf");
        assert!(sync_file_ready(&fresh), "a fresh dma-buf export is already signaled");
        drop(fresh);

        let texture = renderer.import_dmabuf(dmabuf).expect("import the dma-buf");
        let blocked = BlockedSyncFile::open(&renderer.device).expect("unsignaled sync file");
        renderer
            .wait_acquire(&AcquireSync::from_sync_file(dup_fd(blocked.fd())))
            .expect("stash the acquire");
        let rect = RectI::new(0, 0, 8, 4).expect("rect");
        let quad = FrameElement::Texture {
            phase: FramePhase::Content,
            element: TextureElement {
                source: TextureSource::Texture,
                texture,
                x: 0,
                y: 0,
                width: 8,
                height: 4,
                tex_transform: Mat3::identity(),
                damage: DamageRegion::from_rect(rect),
                blend: false,
                alpha: 255,
                mask: TextureMask::None,
            },
        };
        let (_target, completion) =
            render_offscreen(&mut renderer, vec![quad], RenderSyncMode::ExplicitInFence);
        let render_fence = completion.in_fence_fd.expect("explicit render fence");
        assert!(!sync_file_ready(&render_fence), "the render fence signaled early");
        let write_fence = crate::dmabuf::export_sync_file(
            &dmabuf.planes[0].fd,
            crate::dmabuf::DMA_BUF_SYNC_WRITE,
        )
        .expect("export the dma-buf write fence");
        assert!(
            !sync_file_ready(&write_fence),
            "the dma-buf became writable before the render finished"
        );
        blocked.release().expect("release the acquire");
        renderer
            .wait_fence(render_fence)
            .expect("wait for the render");
        assert!(
            sync_file_ready(&write_fence),
            "the dma-buf write fence did not follow the render fence"
        );
    }

    #[test]
    fn vulkan_draws_again_after_a_failed_submit() {
        let Some((_drm, mut renderer)) = vulkan_device() else {
            eprintln!("skipping vulkan submit recovery: no matching DRM node");
            return;
        };
        let failed = renderer
            .device
            .submit(crate::render::RendererError::RenderFailed, |_device, _command| {
                Err("injected vulkan submit failure".to_string())
            });
        assert!(failed.is_err());
        let (target, completion) =
            render_offscreen(&mut renderer, Vec::new(), RenderSyncMode::BlockingFinishFallback);
        assert!(completion.used_blocking_fallback);
        assert_eq!(read_pixel(&mut renderer, target.as_ref()), [0, 0, 0, 0]);
    }

    #[test]
    fn vulkan_drop_waits_for_an_in_flight_frame() {
        let Some((drm, mut renderer)) = vulkan_device() else {
            eprintln!("skipping vulkan in-flight drop: no matching DRM node");
            return;
        };
        let (_target, completion) =
            render_offscreen(&mut renderer, Vec::new(), RenderSyncMode::ExplicitInFence);
        assert!(completion.in_fence_fd.is_some());
        drop(completion);
        drop(renderer);
        drop(drm);
    }
}
