//! Top-left readback of a color image.
//!
//! Row 0 of the copy is image row 0. With the FlipY projection that row is the
//! same pixel GLES `read_flip_y_framebuffer` stores first. Output dma-bufs are
//! imported with `TRANSFER_SRC`, so both offscreen images and output targets
//! can be copied.

use std::rc::Rc;

use {ash::vk, gpu_alloc::UsageFlags};

use crate::{region::RectI, render::RendererError};

use super::{allocator::AllocatedBuffer, command, device::VulkanDevice, resources::PaintedTarget};

pub(super) fn read_color(
    device: &Rc<VulkanDevice>,
    target: &dyn PaintedTarget,
    rect: RectI,
    stride: usize,
    dst: &mut [u8],
) -> Result<(), RendererError> {
    if !target.can_readback() {
        log::debug!("vulkan output image cannot be copied");
        return Err(RendererError::ReadbackFailed);
    }
    if rect.width <= 0 || rect.height <= 0 {
        return Err(RendererError::ReadbackFailed);
    }
    let row_bytes = (rect.width as usize).saturating_mul(4);
    if stride < row_bytes {
        return Err(RendererError::ReadbackFailed);
    }
    let Some(needed) = stride.checked_mul(rect.height as usize) else {
        return Err(RendererError::ReadbackFailed);
    };
    if dst.len() < needed
        || rect.x < 0
        || rect.y < 0
        || rect.x.saturating_add(rect.width) > target.width()
        || rect.y.saturating_add(rect.height) > target.height()
    {
        return Err(RendererError::ReadbackFailed);
    }
    let tight = row_bytes * rect.height as usize;
    let buffer = AllocatedBuffer::create(
        device,
        tight as u64,
        vk::BufferUsageFlags::TRANSFER_DST,
        UsageFlags::DOWNLOAD | UsageFlags::TRANSIENT,
    )
    .map_err(|error| {
        log::error!("{error}");
        RendererError::ReadbackFailed
    })?;
    let old_layout = target.layout().get();
    let external = target.external();
    let graphics_family = device.graphics_queue_family;
    device.submit(RendererError::ReadbackFailed, |vulkan, command| {
        if external {
            command::record_barrier(
                vulkan,
                command,
                target.image(),
                command::foreign_acquire(
                    old_layout == vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::AccessFlags::TRANSFER_READ,
                    graphics_family,
                ),
            );
        } else {
            let (src_stage, src_access) = if old_layout == vk::ImageLayout::UNDEFINED {
                (vk::PipelineStageFlags::TOP_OF_PIPE, vk::AccessFlags::empty())
            } else {
                (
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                )
            };
            command::image_barrier(
                vulkan,
                command,
                target.image(),
                old_layout,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                src_stage,
                vk::PipelineStageFlags::TRANSFER,
                src_access,
                vk::AccessFlags::TRANSFER_READ,
            );
        }
        let region = vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_offset(vk::Offset3D { x: rect.x, y: rect.y, z: 0 })
            .image_extent(vk::Extent3D {
                width: rect.width as u32,
                height: rect.height as u32,
                depth: 1,
            });
        unsafe {
            vulkan.cmd_copy_image_to_buffer(
                command,
                target.image(),
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer.buffer(),
                std::slice::from_ref(&region),
            );
        }
        if external {
            command::record_barrier(
                vulkan,
                command,
                target.image(),
                command::foreign_release(
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::AccessFlags::TRANSFER_READ,
                    graphics_family,
                ),
            );
        } else {
            command::image_barrier(
                vulkan,
                command,
                target.image(),
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags::TRANSFER_READ,
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            );
        }
        Ok(())
    })?;
    target.layout().set(if external {
        vk::ImageLayout::GENERAL
    } else {
        vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
    });
    let mut tightly = vec![0_u8; tight];
    buffer.read(&mut tightly).map_err(|error| {
        log::error!("{error}");
        RendererError::ReadbackFailed
    })?;
    for row in 0..rect.height as usize {
        let src = row * row_bytes;
        let dst_row = row * stride;
        dst[dst_row..dst_row + row_bytes].copy_from_slice(&tightly[src..src + row_bytes]);
    }
    Ok(())
}
