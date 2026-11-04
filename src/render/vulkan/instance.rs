use std::{ffi::CString, sync::OnceLock};

use ash::{vk, Entry, Instance};

/// Process-wide Vulkan instance. One logical device is created per DRM node.
pub(super) struct VulkanInstance {
    _entry: Entry,
    pub(super) instance: Instance,
}

impl VulkanInstance {
    pub(super) fn shared() -> Result<&'static Self, String> {
        static INSTANCE: OnceLock<Result<VulkanInstance, String>> = OnceLock::new();
        INSTANCE
            .get_or_init(Self::create)
            .as_ref()
            .map_err(Clone::clone)
    }

    fn create() -> Result<Self, String> {
        // The loader applies VK_INSTANCE_LAYERS itself. A missing validation
        // layer stays a loader warning and does not fail instance creation.
        let entry = unsafe { Entry::load() }
            .map_err(|error| format!("failed to load the vulkan loader: {error}"))?;
        let name = CString::new("my-wayland").expect("application name");
        let app_info = vk::ApplicationInfo::default()
            .application_name(&name)
            .application_version(1)
            .engine_name(&name)
            .engine_version(1)
            .api_version(vk::API_VERSION_1_2);
        let create_info = vk::InstanceCreateInfo::default().application_info(&app_info);
        let instance = unsafe { entry.create_instance(&create_info, None) }
            .map_err(|error| format!("failed to create the vulkan instance: {error}"))?;
        Ok(Self { _entry: entry, instance })
    }
}
