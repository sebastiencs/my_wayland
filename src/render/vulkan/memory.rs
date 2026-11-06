use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};

use ash::vk;

use crate::render::RendererError;

use super::device::VulkanDevice;

pub(super) fn import_memory(
    device: &VulkanDevice,
    image: vk::Image,
    plane: &OwnedFd,
    plane_aspect: Option<vk::ImageAspectFlags>,
) -> Result<vk::DeviceMemory, RendererError> {
    let mut fd_properties = vk::MemoryFdPropertiesKHR::default();
    unsafe {
        device
            .external_memory
            .get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                plane.as_raw_fd(),
                &mut fd_properties,
            )
            .map_err(|error| {
                log::error!("vulkan dma-buf memory properties failed: {error}");
                RendererError::ImportFailed
            })?;
    }
    let requirements = memory_requirements(device, image, plane_aspect);
    let type_bits = requirements.memory_type_bits & fd_properties.memory_type_bits;
    let Some(memory_type_index) = memory_type_index(&device.memory_properties, type_bits) else {
        log::error!("no vulkan memory type can import the dma-buf");
        return Err(RendererError::ImportFailed);
    };
    let duplicated = duplicate_fd(plane)?;
    let raw_fd = duplicated.into_raw_fd();
    let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
    let mut imported = vk::ImportMemoryFdInfoKHR::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
        .fd(raw_fd);
    let allocate = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type_index)
        .push_next(&mut imported)
        .push_next(&mut dedicated);
    match unsafe { device.device.allocate_memory(&allocate, None) } {
        Ok(memory) => Ok(memory),
        Err(error) => {
            log::error!("vulkan dma-buf memory import failed: {error}");
            Err(RendererError::ImportFailed)
        }
    }
}

fn memory_requirements(
    device: &VulkanDevice,
    image: vk::Image,
    plane_aspect: Option<vk::ImageAspectFlags>,
) -> vk::MemoryRequirements {
    let mut requirements = vk::MemoryRequirements2::default();
    let mut info = vk::ImageMemoryRequirementsInfo2::default().image(image);
    if let Some(aspect) = plane_aspect {
        let mut plane = vk::ImagePlaneMemoryRequirementsInfo::default().plane_aspect(aspect);
        info = info.push_next(&mut plane);
        unsafe {
            device
                .device
                .get_image_memory_requirements2(&info, &mut requirements);
        }
    } else {
        unsafe {
            device
                .device
                .get_image_memory_requirements2(&info, &mut requirements);
        }
    }
    requirements.memory_requirements
}

fn memory_type_index(
    properties: &vk::PhysicalDeviceMemoryProperties,
    type_bits: u32,
) -> Option<u32> {
    (0..properties.memory_type_count).find(|index| type_bits & (1 << index) != 0)
}

pub(super) fn duplicate_fd(fd: &OwnedFd) -> Result<OwnedFd, RendererError> {
    let raw = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if raw < 0 {
        log::error!("failed to duplicate a dma-buf fd: {}", std::io::Error::last_os_error());
        return Err(RendererError::ImportFailed);
    }
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}
