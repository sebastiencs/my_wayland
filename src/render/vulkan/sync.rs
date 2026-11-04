use std::{
    os::fd::{IntoRawFd, OwnedFd},
    rc::Rc,
};

use ash::vk;

use super::{device::VulkanDevice, instance::VulkanInstance};

/// Sync-file import and export are required. Phase 11 uses them for KMS fences.
///
/// Jay imports each dma-buf sync file as a temporary binary semaphore and keeps a
/// free list of command buffers. A client timeline point is exported to a sync
/// file first, the same reduction Hyprland and the GLES renderer already use.
/// The completion object is a binary semaphore exported as `SYNC_FD` for KMS
/// `IN_FENCE_FD`. Jay signals that completion with a fence export instead.
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

/// Binary semaphore whose payload is one imported sync file.
///
/// The import is temporary, matching Jay: the next queue wait consumes it.
pub(super) struct StashedSemaphore {
    device: Rc<VulkanDevice>,
    semaphore: vk::Semaphore,
}

impl StashedSemaphore {
    pub(super) fn handle(&self) -> vk::Semaphore {
        self.semaphore
    }

    pub(super) fn into_handle(mut self) -> vk::Semaphore {
        std::mem::replace(&mut self.semaphore, vk::Semaphore::null())
    }
}

impl Drop for StashedSemaphore {
    fn drop(&mut self) {
        if self.semaphore != vk::Semaphore::null() {
            unsafe {
                self.device.device.destroy_semaphore(self.semaphore, None);
            }
        }
    }
}

/// Wait semaphores owned by one submit. Drop destroys any handle not moved out.
pub(super) struct OwnedSemaphores {
    device: Rc<VulkanDevice>,
    handles: Vec<vk::Semaphore>,
}

impl OwnedSemaphores {
    pub(super) fn from_stashed(device: &Rc<VulkanDevice>, stashed: Vec<StashedSemaphore>) -> Self {
        Self {
            device: device.clone(),
            handles: stashed
                .into_iter()
                .map(StashedSemaphore::into_handle)
                .collect(),
        }
    }

    pub(super) fn as_slice(&self) -> &[vk::Semaphore] {
        &self.handles
    }

    pub(super) fn into_handles(mut self) -> Vec<vk::Semaphore> {
        std::mem::take(&mut self.handles)
    }
}

impl Drop for OwnedSemaphores {
    fn drop(&mut self) {
        for semaphore in self.handles.drain(..) {
            unsafe {
                self.device.device.destroy_semaphore(semaphore, None);
            }
        }
    }
}

pub(super) fn import_sync_file(
    device: &Rc<VulkanDevice>,
    sync_file: OwnedFd,
) -> Result<StashedSemaphore, String> {
    let semaphore = unsafe {
        device
            .device
            .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
    }
    .map_err(|error| format!("vulkan semaphore creation failed: {error}"))?;
    // `vkImportSemaphoreFdKHR` closes the fd on both success and failure.
    let raw = sync_file.into_raw_fd();
    let info = vk::ImportSemaphoreFdInfoKHR::default()
        .semaphore(semaphore)
        .flags(vk::SemaphoreImportFlags::TEMPORARY)
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
        .fd(raw);
    if let Err(error) = unsafe { device.external_semaphore.import_semaphore_fd(&info) } {
        unsafe { device.device.destroy_semaphore(semaphore, None) };
        return Err(format!("vulkan sync-file import failed: {error}"));
    }
    Ok(StashedSemaphore { device: device.clone(), semaphore })
}
