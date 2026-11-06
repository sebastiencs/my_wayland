//! One textured frame.
//!
//! `BlockingFinishFallback` waits before it returns. `ExplicitInFence` exports a
//! sync file and leaves the command buffer, descriptors, and blur images alive
//! until that fence signals.

use {ash::vk, gpu_alloc::UsageFlags};

use std::{os::fd::AsRawFd, rc::Rc};

use crate::{
    config::RgbaColor,
    region::{DamageRegion, RectI},
    render::{
        shadow_geometry, BlurVisualStyle, BorderGradient, EffectElement, FrameElement,
        RenderCompletion, RenderFrame, RenderSyncMode, RendererError, RoundedRing,
        ShadowVisualStyle, TextureElement, TextureMask, TextureSource,
    },
    state::RootSurfaceClip,
    transform::{self, Mat3, OutputPresentationTransform},
};

use super::{
    allocator::AllocatedBuffer,
    command,
    device::{PendingFrame, RenderedSubmit, VulkanDevice},
    effects::BlurJob,
    pipeline::DrawState,
    resources::PaintedTarget,
    sync::OwnedSemaphores,
    texture,
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

struct QuadDraw {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    set: vk::DescriptorSet,
    _uniform: AllocatedBuffer,
    scissors: Vec<vk::Rect2D>,
}

enum FrameOp {
    Quad(QuadDraw),
    Blur { job: BlurJob, result: QuadDraw },
}

#[repr(C)]
struct BorderUniforms {
    transform_c0: [f32; 4],
    transform_c1: [f32; 4],
    transform_c2: [f32; 4],
    item_size: [f32; 2],
    outer_radius: f32,
    outer_power: f32,
    inner_radius: f32,
    inner_power: f32,
    angle: f32,
    alpha: f32,
    outer_rect: [f32; 4],
    inner_rect: [f32; 4],
    gradient_start: [f32; 4],
    gradient_end: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<BorderUniforms>() == 144);
const _: () = assert!(std::mem::offset_of!(BorderUniforms, outer_rect) == 80);
const _: () = assert!(std::mem::offset_of!(BorderUniforms, gradient_start) == 112);

#[repr(C)]
struct ShadowUniforms {
    transform_c0: [f32; 4],
    transform_c1: [f32; 4],
    transform_c2: [f32; 4],
    item_size: [f32; 2],
    corner_radius: f32,
    rounding_power: f32,
    shadow_range: f32,
    shadow_power: f32,
    _pad: [f32; 2],
    shadow_color: [f32; 4],
    inner_rect: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<ShadowUniforms>() == 112);
const _: () = assert!(std::mem::offset_of!(ShadowUniforms, shadow_color) == 80);
const _: () = assert!(std::mem::offset_of!(ShadowUniforms, inner_rect) == 96);

pub(super) fn paint(
    device: &Rc<VulkanDevice>,
    draw: &mut DrawState,
    target: &dyn PaintedTarget,
    frame: RenderFrame<'_>,
    pending: &mut Vec<PendingFrame>,
    acquires: Vec<super::sync::StashedSemaphore>,
) -> Result<RenderCompletion, RendererError> {
    if target.width() <= 0 || target.height() <= 0 {
        log::error!("vulkan render target has no positive size");
        return Err(RendererError::RenderFailed);
    }
    draw.reset_descriptors().map_err(|error| {
        log::error!("{error}");
        RendererError::RenderFailed
    })?;
    let mut ops = Vec::new();
    for element in &frame.recipe.elements {
        match element {
            FrameElement::Texture { element, .. } => {
                if let Some(quad) = prepare_texture(device, draw, target, element, &frame)? {
                    ops.push(FrameOp::Quad(quad));
                }
            }
            FrameElement::Effect { element, .. } => {
                if let Some(op) = prepare_effect(device, draw, target, element, &frame) {
                    ops.push(op);
                }
            }
        }
    }
    let clear_rects =
        clear_rects(target, frame.damage, frame.preserve_contents, frame.presentation_transform);
    let clear_color = frame.clear_color.unwrap_or_else(|| target.clear_color());
    let explicit = frame.sync_mode == RenderSyncMode::ExplicitInFence;
    let waits = OwnedSemaphores::from_stashed(device, acquires);
    let submitted = device.submit_rendered(explicit, waits.as_slice(), |vulkan, command| {
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
        let mut layout = vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL;
        let mut pass_open = false;
        let mut cleared = false;
        for op in &ops {
            match op {
                FrameOp::Quad(quad) => {
                    if !pass_open {
                        begin_output_pass(
                            vulkan,
                            command,
                            draw,
                            target,
                            &clear_rects,
                            clear_color,
                            &mut cleared,
                        );
                        pass_open = true;
                    }
                    draw_quad(vulkan, command, quad);
                }
                FrameOp::Blur { job, result } => {
                    if !pass_open {
                        begin_output_pass(
                            vulkan,
                            command,
                            draw,
                            target,
                            &clear_rects,
                            clear_color,
                            &mut cleared,
                        );
                    }
                    unsafe { vulkan.cmd_end_render_pass(command) };
                    job.record(vulkan, command, draw, target.image(), &mut layout);
                    begin_output_pass(
                        vulkan,
                        command,
                        draw,
                        target,
                        &clear_rects,
                        clear_color,
                        &mut cleared,
                    );
                    pass_open = true;
                    draw_quad(vulkan, command, result);
                }
            }
        }
        if !pass_open {
            begin_output_pass(
                vulkan,
                command,
                draw,
                target,
                &clear_rects,
                clear_color,
                &mut cleared,
            );
        }
        unsafe { vulkan.cmd_end_render_pass(command) };
        Ok(())
    })?;
    target
        .layout()
        .set(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    match submitted {
        RenderedSubmit::Blocked => Ok(RenderCompletion::blocking_fallback()),
        RenderedSubmit::Explicit(detached) => {
            let pool = draw.park_pool();
            let (sync_fd, parked) =
                detached.into_frame(device, waits.into_handles(), pool, Box::new(ops));
            if crate::logger::ENABLE_NOISY_LOG {
                log::debug!("vulkan exported explicit render fence fd={}", sync_fd.as_raw_fd());
            }
            pending.push(parked);
            Ok(RenderCompletion::explicit(sync_fd))
        }
    }
}

fn prepare_texture(
    device: &Rc<VulkanDevice>,
    draw: &mut DrawState,
    target: &dyn PaintedTarget,
    element: &TextureElement,
    frame: &RenderFrame<'_>,
) -> Result<Option<QuadDraw>, RendererError> {
    let Some(texture) = texture::sampled_texture(element.texture.as_ref(), device.device_id())
    else {
        return Ok(None);
    };
    prepare_sampled(
        device,
        draw,
        target,
        frame,
        texture.view(),
        element.x,
        element.y,
        element.width,
        element.height,
        element.tex_transform,
        element.alpha,
        &element.mask,
        &element.damage,
    )
}

fn prepare_sampled(
    device: &Rc<VulkanDevice>,
    draw: &mut DrawState,
    target: &dyn PaintedTarget,
    frame: &RenderFrame<'_>,
    view: vk::ImageView,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    tex_transform: Mat3,
    alpha: u8,
    mask: &TextureMask,
    damage: &DamageRegion,
) -> Result<Option<QuadDraw>, RendererError> {
    let Some(item_bounds) = RectI::new(x, y, width, height) else {
        return Ok(None);
    };
    let Some(output_bounds) = RectI::new(0, 0, target.width(), target.height()) else {
        return Ok(None);
    };
    let mut clipped = frame.damage.clone();
    clipped.intersect_rect(output_bounds);
    clipped.intersect_rect(item_bounds);
    clipped.intersect_rect(damage.extents().unwrap_or(item_bounds));
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
    let transform = columns(transform::project_box(
        x as f32,
        y as f32,
        width as f32,
        height as f32,
        projection,
    ));
    let tex = columns(tex_transform);
    let clip = clip_uniforms(mask);
    let uniforms = DrawUniforms {
        transform_c0: transform[0],
        transform_c1: transform[1],
        transform_c2: transform[2],
        tex_c0: tex[0],
        tex_c1: tex[1],
        tex_c2: tex[2],
        item_size: [width as f32, height as f32],
        alpha: f32::from(alpha) / 255.0,
        clip_mode: clip.mode,
        outer_rect: clip.outer,
        inner_rect: clip.inner,
        outer_radius: clip.outer_radius,
        outer_power: clip.outer_power,
        inner_radius: clip.inner_radius,
        inner_power: clip.inner_power,
    };
    let uniform = uniform_buffer(device, &uniforms).map_err(|error| {
        log::error!("{error}");
        RendererError::RenderFailed
    })?;
    let set = draw.allocate_set().map_err(|error| {
        log::error!("{error}");
        RendererError::RenderFailed
    })?;
    write_set(
        &device.device,
        set,
        draw.sampler(),
        view,
        uniform.buffer(),
        std::mem::size_of::<DrawUniforms>() as u64,
    );
    Ok(Some(QuadDraw {
        pipeline: draw.pipeline(),
        layout: draw.pipeline_layout(),
        set,
        _uniform: uniform,
        scissors,
    }))
}

fn write_set(
    device: &ash::Device,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    view: vk::ImageView,
    buffer: vk::Buffer,
    range: u64,
) {
    let image = vk::DescriptorImageInfo {
        sampler,
        image_view: view,
        image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
    };
    let uniform = vk::DescriptorBufferInfo { buffer, offset: 0, range };
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
        // Corner-only rounding: pixels outside the window box are kept, so
        // client-drawn CSD shadows survive.
        TextureMask::CornerRoundedClip { clip } => ClipUniforms {
            mode: 3,
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

fn uniform_buffer<T>(device: &Rc<VulkanDevice>, value: &T) -> Result<AllocatedBuffer, String> {
    let bytes = unsafe {
        std::slice::from_raw_parts((value as *const T).cast::<u8>(), std::mem::size_of::<T>())
    };
    let buffer = AllocatedBuffer::create(
        device,
        bytes.len() as u64,
        vk::BufferUsageFlags::UNIFORM_BUFFER,
        UsageFlags::UPLOAD | UsageFlags::TRANSIENT,
    )?;
    buffer.write(bytes)?;
    Ok(buffer)
}

fn prepare_effect(
    device: &Rc<VulkanDevice>,
    draw: &mut DrawState,
    target: &dyn PaintedTarget,
    element: &EffectElement,
    frame: &RenderFrame<'_>,
) -> Option<FrameOp> {
    match *element {
        EffectElement::WindowBorder { ring, gradient, alpha } => {
            prepare_border(device, draw, target, frame, ring, gradient, alpha).map(FrameOp::Quad)
        }
        EffectElement::WindowShadow { clip, style, alpha } => {
            prepare_shadow(device, draw, target, frame, clip, style, alpha).map(FrameOp::Quad)
        }
        EffectElement::DimOverlay { clip, alpha } => {
            prepare_dim(device, draw, target, frame, clip, alpha)
        }
        EffectElement::WindowBlurBackdrop { ring, style, alpha } => {
            prepare_blur(device, draw, target, frame, ring, style, alpha)
        }
    }
}

fn prepare_border(
    device: &Rc<VulkanDevice>,
    draw: &mut DrawState,
    target: &dyn PaintedTarget,
    frame: &RenderFrame<'_>,
    ring: RoundedRing,
    gradient: BorderGradient,
    alpha: u8,
) -> Option<QuadDraw> {
    let bounds = ring.outer.outer_rect;
    let scissors = effect_scissors(target, frame, bounds)?;
    let projection = transform::output_projection(
        target.width() as f32,
        target.height() as f32,
        frame.presentation_transform,
    );
    let transform = columns(transform::project_box(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        projection,
    ));
    let uniforms = BorderUniforms {
        transform_c0: transform[0],
        transform_c1: transform[1],
        transform_c2: transform[2],
        item_size: [bounds.width as f32, bounds.height as f32],
        outer_radius: ring.outer.corner_radius.max(0) as f32,
        outer_power: ring.outer.rounding_power.get().max(0.0001),
        inner_radius: ring.inner.corner_radius.max(0) as f32,
        inner_power: ring.inner.rounding_power.get().max(0.0001),
        angle: gradient.angle_degrees as f32,
        alpha: f32::from(alpha) / 255.0,
        outer_rect: [0.0, 0.0, bounds.width as f32, bounds.height as f32],
        inner_rect: [
            ring.inner.outer_rect.x.saturating_sub(bounds.x) as f32,
            ring.inner.outer_rect.y.saturating_sub(bounds.y) as f32,
            ring.inner.outer_rect.width as f32,
            ring.inner.outer_rect.height as f32,
        ],
        gradient_start: color_vec(gradient.start_color),
        gradient_end: color_vec(gradient.end_color),
    };
    solid_quad(
        device,
        draw,
        draw.border_pipeline(),
        draw.border_layout(),
        draw.border_set_layout(),
        &uniforms,
        scissors,
        "border",
    )
}

fn prepare_shadow(
    device: &Rc<VulkanDevice>,
    draw: &mut DrawState,
    target: &dyn PaintedTarget,
    frame: &RenderFrame<'_>,
    clip: RootSurfaceClip,
    style: ShadowVisualStyle,
    alpha: u8,
) -> Option<QuadDraw> {
    let geometry = shadow_geometry(clip, style)?;
    let bounds = geometry.bounds;
    let scissors = effect_scissors(target, frame, bounds)?;
    let projection = transform::output_projection(
        target.width() as f32,
        target.height() as f32,
        frame.presentation_transform,
    );
    let transform = columns(transform::project_box(
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
        projection,
    ));
    let uniforms = ShadowUniforms {
        transform_c0: transform[0],
        transform_c1: transform[1],
        transform_c2: transform[2],
        item_size: [bounds.width as f32, bounds.height as f32],
        corner_radius: clip.corner_radius.max(0) as f32,
        rounding_power: clip.rounding_power.get().max(0.0001),
        shadow_range: geometry.shadow_range as f32,
        shadow_power: style.power.get(),
        _pad: [0.0; 2],
        shadow_color: shadow_color(style.color, alpha),
        inner_rect: [
            geometry.inner_rect.x as f32,
            geometry.inner_rect.y as f32,
            geometry.inner_rect.width as f32,
            geometry.inner_rect.height as f32,
        ],
    };
    solid_quad(
        device,
        draw,
        draw.shadow_pipeline(),
        draw.shadow_layout(),
        draw.shadow_set_layout(),
        &uniforms,
        scissors,
        "shadow",
    )
}

fn prepare_dim(
    device: &Rc<VulkanDevice>,
    draw: &mut DrawState,
    target: &dyn PaintedTarget,
    frame: &RenderFrame<'_>,
    clip: RootSurfaceClip,
    alpha: u8,
) -> Option<FrameOp> {
    let bounds = clip.outer_rect;
    let Some(local) = RectI::new(0, 0, bounds.width, bounds.height) else {
        return None;
    };
    let element = TextureElement {
        source: TextureSource::Texture,
        texture: draw.dim_texture(),
        x: bounds.x,
        y: bounds.y,
        width: bounds.width,
        height: bounds.height,
        tex_transform: Mat3::identity(),
        damage: frame.damage.clone(),
        blend: true,
        alpha,
        mask: TextureMask::RoundedClip {
            clip: RootSurfaceClip::new(local, clip.corner_radius, clip.rounding_power),
        },
    };
    match prepare_texture(device, draw, target, &element, frame) {
        Ok(Some(quad)) => Some(FrameOp::Quad(quad)),
        Ok(None) => None,
        Err(_) => {
            log::error!("vulkan skipped dim overlay");
            None
        }
    }
}

fn prepare_blur(
    device: &Rc<VulkanDevice>,
    draw: &mut DrawState,
    target: &dyn PaintedTarget,
    frame: &RenderFrame<'_>,
    ring: RoundedRing,
    style: BlurVisualStyle,
    alpha: u8,
) -> Option<FrameOp> {
    let bounds = blur_snapshot_bounds(target, frame.blur_reference, ring.outer, style)?;
    let copy = framebuffer_rect(target.height(), bounds, frame.presentation_transform)
        .and_then(vulkan_scissor)?;
    let mask = TextureMask::FrameRing {
        ring: RoundedRing {
            outer: RootSurfaceClip::new(
                ring.outer
                    .outer_rect
                    .translate(bounds.x.saturating_neg(), bounds.y.saturating_neg()),
                ring.outer.corner_radius,
                ring.outer.rounding_power,
            ),
            inner: RootSurfaceClip::new(
                ring.inner
                    .outer_rect
                    .translate(bounds.x.saturating_neg(), bounds.y.saturating_neg()),
                ring.inner.corner_radius,
                ring.inner.rounding_power,
            ),
        },
    };
    let job = BlurJob::new(
        device,
        draw,
        bounds.width,
        bounds.height,
        style.radius.max(1) as f32,
        style.passes.max(1),
        frame.presentation_transform,
        copy,
    )?;
    let result = match prepare_sampled(
        device,
        draw,
        target,
        frame,
        job.result_view(),
        bounds.x,
        bounds.y,
        bounds.width,
        bounds.height,
        Mat3::identity(),
        alpha,
        &mask,
        frame.damage,
    ) {
        Ok(Some(quad)) => quad,
        Ok(None) => return None,
        Err(_) => {
            log::error!("vulkan skipped blur");
            return None;
        }
    };
    Some(FrameOp::Blur { job, result })
}

fn blur_snapshot_bounds(
    target: &dyn PaintedTarget,
    reference: Option<(i32, i32)>,
    clip: RootSurfaceClip,
    style: BlurVisualStyle,
) -> Option<RectI> {
    let (reference_width, reference_height) =
        reference.unwrap_or((target.width(), target.height()));
    let reference_bounds = RectI::new(0, 0, reference_width, reference_height)?;
    let target_bounds = RectI::new(0, 0, target.width(), target.height())?;
    let padding = style
        .radius
        .max(1)
        .saturating_mul(i32::from(style.passes.max(1)))
        .saturating_mul(4);
    RectI::from_extents(
        clip.outer_rect.x.saturating_sub(padding),
        clip.outer_rect.y.saturating_sub(padding),
        clip.outer_rect.x2().saturating_add(padding),
        clip.outer_rect.y2().saturating_add(padding),
    )?
    .intersection(reference_bounds)?
    .intersection(target_bounds)
}

fn effect_scissors(
    target: &dyn PaintedTarget,
    frame: &RenderFrame<'_>,
    bounds: RectI,
) -> Option<Vec<vk::Rect2D>> {
    let output_bounds = RectI::new(0, 0, target.width(), target.height())?;
    let mut clipped = frame.damage.clone();
    clipped.intersect_rect(output_bounds);
    clipped.intersect_rect(bounds);
    if clipped.is_empty() {
        return None;
    }
    let scissors = clipped
        .rects()
        .iter()
        .filter_map(|rect| framebuffer_rect(target.height(), *rect, frame.presentation_transform))
        .filter_map(vulkan_scissor)
        .collect::<Vec<_>>();
    if scissors.is_empty() {
        None
    } else {
        Some(scissors)
    }
}

fn solid_quad<T>(
    device: &Rc<VulkanDevice>,
    draw: &mut DrawState,
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    uniforms: &T,
    scissors: Vec<vk::Rect2D>,
    label: &str,
) -> Option<QuadDraw> {
    let uniform = match uniform_buffer(device, uniforms) {
        Ok(uniform) => uniform,
        Err(error) => {
            log::error!("vulkan skipped {label}: {error}");
            return None;
        }
    };
    let set = match draw.allocate_set_for(set_layout) {
        Ok(set) => set,
        Err(error) => {
            log::error!("vulkan skipped {label}: {error}");
            return None;
        }
    };
    let info = vk::DescriptorBufferInfo {
        buffer: uniform.buffer(),
        offset: 0,
        range: std::mem::size_of::<T>() as u64,
    };
    let write = vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(0)
        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
        .buffer_info(std::slice::from_ref(&info));
    unsafe {
        device
            .device
            .update_descriptor_sets(std::slice::from_ref(&write), &[])
    };
    Some(QuadDraw { pipeline, layout, set, _uniform: uniform, scissors })
}

fn color_vec(color: RgbaColor) -> [f32; 4] {
    let bytes = color.to_premultiplied_rgba_bytes();
    [
        f32::from(bytes[0]) / 255.0,
        f32::from(bytes[1]) / 255.0,
        f32::from(bytes[2]) / 255.0,
        f32::from(bytes[3]) / 255.0,
    ]
}

fn shadow_color(color: RgbaColor, alpha: u8) -> [f32; 4] {
    let premultiplied = color_vec(color);
    let global_alpha = f32::from(alpha) / 255.0;
    [
        premultiplied[0] * global_alpha,
        premultiplied[1] * global_alpha,
        premultiplied[2] * global_alpha,
        premultiplied[3] * global_alpha,
    ]
}

fn begin_output_pass(
    vulkan: &ash::Device,
    command: vk::CommandBuffer,
    draw: &DrawState,
    target: &dyn PaintedTarget,
    clear_rects: &[vk::Rect2D],
    clear_color: [f32; 4],
    cleared: &mut bool,
) {
    let begin = vk::RenderPassBeginInfo::default()
        .render_pass(target.render_pass())
        .framebuffer(target.framebuffer())
        .render_area(vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: vk::Extent2D { width: target.width() as u32, height: target.height() as u32 },
        });
    unsafe {
        vulkan.cmd_begin_render_pass(command, &begin, vk::SubpassContents::INLINE);
        if !*cleared {
            if !clear_rects.is_empty() {
                let attachment = vk::ClearAttachment::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .color_attachment(0)
                    .clear_value(vk::ClearValue {
                        color: vk::ClearColorValue { float32: clear_color },
                    });
                let regions: Vec<vk::ClearRect> = clear_rects
                    .iter()
                    .copied()
                    .map(|rect| vk::ClearRect { rect, base_array_layer: 0, layer_count: 1 })
                    .collect();
                vulkan.cmd_clear_attachments(command, std::slice::from_ref(&attachment), &regions);
            }
            *cleared = true;
        }
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: target.width() as f32,
            height: target.height() as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        vulkan.cmd_set_viewport(command, 0, std::slice::from_ref(&viewport));
        vulkan.cmd_bind_vertex_buffers(command, 0, std::slice::from_ref(&draw.vertices()), &[0]);
    }
}

fn draw_quad(vulkan: &ash::Device, command: vk::CommandBuffer, quad: &QuadDraw) {
    unsafe {
        vulkan.cmd_bind_pipeline(command, vk::PipelineBindPoint::GRAPHICS, quad.pipeline);
        vulkan.cmd_bind_descriptor_sets(
            command,
            vk::PipelineBindPoint::GRAPHICS,
            quad.layout,
            0,
            std::slice::from_ref(&quad.set),
            &[],
        );
        for scissor in &quad.scissors {
            vulkan.cmd_set_scissor(command, 0, std::slice::from_ref(scissor));
            vulkan.cmd_draw(command, 4, 1, 0, 0);
        }
    }
}
