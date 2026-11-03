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
    let barrier = vk::ImageMemoryBarrier::default()
        .image(image)
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
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
