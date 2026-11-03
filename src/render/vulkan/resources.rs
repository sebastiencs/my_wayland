use std::{
    any::Any,
    fmt::{self, Debug},
    os::fd::AsRawFd,
    rc::Rc,
};

use ash::vk;

use crate::{
    dmabuf::{DmaBuf, DmaBufPlane},
    render::{RenderTarget, RendererError},
};

use super::{device::VulkanDevice, format::vk_format, memory};

pub(super) struct OutputTarget {
    device: Rc<VulkanDevice>,
    image: vk::Image,
    memories: Vec<vk::DeviceMemory>,
    view: vk::ImageView,
    render_pass: vk::RenderPass,
    framebuffer: vk::Framebuffer,
    width: i32,
    height: i32,
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
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
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
        let view = create_view(&device.device, image, vk_format)?;
        let view_guard = DestroyView { device: &device.device, view, disarm: false };
        let render_pass = create_render_pass(&device.device, vk_format)?;
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
        })
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

fn planes_are_disjoint(planes: &[DmaBufPlane]) -> Result<bool, RendererError> {
    if planes.len() <= 1 {
        return Ok(false);
    }
    let first = nix::sys::stat::fstat(planes[0].fd.as_raw_fd()).map_err(|error| {
        log::error!("failed to stat a dma-buf plane: {error}");
        RendererError::ImportFailed
    })?;
    for plane in &planes[1..] {
        let stat = nix::sys::stat::fstat(plane.fd.as_raw_fd()).map_err(|error| {
            log::error!("failed to stat a dma-buf plane: {error}");
            RendererError::ImportFailed
        })?;
        if stat.st_dev != first.st_dev || stat.st_ino != first.st_ino {
            return Ok(true);
        }
    }
    Ok(false)
}

fn memory_plane(index: usize) -> Result<vk::ImageAspectFlags, RendererError> {
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

fn bind_memory(
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

fn create_view(
    device: &ash::Device,
    image: vk::Image,
    format: vk::Format,
) -> Result<vk::ImageView, RendererError> {
    let info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
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

fn create_render_pass(
    device: &ash::Device,
    format: vk::Format,
) -> Result<vk::RenderPass, RendererError> {
    let attachment = vk::AttachmentDescription::default()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::DONT_CARE)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    let reference = vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    };
    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(std::slice::from_ref(&reference));
    let info = vk::RenderPassCreateInfo::default()
        .attachments(std::slice::from_ref(&attachment))
        .subpasses(std::slice::from_ref(&subpass));
    unsafe { device.create_render_pass(&info, None) }.map_err(|error| {
        log::error!("vulkan output render pass failed: {error}");
        RendererError::ImportFailed
    })
}

fn create_framebuffer(
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
