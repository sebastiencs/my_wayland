//! Sampled images. Shared memory is copied through a host-visible staging buffer.
//! dma-buf planes stay on dedicated imported memory.

use std::{any::Any, fmt::Debug, rc::Rc};

use {ash::vk, gpu_alloc::UsageFlags};

use crate::{
    dmabuf::DmaBuf,
    drm::{ARGB8888, XRGB8888},
    render::{RenderDeviceId, RendererError, ShmImage, Texture},
};

use super::{
    allocator::GpuAllocation,
    command,
    device::VulkanDevice,
    format::vk_format,
    resources::{self, create_view},
};

pub(super) struct VulkanTexture {
    device: Rc<VulkanDevice>,
    device_id: RenderDeviceId,
    image: vk::Image,
    view: vk::ImageView,
    memory: TextureMemory,
}

enum TextureMemory {
    Gpu(GpuAllocation),
    Imported(Vec<vk::DeviceMemory>),
}

impl Debug for VulkanTexture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("VulkanTexture").finish()
    }
}

impl Texture for VulkanTexture {}

impl Drop for VulkanTexture {
    fn drop(&mut self) {
        unsafe {
            self.device.device.destroy_image_view(self.view, None);
            self.device.device.destroy_image(self.image, None);
            if let TextureMemory::Imported(memories) = &mut self.memory {
                for memory in memories.drain(..) {
                    self.device.device.free_memory(memory, None);
                }
            }
        }
    }
}

impl VulkanTexture {
    pub(super) fn device_id(&self) -> RenderDeviceId {
        self.device_id
    }

    pub(super) fn view(&self) -> vk::ImageView {
        self.view
    }

    pub(super) fn import_dmabuf(
        device: &Rc<VulkanDevice>,
        source: &DmaBuf,
    ) -> Result<Self, RendererError> {
        if source.width <= 0 || source.height <= 0 || source.planes.is_empty() {
            log::warn!("vulkan texture has no positive size or planes");
            return Err(RendererError::ImportFailed);
        }
        let Some(modifiers) = device.formats.by_fourcc.get(&source.format.drm) else {
            log::warn!("vulkan cannot sample {:?}", source.format.drm);
            return Err(RendererError::UnsupportedFormat);
        };
        let Some(modifier) = modifiers.get(&source.modifier.0) else {
            log::warn!(
                "vulkan cannot sample {:?} modifier {:#x}",
                source.format.drm,
                source.modifier.0
            );
            return Err(RendererError::UnsupportedFormat);
        };
        let Some(max) = modifier.sample_max else {
            log::warn!(
                "vulkan cannot sample {:?} modifier {:#x}",
                source.format.drm,
                source.modifier.0
            );
            return Err(RendererError::UnsupportedFormat);
        };
        if source.width as u32 > max.width
            || source.height as u32 > max.height
            || modifier.planes as usize != source.planes.len()
        {
            log::warn!("vulkan texture does not match modifier {:#x} layout", source.modifier.0);
            return Err(RendererError::UnsupportedFormat);
        }
        let disjoint = resources::planes_are_disjoint(&source.planes)?;
        if disjoint && !modifier.disjoint {
            log::warn!("vulkan modifier {:#x} does not support disjoint planes", source.modifier.0);
            return Err(RendererError::UnsupportedFormat);
        }
        let format = vk_format(source.format.drm);
        if format == vk::Format::UNDEFINED {
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
            .format(format)
            .extent(vk::Extent3D {
                width: source.width as u32,
                height: source.height as u32,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .flags(flags)
            .push_next(&mut external)
            .push_next(&mut modifier_info);
        let image = unsafe { device.device.create_image(&image_info, None) }.map_err(|error| {
            log::error!("vulkan texture image creation failed: {error}");
            RendererError::ImportFailed
        })?;
        let memory_count = if disjoint { source.planes.len() } else { 1 };
        let mut memories = Vec::with_capacity(memory_count);
        for index in 0..memory_count {
            let aspect = if disjoint { Some(resources::memory_plane(index)?) } else { None };
            match super::memory::import_memory(device, image, &source.planes[index].fd, aspect) {
                Ok(memory) => memories.push(memory),
                Err(error) => {
                    unsafe {
                        for memory in memories {
                            device.device.free_memory(memory, None);
                        }
                        device.device.destroy_image(image, None);
                    }
                    return Err(error);
                }
            }
        }
        if let Err(error) = resources::bind_memory(device, image, &memories, disjoint) {
            unsafe {
                for memory in memories {
                    device.device.free_memory(memory, None);
                }
                device.device.destroy_image(image, None);
            }
            return Err(error);
        }
        let view = match create_view(&device.device, image, format) {
            Ok(view) => view,
            Err(error) => {
                unsafe {
                    for memory in memories {
                        device.device.free_memory(memory, None);
                    }
                    device.device.destroy_image(image, None);
                }
                return Err(error);
            }
        };
        let texture = Self {
            device: device.clone(),
            device_id: device.device_id(),
            image,
            view,
            memory: TextureMemory::Imported(memories),
        };
        transition_sampled(device, image)?;
        Ok(texture)
    }

    pub(super) fn import_shm(
        device: &Rc<VulkanDevice>,
        image: ShmImage<'_>,
    ) -> Result<Self, RendererError> {
        if image.fourcc != ARGB8888.drm && image.fourcc != XRGB8888.drm {
            log::warn!("vulkan shared-memory import does not support {:?}", image.fourcc);
            return Err(RendererError::UnsupportedFormat);
        }
        checked_rows(image.bytes, image.width, image.height, image.stride)?;
        upload_sampled(
            device,
            image.bytes,
            image.width,
            image.height,
            image.stride,
            vk::Format::B8G8R8A8_UNORM,
            RendererError::ImportFailed,
        )
    }

    pub(super) fn upload_rgba(
        device: &Rc<VulkanDevice>,
        pixels: &[u8],
        width: i32,
        height: i32,
    ) -> Result<Self, RendererError> {
        let row_bytes = usize::try_from(width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or_else(|| {
                log::warn!("vulkan rgba upload has no positive size");
                RendererError::UploadFailed
            })?;
        checked_rows(pixels, width, height, row_bytes)?;
        upload_sampled(
            device,
            pixels,
            width,
            height,
            row_bytes,
            vk::Format::R8G8B8A8_UNORM,
            RendererError::UploadFailed,
        )
    }
}

pub(super) fn sampled_texture<'a>(
    texture: &'a dyn Texture,
    device_id: RenderDeviceId,
) -> Option<&'a VulkanTexture> {
    let Some(texture) = (texture as &dyn Any).downcast_ref::<VulkanTexture>() else {
        log::error!("renderer received a texture from a different backend");
        return None;
    };
    if texture.device_id != device_id {
        log::error!("renderer received a texture from a different device");
        return None;
    }
    Some(texture)
}

fn checked_rows(bytes: &[u8], width: i32, height: i32, stride: usize) -> Result<(), RendererError> {
    if width <= 0 || height <= 0 {
        log::warn!("vulkan texture upload has no positive size");
        return Err(RendererError::UploadFailed);
    }
    let row_bytes = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(4));
    let Some(row_bytes) = row_bytes else {
        return Err(RendererError::UploadFailed);
    };
    if stride < row_bytes || stride % 4 != 0 {
        log::warn!("vulkan texture stride {stride} does not cover {width} pixels");
        return Err(RendererError::UploadFailed);
    }
    let Some(needed) = usize::try_from(height)
        .ok()
        .and_then(|height| stride.checked_mul(height))
    else {
        return Err(RendererError::UploadFailed);
    };
    if bytes.len() < needed {
        log::warn!("vulkan texture upload is shorter than its stride");
        return Err(RendererError::UploadFailed);
    }
    Ok(())
}

fn upload_sampled(
    device: &Rc<VulkanDevice>,
    bytes: &[u8],
    width: i32,
    height: i32,
    stride: usize,
    format: vk::Format,
    failed: RendererError,
) -> Result<VulkanTexture, RendererError> {
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D { width: width as u32, height: height as u32, depth: 1 })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let image = unsafe { device.device.create_image(&info, None) }.map_err(|error| {
        log::error!("vulkan sampled image creation failed: {error}");
        failed.clone()
    })?;
    let requirements = unsafe { device.device.get_image_memory_requirements(image) };
    let allocation = match device.allocate(requirements, UsageFlags::FAST_DEVICE_ACCESS) {
        Ok(allocation) => allocation,
        Err(error) => {
            log::error!("{error}");
            unsafe { device.device.destroy_image(image, None) };
            return Err(failed);
        }
    };
    if let Err(error) = unsafe {
        device
            .device
            .bind_image_memory(image, allocation.memory(), allocation.offset())
    } {
        log::error!("vulkan sampled image bind failed: {error}");
        unsafe { device.device.destroy_image(image, None) };
        return Err(failed);
    }
    let view = match create_view(&device.device, image, format) {
        Ok(view) => view,
        Err(error) => {
            unsafe { device.device.destroy_image(image, None) };
            return Err(error);
        }
    };
    let copy_bytes = stride * height as usize;
    let staging = match super::allocator::AllocatedBuffer::create(
        device,
        copy_bytes as u64,
        vk::BufferUsageFlags::TRANSFER_SRC,
        UsageFlags::UPLOAD | UsageFlags::TRANSIENT,
    ) {
        Ok(staging) => staging,
        Err(error) => {
            log::error!("{error}");
            unsafe {
                device.device.destroy_image_view(view, None);
                device.device.destroy_image(image, None);
            }
            return Err(failed);
        }
    };
    if let Err(error) = staging.write(&bytes[..copy_bytes]) {
        log::error!("{error}");
        unsafe {
            device.device.destroy_image_view(view, None);
            device.device.destroy_image(image, None);
        }
        return Err(failed);
    }
    let row_texels = (stride / 4) as u32;
    let copied = device.submit(failed, |device, command| {
        command::image_barrier(
            device,
            command,
            image,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::empty(),
            vk::AccessFlags::TRANSFER_WRITE,
        );
        let region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(row_texels)
            .buffer_image_height(0)
            .image_subresource(color_layers())
            .image_extent(vk::Extent3D { width: width as u32, height: height as u32, depth: 1 });
        unsafe {
            device.cmd_copy_buffer_to_image(
                command,
                staging.buffer(),
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                std::slice::from_ref(&region),
            );
        }
        command::image_barrier(
            device,
            command,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::TRANSFER_WRITE,
            vk::AccessFlags::SHADER_READ,
        );
        Ok(())
    });
    if let Err(error) = copied {
        unsafe {
            device.device.destroy_image_view(view, None);
            device.device.destroy_image(image, None);
        }
        return Err(error);
    }
    Ok(VulkanTexture {
        device: device.clone(),
        device_id: device.device_id(),
        image,
        view,
        memory: TextureMemory::Gpu(allocation),
    })
}

fn transition_sampled(device: &VulkanDevice, image: vk::Image) -> Result<(), RendererError> {
    device.submit(RendererError::ImportFailed, |device, command| {
        command::image_barrier(
            device,
            command,
            image,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::empty(),
            vk::AccessFlags::SHADER_READ,
        );
        Ok(())
    })
}

fn color_layers() -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        mip_level: 0,
        base_array_layer: 0,
        layer_count: 1,
    }
}
