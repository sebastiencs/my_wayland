use std::{cell::OnceCell, rc::Rc};

use khronos_types::{egl, gl};

use {
    super::{import::GlTexture, loader::EGL},
    crate::{drm::Format, render::RenderDeviceId},
};

#[derive(Debug)]
pub struct EglRenderBuffer {
    pub(super) egl: EGL,
    pub(super) display: egl::EGLDisplay,
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) format: &'static Format,
    pub(super) device_id: RenderDeviceId,
    pub renderbuffer_object: gl::GLuint,
}

impl Drop for EglRenderBuffer {
    fn drop(&mut self) {
        unsafe { gl::DeleteRenderbuffers(1, &self.renderbuffer_object) }
    }
}

impl EglRenderBuffer {
    pub(super) fn make_framebuffer(self, programs: Rc<GlesPrograms>) -> EglFrameBuffer {
        let mut framebuffer_object = 0;

        unsafe {
            gl::GenFramebuffers(1, &mut framebuffer_object);
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer_object);
            gl::FramebufferRenderbuffer(
                gl::FRAMEBUFFER as _,
                gl::COLOR_ATTACHMENT0 as _,
                gl::RENDERBUFFER as _,
                self.renderbuffer_object,
            );
            let status = gl::CheckFramebufferStatus(gl::FRAMEBUFFER as _);
            assert_eq!(status, gl::FRAMEBUFFER_COMPLETE as _);
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, 0);
        }

        EglFrameBuffer {
            egl: self.egl.clone(),
            display: self.display,
            width: self.width,
            height: self.height,
            device_id: self.device_id,
            render_buffer: self,
            framebuffer_object,
            programs,
            imported_image: None,
        }
    }
}

#[derive(Debug)]
pub struct EglFrameBuffer {
    pub(super) egl: EGL,
    pub(super) display: egl::EGLDisplay,
    pub width: i32,
    pub height: i32,
    pub(super) device_id: RenderDeviceId,
    pub(super) render_buffer: EglRenderBuffer,
    pub framebuffer_object: gl::GLuint,
    /// Shader programs and the dim texture shared with every framebuffer on this device.
    pub(super) programs: Rc<GlesPrograms>,
    /// Keeps an imported output image alive for the renderbuffer that samples it.
    pub(super) imported_image: Option<GlTexture>,
}

impl Drop for EglFrameBuffer {
    fn drop(&mut self) {
        unsafe {
            gl::DeleteFramebuffers(1, &self.framebuffer_object);
        }
    }
}

#[derive(Debug)]
pub(super) struct TextureProgram {
    pub(super) program: gl::Uint,
    pub(super) position_location: gl::Uint,
    pub(super) texcoord_location: gl::Uint,
    pub(super) sampler_location: gl::Int,
    pub(super) transform_location: gl::Int,
    pub(super) tex_transform_location: gl::Int,
    pub(super) item_size_location: gl::Int,
    pub(super) clip_mode_location: gl::Int,
    pub(super) clip_outer_rect_location: gl::Int,
    pub(super) clip_inner_rect_location: gl::Int,
    pub(super) outer_corner_radius_location: gl::Int,
    pub(super) outer_rounding_power_location: gl::Int,
    pub(super) inner_corner_radius_location: gl::Int,
    pub(super) inner_rounding_power_location: gl::Int,
    pub(super) alpha_location: gl::Int,
}

#[derive(Debug)]
pub(super) struct BlurProgram {
    pub(super) program: gl::Uint,
    pub(super) position_location: gl::Uint,
    pub(super) texcoord_location: gl::Uint,
    pub(super) sampler_location: gl::Int,
    pub(super) direction_location: gl::Int,
    pub(super) texel_size_location: gl::Int,
    pub(super) radius_location: gl::Int,
    pub(super) transform_location: gl::Int,
    pub(super) tex_transform_location: gl::Int,
    pub(super) item_size_location: gl::Int,
}

#[derive(Debug)]
pub(super) struct BorderProgram {
    pub(super) program: gl::Uint,
    pub(super) position_location: gl::Uint,
    pub(super) transform_location: gl::Int,
    pub(super) item_size_location: gl::Int,
    pub(super) outer_rect_location: gl::Int,
    pub(super) inner_rect_location: gl::Int,
    pub(super) outer_corner_radius_location: gl::Int,
    pub(super) outer_rounding_power_location: gl::Int,
    pub(super) inner_corner_radius_location: gl::Int,
    pub(super) inner_rounding_power_location: gl::Int,
    pub(super) start_color_location: gl::Int,
    pub(super) end_color_location: gl::Int,
    pub(super) angle_location: gl::Int,
    pub(super) alpha_location: gl::Int,
}

#[derive(Debug)]
pub(super) struct ShadowProgram {
    pub(super) program: gl::Uint,
    pub(super) position_location: gl::Uint,
    pub(super) transform_location: gl::Int,
    pub(super) item_size_location: gl::Int,
    pub(super) shadow_color_location: gl::Int,
    pub(super) inner_rect_location: gl::Int,
    pub(super) corner_radius_location: gl::Int,
    pub(super) rounding_power_location: gl::Int,
    pub(super) shadow_range_location: gl::Int,
    pub(super) shadow_power_location: gl::Int,
}

/// Programs compiled once per GLES device and shared by that device's swapchain buffers.
#[derive(Debug)]
pub(super) struct GlesPrograms {
    pub(super) texture: OnceCell<Result<TextureProgram, String>>,
    pub(super) blur: OnceCell<Result<BlurProgram, String>>,
    pub(super) border: OnceCell<Result<BorderProgram, String>>,
    pub(super) shadow: OnceCell<Result<ShadowProgram, String>>,
    pub(super) dim: OnceCell<Rc<GlTexture>>,
}

impl GlesPrograms {
    pub(super) fn new() -> Self {
        Self {
            texture: OnceCell::new(),
            blur: OnceCell::new(),
            border: OnceCell::new(),
            shadow: OnceCell::new(),
            dim: OnceCell::new(),
        }
    }

    /// Delete GL objects while this device's context is current.
    pub(super) fn destroy_gl(&mut self) {
        if let Some(Ok(program)) = self.texture.take() {
            unsafe { gl::DeleteProgram(program.program) };
        }
        if let Some(Ok(program)) = self.blur.take() {
            unsafe { gl::DeleteProgram(program.program) };
        }
        if let Some(Ok(program)) = self.border.take() {
            unsafe { gl::DeleteProgram(program.program) };
        }
        if let Some(Ok(program)) = self.shadow.take() {
            unsafe { gl::DeleteProgram(program.program) };
        }
        self.dim.take();
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::GlesPrograms;

    #[test]
    fn one_device_shares_its_program_cache_across_swapchain_buffers() {
        let programs = Rc::new(GlesPrograms::new());
        let first_buffer = Rc::clone(&programs);
        let second_buffer = Rc::clone(&programs);

        assert!(Rc::ptr_eq(&first_buffer, &second_buffer));
    }
}
