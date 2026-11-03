use std::{
    any::Any,
    cell::{Cell, RefCell, UnsafeCell},
    ffi::{c_char, CStr, CString},
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
    rc::Rc,
};

use {
    ash::{vk, Device},
    drm::buffer::DrmFourcc,
    gpu_alloc::{Config, GpuAllocator},
    gpu_alloc_ash::AshMemoryDevice,
};

use {
    super::{
        command,
        format::{self, FormatSet},
        instance::VulkanInstance,
        sync,
    },
    crate::render::{RenderDeviceId, RendererError},
};

pub(super) struct VulkanDevice {
    pub(super) device: Device,
    pub(super) graphics_queue: vk::Queue,
    pub(super) graphics_queue_family: u32,
    command_pool: vk::CommandPool,
    /// Command buffers whose GPU work has finished. Jay keeps the same free list.
    ///
    /// Interior mutability because a finished `PendingFrame` recycles its
    /// command buffer from its `Drop`, where only the shared `Rc<VulkanDevice>`
    /// is reachable. `Rc::get_mut` cannot work while resources hold clones, so
    /// mutating through `&self` is the only option. The alternative is an
    /// arena design where the renderer is the sole owner and each frame is
    /// disposed through an explicit call that has `&mut` access.
    free_commands: RefCell<Vec<vk::CommandBuffer>>,
    /// Interior mutability because gpu-alloc takes `&mut GpuAllocator`, while
    /// a `GpuAllocation` frees its memory from its `Drop`, where only the
    /// shared `Rc<VulkanDevice>` is reachable. `RefCell` cannot hand out a
    /// `&mut` that outlives the accessor, so this is a raw cell. Sound because
    /// the renderer is `!Send`/`!Sync` and allocation calls never re-enter.
    /// The alternative is explicit destruction through the renderer, arena
    /// style, with every drop site rerouted to a `&mut` renderer call.
    allocator: UnsafeCell<GpuAllocator<vk::DeviceMemory>>,
    pub(super) external_memory: ash::khr::external_memory_fd::Device,
    pub(super) external_semaphore: ash::khr::external_semaphore_fd::Device,
    pub(super) memory_properties: vk::PhysicalDeviceMemoryProperties,
    pub(super) formats: FormatSet,
    device_id: RenderDeviceId,
}

/// One explicit submission. The renderer keeps it until the fence signals.
pub(super) struct PendingFrame {
    device: Rc<VulkanDevice>,
    resources: Option<Box<dyn Any>>,
    fence: vk::Fence,
    semaphores: Vec<vk::Semaphore>,
    descriptor_pool: vk::DescriptorPool,
    command: vk::CommandBuffer,
}

impl PendingFrame {
    pub(super) fn signaled(&self) -> bool {
        match unsafe { self.device.device.get_fence_status(self.fence) } {
            Ok(signaled) => signaled,
            Err(error) => {
                log::warn!("vulkan fence status failed: {error}");
                false
            }
        }
    }
}

impl Drop for PendingFrame {
    fn drop(&mut self) {
        self.resources.take();
        unsafe {
            self.device.device.destroy_fence(self.fence, None);
            for semaphore in self.semaphores.drain(..) {
                self.device.device.destroy_semaphore(semaphore, None);
            }
            if self.descriptor_pool != vk::DescriptorPool::null() {
                self.device
                    .device
                    .destroy_descriptor_pool(self.descriptor_pool, None);
            }
        }
        self.device.recycle_command(self.command);
    }
}

pub(super) struct DetachedSubmit {
    command: vk::CommandBuffer,
    fence: vk::Fence,
    signal: vk::Semaphore,
    sync_fd: OwnedFd,
}

impl DetachedSubmit {
    pub(super) fn into_frame(
        self,
        device: &Rc<VulkanDevice>,
        mut waits: Vec<vk::Semaphore>,
        descriptor_pool: vk::DescriptorPool,
        resources: Box<dyn Any>,
    ) -> (OwnedFd, PendingFrame) {
        waits.push(self.signal);
        let frame = PendingFrame {
            device: device.clone(),
            resources: Some(resources),
            fence: self.fence,
            semaphores: waits,
            descriptor_pool,
            command: self.command,
        };
        (self.sync_fd, frame)
    }
}

pub(super) enum RenderedSubmit {
    Blocked,
    Explicit(DetachedSubmit),
}

impl Drop for VulkanDevice {
    fn drop(&mut self) {
        unsafe {
            if let Err(error) = self.device.device_wait_idle() {
                log::error!("vulkan device wait before destruction failed: {error}");
            }
            self.allocator
                .get_mut()
                .cleanup(AshMemoryDevice::wrap(&self.device));
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
        let command_buffer = match command::allocate_command(&device, command_pool) {
            Ok(buffer) => buffer,
            Err(error) => {
                abandon(device, Some(command_pool));
                return Err(error);
            }
        };
        let external_semaphore =
            ash::khr::external_semaphore_fd::Device::new(&instance.instance, &device);
        let mut memory_info = match unsafe {
            gpu_alloc_ash::device_properties(&instance.instance, vk::API_VERSION_1_2, physical)
        } {
            Ok(properties) => properties,
            Err(error) => {
                abandon(device, Some(command_pool));
                return Err(format!("failed to query vulkan memory properties: {error}"));
            }
        };
        // Buffer device addresses are not enabled on this device.
        memory_info.buffer_device_address = false;
        let allocator = GpuAllocator::new(Config::i_am_prototyping(), memory_info);
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
            free_commands: RefCell::new(vec![command_buffer]),
            allocator: UnsafeCell::new(allocator),
            external_memory,
            external_semaphore,
            memory_properties,
            formats,
            device_id: RenderDeviceId::next(),
        }))
    }

    pub(super) fn device_id(&self) -> RenderDeviceId {
        self.device_id
    }

    pub(super) fn allocator(&self) -> &mut GpuAllocator<vk::DeviceMemory> {
        // The compositor records on one thread. Allocations are not shared.
        unsafe { &mut *self.allocator.get() }
    }

    /// Record, submit, and wait. Uploads and readback use this while a frame is in flight.
    pub(super) fn submit(
        &self,
        failed: RendererError,
        record: impl FnOnce(&Device, vk::CommandBuffer) -> Result<(), String>,
    ) -> Result<(), RendererError> {
        match self.record_submit(&[], false, record) {
            Ok(RenderedSubmit::Blocked) => Ok(()),
            Ok(RenderedSubmit::Explicit(_)) => {
                log::error!("vulkan blocking submit returned an explicit fence");
                Err(failed)
            }
            Err(error) => {
                log::error!("{error}");
                Err(failed)
            }
        }
    }

    /// Record a frame. Explicit mode returns a sync file without waiting.
    pub(super) fn submit_rendered(
        &self,
        explicit: bool,
        waits: &[vk::Semaphore],
        record: impl FnOnce(&Device, vk::CommandBuffer) -> Result<(), String>,
    ) -> Result<RenderedSubmit, RendererError> {
        self.record_submit(waits, explicit, record)
            .map_err(|error| {
                log::error!("{error}");
                RendererError::RenderFailed
            })
    }

    fn record_submit(
        &self,
        waits: &[vk::Semaphore],
        explicit: bool,
        record: impl FnOnce(&Device, vk::CommandBuffer) -> Result<(), String>,
    ) -> Result<RenderedSubmit, String> {
        let command = self.checkout()?;
        unsafe {
            if let Err(error) = self.device.begin_command_buffer(
                command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            ) {
                self.recycle_command(command);
                return Err(format!("failed to begin a vulkan command buffer: {error}"));
            }
            if let Err(error) = record(&self.device, command) {
                let _ = self.device.end_command_buffer(command);
                self.recycle_command(command);
                return Err(error);
            }
            if let Err(error) = self.device.end_command_buffer(command) {
                self.recycle_command(command);
                return Err(format!("failed to end a vulkan command buffer: {error}"));
            }
        }
        let fence = match unsafe {
            self.device
                .create_fence(&vk::FenceCreateInfo::default(), None)
        } {
            Ok(fence) => fence,
            Err(error) => {
                self.recycle_command(command);
                return Err(format!("failed to create a vulkan fence: {error}"));
            }
        };
        let signal = if explicit {
            match self.create_export_semaphore() {
                Ok(semaphore) => Some(semaphore),
                Err(error) => {
                    log::warn!(
                        "vulkan explicit fence export failed, falling back to a blocking finish: {error}"
                    );
                    if let Err(error) = self.queue_submit(command, waits, None, fence) {
                        unsafe { self.device.destroy_fence(fence, None) };
                        self.recycle_command(command);
                        return Err(error);
                    }
                    return self.finish_blocked(fence, command, None);
                }
            }
        } else {
            None
        };
        // Creating the export semaphore can fail before anything is queued. A
        // later export failure waits for the submission that already ran.
        if let Err(error) = self.queue_submit(command, waits, signal, fence) {
            unsafe {
                self.device.destroy_fence(fence, None);
                if let Some(semaphore) = signal {
                    self.device.destroy_semaphore(semaphore, None);
                }
            }
            self.recycle_command(command);
            return Err(error);
        }
        let Some(signal) = signal else {
            self.wait_fence_and_recycle(fence, command)?;
            return Ok(RenderedSubmit::Blocked);
        };
        match self.export_sync_fd(signal) {
            Ok(Some(sync_fd)) => {
                Ok(RenderedSubmit::Explicit(DetachedSubmit { command, fence, signal, sync_fd }))
            }
            Ok(None) => {
                log::warn!(
                    "vulkan explicit fence was already signaled, falling back to a blocking finish"
                );
                self.finish_blocked(fence, command, Some(signal))
            }
            Err(error) => {
                log::warn!(
                    "vulkan explicit fence export failed, falling back to a blocking finish: {error}"
                );
                self.finish_blocked(fence, command, Some(signal))
            }
        }
    }

    fn finish_blocked(
        &self,
        fence: vk::Fence,
        command: vk::CommandBuffer,
        signal: Option<vk::Semaphore>,
    ) -> Result<RenderedSubmit, String> {
        let waited = self.wait_fence_and_recycle(fence, command);
        if let Some(semaphore) = signal {
            unsafe { self.device.destroy_semaphore(semaphore, None) };
        }
        waited?;
        Ok(RenderedSubmit::Blocked)
    }

    fn wait_fence_and_recycle(
        &self,
        fence: vk::Fence,
        command: vk::CommandBuffer,
    ) -> Result<(), String> {
        let waited = unsafe {
            self.device
                .wait_for_fences(std::slice::from_ref(&fence), true, u64::MAX)
        };
        if let Err(error) = waited {
            log::error!("vulkan fence wait failed: {error}");
            unsafe {
                let _ = self.device.device_wait_idle();
            }
        }
        unsafe { self.device.destroy_fence(fence, None) };
        self.recycle_command(command);
        waited.map_err(|error| format!("vulkan fence wait failed: {error}"))
    }

    fn queue_submit(
        &self,
        command: vk::CommandBuffer,
        waits: &[vk::Semaphore],
        signal: Option<vk::Semaphore>,
        fence: vk::Fence,
    ) -> Result<(), String> {
        let stages = vec![vk::PipelineStageFlags::TOP_OF_PIPE; waits.len()];
        let signals: Vec<vk::Semaphore> = signal.into_iter().collect();
        let mut info = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&command));
        if !waits.is_empty() {
            info = info.wait_semaphores(waits).wait_dst_stage_mask(&stages);
        }
        if !signals.is_empty() {
            info = info.signal_semaphores(&signals);
        }
        unsafe {
            self.device
                .queue_submit(self.graphics_queue, std::slice::from_ref(&info), fence)
        }
        .map_err(|error| format!("vulkan queue submit failed: {error}"))
    }

    fn create_export_semaphore(&self) -> Result<vk::Semaphore, String> {
        let mut export = vk::ExportSemaphoreCreateInfo::default()
            .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
        let info = vk::SemaphoreCreateInfo::default().push_next(&mut export);
        unsafe { self.device.create_semaphore(&info, None) }
            .map_err(|error| format!("vulkan export semaphore creation failed: {error}"))
    }

    fn export_sync_fd(&self, semaphore: vk::Semaphore) -> Result<Option<OwnedFd>, String> {
        let info = vk::SemaphoreGetFdInfoKHR::default()
            .semaphore(semaphore)
            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
        let fd = unsafe { self.external_semaphore.get_semaphore_fd(&info) }
            .map_err(|error| format!("vulkan sync-file export failed: {error}"))?;
        if fd < 0 {
            Ok(None)
        } else {
            Ok(Some(unsafe { OwnedFd::from_raw_fd(fd) }))
        }
    }

    fn checkout(&self) -> Result<vk::CommandBuffer, String> {
        let command = match self.free_commands.borrow_mut().pop() {
            Some(command) => command,
            None => command::allocate_command(&self.device, self.command_pool)?,
        };
        if let Err(error) = unsafe {
            self.device
                .reset_command_buffer(command, vk::CommandBufferResetFlags::empty())
        } {
            log::error!("vulkan command buffer reset failed: {error}");
            return Err(format!("failed to reset a vulkan command buffer: {error}"));
        }
        Ok(command)
    }

    pub(super) fn recycle_command(&self, command: vk::CommandBuffer) {
        self.free_commands.borrow_mut().push(command);
    }
}

/// Sync file that stays unsignaled until [`BlockedSyncFile::release`].
///
/// An empty DRM syncobj has no dma-fence, so the kernel rejects exporting it.
/// The file is the pending signal of a command buffer that waits on a host
/// event. Tests use it as a client acquire without a kernel sw_sync device.
#[cfg(test)]
pub(super) struct BlockedSyncFile {
    device: Rc<VulkanDevice>,
    event: vk::Event,
    signal: vk::Semaphore,
    fence: vk::Fence,
    command: vk::CommandBuffer,
    fd: OwnedFd,
    released: Cell<bool>,
}

#[cfg(test)]
impl BlockedSyncFile {
    pub(super) fn open(device: &Rc<VulkanDevice>) -> Result<Self, String> {
        let event = unsafe {
            device
                .device
                .create_event(&vk::EventCreateInfo::default(), None)
        }
        .map_err(|error| format!("vulkan event creation failed: {error}"))?;
        let signal = match device.create_export_semaphore() {
            Ok(semaphore) => semaphore,
            Err(error) => {
                unsafe { device.device.destroy_event(event, None) };
                return Err(error);
            }
        };
        let command = match device.checkout() {
            Ok(command) => command,
            Err(error) => {
                unsafe {
                    device.device.destroy_event(event, None);
                    device.device.destroy_semaphore(signal, None);
                }
                return Err(error);
            }
        };
        let fail_recording = |error: String| -> Result<Self, String> {
            device.recycle_command(command);
            unsafe {
                device.device.destroy_event(event, None);
                device.device.destroy_semaphore(signal, None);
            }
            Err(error)
        };
        unsafe {
            if let Err(error) = device.device.begin_command_buffer(
                command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            ) {
                return fail_recording(format!("failed to begin a vulkan command buffer: {error}"));
            }
            device.device.cmd_wait_events(
                command,
                std::slice::from_ref(&event),
                vk::PipelineStageFlags::HOST,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                &[],
                &[],
                &[],
            );
            if let Err(error) = device.device.end_command_buffer(command) {
                return fail_recording(format!("failed to end a vulkan command buffer: {error}"));
            }
        }
        let fence = match unsafe {
            device
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)
        } {
            Ok(fence) => fence,
            Err(error) => {
                let _ = fail_recording(String::new());
                return Err(format!("failed to create a vulkan fence: {error}"));
            }
        };
        if let Err(error) = device.queue_submit(command, &[], Some(signal), fence) {
            unsafe {
                device.device.destroy_fence(fence, None);
                device.device.destroy_event(event, None);
                device.device.destroy_semaphore(signal, None);
            }
            device.recycle_command(command);
            return Err(error);
        }
        let fd = match device.export_sync_fd(signal)? {
            Some(fd) => fd,
            None => {
                unsafe {
                    let _ = device.device.set_event(event);
                    let _ = device.device.wait_for_fences(
                        std::slice::from_ref(&fence),
                        true,
                        2_000_000_000,
                    );
                    device.device.destroy_fence(fence, None);
                    device.device.destroy_event(event, None);
                    device.device.destroy_semaphore(signal, None);
                }
                device.recycle_command(command);
                return Err("vulkan sync file was already signaled".to_string());
            }
        };
        Ok(Self {
            device: device.clone(),
            event,
            signal,
            fence,
            command,
            fd,
            released: Cell::new(false),
        })
    }

    pub(super) fn fd(&self) -> &OwnedFd {
        &self.fd
    }

    pub(super) fn release(&self) -> Result<(), String> {
        if self.released.replace(true) {
            return Ok(());
        }
        unsafe { self.device.device.set_event(self.event) }
            .map_err(|error| format!("vulkan event signal failed: {error}"))
    }
}

#[cfg(test)]
impl Drop for BlockedSyncFile {
    fn drop(&mut self) {
        let _ = self.release();
        unsafe {
            let waited = self.device.device.wait_for_fences(
                std::slice::from_ref(&self.fence),
                true,
                2_000_000_000,
            );
            if waited.is_err() {
                log::error!("vulkan blocked sync file was still pending at drop");
                return;
            }
            self.device.device.destroy_fence(self.fence, None);
            self.device.device.destroy_event(self.event, None);
            self.device.device.destroy_semaphore(self.signal, None);
        }
        self.device.recycle_command(self.command);
    }
}

fn abandon(device: Device, pool: Option<vk::CommandPool>) {
    unsafe {
        if let Some(pool) = pool {
            device.destroy_command_pool(pool, None);
        }
        device.destroy_device(None);
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
