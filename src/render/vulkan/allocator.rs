//! Host-visible staging and device-local image memory.
//!
//! Jay allocates the same way (`Config::i_am_prototyping` plus `gpu-alloc-ash`).
//! dma-buf import stays on dedicated memory in `memory::import_memory`.

use std::{cell::Cell, rc::Rc};

use {
    ash::vk,
    gpu_alloc::{MemoryBlock, UsageFlags},
    gpu_alloc_ash::AshMemoryDevice,
};

use {super::device::VulkanDevice, crate::render::RendererError};

pub(super) struct GpuAllocation {
    device: Rc<VulkanDevice>,
    memory: vk::DeviceMemory,
    offset: u64,
    block: Cell<Option<MemoryBlock<vk::DeviceMemory>>>,
}

impl GpuAllocation {
    pub(super) fn memory(&self) -> vk::DeviceMemory {
        self.memory
    }

    pub(super) fn offset(&self) -> u64 {
        self.offset
    }

    pub(super) fn write_bytes(&self, offset: u64, data: &[u8]) -> Result<(), String> {
        self.with_block(|block| {
            unsafe { block.write_bytes(AshMemoryDevice::wrap(&self.device.device), offset, data) }
                .map_err(|error| format!("vulkan host write failed: {error}"))
        })
    }

    pub(super) fn read_bytes(&self, offset: u64, data: &mut [u8]) -> Result<(), String> {
        self.with_block(|block| {
            unsafe { block.read_bytes(AshMemoryDevice::wrap(&self.device.device), offset, data) }
                .map_err(|error| format!("vulkan host read failed: {error}"))
        })
    }

    fn with_block<T>(
        &self,
        body: impl FnOnce(&mut MemoryBlock<vk::DeviceMemory>) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut block = self
            .block
            .take()
            .expect("vulkan allocation used while it was being mapped");
        let result = body(&mut block);
        self.block.set(Some(block));
        result
    }
}

impl Drop for GpuAllocation {
    fn drop(&mut self) {
        let Some(block) = self.block.take() else {
            return;
        };
        unsafe {
            self.device
                .allocator()
                .dealloc(AshMemoryDevice::wrap(&self.device.device), block);
        }
    }
}

pub(super) struct AllocatedBuffer {
    device: Rc<VulkanDevice>,
    buffer: vk::Buffer,
    allocation: GpuAllocation,
}

impl AllocatedBuffer {
    pub(super) fn create(
        device: &Rc<VulkanDevice>,
        size: u64,
        usage: vk::BufferUsageFlags,
        memory_usage: UsageFlags,
    ) -> Result<Self, String> {
        let info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { device.device.create_buffer(&info, None) }
            .map_err(|error| format!("vulkan buffer creation failed: {error}"))?;
        let requirements = unsafe { device.device.get_buffer_memory_requirements(buffer) };
        let allocation = match device.allocate(requirements, memory_usage) {
            Ok(allocation) => allocation,
            Err(error) => {
                unsafe { device.device.destroy_buffer(buffer, None) };
                return Err(error);
            }
        };
        if let Err(error) = unsafe {
            device
                .device
                .bind_buffer_memory(buffer, allocation.memory(), allocation.offset())
        } {
            unsafe { device.device.destroy_buffer(buffer, None) };
            return Err(format!("vulkan buffer bind failed: {error}"));
        }
        Ok(Self { device: device.clone(), buffer, allocation })
    }

    pub(super) fn buffer(&self) -> vk::Buffer {
        self.buffer
    }

    pub(super) fn write(&self, bytes: &[u8]) -> Result<(), String> {
        self.allocation.write_bytes(0, bytes)
    }

    pub(super) fn read(&self, bytes: &mut [u8]) -> Result<(), String> {
        self.allocation.read_bytes(0, bytes)
    }
}

impl Drop for AllocatedBuffer {
    fn drop(&mut self) {
        unsafe { self.device.device.destroy_buffer(self.buffer, None) };
    }
}

impl VulkanDevice {
    pub(super) fn allocate(
        self: &Rc<Self>,
        requirements: vk::MemoryRequirements,
        usage: UsageFlags,
    ) -> Result<GpuAllocation, String> {
        let request = gpu_alloc::Request {
            size: requirements.size,
            align_mask: requirements.alignment.saturating_sub(1),
            usage,
            memory_types: requirements.memory_type_bits,
        };
        let block = unsafe {
            self.allocator()
                .alloc(AshMemoryDevice::wrap(&self.device), request)
        }
        .map_err(|error| format!("vulkan memory allocation failed: {error}"))?;
        Ok(GpuAllocation {
            device: self.clone(),
            memory: *block.memory(),
            offset: block.offset(),
            block: Cell::new(Some(block)),
        })
    }
}

pub(super) fn map_alloc_error(error: String, failed: RendererError) -> RendererError {
    log::error!("{error}");
    failed
}
