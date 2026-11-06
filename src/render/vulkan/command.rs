use ash::{vk, Device};

pub(super) fn create_pool(device: &Device, queue_family: u32) -> Result<vk::CommandPool, String> {
    let info = vk::CommandPoolCreateInfo::default()
        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
        .queue_family_index(queue_family);
    unsafe { device.create_command_pool(&info, None) }
        .map_err(|error| format!("failed to create a vulkan command pool: {error}"))
}

pub(super) fn allocate_command(
    device: &Device,
    pool: vk::CommandPool,
) -> Result<vk::CommandBuffer, String> {
    let info = vk::CommandBufferAllocateInfo::default()
        .command_pool(pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    unsafe { device.allocate_command_buffers(&info) }
        .map_err(|error| format!("failed to allocate a vulkan command buffer: {error}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "vulkan allocated no command buffer".to_string())
}

/// Layout and queue-family transfer recorded by [`image_ownership_barrier`].
///
/// Jay (`src/gfx_apis/vulkan/renderer.rs`) and wlroots (`render/vulkan/pass.c`)
/// use the same acquire and release for dma-buf images: take the image from
/// `VK_QUEUE_FAMILY_FOREIGN_EXT`, and give it back in `GENERAL`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BarrierSpec {
    pub(super) old_layout: vk::ImageLayout,
    pub(super) new_layout: vk::ImageLayout,
    pub(super) src_family: u32,
    pub(super) dst_family: u32,
    pub(super) src_stage: vk::PipelineStageFlags,
    pub(super) dst_stage: vk::PipelineStageFlags,
    pub(super) src_access: vk::AccessFlags,
    pub(super) dst_access: vk::AccessFlags,
}

/// Acquire a dma-buf image from the foreign queue onto `graphics_family`.
///
/// The first use discards (`UNDEFINED`). Later uses keep the contents, which
/// the matching release left in `GENERAL`.
pub(super) fn foreign_acquire(
    first_use: bool,
    new_layout: vk::ImageLayout,
    dst_stage: vk::PipelineStageFlags,
    dst_access: vk::AccessFlags,
    graphics_family: u32,
) -> BarrierSpec {
    BarrierSpec {
        old_layout: if first_use { vk::ImageLayout::UNDEFINED } else { vk::ImageLayout::GENERAL },
        new_layout,
        src_family: vk::QUEUE_FAMILY_FOREIGN_EXT,
        dst_family: graphics_family,
        src_stage: vk::PipelineStageFlags::TOP_OF_PIPE,
        dst_stage,
        src_access: vk::AccessFlags::empty(),
        dst_access,
    }
}

/// Release a dma-buf image from `graphics_family` back to the foreign queue.
///
/// The foreign owner always receives the image in `GENERAL`.
pub(super) fn foreign_release(
    old_layout: vk::ImageLayout,
    src_stage: vk::PipelineStageFlags,
    src_access: vk::AccessFlags,
    graphics_family: u32,
) -> BarrierSpec {
    BarrierSpec {
        old_layout,
        new_layout: vk::ImageLayout::GENERAL,
        src_family: graphics_family,
        dst_family: vk::QUEUE_FAMILY_FOREIGN_EXT,
        src_stage,
        dst_stage: vk::PipelineStageFlags::BOTTOM_OF_PIPE,
        src_access,
        dst_access: vk::AccessFlags::empty(),
    }
}

pub(super) fn record_barrier(
    device: &Device,
    command: vk::CommandBuffer,
    image: vk::Image,
    spec: BarrierSpec,
) {
    image_ownership_barrier(
        device,
        command,
        image,
        spec.old_layout,
        spec.new_layout,
        spec.src_stage,
        spec.dst_stage,
        spec.src_access,
        spec.dst_access,
        spec.src_family,
        spec.dst_family,
    );
}

pub(super) fn image_barrier(
    device: &Device,
    command: vk::CommandBuffer,
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    src_stage: vk::PipelineStageFlags,
    dst_stage: vk::PipelineStageFlags,
    src_access: vk::AccessFlags,
    dst_access: vk::AccessFlags,
) {
    image_ownership_barrier(
        device,
        command,
        image,
        old_layout,
        new_layout,
        src_stage,
        dst_stage,
        src_access,
        dst_access,
        vk::QUEUE_FAMILY_IGNORED,
        vk::QUEUE_FAMILY_IGNORED,
    );
}

pub(super) fn image_ownership_barrier(
    device: &Device,
    command: vk::CommandBuffer,
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    src_stage: vk::PipelineStageFlags,
    dst_stage: vk::PipelineStageFlags,
    src_access: vk::AccessFlags,
    dst_access: vk::AccessFlags,
    src_family: u32,
    dst_family: u32,
) {
    let barrier = vk::ImageMemoryBarrier::default()
        .image(image)
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_queue_family_index(src_family)
        .dst_queue_family_index(dst_family)
        .src_access_mask(src_access)
        .dst_access_mask(dst_access)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    unsafe {
        device.cmd_pipeline_barrier(
            command,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            std::slice::from_ref(&barrier),
        );
    }
}

#[cfg(test)]
mod tests {
    use ash::vk;

    use super::{foreign_acquire, foreign_release};

    #[test]
    fn foreign_acquire_discards_on_the_first_use() {
        let spec = foreign_acquire(
            true,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            0,
        );
        assert_eq!(spec.old_layout, vk::ImageLayout::UNDEFINED);
        assert_eq!(spec.new_layout, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        assert_eq!(spec.src_family, vk::QUEUE_FAMILY_FOREIGN_EXT);
        assert_eq!(spec.dst_family, 0);
        assert_eq!(spec.src_stage, vk::PipelineStageFlags::TOP_OF_PIPE);
        assert_eq!(spec.dst_stage, vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT);
        assert_eq!(spec.src_access, vk::AccessFlags::empty());
        assert_eq!(
            spec.dst_access,
            vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE
        );
        assert_ne!(spec.src_family, vk::QUEUE_FAMILY_IGNORED);
    }

    #[test]
    fn foreign_acquire_keeps_contents_after_the_first_use() {
        let spec = foreign_acquire(
            false,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::SHADER_READ,
            2,
        );
        assert_eq!(spec.old_layout, vk::ImageLayout::GENERAL);
        assert_eq!(spec.new_layout, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        assert_eq!(spec.src_family, vk::QUEUE_FAMILY_FOREIGN_EXT);
        assert_eq!(spec.dst_family, 2);
        assert_eq!(spec.src_stage, vk::PipelineStageFlags::TOP_OF_PIPE);
        assert_eq!(spec.dst_stage, vk::PipelineStageFlags::FRAGMENT_SHADER);
        assert_eq!(spec.src_access, vk::AccessFlags::empty());
        assert_eq!(spec.dst_access, vk::AccessFlags::SHADER_READ);
    }

    #[test]
    fn foreign_release_returns_general_to_the_foreign_queue() {
        let spec = foreign_release(
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            1,
        );
        assert_eq!(spec.old_layout, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        assert_eq!(spec.new_layout, vk::ImageLayout::GENERAL);
        assert_eq!(spec.src_family, 1);
        assert_eq!(spec.dst_family, vk::QUEUE_FAMILY_FOREIGN_EXT);
        assert_eq!(spec.src_stage, vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT);
        assert_eq!(spec.dst_stage, vk::PipelineStageFlags::BOTTOM_OF_PIPE);
        assert_eq!(spec.src_access, vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
        assert_eq!(spec.dst_access, vk::AccessFlags::empty());
        assert_ne!(spec.dst_family, vk::QUEUE_FAMILY_IGNORED);
    }
}
