use std::ptr;

use khronos_types::gl;

use {
    super::{
        import::GlTexture, pipeline::GlesDraw, readback::read_flip_y_framebuffer,
        resources::EglFrameBuffer,
    },
    crate::{
        render::{FrameElement, OutputFrameRecipe, RenderDeviceId},
        transform::OutputPresentationTransform,
    },
};

#[derive(Debug)]
pub(super) struct TextureFrameBuffer {
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) framebuffer_object: gl::GLuint,
    pub(super) texture: gl::GLuint,
}

impl TextureFrameBuffer {
    pub(super) fn new(width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let mut texture = 0;
        let mut framebuffer_object = 0;
        unsafe {
            gl::GenTextures(1, &mut texture);
            gl::BindTexture(gl::TEXTURE_2D as _, texture);
            gl::TexParameteri(gl::TEXTURE_2D as _, gl::TEXTURE_WRAP_S as _, gl::CLAMP_TO_EDGE);
            gl::TexParameteri(gl::TEXTURE_2D as _, gl::TEXTURE_WRAP_T as _, gl::CLAMP_TO_EDGE);
            gl::TexParameteri(gl::TEXTURE_2D as _, gl::TEXTURE_MIN_FILTER as _, gl::LINEAR as _);
            gl::TexParameteri(gl::TEXTURE_2D as _, gl::TEXTURE_MAG_FILTER as _, gl::LINEAR as _);
            gl::TexImage2D(
                gl::TEXTURE_2D as _,
                0,
                gl::RGBA as _,
                width,
                height,
                0,
                gl::RGBA as _,
                gl::UNSIGNED_BYTE as _,
                ptr::null(),
            );
            gl::BindTexture(gl::TEXTURE_2D as _, 0);
            gl::GenFramebuffers(1, &mut framebuffer_object);
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer_object);
            gl::FramebufferTexture2D(
                gl::FRAMEBUFFER as _,
                gl::COLOR_ATTACHMENT0 as _,
                gl::TEXTURE_2D as _,
                texture,
                0,
            );
            let status = gl::CheckFramebufferStatus(gl::FRAMEBUFFER as _);
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, 0);
            if status != gl::FRAMEBUFFER_COMPLETE as _ {
                gl::DeleteFramebuffers(1, &framebuffer_object);
                gl::DeleteTextures(1, &texture);
                return None;
            }
        }
        Some(Self { width, height, framebuffer_object, texture })
    }

    pub(super) fn into_texture(mut self, device_id: RenderDeviceId) -> GlTexture {
        let texture = self.texture;
        let framebuffer_object = self.framebuffer_object;
        self.texture = 0;
        self.framebuffer_object = 0;
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, 0);
            if framebuffer_object != 0 {
                gl::DeleteFramebuffers(1, &framebuffer_object);
            }
        }
        GlTexture { image: None, texture, delete_on_drop: true, device_id }
    }
}

/// Offscreen color target that is not a KMS buffer. Readback uses the same top-left row order
/// as a FlipY output framebuffer.
#[derive(Debug)]
pub(crate) struct OffscreenColorTarget {
    framebuffer: TextureFrameBuffer,
}

impl OffscreenColorTarget {
    pub(crate) fn new(width: i32, height: i32) -> Option<Self> {
        TextureFrameBuffer::new(width, height).map(|framebuffer| Self { framebuffer })
    }

    pub(super) fn render_elements(
        &self,
        draw: &GlesDraw,
        elements: &[FrameElement],
        presentation_transform: OutputPresentationTransform,
    ) {
        draw.paint_transparent_target(
            self.framebuffer.framebuffer_object,
            self.framebuffer.width,
            self.framebuffer.height,
            elements,
            presentation_transform,
        );
    }

    pub(crate) fn render_recipe(&self, renderer: &EglFrameBuffer, recipe: &OutputFrameRecipe) {
        renderer.render_into_transparent(
            self.framebuffer.framebuffer_object,
            self.framebuffer.width,
            self.framebuffer.height,
            recipe,
        );
    }

    pub(super) fn width(&self) -> i32 {
        self.framebuffer.width
    }

    pub(super) fn height(&self) -> i32 {
        self.framebuffer.height
    }

    pub(crate) fn read_top_left(&self, dst_stride: usize, dst: &mut [u8]) -> bool {
        self.read_rect(0, 0, self.framebuffer.width, self.framebuffer.height, dst_stride, dst)
    }

    pub(super) fn read_rect(
        &self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        dst_stride: usize,
        dst: &mut [u8],
    ) -> bool {
        read_flip_y_framebuffer(
            self.framebuffer.framebuffer_object,
            x,
            y,
            width,
            height,
            dst_stride,
            dst,
        )
    }
}

impl Drop for TextureFrameBuffer {
    fn drop(&mut self) {
        unsafe {
            if self.framebuffer_object != 0 {
                gl::DeleteFramebuffers(1, &self.framebuffer_object);
            }
            if self.texture != 0 {
                gl::DeleteTextures(1, &self.texture);
            }
        }
    }
}
