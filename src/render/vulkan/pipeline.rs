//! Textured-quad pipeline. One layout is reused for every draw in a frame.

use std::{ffi::CString, rc::Rc};

use {ash::vk, gpu_alloc::UsageFlags};

use super::{allocator::AllocatedBuffer, compiled_shaders, device::VulkanDevice, resources};

pub(super) struct DrawState {
    device: Rc<VulkanDevice>,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    render_pass: vk::RenderPass,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    sampler: vk::Sampler,
    vertices: AllocatedBuffer,
}

impl DrawState {
    pub(super) fn new(device: &Rc<VulkanDevice>) -> Result<Self, String> {
        let sampler = unsafe {
            device.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )
        }
        .map_err(|error| format!("vulkan sampler creation failed: {error}"))?;
        let set_layout = match set_layout(&device.device, sampler) {
            Ok(layout) => layout,
            Err(error) => {
                unsafe { device.device.destroy_sampler(sampler, None) };
                return Err(error);
            }
        };
        let pool = match descriptor_pool(&device.device) {
            Ok(pool) => pool,
            Err(error) => {
                unsafe {
                    device
                        .device
                        .destroy_descriptor_set_layout(set_layout, None);
                    device.device.destroy_sampler(sampler, None);
                }
                return Err(error);
            }
        };
        let pipeline_layout = match pipeline_layout(&device.device, set_layout) {
            Ok(layout) => layout,
            Err(error) => {
                unsafe {
                    device.device.destroy_descriptor_pool(pool, None);
                    device
                        .device
                        .destroy_descriptor_set_layout(set_layout, None);
                    device.device.destroy_sampler(sampler, None);
                }
                return Err(error);
            }
        };
        let render_pass =
            match resources::color_render_pass(&device.device, vk::Format::B8G8R8A8_UNORM) {
                Ok(pass) => pass,
                Err(error) => {
                    unsafe {
                        device.device.destroy_pipeline_layout(pipeline_layout, None);
                        device.device.destroy_descriptor_pool(pool, None);
                        device
                            .device
                            .destroy_descriptor_set_layout(set_layout, None);
                        device.device.destroy_sampler(sampler, None);
                    }
                    return Err(error);
                }
            };
        let pipeline = match graphics_pipeline(&device.device, pipeline_layout, render_pass) {
            Ok(pipeline) => pipeline,
            Err(error) => {
                unsafe {
                    device.device.destroy_render_pass(render_pass, None);
                    device.device.destroy_pipeline_layout(pipeline_layout, None);
                    device.device.destroy_descriptor_pool(pool, None);
                    device
                        .device
                        .destroy_descriptor_set_layout(set_layout, None);
                    device.device.destroy_sampler(sampler, None);
                }
                return Err(error);
            }
        };
        let vertices = match quad_vertices(device) {
            Ok(vertices) => vertices,
            Err(error) => {
                unsafe {
                    device.device.destroy_pipeline(pipeline, None);
                    device.device.destroy_render_pass(render_pass, None);
                    device.device.destroy_pipeline_layout(pipeline_layout, None);
                    device.device.destroy_descriptor_pool(pool, None);
                    device
                        .device
                        .destroy_descriptor_set_layout(set_layout, None);
                    device.device.destroy_sampler(sampler, None);
                }
                return Err(error);
            }
        };
        Ok(Self {
            device: device.clone(),
            pipeline,
            pipeline_layout,
            render_pass,
            set_layout,
            pool,
            sampler,
            vertices,
        })
    }

    pub(super) fn pipeline(&self) -> vk::Pipeline {
        self.pipeline
    }

    pub(super) fn pipeline_layout(&self) -> vk::PipelineLayout {
        self.pipeline_layout
    }

    pub(super) fn set_layout(&self) -> vk::DescriptorSetLayout {
        self.set_layout
    }

    pub(super) fn sampler(&self) -> vk::Sampler {
        self.sampler
    }

    pub(super) fn vertices(&self) -> vk::Buffer {
        self.vertices.buffer()
    }

    pub(super) fn allocate_set(&mut self) -> Result<vk::DescriptorSet, String> {
        let layouts = [self.set_layout];
        let info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(self.pool)
            .set_layouts(&layouts);
        unsafe { self.device.device.allocate_descriptor_sets(&info) }
            .map_err(|error| format!("vulkan descriptor allocation failed: {error}"))?
            .into_iter()
            .next()
            .ok_or_else(|| "vulkan allocated no descriptor set".to_string())
    }

    pub(super) fn reset_descriptors(&mut self) -> Result<(), String> {
        unsafe {
            self.device
                .device
                .reset_descriptor_pool(self.pool, vk::DescriptorPoolResetFlags::empty())
        }
        .map_err(|error| format!("vulkan descriptor reset failed: {error}"))
    }
}

impl Drop for DrawState {
    fn drop(&mut self) {
        unsafe {
            if let Err(error) = self.device.device.device_wait_idle() {
                log::error!("vulkan wait before destroying the pipeline failed: {error}");
            }
            self.device.device.destroy_pipeline(self.pipeline, None);
            self.device
                .device
                .destroy_render_pass(self.render_pass, None);
            self.device
                .device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.device.destroy_descriptor_pool(self.pool, None);
            self.device
                .device
                .destroy_descriptor_set_layout(self.set_layout, None);
            self.device.device.destroy_sampler(self.sampler, None);
        }
    }
}

fn set_layout(
    device: &ash::Device,
    sampler: vk::Sampler,
) -> Result<vk::DescriptorSetLayout, String> {
    let sampler_binding = vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)
        .immutable_samplers(std::slice::from_ref(&sampler));
    let uniform_binding = vk::DescriptorSetLayoutBinding::default()
        .binding(1)
        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT);
    let bindings = [sampler_binding, uniform_binding];
    let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
    unsafe { device.create_descriptor_set_layout(&info, None) }
        .map_err(|error| format!("vulkan descriptor layout failed: {error}"))
}

fn descriptor_pool(device: &ash::Device) -> Result<vk::DescriptorPool, String> {
    let sizes = [
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            descriptor_count: 256,
        },
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::UNIFORM_BUFFER,
            descriptor_count: 256,
        },
    ];
    let info = vk::DescriptorPoolCreateInfo::default()
        .max_sets(256)
        .pool_sizes(&sizes);
    unsafe { device.create_descriptor_pool(&info, None) }
        .map_err(|error| format!("vulkan descriptor pool failed: {error}"))
}

fn pipeline_layout(
    device: &ash::Device,
    set_layout: vk::DescriptorSetLayout,
) -> Result<vk::PipelineLayout, String> {
    let info =
        vk::PipelineLayoutCreateInfo::default().set_layouts(std::slice::from_ref(&set_layout));
    unsafe { device.create_pipeline_layout(&info, None) }
        .map_err(|error| format!("vulkan pipeline layout failed: {error}"))
}

fn graphics_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    render_pass: vk::RenderPass,
) -> Result<vk::Pipeline, String> {
    let vertex = shader_module(device, compiled_shaders::TEXTURE_VERT)?;
    let fragment = match shader_module(device, compiled_shaders::TEXTURE_FRAG) {
        Ok(module) => module,
        Err(error) => {
            unsafe { device.destroy_shader_module(vertex, None) };
            return Err(error);
        }
    };
    let entry = CString::new("main").expect("shader entry point");
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vertex)
            .name(&entry),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(fragment)
            .name(&entry),
    ];
    let binding = vk::VertexInputBindingDescription {
        binding: 0,
        stride: 16,
        input_rate: vk::VertexInputRate::VERTEX,
    };
    let attributes = [
        vk::VertexInputAttributeDescription {
            location: 0,
            binding: 0,
            format: vk::Format::R32G32_SFLOAT,
            offset: 0,
        },
        vk::VertexInputAttributeDescription {
            location: 1,
            binding: 0,
            format: vk::Format::R32G32_SFLOAT,
            offset: 8,
        },
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(std::slice::from_ref(&binding))
        .vertex_attribute_descriptions(&attributes);
    let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_STRIP);
    let viewport = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let raster = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(true)
        .src_color_blend_factor(vk::BlendFactor::ONE)
        .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .color_blend_op(vk::BlendOp::ADD)
        .src_alpha_blend_factor(vk::BlendFactor::ONE)
        .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .alpha_blend_op(vk::BlendOp::ADD)
        .color_write_mask(vk::ColorComponentFlags::RGBA);
    let blend = vk::PipelineColorBlendStateCreateInfo::default()
        .attachments(std::slice::from_ref(&blend_attachment));
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&assembly)
        .viewport_state(&viewport)
        .rasterization_state(&raster)
        .multisample_state(&multisample)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .render_pass(render_pass)
        .subpass(0);
    let pipelines = unsafe {
        device.create_graphics_pipelines(
            vk::PipelineCache::null(),
            std::slice::from_ref(&info),
            None,
        )
    };
    unsafe {
        device.destroy_shader_module(vertex, None);
        device.destroy_shader_module(fragment, None);
    }
    match pipelines {
        Ok(pipelines) => pipelines
            .into_iter()
            .next()
            .ok_or_else(|| "vulkan created no graphics pipeline".to_string()),
        Err((_, error)) => Err(format!("vulkan graphics pipeline failed: {error}")),
    }
}

fn shader_module(device: &ash::Device, bytes: &[u8]) -> Result<vk::ShaderModule, String> {
    let mut words = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        let mut word = [0; 4];
        word.copy_from_slice(chunk);
        words.push(u32::from_le_bytes(word));
    }
    if words.is_empty() {
        return Err("vulkan shader spir-v is empty".to_string());
    }
    let info = vk::ShaderModuleCreateInfo::default().code(&words);
    unsafe { device.create_shader_module(&info, None) }
        .map_err(|error| format!("vulkan shader module failed: {error}"))
}

fn quad_vertices(device: &Rc<VulkanDevice>) -> Result<AllocatedBuffer, String> {
    let vertices: [f32; 16] =
        [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0];
    let bytes = unsafe {
        std::slice::from_raw_parts(vertices.as_ptr().cast::<u8>(), std::mem::size_of_val(&vertices))
    };
    let buffer = AllocatedBuffer::create(
        device,
        bytes.len() as u64,
        vk::BufferUsageFlags::VERTEX_BUFFER,
        UsageFlags::UPLOAD,
    )?;
    buffer.write(bytes)?;
    Ok(buffer)
}
