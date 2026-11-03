use std::{
    ffi::{c_char, CStr, CString},
    os::fd::{AsFd, AsRawFd},
    rc::Rc,
};

use {
    ash::{vk, Device},
    drm::buffer::DrmFourcc,
};

use super::{
    command,
    format::{self, FormatSet},
    instance::VulkanInstance,
    sync,
};

pub(super) struct VulkanDevice {
    pub(super) device: Device,
    pub(super) graphics_queue: vk::Queue,
    pub(super) graphics_queue_family: u32,
    command_pool: vk::CommandPool,
    pub(super) external_memory: ash::khr::external_memory_fd::Device,
    pub(super) memory_properties: vk::PhysicalDeviceMemoryProperties,
    pub(super) formats: FormatSet,
}

impl Drop for VulkanDevice {
    fn drop(&mut self) {
        unsafe {
            if let Err(error) = self.device.device_wait_idle() {
                log::error!("vulkan device wait before destruction failed: {error}");
            }
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
        }
    }
}

impl VulkanDevice {
    pub(super) fn open(drm: &impl AsFd) -> Result<Rc<Self>, String> {
        let instance = VulkanInstance::shared()?;
        let (node_major, node_minor) = drm_node(drm)?;
        let physical = find_physical_device(instance, node_major, node_minor)?;
        require_api_version(instance, physical, node_major, node_minor)?;
        require_extensions(instance, physical)?;
        let queue_family = graphics_queue_family(instance, physical, node_major, node_minor)?;
        sync::require_sync_file(instance, physical)?;
        let formats = format::query_formats(instance, physical)?;
        require_xrgb8888(&formats)?;
        let device = create_device(instance, physical, queue_family)?;
        let command_pool = match command::create_pool(&device, queue_family) {
            Ok(pool) => pool,
            Err(error) => {
                unsafe { device.destroy_device(None) };
                return Err(error);
            }
        };
        let external_memory =
            ash::khr::external_memory_fd::Device::new(&instance.instance, &device);
        let memory_properties = unsafe {
            instance
                .instance
                .get_physical_device_memory_properties(physical)
        };
        let graphics_queue = unsafe { device.get_device_queue(queue_family, 0) };
        Ok(Rc::new(Self {
            device,
            graphics_queue,
            graphics_queue_family: queue_family,
            command_pool,
            external_memory,
            memory_properties,
            formats,
        }))
    }
}

fn drm_node(drm: &impl AsFd) -> Result<(u64, u64), String> {
    let stat = nix::sys::stat::fstat(drm.as_fd().as_raw_fd())
        .map_err(|error| format!("failed to stat the drm node: {error}"))?;
    Ok((nix::sys::stat::major(stat.st_rdev), nix::sys::stat::minor(stat.st_rdev)))
}

fn find_physical_device(
    instance: &VulkanInstance,
    node_major: u64,
    node_minor: u64,
) -> Result<vk::PhysicalDevice, String> {
    let devices = unsafe { instance.instance.enumerate_physical_devices() }
        .map_err(|error| format!("failed to enumerate vulkan physical devices: {error}"))?;
    for physical in devices {
        let extensions = device_extension_names(instance, physical)?;
        if !extensions
            .iter()
            .any(|name| name.as_c_str() == vk::EXT_PHYSICAL_DEVICE_DRM_NAME)
        {
            continue;
        }
        let mut drm_properties = vk::PhysicalDeviceDrmPropertiesEXT::default();
        let mut properties =
            vk::PhysicalDeviceProperties2::default().push_next(&mut drm_properties);
        unsafe {
            instance
                .instance
                .get_physical_device_properties2(physical, &mut properties);
        }
        let name = unsafe { CStr::from_ptr(properties.properties.device_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let matches_primary = same_node(
            drm_properties.has_primary,
            drm_properties.primary_major,
            drm_properties.primary_minor,
            node_major,
            node_minor,
        );
        let matches_render = same_node(
            drm_properties.has_render,
            drm_properties.render_major,
            drm_properties.render_minor,
            node_major,
            node_minor,
        );
        if matches_primary || matches_render {
            log::info!("vulkan device {name} matches drm {node_major}:{node_minor}");
            return Ok(physical);
        }
    }
    Err(format!("no vulkan physical device matches drm node {node_major}:{node_minor}"))
}

fn same_node(
    has_node: vk::Bool32,
    major: i64,
    minor: i64,
    node_major: u64,
    node_minor: u64,
) -> bool {
    has_node != 0
        && major >= 0
        && minor >= 0
        && major as u64 == node_major
        && minor as u64 == node_minor
}

fn device_extension_names(
    instance: &VulkanInstance,
    physical: vk::PhysicalDevice,
) -> Result<Vec<CString>, String> {
    let listed = unsafe {
        instance
            .instance
            .enumerate_device_extension_properties(physical)
    }
    .map_err(|error| format!("failed to enumerate vulkan device extensions: {error}"))?;
    Ok(listed
        .iter()
        .map(|extension| unsafe { CStr::from_ptr(extension.extension_name.as_ptr()).to_owned() })
        .collect())
}

fn require_api_version(
    instance: &VulkanInstance,
    physical: vk::PhysicalDevice,
    node_major: u64,
    node_minor: u64,
) -> Result<(), String> {
    let properties = unsafe { instance.instance.get_physical_device_properties(physical) };
    if properties.api_version >= vk::API_VERSION_1_2 {
        return Ok(());
    }
    Err(format!(
        "vulkan device matching drm {node_major}:{node_minor} supports api {}.{}; vulkan 1.2 is required",
        vk::api_version_major(properties.api_version),
        vk::api_version_minor(properties.api_version)
    ))
}

const REQUIRED_EXTENSIONS: &[&CStr] = &[
    vk::KHR_EXTERNAL_MEMORY_FD_NAME,
    vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME,
    vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME,
    vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME,
    vk::EXT_PHYSICAL_DEVICE_DRM_NAME,
];

fn require_extensions(
    instance: &VulkanInstance,
    physical: vk::PhysicalDevice,
) -> Result<(), String> {
    let present = device_extension_names(instance, physical)?;
    let missing: Vec<&str> = REQUIRED_EXTENSIONS
        .iter()
        .filter(|required| !present.iter().any(|name| name.as_c_str() == **required))
        .map(|required| required.to_str().unwrap_or("unknown extension"))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("vulkan device is missing {}", missing.join(", ")))
    }
}

fn graphics_queue_family(
    instance: &VulkanInstance,
    physical: vk::PhysicalDevice,
    node_major: u64,
    node_minor: u64,
) -> Result<u32, String> {
    let families = unsafe {
        instance
            .instance
            .get_physical_device_queue_family_properties(physical)
    };
    families
        .iter()
        .position(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        .map(|index| index as u32)
        .ok_or_else(|| {
            format!("vulkan device matching drm {node_major}:{node_minor} has no graphics queue")
        })
}

fn require_xrgb8888(formats: &FormatSet) -> Result<(), String> {
    let modifiers = formats.by_fourcc.get(&DrmFourcc::Xrgb8888);
    let can_sample = modifiers.is_some_and(|modifiers| {
        modifiers
            .values()
            .any(|modifier| modifier.sample_max.is_some())
    });
    let can_render = modifiers.is_some_and(|modifiers| {
        modifiers
            .values()
            .any(|modifier| modifier.render_max.is_some())
    });
    if can_sample && can_render {
        return Ok(());
    }
    let use_name = match (can_sample, can_render) {
        (false, false) => "sample or render",
        (false, true) => "sample",
        (true, false) => "render",
        (true, true) => unreachable!(),
    };
    Err(format!("vulkan device cannot {use_name} DRM XRGB8888"))
}

fn create_device(
    instance: &VulkanInstance,
    physical: vk::PhysicalDevice,
    queue_family: u32,
) -> Result<Device, String> {
    let priorities = [1.0_f32];
    let queue_info = vk::DeviceQueueCreateInfo::default()
        .queue_family_index(queue_family)
        .queue_priorities(&priorities);
    let extensions: Vec<*const c_char> = REQUIRED_EXTENSIONS
        .iter()
        .map(|name| name.as_ptr())
        .collect();
    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(std::slice::from_ref(&queue_info))
        .enabled_extension_names(&extensions);
    unsafe {
        instance
            .instance
            .create_device(physical, &create_info, None)
    }
    .map_err(|error| format!("failed to create the vulkan device: {error}"))
}
