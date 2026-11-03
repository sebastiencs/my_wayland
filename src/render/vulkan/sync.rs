use ash::vk;

use super::instance::VulkanInstance;

/// Sync-file import and export are required. Phase 11 uses them for KMS fences.
pub(super) fn require_sync_file(
    instance: &VulkanInstance,
    physical_device: vk::PhysicalDevice,
) -> Result<(), String> {
    let info = vk::PhysicalDeviceExternalSemaphoreInfo::default()
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    let mut properties = vk::ExternalSemaphoreProperties::default();
    unsafe {
        instance
            .instance
            .get_physical_device_external_semaphore_properties(
                physical_device,
                &info,
                &mut properties,
            );
    }
    let features = properties.external_semaphore_features;
    let importable = features.contains(vk::ExternalSemaphoreFeatureFlags::IMPORTABLE);
    let exportable = features.contains(vk::ExternalSemaphoreFeatureFlags::EXPORTABLE);
    if importable && exportable {
        return Ok(());
    }
    let missing = match (importable, exportable) {
        (false, false) => "import and export",
        (false, true) => "import",
        (true, false) => "export",
        (true, true) => unreachable!(),
    };
    Err(format!("vulkan device does not support sync-file semaphore {missing}"))
}
