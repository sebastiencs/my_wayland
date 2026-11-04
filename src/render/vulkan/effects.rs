//! Blur copies the color target out of the render pass, then ping-pongs scratch images.
//!
//! GLES does the same with `CopyTexSubImage2D` and two `TextureFrameBuffer`s per call.
//! Jay's framebuffer images include `TRANSFER_SRC` so that copy is valid.

use std::rc::Rc;

use {ash::vk, gpu_alloc::UsageFlags};

use crate::transform::{self, Mat3, OutputPresentationTransform};

use super::{
    allocator::{AllocatedBuffer, GpuAllocation},
    command,
    device::VulkanDevice,
    pipeline::DrawState,
    resources::{self, create_view},
};

#[repr(C)]
struct BlurUniforms {
    transform_c0: [f32; 4],
    transform_c1: [f32; 4],
    transform_c2: [f32; 4],
    tex_c0: [f32; 4],
    tex_c1: [f32; 4],
    tex_c2: [f32; 4],
    item_size: [f32; 2],
    direction: [f32; 2],
    texel_size: [f32; 2],
    radius: f32,
    _pad: f32,
}

const _: () = assert!(std::mem::size_of::<BlurUniforms>() == 128);
const _: () = assert!(std::mem::offset_of!(BlurUniforms, direction) == 104);
const _: () = assert!(std::mem::offset_of!(BlurUniforms, texel_size) == 112);
const _: () = assert!(std::mem::offset_of!(BlurUniforms, radius) == 120);

struct BlurPass {
    set: vk::DescriptorSet,
    _uniform: AllocatedBuffer,
    target_a: bool,
}

struct BlurRender {
    render_pass: vk::RenderPass,
    framebuffer: vk::Framebuffer,
}

struct BlurImage {
    device: Rc<VulkanDevice>,
    image: vk::Image,
    view: vk::ImageView,
    render: Option<BlurRender>,
    allocation: GpuAllocation,
}

impl BlurImage {
    fn new(
        device: &Rc<VulkanDevice>,
        width: u32,
        height: u32,
        usage: vk::ImageUsageFlags,
        render: bool,
    ) -> Result<Self, String> {
        let format = vk::Format::B8G8R8A8_UNORM;
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width, height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { device.device.create_image(&info, None) }
            .map_err(|error| format!("vulkan blur image creation failed: {error}"))?;
        let requirements = unsafe { device.device.get_image_memory_requirements(image) };
        let allocation = match device.allocate(requirements, UsageFlags::FAST_DEVICE_ACCESS) {
            Ok(allocation) => allocation,
            Err(error) => {
                unsafe { device.device.destroy_image(image, None) };
                return Err(error);
            }
        };
        if let Err(error) = unsafe {
            device
                .device
                .bind_image_memory(image, allocation.memory(), allocation.offset())
        } {
            unsafe { device.device.destroy_image(image, None) };
            return Err(format!("vulkan blur image bind failed: {error}"));
        }
        let view = match create_view(&device.device, image, format) {
            Ok(view) => view,
            Err(_) => {
                unsafe { device.device.destroy_image(image, None) };
                return Err("vulkan blur image view failed".to_string());
            }
        };
        let render = if render {
            let render_pass = match resources::color_render_pass(&device.device, format) {
                Ok(pass) => pass,
                Err(error) => {
                    unsafe {
                        device.device.destroy_image_view(view, None);
                        device.device.destroy_image(image, None);
                    }
                    return Err(error);
                }
            };
            match resources::create_framebuffer(&device.device, render_pass, view, width, height) {
                Ok(framebuffer) => Some(BlurRender { render_pass, framebuffer }),
                Err(_) => {
                    unsafe {
                        device.device.destroy_render_pass(render_pass, None);
                        device.device.destroy_image_view(view, None);
                        device.device.destroy_image(image, None);
                    }
                    return Err("vulkan blur framebuffer failed".to_string());
                }
            }
        } else {
            None
        };
        Ok(Self { device: device.clone(), image, view, render, allocation })
    }

    fn image(&self) -> vk::Image {
        self.image
    }

    fn view(&self) -> vk::ImageView {
        self.view
    }

    fn render_pass(&self) -> vk::RenderPass {
        self.render
            .as_ref()
            .map(|render| render.render_pass)
            .unwrap_or_else(vk::RenderPass::null)
    }

    fn framebuffer(&self) -> vk::Framebuffer {
        self.render
            .as_ref()
            .map(|render| render.framebuffer)
            .unwrap_or_else(vk::Framebuffer::null)
    }
}

impl Drop for BlurImage {
    fn drop(&mut self) {
        unsafe {
            if let Some(render) = self.render.take() {
                self.device
                    .device
                    .destroy_framebuffer(render.framebuffer, None);
                self.device
                    .device
                    .destroy_render_pass(render.render_pass, None);
            }
            self.device.device.destroy_image_view(self.view, None);
            self.device.device.destroy_image(self.image, None);
        }
    }
}

pub(super) struct BlurJob {
    input: BlurImage,
    scratch_a: BlurImage,
    scratch_b: BlurImage,
    passes: Vec<BlurPass>,
    copy: vk::Rect2D,
    width: u32,
    height: u32,
}

impl BlurJob {
    pub(super) fn new(
        device: &Rc<VulkanDevice>,
        draw: &mut DrawState,
        width: i32,
        height: i32,
        radius: f32,
        passes: u8,
        presentation: OutputPresentationTransform,
        copy: vk::Rect2D,
    ) -> Option<Self> {
        let Ok(width) = u32::try_from(width) else {
            return None;
        };
        let Ok(height) = u32::try_from(height) else {
            return None;
        };
        if width == 0 || height == 0 {
            return None;
        }
        let input = match BlurImage::new(
            device,
            width,
            height,
            vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
            false,
        ) {
            Ok(image) => image,
            Err(error) => {
                log::error!("vulkan skipped blur: {error}");
                return None;
            }
        };
        let scratch_usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
        let scratch_a = match BlurImage::new(device, width, height, scratch_usage, true) {
            Ok(image) => image,
            Err(error) => {
                log::error!("vulkan skipped blur: {error}");
                return None;
            }
        };
        let scratch_b = match BlurImage::new(device, width, height, scratch_usage, true) {
            Ok(image) => image,
            Err(error) => {
                log::error!("vulkan skipped blur: {error}");
                return None;
            }
        };
        let projection = transform::output_projection(width as f32, height as f32, presentation);
        let transform =
            columns(transform::project_box(0.0, 0.0, width as f32, height as f32, projection));
        let tex = columns(Mat3::identity());
        let mut source = input.view();
        let mut recorded = Vec::new();
        for _ in 0..passes.max(1) {
            for (target_a, direction) in [(true, [1.0_f32, 0.0]), (false, [0.0, 1.0])] {
                let uniforms = BlurUniforms {
                    transform_c0: transform[0],
                    transform_c1: transform[1],
                    transform_c2: transform[2],
                    tex_c0: tex[0],
                    tex_c1: tex[1],
                    tex_c2: tex[2],
                    item_size: [width as f32, height as f32],
                    direction,
                    texel_size: [1.0 / width as f32, 1.0 / height as f32],
                    radius: radius.max(1.0),
                    _pad: 0.0,
                };
                let uniform = match uniform_buffer(device, &uniforms) {
                    Ok(uniform) => uniform,
                    Err(error) => {
                        log::error!("vulkan skipped blur: {error}");
                        return None;
                    }
                };
                let set = match draw.allocate_set() {
                    Ok(set) => set,
                    Err(error) => {
                        log::error!("vulkan skipped blur: {error}");
                        return None;
                    }
                };
                write_blur_set(
                    &device.device,
                    set,
                    draw.sampler(),
                    source,
                    uniform.buffer(),
                    std::mem::size_of::<BlurUniforms>() as u64,
                );
                recorded.push(BlurPass { set, _uniform: uniform, target_a });
                source = if target_a { scratch_a.view() } else { scratch_b.view() };
            }
        }
        Some(Self {
            input,
            scratch_a,
            scratch_b,
            passes: recorded,
            copy,
            width,
            height,
        })
    }

    pub(super) fn result_view(&self) -> vk::ImageView {
        self.scratch_b.view()
    }

    pub(super) fn record(
        &self,
        vulkan: &ash::Device,
        command: vk::CommandBuffer,
        draw: &DrawState,
        target: vk::Image,
        layout: &mut vk::ImageLayout,
    ) {
        command::image_barrier(
            vulkan,
            command,
            target,
            *layout,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            vk::AccessFlags::TRANSFER_READ,
        );
        *layout = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
        command::image_barrier(
            vulkan,
            command,
            self.input.image(),
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::empty(),
            vk::AccessFlags::TRANSFER_WRITE,
        );
        let region = vk::ImageCopy {
            src_subresource: color_layers(),
            src_offset: vk::Offset3D { x: self.copy.offset.x, y: self.copy.offset.y, z: 0 },
            dst_subresource: color_layers(),
            dst_offset: vk::Offset3D::default(),
            extent: vk::Extent3D {
                width: self.copy.extent.width,
                height: self.copy.extent.height,
                depth: 1,
            },
        };
        unsafe {
            vulkan.cmd_copy_image(
                command,
                target,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.input.image(),
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                std::slice::from_ref(&region),
            );
        }
        command::image_barrier(
            vulkan,
            command,
            target,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags::TRANSFER_READ,
            vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
        );
        *layout = vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL;
        command::image_barrier(
            vulkan,
            command,
            self.input.image(),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::TRANSFER_WRITE,
            vk::AccessFlags::SHADER_READ,
        );
        let mut layout_a = vk::ImageLayout::UNDEFINED;
        let mut layout_b = vk::ImageLayout::UNDEFINED;
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: self.width as f32,
            height: self.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let scissor = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: vk::Extent2D { width: self.width, height: self.height },
        };
        unsafe {
            vulkan.cmd_bind_pipeline(
                command,
                vk::PipelineBindPoint::GRAPHICS,
                draw.blur_pipeline(),
            );
            vulkan.cmd_bind_vertex_buffers(
                command,
                0,
                std::slice::from_ref(&draw.vertices()),
                &[0],
            );
            vulkan.cmd_set_viewport(command, 0, std::slice::from_ref(&viewport));
            vulkan.cmd_set_scissor(command, 0, std::slice::from_ref(&scissor));
        }
        for pass in &self.passes {
            let (image, render_pass, framebuffer, current) = if pass.target_a {
                (
                    self.scratch_a.image(),
                    self.scratch_a.render_pass(),
                    self.scratch_a.framebuffer(),
                    &mut layout_a,
                )
            } else {
                (
                    self.scratch_b.image(),
                    self.scratch_b.render_pass(),
                    self.scratch_b.framebuffer(),
                    &mut layout_b,
                )
            };
            if *current != vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL {
                let (src_stage, src_access) = if *current == vk::ImageLayout::UNDEFINED {
                    (vk::PipelineStageFlags::TOP_OF_PIPE, vk::AccessFlags::empty())
                } else {
                    (vk::PipelineStageFlags::FRAGMENT_SHADER, vk::AccessFlags::SHADER_READ)
                };
                command::image_barrier(
                    vulkan,
                    command,
                    image,
                    *current,
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                    src_stage,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    src_access,
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                );
            }
            let begin = vk::RenderPassBeginInfo::default()
                .render_pass(render_pass)
                .framebuffer(framebuffer)
                .render_area(scissor);
            unsafe {
                vulkan.cmd_begin_render_pass(command, &begin, vk::SubpassContents::INLINE);
                vulkan.cmd_bind_descriptor_sets(
                    command,
                    vk::PipelineBindPoint::GRAPHICS,
                    draw.pipeline_layout(),
                    0,
                    std::slice::from_ref(&pass.set),
                    &[],
                );
                vulkan.cmd_draw(command, 4, 1, 0, 0);
                vulkan.cmd_end_render_pass(command);
            }
            command::image_barrier(
                vulkan,
                command,
                image,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                vk::AccessFlags::SHADER_READ,
            );
            *current = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
        }
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

fn write_blur_set(
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

fn color_layers() -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        mip_level: 0,
        base_array_layer: 0,
        layer_count: 1,
    }
}
