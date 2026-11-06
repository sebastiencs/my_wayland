use std::{
    any::Any,
    cell::Cell,
    fmt::{self, Debug},
    os::fd::AsRawFd,
    rc::Rc,
};

use {ash::vk, gpu_alloc::UsageFlags};

use crate::{
    dmabuf::{DmaBuf, DmaBufPlane},
    render::{OffscreenTarget, RenderTarget, RendererError},
};

use super::{allocator::GpuAllocation, device::VulkanDevice, format::vk_format, memory};

pub(super) struct OutputTarget {
    device: Rc<VulkanDevice>,
    image: vk::Image,
    memories: Vec<vk::DeviceMemory>,
    view: vk::ImageView,
    render_pass: vk::RenderPass,
    framebuffer: vk::Framebuffer,
    width: i32,
    height: i32,
    /// `UNDEFINED` before the first successful use. After each release to the
    /// foreign queue it is `GENERAL`.
    layout: Cell<vk::ImageLayout>,
}

impl OutputTarget {
    pub(super) fn import(
        device: &Rc<VulkanDevice>,
        source: &DmaBuf,
    ) -> Result<Self, RendererError> {
        if source.width <= 0 || source.height <= 0 || source.planes.is_empty() {
            log::warn!("vulkan output buffer has no positive size or planes");
            return Err(RendererError::ImportFailed);
        }
        let Some(modifiers) = device.formats.by_fourcc.get(&source.format.drm) else {
            log::warn!("vulkan cannot render {:?}", source.format.drm);
            return Err(RendererError::UnsupportedFormat);
        };
        let Some(modifier) = modifiers.get(&source.modifier.0) else {
            log::warn!(
                "vulkan cannot render {:?} modifier {:#x}",
                source.format.drm,
                source.modifier.0
            );
            return Err(RendererError::UnsupportedFormat);
        };
        let Some(max) = modifier.render_max else {
            log::warn!(
                "vulkan can sample but not render {:?} modifier {:#x}",
                source.format.drm,
                source.modifier.0
            );
            return Err(RendererError::UnsupportedFormat);
        };
        if source.width as u32 > max.width
            || source.height as u32 > max.height
            || modifier.planes as usize != source.planes.len()
        {
            log::warn!(
                "vulkan output buffer does not match modifier {:#x} layout",
                source.modifier.0
            );
            return Err(RendererError::UnsupportedFormat);
        }
        let disjoint = planes_are_disjoint(&source.planes)?;
        if disjoint && !modifier.disjoint {
            log::warn!("vulkan modifier {:#x} does not support disjoint planes", source.modifier.0);
            return Err(RendererError::UnsupportedFormat);
        }
        let vk_format = vk_format(source.format.drm);
        if vk_format == vk::Format::UNDEFINED {
            return Err(RendererError::UnsupportedFormat);
        }

        let layouts: Vec<vk::SubresourceLayout> = source
            .planes
            .iter()
            .map(|plane| vk::SubresourceLayout {
                offset: plane.offset as u64,
                size: 0,
                row_pitch: plane.stride as u64,
                array_pitch: 0,
                depth_pitch: 0,
            })
            .collect();
        let mut modifier_info = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(source.modifier.0)
            .plane_layouts(&layouts);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let flags =
            if disjoint { vk::ImageCreateFlags::DISJOINT } else { vk::ImageCreateFlags::empty() };
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format)
            .extent(vk::Extent3D {
                width: source.width as u32,
                height: source.height as u32,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .flags(flags)
            .push_next(&mut external)
            .push_next(&mut modifier_info);
        let image = unsafe { device.device.create_image(&image_info, None) }.map_err(|error| {
            log::error!("vulkan output image creation failed: {error}");
            RendererError::ImportFailed
        })?;
        let image_guard = DestroyImage { device: &device.device, image, disarm: false };

        let memory_count = if disjoint { source.planes.len() } else { 1 };
        let mut imported = FreeMemories {
            device: &device.device,
            memories: Vec::with_capacity(memory_count),
            disarm: false,
        };
        for index in 0..memory_count {
            let aspect = if disjoint { Some(memory_plane(index)?) } else { None };
            imported.memories.push(memory::import_memory(
                device,
                image,
                &source.planes[index].fd,
                aspect,
            )?);
        }
        bind_memory(device, image, &imported.memories, disjoint)?;
        let view = create_view(&device.device, image, vk_format, component_mapping(false))?;
        let view_guard = DestroyView { device: &device.device, view, disarm: false };
        let render_pass = color_render_pass(&device.device, vk_format).map_err(|error| {
            log::error!("{error}");
            RendererError::ImportFailed
        })?;
        let pass_guard = DestroyPass { device: &device.device, render_pass, disarm: false };
        let framebuffer = create_framebuffer(
            &device.device,
            render_pass,
            view,
            source.width as u32,
            source.height as u32,
        )?;
        let memories = imported.disarm();
        view_guard.disarm();
        pass_guard.disarm();
        image_guard.disarm();
        Ok(Self {
            device: device.clone(),
            image,
            memories,
            view,
            render_pass,
            framebuffer,
            width: source.width,
            height: source.height,
            layout: Cell::new(vk::ImageLayout::UNDEFINED),
        })
    }
}

pub(super) struct OffscreenImage {
    device: Rc<VulkanDevice>,
    image: vk::Image,
    allocation: GpuAllocation,
    view: vk::ImageView,
    render_pass: vk::RenderPass,
    framebuffer: vk::Framebuffer,
    width: i32,
    height: i32,
    layout: Cell<vk::ImageLayout>,
}

impl OffscreenImage {
    pub(super) fn new(
        device: &Rc<VulkanDevice>,
        width: i32,
        height: i32,
    ) -> Result<Self, RendererError> {
        if width <= 0 || height <= 0 {
            log::warn!("vulkan offscreen target has no positive size");
            return Err(RendererError::TargetFailed);
        }
        let format = vk::Format::B8G8R8A8_UNORM;
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width: width as u32, height: height as u32, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { device.device.create_image(&info, None) }.map_err(|error| {
            log::error!("vulkan offscreen image creation failed: {error}");
            RendererError::TargetFailed
        })?;
        let requirements = unsafe { device.device.get_image_memory_requirements(image) };
        let allocation = match device.allocate(requirements, UsageFlags::FAST_DEVICE_ACCESS) {
            Ok(allocation) => allocation,
            Err(error) => {
                log::error!("{error}");
                unsafe { device.device.destroy_image(image, None) };
                return Err(RendererError::TargetFailed);
            }
        };
        if let Err(error) = unsafe {
            device
                .device
                .bind_image_memory(image, allocation.memory(), allocation.offset())
        } {
            log::error!("vulkan offscreen image bind failed: {error}");
            unsafe { device.device.destroy_image(image, None) };
            return Err(RendererError::TargetFailed);
        }
        let view = match create_view(&device.device, image, format, component_mapping(false)) {
            Ok(view) => view,
            Err(error) => {
                unsafe { device.device.destroy_image(image, None) };
                return Err(error);
            }
        };
        let render_pass = match color_render_pass(&device.device, format) {
            Ok(pass) => pass,
            Err(error) => {
                log::error!("{error}");
                unsafe {
                    device.device.destroy_image_view(view, None);
                    device.device.destroy_image(image, None);
                }
                return Err(RendererError::TargetFailed);
            }
        };
        let framebuffer = match create_framebuffer(
            &device.device,
            render_pass,
            view,
            width as u32,
            height as u32,
        ) {
            Ok(framebuffer) => framebuffer,
            Err(error) => {
                unsafe {
                    device.device.destroy_render_pass(render_pass, None);
                    device.device.destroy_image_view(view, None);
                    device.device.destroy_image(image, None);
                }
                return Err(error);
            }
        };
        Ok(Self {
            device: device.clone(),
            image,
            allocation,
            view,
            render_pass,
            framebuffer,
            width,
            height,
            layout: Cell::new(vk::ImageLayout::UNDEFINED),
        })
    }
}

pub(super) trait PaintedTarget {
    fn image(&self) -> vk::Image;
    fn framebuffer(&self) -> vk::Framebuffer;
    fn render_pass(&self) -> vk::RenderPass;
    fn width(&self) -> i32;
    fn height(&self) -> i32;
    fn layout(&self) -> &Cell<vk::ImageLayout>;
    fn clear_color(&self) -> [f32; 4];
    fn can_readback(&self) -> bool;
    /// Imported dma-buf. Ownership moves to and from the foreign queue, and
    /// [`Self::layout`] is then `UNDEFINED` or `GENERAL`.
    fn external(&self) -> bool;
}

impl PaintedTarget for OutputTarget {
    fn image(&self) -> vk::Image {
        self.image
    }

    fn framebuffer(&self) -> vk::Framebuffer {
        self.framebuffer
    }

    fn render_pass(&self) -> vk::RenderPass {
        self.render_pass
    }

    fn width(&self) -> i32 {
        self.width
    }

    fn height(&self) -> i32 {
        self.height
    }

    fn layout(&self) -> &Cell<vk::ImageLayout> {
        &self.layout
    }

    fn clear_color(&self) -> [f32; 4] {
        crate::render::OUTPUT_BACKGROUND
    }

    fn can_readback(&self) -> bool {
        true
    }

    fn external(&self) -> bool {
        true
    }
}

impl PaintedTarget for OffscreenImage {
    fn image(&self) -> vk::Image {
        self.image
    }

    fn framebuffer(&self) -> vk::Framebuffer {
        self.framebuffer
    }

    fn render_pass(&self) -> vk::RenderPass {
        self.render_pass
    }

    fn width(&self) -> i32 {
        self.width
    }

    fn height(&self) -> i32 {
        self.height
    }

    fn layout(&self) -> &Cell<vk::ImageLayout> {
        &self.layout
    }

    fn clear_color(&self) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn can_readback(&self) -> bool {
        true
    }

    fn external(&self) -> bool {
        false
    }
}

impl Debug for OutputTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VulkanOutputTarget")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

impl RenderTarget for OutputTarget {
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

impl Debug for OffscreenImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VulkanOffscreenImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

impl RenderTarget for OffscreenImage {
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

impl OffscreenTarget for OffscreenImage {}

impl Drop for OffscreenImage {
    fn drop(&mut self) {
        unsafe {
            if let Err(error) = self.device.device.device_wait_idle() {
                log::error!("vulkan wait before destroying an offscreen image failed: {error}");
            }
            self.device
                .device
                .destroy_framebuffer(self.framebuffer, None);
            self.device
                .device
                .destroy_render_pass(self.render_pass, None);
            self.device.device.destroy_image_view(self.view, None);
            self.device.device.destroy_image(self.image, None);
        }
    }
}

impl Drop for OutputTarget {
    fn drop(&mut self) {
        // Finish command use of this image before freeing it. Frame submission
        // does not idle; this wait belongs to teardown.
        unsafe {
            if let Err(error) = self.device.device.device_wait_idle() {
                log::error!("vulkan wait before destroying an output image failed: {error}");
            }
            self.device
                .device
                .destroy_framebuffer(self.framebuffer, None);
            self.device
                .device
                .destroy_render_pass(self.render_pass, None);
            self.device.device.destroy_image_view(self.view, None);
            for memory in self.memories.drain(..) {
                self.device.device.free_memory(memory, None);
            }
            self.device.device.destroy_image(self.image, None);
        }
    }
}

pub(super) fn plane_file(plane: &DmaBufPlane) -> Result<(u64, u64), RendererError> {
    let stat = nix::sys::stat::fstat(plane.fd.as_raw_fd()).map_err(|error| {
        log::error!("failed to stat a dma-buf plane: {error}");
        RendererError::ImportFailed
    })?;
    Ok((stat.st_dev, stat.st_ino))
}

pub(super) fn planes_are_disjoint(planes: &[DmaBufPlane]) -> Result<bool, RendererError> {
    if planes.len() <= 1 {
        return Ok(false);
    }
    let first = plane_file(&planes[0])?;
    for plane in &planes[1..] {
        if plane_file(plane)? != first {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn memory_plane(index: usize) -> Result<vk::ImageAspectFlags, RendererError> {
    match index {
        0 => Ok(vk::ImageAspectFlags::MEMORY_PLANE_0_EXT),
        1 => Ok(vk::ImageAspectFlags::MEMORY_PLANE_1_EXT),
        2 => Ok(vk::ImageAspectFlags::MEMORY_PLANE_2_EXT),
        3 => Ok(vk::ImageAspectFlags::MEMORY_PLANE_3_EXT),
        _ => {
            log::error!("vulkan dma-buf has more than four planes");
            Err(RendererError::ImportFailed)
        }
    }
}

pub(super) fn bind_memory(
    device: &VulkanDevice,
    image: vk::Image,
    memories: &[vk::DeviceMemory],
    disjoint: bool,
) -> Result<(), RendererError> {
    if !disjoint {
        return unsafe { device.device.bind_image_memory(image, memories[0], 0) }.map_err(
            |error| {
                log::error!("vulkan output image bind failed: {error}");
                RendererError::ImportFailed
            },
        );
    }
    for (index, memory) in memories.iter().copied().enumerate() {
        let mut plane = vk::BindImagePlaneMemoryInfo::default().plane_aspect(memory_plane(index)?);
        let info = vk::BindImageMemoryInfo::default()
            .image(image)
            .memory(memory)
            .push_next(&mut plane);
        unsafe {
            device
                .device
                .bind_image_memory2(std::slice::from_ref(&info))
        }
        .map_err(|error| {
            log::error!("vulkan output image plane bind failed: {error}");
            RendererError::ImportFailed
        })?;
    }
    Ok(())
}

pub(super) fn component_mapping(opaque_alpha: bool) -> vk::ComponentMapping {
    vk::ComponentMapping {
        r: vk::ComponentSwizzle::IDENTITY,
        g: vk::ComponentSwizzle::IDENTITY,
        b: vk::ComponentSwizzle::IDENTITY,
        a: if opaque_alpha { vk::ComponentSwizzle::ONE } else { vk::ComponentSwizzle::IDENTITY },
    }
}

pub(super) fn create_view(
    device: &ash::Device,
    image: vk::Image,
    format: vk::Format,
    components: vk::ComponentMapping,
) -> Result<vk::ImageView, RendererError> {
    let info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .components(components)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    unsafe { device.create_image_view(&info, None) }.map_err(|error| {
        log::error!("vulkan output image view failed: {error}");
        RendererError::ImportFailed
    })
}

pub(super) fn color_render_pass(
    device: &ash::Device,
    format: vk::Format,
) -> Result<vk::RenderPass, String> {
    let attachment = vk::AttachmentDescription::default()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::LOAD)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    let reference = vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    };
    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(std::slice::from_ref(&reference));
    let dependency = vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
        .dst_access_mask(
            vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
        );
    let info = vk::RenderPassCreateInfo::default()
        .attachments(std::slice::from_ref(&attachment))
        .subpasses(std::slice::from_ref(&subpass))
        .dependencies(std::slice::from_ref(&dependency));
    unsafe { device.create_render_pass(&info, None) }
        .map_err(|error| format!("vulkan color render pass failed: {error}"))
}

pub(super) fn create_framebuffer(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    view: vk::ImageView,
    width: u32,
    height: u32,
) -> Result<vk::Framebuffer, RendererError> {
    let info = vk::FramebufferCreateInfo::default()
        .render_pass(render_pass)
        .attachments(std::slice::from_ref(&view))
        .width(width)
        .height(height)
        .layers(1);
    unsafe { device.create_framebuffer(&info, None) }.map_err(|error| {
        log::error!("vulkan output framebuffer failed: {error}");
        RendererError::ImportFailed
    })
}

struct DestroyImage<'a> {
    device: &'a ash::Device,
    image: vk::Image,
    disarm: bool,
}

impl DestroyImage<'_> {
    fn disarm(mut self) {
        self.disarm = true;
    }
}

impl Drop for DestroyImage<'_> {
    fn drop(&mut self) {
        if !self.disarm {
            unsafe { self.device.destroy_image(self.image, None) }
        }
    }
}

struct DestroyView<'a> {
    device: &'a ash::Device,
    view: vk::ImageView,
    disarm: bool,
}

impl DestroyView<'_> {
    fn disarm(mut self) {
        self.disarm = true;
    }
}

impl Drop for DestroyView<'_> {
    fn drop(&mut self) {
        if !self.disarm {
            unsafe { self.device.destroy_image_view(self.view, None) }
        }
    }
}

struct DestroyPass<'a> {
    device: &'a ash::Device,
    render_pass: vk::RenderPass,
    disarm: bool,
}

impl DestroyPass<'_> {
    fn disarm(mut self) {
        self.disarm = true;
    }
}

impl Drop for DestroyPass<'_> {
    fn drop(&mut self) {
        if !self.disarm {
            unsafe { self.device.destroy_render_pass(self.render_pass, None) }
        }
    }
}

struct FreeMemories<'a> {
    device: &'a ash::Device,
    memories: Vec<vk::DeviceMemory>,
    disarm: bool,
}

impl FreeMemories<'_> {
    fn disarm(mut self) -> Vec<vk::DeviceMemory> {
        self.disarm = true;
        std::mem::take(&mut self.memories)
    }
}

impl Drop for FreeMemories<'_> {
    fn drop(&mut self) {
        if self.disarm {
            return;
        }
        for memory in self.memories.drain(..) {
            unsafe { self.device.free_memory(memory, None) }
        }
    }
}
