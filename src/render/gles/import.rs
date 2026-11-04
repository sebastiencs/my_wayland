use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
};

use {
    drm::buffer::DrmFourcc,
    khronos_types::{egl, gl},
};

use {
    super::loader::ImageKHR,
    crate::{
        drm::{Format, Modifier},
        render::{RenderDeviceId, Texture},
        wayland_objects::wl_shm_pool::SliceSharedMemory,
    },
};

#[derive(Debug)]
pub struct EglModifier {
    pub modifier: Modifier,
    pub external_only: bool,
}

#[derive(Debug)]
pub struct EglFormat {
    pub format: &'static Format,
    pub implicit_external_only: bool,
    pub modifiers: BTreeMap<Modifier, EglModifier>,
}

#[derive(Debug)]
pub struct OpenGlFormat {
    pub format: &'static Format,
    pub read_modifiers: BTreeSet<Modifier>,
    pub write_modifiers: BTreeSet<Modifier>,
}

#[derive(Debug)]
pub struct EglImage {
    pub(super) display: egl::EGLDisplay,
    pub(super) image: ImageKHR,
    pub(super) external_only: bool,
    pub(super) destroyed: bool,
}

impl Drop for EglImage {
    fn drop(&mut self) {
        if self.destroyed || self.image.ptr.is_null() {
            return;
        }
        unsafe {
            let _ = egl::DestroyImageKHR(self.display, self.image.ptr);
        }
        self.image.ptr = std::ptr::null_mut();
        self.destroyed = true;
    }
}

impl EglImage {
    pub fn into_texture(self, device_id: RenderDeviceId) -> Result<GlTexture, ()> {
        GlTexture::import_image(self, device_id)
    }
}

#[derive(Debug)]
pub struct GlTexture {
    pub(super) image: Option<EglImage>,
    pub(super) texture: gl::Uint,
    pub(super) delete_on_drop: bool,
    pub(super) device_id: RenderDeviceId,
}

impl Drop for GlTexture {
    fn drop(&mut self) {
        if self.delete_on_drop && self.texture != 0 {
            unsafe {
                gl::DeleteTextures(1, &self.texture);
            }
            self.texture = 0;
        }
        self.image.take();
    }
}

impl Texture for GlTexture {
    fn requires_device_context_on_drop(&self) -> bool {
        self.delete_on_drop && self.texture != 0
    }
}

/// Pixel layout GLES uses when uploading a DRM format from shared memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GlPixelUpload {
    pub internal_format: gl::GLint,
    pub format: gl::GLenum,
    pub ty: gl::GLenum,
}

/// `ARGB8888` and `XRGB8888` upload as little-endian `B, G, R, A`.
pub(crate) fn gl_upload_format(fourcc: DrmFourcc) -> Option<GlPixelUpload> {
    match fourcc {
        DrmFourcc::Argb8888 | DrmFourcc::Xrgb8888 => Some(GlPixelUpload {
            internal_format: gl::BGRA_EXT as gl::GLint,
            format: gl::BGRA_EXT as gl::GLenum,
            ty: gl::UNSIGNED_BYTE as gl::GLenum,
        }),
        _ => None,
    }
}

/// GL name for a texture created by `device_id`. A foreign backend or device returns `None`.
pub(super) fn gl_texture_name(
    texture: &dyn Texture,
    device_id: RenderDeviceId,
) -> Option<gl::Uint> {
    let Some(texture) = (texture as &dyn Any).downcast_ref::<GlTexture>() else {
        log::error!("renderer received a texture from a different backend");
        return None;
    };
    if texture.device_id != device_id {
        log::error!(
            "renderer received a texture from DRM device {:?} while drawing for {:?}",
            texture.device_id,
            device_id
        );
        return None;
    }
    Some(texture.texture)
}

pub fn image_target(external_only: bool) -> gl::Enum {
    match external_only {
        true => gl::TEXTURE_EXTERNAL_OES as _,
        false => gl::TEXTURE_2D as _,
    }
}

impl GlTexture {
    pub fn import_image(image: EglImage, device_id: RenderDeviceId) -> Result<GlTexture, ()> {
        let target = image_target(image.external_only);
        let texture = unsafe {
            let mut texture = 0;
            gl::GenTextures(1, &mut texture);
            gl::BindTexture(target, texture);
            gl::TexParameteri(target, gl::TEXTURE_WRAP_S as _, gl::CLAMP_TO_EDGE);
            gl::TexParameteri(target, gl::TEXTURE_WRAP_T as _, gl::CLAMP_TO_EDGE);
            gl::TexParameteri(target, gl::TEXTURE_MIN_FILTER as _, gl::LINEAR as _);
            gl::TexParameteri(target, gl::TEXTURE_MAG_FILTER as _, gl::LINEAR as _);
            gl::EGLImageTargetTexture2DOES(target, image.image.ptr);
            assert_eq!(gl::GetError(), gl::NO_ERROR as _);
            gl::BindTexture(target, 0);
            texture
        };
        Ok(GlTexture { image: Some(image), texture, delete_on_drop: true, device_id })
    }

    pub fn import_shm(
        mem: SliceSharedMemory,
        width: usize,
        height: usize,
        stride: usize,
        format: &Format,
        device_id: RenderDeviceId,
    ) -> Result<GlTexture, ()> {
        let Some(upload) = gl_upload_format(format.drm) else {
            return Err(());
        };
        Self::upload_bgra(
            mem.as_ptr(),
            width as i32,
            height as i32,
            (stride as u32 / format.bpp) as usize,
            upload,
            device_id,
        )
    }

    pub(super) fn import_shm_bytes(
        bytes: &[u8],
        width: i32,
        height: i32,
        stride: usize,
        fourcc: DrmFourcc,
        device_id: RenderDeviceId,
    ) -> Result<GlTexture, ()> {
        if width <= 0 || height <= 0 {
            return Err(());
        }
        let Some(upload) = gl_upload_format(fourcc) else {
            return Err(());
        };
        let row_bytes = usize::try_from(width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or(())?;
        if stride < row_bytes {
            return Err(());
        }
        let needed = usize::try_from(height)
            .ok()
            .and_then(|height| stride.checked_mul(height))
            .ok_or(())?;
        if bytes.len() < needed {
            return Err(());
        }
        Self::upload_bgra(bytes.as_ptr(), width, height, stride / 4, upload, device_id)
    }

    fn upload_bgra(
        pixels: *const u8,
        width: i32,
        height: i32,
        row_pixels: usize,
        upload: GlPixelUpload,
        device_id: RenderDeviceId,
    ) -> Result<Self, ()> {
        let target = gl::TEXTURE_2D as gl::Enum;
        let texture = unsafe {
            let mut texture = 0;
            gl::GenTextures(1, &mut texture);
            gl::BindTexture(target, texture);
            gl::TexParameteri(target, gl::TEXTURE_WRAP_S as _, gl::CLAMP_TO_EDGE);
            gl::TexParameteri(target, gl::TEXTURE_WRAP_T as _, gl::CLAMP_TO_EDGE);
            gl::TexParameteri(target, gl::TEXTURE_MIN_FILTER as _, gl::LINEAR as _);
            gl::TexParameteri(target, gl::TEXTURE_MAG_FILTER as _, gl::LINEAR as _);
            gl::PixelStorei(gl::UNPACK_ROW_LENGTH_EXT as _, row_pixels as i32);
            gl::TexImage2D(
                target,
                0,
                upload.internal_format,
                width,
                height,
                0,
                upload.format,
                upload.ty,
                pixels.cast(),
            );
            gl::PixelStorei(gl::UNPACK_ROW_LENGTH_EXT as _, 0);
            gl::BindTexture(target, 0);
            texture
        };
        Ok(Self { image: None, texture, delete_on_drop: true, device_id })
    }

    pub fn from_rgba(
        pixels: &[u8],
        width: i32,
        height: i32,
        device_id: RenderDeviceId,
    ) -> Result<GlTexture, ()> {
        if width <= 0 || height <= 0 {
            return Err(());
        }
        let expected_len = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .map(|height| width * height * 4)
            })
            .ok_or(())?;
        if pixels.len() != expected_len {
            return Err(());
        }

        let target = gl::TEXTURE_2D as gl::Enum;
        let texture = unsafe {
            let mut texture = 0;
            gl::GenTextures(1, &mut texture);
            gl::BindTexture(target, texture);
            gl::TexParameteri(target, gl::TEXTURE_WRAP_S as _, gl::CLAMP_TO_EDGE);
            gl::TexParameteri(target, gl::TEXTURE_WRAP_T as _, gl::CLAMP_TO_EDGE);
            gl::TexParameteri(target, gl::TEXTURE_MIN_FILTER as _, gl::LINEAR as _);
            gl::TexParameteri(target, gl::TEXTURE_MAG_FILTER as _, gl::LINEAR as _);
            gl::PixelStorei(gl::UNPACK_ALIGNMENT as _, 1);
            gl::TexImage2D(
                target,
                0,
                gl::RGBA as _,
                width,
                height,
                0,
                gl::RGBA as _,
                gl::UNSIGNED_BYTE as _,
                pixels.as_ptr().cast(),
            );
            gl::PixelStorei(gl::UNPACK_ALIGNMENT as _, 4);
            gl::BindTexture(target, 0);
            texture
        };
        Ok(Self { image: None, texture, delete_on_drop: true, device_id })
    }
}

#[cfg(test)]
mod tests {
    use {drm::buffer::DrmFourcc, khronos_types::gl};

    use super::gl_upload_format;

    #[test]
    fn argb_and_xrgb_shared_memory_uploads_as_bgra_bytes() {
        let argb = gl_upload_format(DrmFourcc::Argb8888).unwrap();
        let xrgb = gl_upload_format(DrmFourcc::Xrgb8888).unwrap();

        assert_eq!(argb, xrgb);
        assert_eq!(argb.format, gl::BGRA_EXT as gl::GLenum);
        assert_eq!(argb.ty, gl::UNSIGNED_BYTE as gl::GLenum);
        assert!(gl_upload_format(DrmFourcc::Abgr8888).is_none());
    }
}
