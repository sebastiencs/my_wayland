use ash::{vk, Device};

pub(super) fn create_pool(device: &Device, queue_family: u32) -> Result<vk::CommandPool, String> {
    let info = vk::CommandPoolCreateInfo::default()
        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
        .queue_family_index(queue_family);
    unsafe { device.create_command_pool(&info, None) }
        .map_err(|error| format!("failed to create a vulkan command pool: {error}"))
}
