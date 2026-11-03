//! One textured frame. The command buffer is waited on before `render` returns.

use {ash::vk, gpu_alloc::UsageFlags};

use std::rc::Rc;

use crate::{
    region::{DamageRegion, RectI},
    render::{
        FrameElement, RenderCompletion, RenderFrame, RendererError, TextureElement, TextureMask,
    },
    transform::{self, Mat3, OutputPresentationTransform},
};

use super::{
    allocator::AllocatedBuffer, command, device::VulkanDevice, pipeline::DrawState,
    resources::PaintedTarget, texture,
};

#[repr(C)]
struct DrawUniforms {
    transform_c0: [f32; 4],
    transform_c1: [f32; 4],
    transform_c2: [f32; 4],
    tex_c0: [f32; 4],
    tex_c1: [f32; 4],
    tex_c2: [f32; 4],
    item_size: [f32; 2],
    alpha: f32,
    clip_mode: i32,
    outer_rect: [f32; 4],
    inner_rect: [f32; 4],
    outer_radius: f32,
    outer_power: f32,
    inner_radius: f32,
    inner_power: f32,
}

const _: () = assert!(std::mem::size_of::<DrawUniforms>() == 160);
const _: () = assert!(std::mem::offset_of!(DrawUniforms, item_size) == 96);
const _: () = assert!(std::mem::offset_of!(DrawUniforms, outer_rect) == 112);

struct PreparedDraw {
    set: vk::DescriptorSet,
    _uniform: AllocatedBuffer,
    scissors: Vec<vk::Rect2D>,
}

pub(super) fn paint(
    device: &Rc<VulkanDevice>,
    draw: &mut DrawState,
    target: &dyn PaintedTarget,
    frame: RenderFrame<'_>,
) -> Result<RenderCompletion, RendererError> {
    if target.width() <= 0 || target.height() <= 0 {
        log::error!("vulkan render target has no positive size");
        return Err(RendererError::RenderFailed);
    }
    draw.reset_descriptors().map_err(|error| {
        log::error!("{error}");
        RendererError::RenderFailed
    })?;
    let mut skipped_effect = false;
    let mut prepared = Vec::new();
    for element in &frame.recipe.elements {
        match element {
            FrameElement::Texture { element, .. } => {
                if let Some(prepared_draw) = prepare_texture(device, draw, target, element, &frame)?
                {
                    prepared.push(prepared_draw);
                }
            }
            FrameElement::Effect { .. } => skipped_effect = true,
        }
    }
    if skipped_effect {
        log::debug!("vulkan skipped effects in this frame");
    }
    let clear_rects =
        clear_rects(target, frame.damage, frame.preserve_contents, frame.presentation_transform);
    device.submit(RendererError::RenderFailed, |vulkan, command| {
        let old_layout = target.layout().get();
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
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            src_stage,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            src_access,
            vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
        );
        let begin = vk::RenderPassBeginInfo::default()
            .render_pass(target.render_pass())
            .framebuffer(target.framebuffer())
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: vk::Extent2D {
                    width: target.width() as u32,
                    height: target.height() as u32,
                },
            });
        unsafe {
            vulkan.cmd_begin_render_pass(command, &begin, vk::SubpassContents::INLINE);
            if !clear_rects.is_empty() {
                let attachment = vk::ClearAttachment::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .color_attachment(0)
                    .clear_value(vk::ClearValue {
                        color: vk::ClearColorValue { float32: target.clear_color() },
                    });
                let regions: Vec<vk::ClearRect> = clear_rects
                    .iter()
                    .copied()
                    .map(|rect| vk::ClearRect { rect, base_array_layer: 0, layer_count: 1 })
                    .collect();
                vulkan.cmd_clear_attachments(command, std::slice::from_ref(&attachment), &regions);
            }
            vulkan.cmd_bind_pipeline(command, vk::PipelineBindPoint::GRAPHICS, draw.pipeline());
            let viewport = vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: target.width() as f32,
                height: target.height() as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            };
            vulkan.cmd_set_viewport(command, 0, std::slice::from_ref(&viewport));
            vulkan.cmd_bind_vertex_buffers(
                command,
                0,
                std::slice::from_ref(&draw.vertices()),
                &[0],
            );
            for prepared in &prepared {
                vulkan.cmd_bind_descriptor_sets(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    draw.pipeline_layout(),
                    0,
                    std::slice::from_ref(&prepared.set),
                    &[],
                );
                for scissor in &prepared.scissors {
                    vulkan.cmd_set_scissor(command, 0, std::slice::from_ref(scissor));
                    vulkan.cmd_draw(command, 4, 1, 0, 0);
                }
            }
            vulkan.cmd_end_render_pass(command);
        }
        Ok(())
    })?;
    target
        .layout()
        .set(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    Ok(RenderCompletion::blocking_fallback())
}

fn prepare_texture(
    device: &Rc<VulkanDevice>,
    draw: &mut DrawState,
    target: &dyn PaintedTarget,
    element: &TextureElement,
    frame: &RenderFrame<'_>,
) -> Result<Option<PreparedDraw>, RendererError> {
    let Some(texture) = texture::sampled_texture(element.texture.as_ref(), device.device_id())
    else {
        return Ok(None);
    };
    let Some(item_bounds) = RectI::new(element.x, element.y, element.width, element.height) else {
        return Ok(None);
    };
    let Some(output_bounds) = RectI::new(0, 0, target.width(), target.height()) else {
        return Ok(None);
    };
    let mut clipped = frame.damage.clone();
    clipped.intersect_rect(output_bounds);
    clipped.intersect_rect(item_bounds);
    clipped.intersect_rect(element.damage.extents().unwrap_or(item_bounds));
    if clipped.is_empty() {
        return Ok(None);
    }
    let scissors = clipped
        .rects()
        .iter()
        .filter_map(|rect| framebuffer_rect(target.height(), *rect, frame.presentation_transform))
        .filter_map(vulkan_scissor)
        .collect::<Vec<_>>();
    if scissors.is_empty() {
        return Ok(None);
    }
    let projection = transform::output_projection(
        target.width() as f32,
        target.height() as f32,
        frame.presentation_transform,
    );
    let uniforms = uniforms_for(element, projection);
    let uniform = AllocatedBuffer::create(
        device,
        std::mem::size_of::<DrawUniforms>() as u64,
        vk::BufferUsageFlags::UNIFORM_BUFFER,
        UsageFlags::UPLOAD | UsageFlags::TRANSIENT,
    )
    .map_err(|error| {
        log::error!("{error}");
        RendererError::RenderFailed
    })?;
    let bytes = unsafe {
        std::slice::from_raw_parts(
            (&uniforms as *const DrawUniforms).cast::<u8>(),
            std::mem::size_of::<DrawUniforms>(),
        )
    };
    uniform.write(bytes).map_err(|error| {
        log::error!("{error}");
        RendererError::RenderFailed
    })?;
    let set = draw.allocate_set().map_err(|error| {
        log::error!("{error}");
        RendererError::RenderFailed
    })?;
    write_set(&device.device, set, draw.sampler(), texture.view(), uniform.buffer());
    Ok(Some(PreparedDraw { set, _uniform: uniform, scissors }))
}

fn write_set(
    device: &ash::Device,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    view: vk::ImageView,
    buffer: vk::Buffer,
) {
    let image = vk::DescriptorImageInfo {
        sampler,
        image_view: view,
        image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
    };
    let uniform = vk::DescriptorBufferInfo {
        buffer,
        offset: 0,
        range: std::mem::size_of::<DrawUniforms>() as u64,
    };
    let writes = [
        vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(std::slice::from_ref(&image)),
        vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(1)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .buffer_info(std::slice::from_ref(&uniform)),
    ];
    unsafe { device.update_descriptor_sets(&writes, &[]) };
}

fn uniforms_for(element: &TextureElement, projection: Mat3) -> DrawUniforms {
    let transform = columns(transform::project_box(
        element.x as f32,
        element.y as f32,
        element.width as f32,
        element.height as f32,
        projection,
    ));
    let tex = columns(element.tex_transform);
    let clip = clip_uniforms(&element.mask);
    DrawUniforms {
        transform_c0: transform[0],
        transform_c1: transform[1],
        transform_c2: transform[2],
        tex_c0: tex[0],
        tex_c1: tex[1],
        tex_c2: tex[2],
        item_size: [element.width as f32, element.height as f32],
        alpha: f32::from(element.alpha) / 255.0,
        clip_mode: clip.mode,
        outer_rect: clip.outer,
        inner_rect: clip.inner,
        outer_radius: clip.outer_radius,
        outer_power: clip.outer_power,
        inner_radius: clip.inner_radius,
        inner_power: clip.inner_power,
    }
}

fn columns(transform: Mat3) -> [[f32; 4]; 3] {
    let packed = transform.to_gl_array();
    [
        [packed[0], packed[1], packed[2], 0.0],
        [packed[3], packed[4], packed[5], 0.0],
        [packed[6], packed[7], packed[8], 0.0],
    ]
}

struct ClipUniforms {
    mode: i32,
    outer: [f32; 4],
    inner: [f32; 4],
    outer_radius: f32,
    outer_power: f32,
    inner_radius: f32,
    inner_power: f32,
}

fn clip_uniforms(mask: &TextureMask) -> ClipUniforms {
    match mask {
        TextureMask::None => ClipUniforms {
            mode: 0,
            outer: [0.0; 4],
            inner: [0.0; 4],
            outer_radius: 0.0,
            outer_power: 0.0001,
            inner_radius: 0.0,
            inner_power: 0.0001,
        },
        TextureMask::RoundedClip { clip } => ClipUniforms {
            mode: 1,
            outer: rect_vec(clip.outer_rect),
            inner: [0.0; 4],
            outer_radius: clip.corner_radius.max(0) as f32,
            outer_power: clip.rounding_power.get().max(0.0001),
            inner_radius: 0.0,
            inner_power: 0.0001,
        },
        TextureMask::FrameRing { ring } => ClipUniforms {
            mode: 2,
            outer: rect_vec(ring.outer.outer_rect),
            inner: rect_vec(ring.inner.outer_rect),
            outer_radius: ring.outer.corner_radius.max(0) as f32,
            outer_power: ring.outer.rounding_power.get().max(0.0001),
            inner_radius: ring.inner.corner_radius.max(0) as f32,
            inner_power: ring.inner.rounding_power.get().max(0.0001),
        },
    }
}

fn rect_vec(rect: RectI) -> [f32; 4] {
    [rect.x as f32, rect.y as f32, rect.width as f32, rect.height as f32]
}

fn clear_rects(
    target: &dyn PaintedTarget,
    damage: &DamageRegion,
    preserve_contents: bool,
    presentation: OutputPresentationTransform,
) -> Vec<vk::Rect2D> {
    let Some(bounds) = RectI::new(0, 0, target.width(), target.height()) else {
        return Vec::new();
    };
    let mut clipped = damage.clone();
    clipped.intersect_rect(bounds);
    if !preserve_contents || clipped.extents() == Some(bounds) {
        return vec![vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: vk::Extent2D { width: target.width() as u32, height: target.height() as u32 },
        }];
    }
    clipped
        .rects()
        .iter()
        .filter_map(|rect| framebuffer_rect(target.height(), *rect, presentation))
        .filter_map(vulkan_scissor)
        .collect()
}

fn framebuffer_rect(
    target_height: i32,
    rect: RectI,
    presentation: OutputPresentationTransform,
) -> Option<RectI> {
    RectI::new(
        rect.x,
        match presentation {
            OutputPresentationTransform::Normal => target_height - rect.y - rect.height,
            OutputPresentationTransform::FlipY => rect.y,
        },
        rect.width,
        rect.height,
    )
}

fn vulkan_scissor(rect: RectI) -> Option<vk::Rect2D> {
    if rect.width <= 0 || rect.height <= 0 {
        return None;
    }
    Some(vk::Rect2D {
        offset: vk::Offset2D { x: rect.x, y: rect.y },
        extent: vk::Extent2D { width: rect.width as u32, height: rect.height as u32 },
    })
}
