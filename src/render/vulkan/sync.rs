use std::{
    io,
    os::fd::{AsRawFd, IntoRawFd, OwnedFd},
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

/// Export each sampled dma-buf's read fence and turn it into a submit wait.
///
/// Jay does this for `AcquireSync::Implicit` (`renderer.rs`). On a kernel
/// without the ioctl, wlroots polls the dma-buf for up to 1000 ms instead
/// (`renderer.c`, `buffer_export_sync_file`).
pub(super) fn implicit_read_waits(
    device: &Rc<VulkanDevice>,
    planes: &[&OwnedFd],
) -> Result<Vec<StashedSemaphore>, crate::render::RendererError> {
    if planes.is_empty() {
        return Ok(Vec::new());
    }
    if !device.implicit_sync_available() {
        poll_dma_bufs(planes)?;
        return Ok(Vec::new());
    }
    let mut waits = Vec::new();
    for plane in planes {
        match crate::dmabuf::export_sync_file(*plane, crate::dmabuf::DMA_BUF_SYNC_READ) {
            Ok(sync_file) => waits.push(import_sync_file(device, sync_file).map_err(|error| {
                log::error!("{error}");
                crate::render::RendererError::RenderFailed
            })?),
            Err(error) if implicit_sync_unsupported(&error) => {
                device.note_implicit_sync_unsupported();
                drop(waits);
                poll_dma_bufs(planes)?;
                return Ok(Vec::new());
            }
            Err(error) => {
                log::error!("dma-buf sync-file export failed: {error}");
                return Err(crate::render::RendererError::RenderFailed);
            }
        }
    }
    Ok(waits)
}

/// Make the client's next write wait for this read. Blocking submits have
/// already finished, so they skip this. wlroots imports the same sync file
/// onto every plane (`renderer.c`, `vulkan_sync_render_pass_release`).
pub(super) fn attach_implicit_read_fence(
    device: &VulkanDevice,
    planes: &[&OwnedFd],
    render_fence: &OwnedFd,
) {
    if planes.is_empty() || !device.implicit_sync_available() {
        return;
    }
    let Ok(sync_file) = super::memory::duplicate_fd(render_fence) else {
        return;
    };
    for plane in planes {
        if let Err(error) =
            crate::dmabuf::import_sync_file(*plane, crate::dmabuf::DMA_BUF_SYNC_READ, &sync_file)
        {
            if implicit_sync_unsupported(&error) {
                device.note_implicit_sync_unsupported();
            } else {
                log::error!("dma-buf sync-file import failed: {error}");
            }
            return;
        }
    }
}

fn implicit_sync_unsupported(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOTTY) | Some(libc::EINVAL))
}

fn poll_dma_bufs(planes: &[&OwnedFd]) -> Result<(), crate::render::RendererError> {
    for plane in planes {
        let mut polled = libc::pollfd { fd: plane.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ready = unsafe { libc::poll(&mut polled, 1, 1000) };
        if ready < 0 {
            log::error!("dma-buf poll failed: {}", io::Error::last_os_error());
            return Err(crate::render::RendererError::RenderFailed);
        }
        if ready == 0 {
            log::error!("timed out waiting for a dma-buf fence");
            return Err(crate::render::RendererError::RenderFailed);
        }
        if polled.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            log::error!("dma-buf poll failed");
            return Err(crate::render::RendererError::RenderFailed);
        }
    }
    Ok(())
}
