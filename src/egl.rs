use std::{
    cell::OnceCell,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ffi::CStr,
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd},
    ptr,
    time::Duration,
};

use {
    drm::buffer::DrmFourcc,
    khronos_types::{egl, gl},
};

// use {
//     egli::{egl, query_extensions, query_version},
//     std::collections::BTreeSet,
// };

use drm::control::Device;

use crate::{
    dmabuf::DmaBuf,
    drm::{DrmFd, Format, Modifier, RenderSyncMode, FORMATS_MAP},
    gbm::GbmDevice,
    region::{DamageRegion, RectI},
    render::{
        shadow_geometry, BlurVisualStyle, EffectElement, FrameElement, OutputFrameRecipe,
        TextureMask, TextureSource,
    },
    state::RootSurfaceClip,
    transform::{Mat3, OutputPresentationTransform},
    wayland_objects::wl_shm_pool::SliceSharedMemory,
};

const EGL_PLATFORM_GBM_KHR: u32 = 0x31D7;
const EGL_SYNC_NATIVE_FENCE_ANDROID: egl::Enum = 0x3144;
const EGL_SYNC_NATIVE_FENCE_FD_ANDROID: egl::Int = 0x3145;
const EGL_NO_NATIVE_FENCE_FD_ANDROID: egl::Int = -1;

#[derive(Clone, Copy, Debug)]
struct EglSyncKhr(*mut std::ffi::c_void);

impl EglSyncKhr {
    const NO_SYNC: Self = Self(std::ptr::null_mut());

    fn is_null(self) -> bool {
        self.0.is_null()
    }
}

fn load_methods() -> Result<(), String> {
    use khronos_types::{egl::load as load_egl, gl::load as load_gl};

    let get_addr = |s: &CStr| unsafe {
        let ptr = egl::GetProcAddress(s.as_ptr());
        if ptr.is_null() {
            None
        } else {
            Some(ptr)
        }
    };

    for loader in [
        load_egl::CreatePlatformWindowSurfaceEXT as unsafe fn(_) -> Option<_>,
        load_egl::CreateImageKHR as _,
        load_egl::DestroyImageKHR as _,
        load_egl::QueryDmaBufFormatsEXT as _,
        load_egl::QueryDmaBufModifiersEXT as _,
        load_gl::EGLImageTargetRenderbufferStorageOES as _,
        load_gl::DeleteRenderbuffers as _,
        load_gl::GenFramebuffers as _,
        load_gl::BindFramebuffer as _,
        load_gl::FramebufferTexture2D as _,
        load_gl::FramebufferRenderbuffer as _,
        load_gl::CheckFramebufferStatus as _,
        load_gl::DeleteFramebuffers as _,
        load_gl::GenRenderbuffers as _,
        load_gl::BindRenderbuffer as _,
        load_gl::EGLImageTargetTexture2DOES as _,
    ] {
        let entry: Option<*mut std::ffi::c_void> = unsafe { loader(get_addr) };
        if entry.is_none() {
            return Err("failed to load an EGL or GL entry point".into());
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct EGL {}

impl EGL {
    pub fn new() -> Result<Self, String> {
        load_methods()?;
        Ok(Self {})
    }

    fn error_on<T: PartialEq>(result: T, error_value: T) -> Result<T, egl::Error> {
        if result == error_value {
            Err(Self::last_error().unwrap_err())
        } else {
            Ok(result)
        }
    }

    fn error_if(is_error: bool) -> Result<(), egl::Error> {
        if is_error {
            Err(Self::last_error().unwrap_err())
        } else {
            Ok(())
        }
    }

    fn ok_on<T: PartialEq>(result: T, ok_value: T) -> Result<T, egl::Error> {
        if result == ok_value {
            Ok(result)
        } else {
            Err(Self::last_error().unwrap_err())
        }
    }

    fn last_error() -> Result<(), egl::Error> {
        let e = match unsafe { egl::GetError() as _ } {
            egl::SUCCESS => return Ok(()),
            egl::NOT_INITIALIZED => egl::Error::NotInitialized,
            egl::BAD_ACCESS => egl::Error::BadAccess,
            egl::BAD_ALLOC => egl::Error::BadAlloc,
            egl::BAD_ATTRIBUTE => egl::Error::BadAttribute,
            egl::BAD_CONTEXT => egl::Error::BadContext,
            egl::BAD_CONFIG => egl::Error::BadConfig,
            egl::BAD_CURRENT_SURFACE => egl::Error::BadCurrentSurface,
            egl::BAD_DISPLAY => egl::Error::BadDisplay,
            egl::BAD_SURFACE => egl::Error::BadSurface,
            egl::BAD_MATCH => egl::Error::BadMatch,
            egl::BAD_PARAMETER => egl::Error::BadParameter,
            egl::BAD_NATIVE_PIXMAP => egl::Error::BadNativePixmap,
            egl::BAD_NATIVE_WINDOW => egl::Error::BadNativeWindow,
            egl::CONTEXT_LOST => egl::Error::ContextLost,
            code => {
                log::warn!("eglGetError: unknown code {code:#x}, treating as BAD_PARAMETER");
                egl::Error::BadParameter
            }
        };
        Err(e)
    }

    pub fn bind_api(&self, api: i32) -> Result<(), egl::Error> {
        let result = unsafe { egl::BindAPI(api as _) };
        Self::error_on(result, egl::FALSE as _)?;
        Ok(())
    }

    fn get_platform_display(
        &self,
        platform: egl::EGLenum,
        native_display: egl::EGLDisplay,
        attrib_list: &[egl::EGLAttrib],
    ) -> Result<egl::EGLDisplay, egl::Error> {
        Self::assert_attribs_with_none(attrib_list);
        let display =
            unsafe { egl::GetPlatformDisplay(platform, native_display, attrib_list.as_ptr()) };
        Self::error_on(display, egl::NO_DISPLAY)
    }

    fn initialize(&self, display: egl::Display) -> Result<(i32, i32), egl::Error> {
        let (mut major, mut minor) = (0, 0);
        let result = unsafe { egl::Initialize(display, &mut major, &mut minor) };
        Self::ok_on(result, egl::TRUE as _)?;
        Ok((major, minor))
    }

    fn query_string(
        &self,
        display: Option<egl::Display>,
        name: egl::Int,
    ) -> Result<&'static CStr, egl::Error> {
        let display = display.unwrap_or(egl::NO_DISPLAY);
        let s = unsafe { egl::QueryString(display, name) };
        Self::error_if(s.is_null())?;
        Ok(unsafe { CStr::from_ptr(s) })
    }

    fn create_context(
        &self,
        display: egl::Display,
        config: egl::Config,
        share_context: Option<egl::Context>,
        attrib_list: &[egl::Int],
    ) -> Result<egl::Context, egl::Error> {
        Self::assert_ends_with_none(attrib_list);
        let share_context = share_context.unwrap_or(egl::NO_CONTEXT);
        let context =
            unsafe { egl::CreateContext(display, config, share_context, attrib_list.as_ptr()) };
        Self::error_on(context, egl::NO_CONTEXT)
    }

    fn make_current(
        &self,
        display: egl::Display,
        draw: Option<egl::Surface>,
        read: Option<egl::Surface>,
        context: Option<egl::Context>,
    ) -> Result<(), egl::Error> {
        let draw = draw.unwrap_or(egl::NO_SURFACE);
        let read = read.unwrap_or(egl::NO_SURFACE);
        let context = context.unwrap_or(egl::NO_CONTEXT);
        let result = unsafe { egl::MakeCurrent(display, draw, read, context) };
        Self::ok_on(result, egl::TRUE as _)?;
        Ok(())
    }

    pub fn get_config_count(&self, display: egl::Display) -> Result<usize, egl::Error> {
        let mut count = 0;
        let result = unsafe { egl::GetConfigs(display, std::ptr::null_mut(), 0, &mut count) };
        Self::ok_on(result, egl::TRUE as _)?;
        Ok(count as usize)
    }

    pub fn get_configs(&self, display: egl::Display) -> Result<Vec<egl::Config>, egl::Error> {
        let nconfigs = self.get_config_count(display)?;
        if nconfigs == 0 {
            return Ok(Vec::new());
        }
        let mut configs = Vec::<egl::Config>::with_capacity(nconfigs);
        let mut count = 0;
        let result =
            unsafe { egl::GetConfigs(display, configs.as_mut_ptr(), nconfigs as _, &mut count) };
        assert!(count as usize <= nconfigs);
        Self::ok_on(result, egl::TRUE as _)?;
        unsafe {
            configs.set_len(count as _);
        }
        Ok(configs)
    }

    pub fn get_config_attrib(
        &self,
        display: egl::Display,
        config: egl::Config,
        attribute: egl::Int,
    ) -> Result<egl::Int, egl::Error> {
        let mut value = 0;
        let result = unsafe { egl::GetConfigAttrib(display, config, attribute, &mut value) };
        Self::ok_on(result, egl::TRUE as _)?;
        Ok(value)
    }

    fn assert_attribs_with_none(list: &[egl::Attrib]) {
        assert_eq!(*list.last().unwrap(), egl::NONE as _);
    }
    fn assert_ends_with_none(list: &[egl::Int]) {
        assert_eq!(*list.last().unwrap(), egl::NONE);
    }

    fn create_platform_window_surface(
        &self,
        display: egl::Display,
        config: egl::Config,
        native_window: egl::NativeWindowType,
        attrib_list: &[egl::Int],
    ) -> Result<egl::Surface, egl::Error> {
        Self::assert_ends_with_none(attrib_list);
        let result = unsafe {
            egl::CreatePlatformWindowSurfaceEXT(
                display,
                config,
                native_window,
                attrib_list.as_ptr(),
            )
        };
        Self::error_on(result, egl::NO_SURFACE)
    }

    fn swap_buffers(&self, display: egl::Display, surface: egl::Surface) -> Result<(), egl::Error> {
        let result = unsafe { egl::SwapBuffers(display, surface) };
        Self::ok_on(result, egl::TRUE as _)?;
        Ok(())
    }

    fn create_image_khr(
        &self,
        display: egl::Display,
        target: egl::Enum,
        attrib_list: &[egl::Int],
    ) -> Result<ImageKHR, egl::Error> {
        Self::assert_ends_with_none(attrib_list);
        let ptr = unsafe {
            egl::CreateImageKHR(
                display,
                std::ptr::null_mut(),
                target,
                std::ptr::null_mut(),
                attrib_list.as_ptr(),
            )
        };
        Self::error_on(ptr, egl::NO_IMAGE_KHR)?;
        Ok(ImageKHR { ptr })
    }

    fn image_target_renderbuffer_storage_oes(
        &self,
        target: egl::Enum,
        image: ImageKHR,
    ) -> Result<(), egl::Error> {
        unsafe { gl::EGLImageTargetRenderbufferStorageOES(target, image.ptr) };
        Self::last_error()
    }

    fn query_dma_buf_formats_ext(
        &self,
        display: egl::Display,
    ) -> Result<Vec<egl::Int>, egl::Error> {
        let mut num_formats = 0;
        let result = unsafe {
            egl::QueryDmaBufFormatsEXT(display, 0, std::ptr::null_mut(), &mut num_formats)
        };
        Self::ok_on(result, egl::TRUE as _)?;
        if num_formats == 0 {
            return Ok(Vec::new());
        }

        let mut formats: Vec<egl::Int> = vec![0; num_formats as usize];
        let max_formats = num_formats;
        let mut num_formats = 0;
        let result = unsafe {
            egl::QueryDmaBufFormatsEXT(display, max_formats, formats.as_mut_ptr(), &mut num_formats)
        };
        Self::ok_on(result, egl::TRUE as _)?;
        assert_eq!(max_formats, num_formats);
        Ok(formats)
    }

    fn query_dma_buf_modifiers_ext(
        &self,
        display: egl::Display,
        format: egl::Int,
    ) -> Result<(Vec<Modifier>, Vec<egl::Boolean>), egl::Error> {
        let mut num_modifiers = 0;
        let result = unsafe {
            egl::QueryDmaBufModifiersEXT(
                display,
                format,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut num_modifiers,
            )
        };
        Self::ok_on(result, egl::TRUE as _)?;

        static_assertions::assert_eq_size!(Modifier, u64);
        let mut modifiers = vec![Modifier(0); num_modifiers as usize];
        let mut external_only = vec![0; num_modifiers as usize];
        let max_modifiers = num_modifiers;
        let mut num_modifiers = 0;
        let result = unsafe {
            egl::QueryDmaBufModifiersEXT(
                display,
                format,
                max_modifiers,
                modifiers.as_mut_ptr() as _,
                external_only.as_mut_ptr(),
                &mut num_modifiers,
            )
        };
        assert_eq!(modifiers.len(), num_modifiers as _);
        Self::ok_on(result, egl::TRUE as _)?;

        Ok((modifiers, external_only))
    }

    fn create_sync_khr(
        &self,
        display: egl::Display,
        type_: egl::Enum,
        attrib_list: &[egl::Int],
    ) -> Result<EglSyncKhr, egl::Error> {
        type CreateSyncKhr = unsafe extern "system" fn(
            egl::Display,
            egl::Enum,
            *const egl::Int,
        ) -> *mut std::ffi::c_void;

        Self::assert_ends_with_none(attrib_list);
        let proc = unsafe { egl::GetProcAddress(c"eglCreateSyncKHR".as_ptr()) };
        if proc.is_null() {
            return Err(egl::Error::BadDisplay);
        }
        let create_sync: CreateSyncKhr = unsafe { std::mem::transmute(proc) };
        let sync = unsafe { create_sync(display, type_, attrib_list.as_ptr()) };
        let sync = EglSyncKhr(sync);
        if sync.is_null() {
            return Err(Self::last_error().unwrap_err());
        }
        Ok(sync)
    }

    fn destroy_sync_khr(&self, display: egl::Display, sync: EglSyncKhr) -> Result<(), egl::Error> {
        type DestroySyncKhr =
            unsafe extern "system" fn(egl::Display, *mut std::ffi::c_void) -> egl::Boolean;

        if sync.is_null() {
            return Ok(());
        }

        let proc = unsafe { egl::GetProcAddress(c"eglDestroySyncKHR".as_ptr()) };
        if proc.is_null() {
            return Err(egl::Error::BadDisplay);
        }
        let destroy_sync: DestroySyncKhr = unsafe { std::mem::transmute(proc) };
        let result = unsafe { destroy_sync(display, sync.0) };
        Self::ok_on(result, egl::TRUE as _)?;
        Ok(())
    }

    fn dup_native_fence_fd_android(
        &self,
        display: egl::Display,
        sync: EglSyncKhr,
    ) -> Result<OwnedFd, egl::Error> {
        type DupNativeFenceFdAndroid =
            unsafe extern "system" fn(egl::Display, *mut std::ffi::c_void) -> egl::Int;

        let proc = unsafe { egl::GetProcAddress(c"eglDupNativeFenceFDANDROID".as_ptr()) };
        if proc.is_null() {
            return Err(egl::Error::BadDisplay);
        }
        let dup_native_fence: DupNativeFenceFdAndroid = unsafe { std::mem::transmute(proc) };
        let fd = unsafe { dup_native_fence(display, sync.0) };
        if fd == EGL_NO_NATIVE_FENCE_FD_ANDROID {
            return Err(Self::last_error().unwrap_err());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn wait_sync_khr(
        &self,
        display: egl::Display,
        sync: EglSyncKhr,
        flags: egl::Int,
    ) -> Result<(), egl::Error> {
        type WaitSyncKhr = unsafe extern "system" fn(
            egl::Display,
            *mut std::ffi::c_void,
            egl::Int,
        ) -> egl::Boolean;

        let proc = unsafe { egl::GetProcAddress(c"eglWaitSyncKHR".as_ptr()) };
        if proc.is_null() {
            return Err(egl::Error::BadDisplay);
        }
        let wait_sync: WaitSyncKhr = unsafe { std::mem::transmute(proc) };
        let result = unsafe { wait_sync(display, sync.0, flags) };
        Self::ok_on(result, egl::TRUE as _)?;
        Ok(())
    }
}

pub fn init() -> Result<EGL, String> {
    let egl = EGL::new()?;
    egl.bind_api(egl::OPENGL_ES_API)
        .map_err(|error| format!("eglBindAPI: {error:?}"))?;
    Ok(egl)

    // let egl = unsafe {
    //     egl::DynamicInstance::<EGL1_5>::load_required()
    //         .expect("Unable to load libEGL.so or libEGL.so.1")
    // };

    // let egl: EglInstance = Arc::new(egl);

    // egl.bind_api(khronos_egl::OPENGL_ES_API)
    //     .expect("unable to select OpenGL API");

    // egl

    // dbg!(query_extensions().unwrap());
    // dbg!(query_version().unwrap());

    // egl::bind_api(egl::EGL_OPENGL_ES_API).expect("EGL bind_api failed");

    // let extensions: BTreeSet<&'static str> = query_extensions().unwrap().split(' ').collect();
    // dbg!(extensions);
}

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
    display: egl::EGLDisplay,
    image: ImageKHR,
    external_only: bool,
    destroyed: bool,
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
    pub fn into_texture(self) -> Result<GlTexture, ()> {
        GlTexture::import_image(self)
    }
}

#[derive(Debug)]
pub struct GlTexture {
    image: Option<EglImage>,
    texture: gl::Uint,
    delete_on_drop: bool,
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

pub type RenderItemMask = TextureMask;
pub type RenderItem = crate::render::TextureElement;

pub use crate::render::RenderCompletion;

pub fn image_target(external_only: bool) -> gl::Enum {
    match external_only {
        true => gl::TEXTURE_EXTERNAL_OES as _,
        false => gl::TEXTURE_2D as _,
    }
}

impl GlTexture {
    pub fn import_image(image: EglImage) -> Result<GlTexture, ()> {
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
        Ok(GlTexture { image: Some(image), texture, delete_on_drop: true })
    }

    pub fn import_shm(
        mem: SliceSharedMemory,
        width: usize,
        height: usize,
        stride: usize,
        format: &Format,
    ) -> Result<GlTexture, ()> {
        let target = gl::TEXTURE_2D as gl::Enum;
        let texture = unsafe {
            let mut texture = 0;
            gl::GenTextures(1, &mut texture);
            gl::BindTexture(target, texture);
            gl::TexParameteri(target, gl::TEXTURE_WRAP_S as _, gl::CLAMP_TO_EDGE);
            gl::TexParameteri(target, gl::TEXTURE_WRAP_T as _, gl::CLAMP_TO_EDGE);
            gl::TexParameteri(target, gl::TEXTURE_MIN_FILTER as _, gl::LINEAR as _);
            gl::TexParameteri(target, gl::TEXTURE_MAG_FILTER as _, gl::LINEAR as _);
            let param = stride as u32 / format.bpp;
            gl::PixelStorei(gl::UNPACK_ROW_LENGTH_EXT as _, param as i32);
            gl::TexImage2D(
                target,
                0,
                format.gl_format,
                width as i32,
                height as i32,
                0,
                format.gl_format as _,
                format.gl_type as _,
                mem.as_ptr() as _,
            );
            gl::PixelStorei(gl::UNPACK_ROW_LENGTH_EXT as _, 0);
            gl::BindTexture(target, 0);
            texture
        };
        Ok(Self { image: None, texture, delete_on_drop: true })
    }

    pub fn from_rgba(pixels: &[u8], width: i32, height: i32) -> Result<GlTexture, ()> {
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
        Ok(Self { image: None, texture, delete_on_drop: true })
    }

    #[cfg(test)]
    pub(crate) fn test_inert() -> Self {
        Self { image: None, texture: 0, delete_on_drop: false }
    }

    pub fn texture_id(&self) -> gl::Uint {
        self.texture
    }
}

#[derive(Debug)]
pub struct EglRenderBuffer {
    egl: EGL,
    display: egl::EGLDisplay,
    width: i32,
    height: i32,
    format: &'static Format,
    pub renderbuffer_object: gl::GLuint,
}

impl Drop for EglRenderBuffer {
    fn drop(&mut self) {
        unsafe { gl::DeleteRenderbuffers(1, &self.renderbuffer_object) }
    }
}

impl EglRenderBuffer {
    pub fn make_framebuffer(self) -> EglFrameBuffer {
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
            render_buffer: self,
            framebuffer_object,
            texture_program: OnceCell::new(),
            blur_program: OnceCell::new(),
            border_program: OnceCell::new(),
            shadow_program: OnceCell::new(),
            dim_texture: OnceCell::new(),
        }
    }
}

#[derive(Debug)]
pub struct EglFrameBuffer {
    egl: EGL,
    display: egl::EGLDisplay,
    pub width: i32,
    pub height: i32,
    render_buffer: EglRenderBuffer,
    pub framebuffer_object: gl::GLuint,
    texture_program: OnceCell<Result<TextureProgram, String>>,
    blur_program: OnceCell<Result<BlurProgram, String>>,
    border_program: OnceCell<Result<BorderProgram, String>>,
    shadow_program: OnceCell<Result<ShadowProgram, String>>,
    dim_texture: OnceCell<GlTexture>,
}

impl Drop for EglFrameBuffer {
    fn drop(&mut self) {
        if let Some(Ok(program)) = self.texture_program.take() {
            unsafe { gl::DeleteProgram(program.program) };
        }
        if let Some(Ok(program)) = self.blur_program.take() {
            unsafe { gl::DeleteProgram(program.program) };
        }
        if let Some(Ok(program)) = self.border_program.take() {
            unsafe { gl::DeleteProgram(program.program) };
        }
        if let Some(Ok(program)) = self.shadow_program.take() {
            unsafe { gl::DeleteProgram(program.program) };
        }
        unsafe {
            gl::DeleteFramebuffers(1, &self.framebuffer_object);
        }
    }
}

#[derive(Debug)]
struct TextureFrameBuffer {
    width: i32,
    height: i32,
    framebuffer_object: gl::GLuint,
    texture: gl::GLuint,
}

impl TextureFrameBuffer {
    fn new(width: i32, height: i32) -> Option<Self> {
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

    fn into_texture(mut self) -> GlTexture {
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
        GlTexture { image: None, texture, delete_on_drop: true }
    }
}

/// Offscreen color target that is not a KMS buffer. Readback uses the same top-left row order
/// as a FlipY output framebuffer.
pub(crate) struct OffscreenColorTarget {
    framebuffer: TextureFrameBuffer,
}

impl OffscreenColorTarget {
    pub(crate) fn new(width: i32, height: i32) -> Option<Self> {
        TextureFrameBuffer::new(width, height).map(|framebuffer| Self { framebuffer })
    }

    pub(crate) fn render_recipe(&self, renderer: &EglFrameBuffer, recipe: &OutputFrameRecipe) {
        renderer.render_into_transparent(
            self.framebuffer.framebuffer_object,
            self.framebuffer.width,
            self.framebuffer.height,
            recipe,
        );
    }

    pub(crate) fn read_top_left(&self, dst_stride: usize, dst: &mut [u8]) -> bool {
        read_flip_y_framebuffer(
            self.framebuffer.framebuffer_object,
            0,
            0,
            self.framebuffer.width,
            self.framebuffer.height,
            dst_stride,
            dst,
        )
    }
}

/// Copy a FlipY framebuffer into top-left rows.
///
/// `y` is the framebuffer y of the visual top, matching [`framebuffer_rect_for_target`] for
/// [`OutputPresentationTransform::FlipY`]. GLES2 has no `GL_PACK_ROW_LENGTH`, so each row is read
/// separately and padding in `dst` is left unchanged. The caller keeps this off the KMS submission
/// path.
pub(crate) fn read_flip_y_framebuffer(
    framebuffer: gl::GLuint,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    dst_stride: usize,
    dst: &mut [u8],
) -> bool {
    if width <= 0 || height <= 0 {
        return false;
    }
    let row_bytes = (width as usize).saturating_mul(4);
    if dst_stride < row_bytes {
        return false;
    }
    let Some(needed) = dst_stride.checked_mul(height as usize) else {
        return false;
    };
    if dst.len() < needed {
        return false;
    }
    unsafe {
        let mut previous_framebuffer = 0;
        let mut previous_alignment = 4;
        gl::GetIntegerv(gl::FRAMEBUFFER_BINDING as _, &mut previous_framebuffer);
        gl::GetIntegerv(gl::PACK_ALIGNMENT as _, &mut previous_alignment);
        gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer);
        gl::PixelStorei(gl::PACK_ALIGNMENT as _, 1);
        gl::Finish();
        while gl::GetError() != gl::NO_ERROR as _ {}
        let mut ok = true;
        for row in 0..height {
            let dst_start = (row as usize).saturating_mul(dst_stride);
            gl::ReadPixels(
                x,
                y.saturating_add(row),
                width,
                1,
                gl::BGRA_EXT as _,
                gl::UNSIGNED_BYTE as _,
                dst[dst_start..].as_mut_ptr().cast(),
            );
            if gl::GetError() != gl::NO_ERROR as _ {
                ok = false;
                break;
            }
        }
        gl::PixelStorei(gl::PACK_ALIGNMENT as _, previous_alignment);
        gl::BindFramebuffer(gl::FRAMEBUFFER as _, previous_framebuffer as gl::GLuint);
        ok
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

#[derive(Debug)]
struct TextureProgram {
    program: gl::Uint,
    position_location: gl::Uint,
    texcoord_location: gl::Uint,
    sampler_location: gl::Int,
    transform_location: gl::Int,
    tex_transform_location: gl::Int,
    item_size_location: gl::Int,
    clip_mode_location: gl::Int,
    clip_outer_rect_location: gl::Int,
    clip_inner_rect_location: gl::Int,
    outer_corner_radius_location: gl::Int,
    outer_rounding_power_location: gl::Int,
    inner_corner_radius_location: gl::Int,
    inner_rounding_power_location: gl::Int,
    alpha_location: gl::Int,
}

#[derive(Debug)]
struct BlurProgram {
    program: gl::Uint,
    position_location: gl::Uint,
    texcoord_location: gl::Uint,
    sampler_location: gl::Int,
    direction_location: gl::Int,
    texel_size_location: gl::Int,
    radius_location: gl::Int,
    transform_location: gl::Int,
    tex_transform_location: gl::Int,
    item_size_location: gl::Int,
}

#[derive(Debug)]
struct BorderProgram {
    program: gl::Uint,
    position_location: gl::Uint,
    transform_location: gl::Int,
    item_size_location: gl::Int,
    outer_rect_location: gl::Int,
    inner_rect_location: gl::Int,
    outer_corner_radius_location: gl::Int,
    outer_rounding_power_location: gl::Int,
    inner_corner_radius_location: gl::Int,
    inner_rounding_power_location: gl::Int,
    start_color_location: gl::Int,
    end_color_location: gl::Int,
    angle_location: gl::Int,
    alpha_location: gl::Int,
}

#[derive(Debug)]
struct ShadowProgram {
    program: gl::Uint,
    position_location: gl::Uint,
    transform_location: gl::Int,
    item_size_location: gl::Int,
    shadow_color_location: gl::Int,
    inner_rect_location: gl::Int,
    corner_radius_location: gl::Int,
    rounding_power_location: gl::Int,
    shadow_range_location: gl::Int,
    shadow_power_location: gl::Int,
}

impl EglFrameBuffer {
    fn texture_program(&self) -> Result<&TextureProgram, &str> {
        self.texture_program
            .get_or_init(create_texture_program)
            .as_ref()
            .map_err(String::as_str)
    }

    fn blur_program(&self) -> Result<&BlurProgram, &str> {
        self.blur_program
            .get_or_init(create_blur_program)
            .as_ref()
            .map_err(String::as_str)
    }

    fn border_program(&self) -> Result<&BorderProgram, &str> {
        self.border_program
            .get_or_init(create_border_program)
            .as_ref()
            .map_err(String::as_str)
    }

    fn shadow_program(&self) -> Result<&ShadowProgram, &str> {
        self.shadow_program
            .get_or_init(create_shadow_program)
            .as_ref()
            .map_err(String::as_str)
    }

    fn dim_texture(&self) -> Option<&GlTexture> {
        if let Some(texture) = self.dim_texture.get() {
            return Some(texture);
        }
        let texture = GlTexture::from_rgba(&[0, 0, 0, u8::MAX], 1, 1).ok()?;
        let _ = self.dim_texture.set(texture);
        self.dim_texture.get()
    }

    fn scissor_y(
        &self,
        target_height: i32,
        rect: RectI,
        output_transform: OutputPresentationTransform,
    ) -> i32 {
        framebuffer_rect_for_target(target_height, rect, output_transform)
            .map(|framebuffer_rect| framebuffer_rect.y)
            .unwrap_or(0)
    }

    pub fn clear(&self) {
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, self.framebuffer_object);
            gl::Viewport(0, 0, self.width, self.height);
            gl::ClearColor(1.0, 0.0, 0.0, 1.0);
            gl::Clear(gl::COLOR_BUFFER_BIT as _);
            gl::BlendFunc(gl::ONE as _, gl::ONE_MINUS_SRC_ALPHA as _);
        }
        unsafe {
            gl::Flush();
        }
    }

    fn output_bounds(&self) -> Option<RectI> {
        RectI::new(0, 0, self.width, self.height)
    }

    fn clear_damage(
        &self,
        damage: &DamageRegion,
        preserve_contents: bool,
        output_transform: OutputPresentationTransform,
    ) {
        let Some(output_bounds) = self.output_bounds() else {
            return;
        };
        let mut clipped_damage = damage.clone();
        clipped_damage.intersect_rect(output_bounds);

        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, self.framebuffer_object);
            gl::Viewport(0, 0, self.width, self.height);
            gl::ClearColor(1.0, 0.0, 0.0, 1.0);
            gl::BlendFunc(gl::ONE as _, gl::ONE_MINUS_SRC_ALPHA as _);
        }

        if !preserve_contents || clipped_damage.extents() == Some(output_bounds) {
            unsafe {
                gl::Disable(gl::SCISSOR_TEST as _);
                gl::Clear(gl::COLOR_BUFFER_BIT as _);
                gl::Flush();
            }
            return;
        }

        unsafe {
            gl::Enable(gl::SCISSOR_TEST as _);
            for rect in clipped_damage.rects() {
                gl::Scissor(
                    rect.x,
                    self.scissor_y(self.height, *rect, output_transform),
                    rect.width,
                    rect.height,
                );
                gl::Clear(gl::COLOR_BUFFER_BIT as _);
            }
            gl::Disable(gl::SCISSOR_TEST as _);
            gl::Flush();
        }
    }

    pub fn clear_with(&self, _r: f32, _g: f32, _b: f32, _a: f32) {
        // let ops = self.take_render_ops();
        // self.render(ops, Some(&Color { r, g, b, a }));
    }

    fn finish_render(&self, render_sync_mode: RenderSyncMode) -> RenderCompletion {
        match render_sync_mode {
            RenderSyncMode::ExplicitInFence => match self.export_native_render_fence() {
                Ok(in_fence_fd) => {
                    if crate::logger::ENABLE_NOISY_LOG {
                        log::debug!(
                            "egl finish_render: exported explicit render fence fd={}",
                            in_fence_fd.as_raw_fd()
                        );
                    }
                    RenderCompletion::explicit(in_fence_fd)
                }
                Err(error) => {
                    log::warn!(
                        "egl finish_render: explicit fence export failed, falling back to glFinish: {:?}",
                        error
                    );
                    unsafe { gl::Finish() };
                    RenderCompletion::blocking_fallback()
                }
            },
            RenderSyncMode::BlockingFinishFallback => {
                unsafe { gl::Finish() };
                RenderCompletion::blocking_fallback()
            }
        }
    }

    fn export_native_render_fence(&self) -> Result<OwnedFd, egl::Error> {
        let sync =
            self.egl
                .create_sync_khr(self.display, EGL_SYNC_NATIVE_FENCE_ANDROID, &[egl::NONE])?;
        unsafe { gl::Flush() };
        let fence_fd = self.egl.dup_native_fence_fd_android(self.display, sync);
        let destroy_result = self.egl.destroy_sync_khr(self.display, sync);
        if let Err(error) = destroy_result {
            log::warn!("egl destroy_sync_khr failed after fence export: {:?}", error);
        }
        fence_fd
    }

    pub fn render(
        &self,
        recipe: &OutputFrameRecipe,
        projection: Mat3,
        render_sync_mode: RenderSyncMode,
        damage: &DamageRegion,
        preserve_contents: bool,
        output_transform: OutputPresentationTransform,
    ) -> RenderCompletion {
        self.clear_damage(damage, preserve_contents, output_transform);

        if recipe.is_empty() {
            return self.finish_render(render_sync_mode);
        }

        self.render_frame_elements_to_target(
            self.framebuffer_object,
            self.width,
            self.height,
            &recipe.elements,
            projection,
            damage,
            output_transform,
        );

        self.finish_render(render_sync_mode)
    }

    fn render_texture_elements_to_target(
        &self,
        framebuffer_object: gl::GLuint,
        target_width: i32,
        target_height: i32,
        items: &[RenderItem],
        projection: Mat3,
        damage: &DamageRegion,
        output_transform: OutputPresentationTransform,
    ) {
        if items.is_empty() {
            return;
        }
        let program = match self.texture_program() {
            Ok(program) => program,
            Err(error) => {
                log::error!("texture shader is unavailable: {error}");
                return;
            }
        };
        let vertices: [f32; 16] =
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let stride = (4 * std::mem::size_of::<f32>()) as i32;
        let output_bounds = RectI::new(0, 0, target_width, target_height);

        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer_object);
            gl::Viewport(0, 0, target_width, target_height);
            gl::UseProgram(program.program);
            gl::BlendFunc(gl::ONE as _, gl::ONE_MINUS_SRC_ALPHA as _);
            gl::Uniform1i(program.sampler_location, 0);
            gl::ActiveTexture(gl::TEXTURE0 as _);
            gl::EnableVertexAttribArray(program.position_location);
            gl::EnableVertexAttribArray(program.texcoord_location);
        }

        for item in items {
            let Some(item_bounds) = RectI::new(item.x, item.y, item.width, item.height) else {
                continue;
            };
            let mut clipped_damage = damage.clone();
            if let Some(output_bounds) = output_bounds {
                clipped_damage.intersect_rect(output_bounds);
            }
            clipped_damage.intersect_rect(item_bounds);
            clipped_damage.intersect_rect(item.damage.extents().unwrap_or(item_bounds));
            if clipped_damage.is_empty() {
                continue;
            }
            let transform = crate::transform::project_box(
                item.x as f32,
                item.y as f32,
                item.width as f32,
                item.height as f32,
                projection,
            )
            .to_gl_array();
            let tex_transform = item.tex_transform.to_gl_array();
            unsafe {
                if item.blend || item.alpha < u8::MAX {
                    gl::Enable(gl::BLEND as _);
                } else {
                    gl::Disable(gl::BLEND as _);
                }
                gl::BindTexture(gl::TEXTURE_2D as _, item.texture);
                gl::UniformMatrix3fv(program.transform_location, 1, 0, transform.as_ptr());
                gl::UniformMatrix3fv(program.tex_transform_location, 1, 0, tex_transform.as_ptr());
                gl::Uniform2f(program.item_size_location, item.width as f32, item.height as f32);
                gl::Uniform1f(program.alpha_location, f32::from(item.alpha) / 255.0);
                match &item.mask {
                    RenderItemMask::None => {
                        gl::Uniform1i(program.clip_mode_location, 0);
                    }
                    RenderItemMask::RoundedClip { clip } => {
                        gl::Uniform1i(program.clip_mode_location, 1);
                        gl::Uniform4f(
                            program.clip_outer_rect_location,
                            clip.outer_rect.x as f32,
                            clip.outer_rect.y as f32,
                            clip.outer_rect.width as f32,
                            clip.outer_rect.height as f32,
                        );
                        gl::Uniform4f(program.clip_inner_rect_location, 0.0, 0.0, 0.0, 0.0);
                        gl::Uniform1f(
                            program.outer_corner_radius_location,
                            clip.corner_radius.max(0) as f32,
                        );
                        gl::Uniform1f(
                            program.outer_rounding_power_location,
                            clip.rounding_power.get().max(0.0001),
                        );
                        gl::Uniform1f(program.inner_corner_radius_location, 0.0);
                        gl::Uniform1f(program.inner_rounding_power_location, 0.0001);
                    }
                    RenderItemMask::FrameRing { ring } => {
                        gl::Uniform1i(program.clip_mode_location, 2);
                        gl::Uniform4f(
                            program.clip_outer_rect_location,
                            ring.outer.outer_rect.x as f32,
                            ring.outer.outer_rect.y as f32,
                            ring.outer.outer_rect.width as f32,
                            ring.outer.outer_rect.height as f32,
                        );
                        gl::Uniform4f(
                            program.clip_inner_rect_location,
                            ring.inner.outer_rect.x as f32,
                            ring.inner.outer_rect.y as f32,
                            ring.inner.outer_rect.width as f32,
                            ring.inner.outer_rect.height as f32,
                        );
                        gl::Uniform1f(
                            program.outer_corner_radius_location,
                            ring.outer.corner_radius.max(0) as f32,
                        );
                        gl::Uniform1f(
                            program.outer_rounding_power_location,
                            ring.outer.rounding_power.get().max(0.0001),
                        );
                        gl::Uniform1f(
                            program.inner_corner_radius_location,
                            ring.inner.corner_radius.max(0) as f32,
                        );
                        gl::Uniform1f(
                            program.inner_rounding_power_location,
                            ring.inner.rounding_power.get().max(0.0001),
                        );
                    }
                }
                gl::VertexAttribPointer(
                    program.position_location,
                    2,
                    gl::FLOAT as _,
                    0,
                    stride,
                    vertices.as_ptr().cast(),
                );
                gl::VertexAttribPointer(
                    program.texcoord_location,
                    2,
                    gl::FLOAT as _,
                    0,
                    stride,
                    vertices.as_ptr().add(2).cast(),
                );
                gl::Enable(gl::SCISSOR_TEST as _);
                for rect in clipped_damage.rects() {
                    gl::Scissor(
                        rect.x,
                        self.scissor_y(target_height, *rect, output_transform),
                        rect.width,
                        rect.height,
                    );
                    gl::DrawArrays(gl::TRIANGLE_STRIP as _, 0, 4);
                }
                gl::Disable(gl::SCISSOR_TEST as _);
            }
        }

        unsafe {
            gl::BindTexture(gl::TEXTURE_2D as _, 0);
            gl::Disable(gl::BLEND as _);
            gl::DisableVertexAttribArray(program.position_location);
            gl::DisableVertexAttribArray(program.texcoord_location);
            gl::UseProgram(0);
        }
    }

    fn blur_snapshot_bounds(
        &self,
        clip: crate::state::RootSurfaceClip,
        blur_style: BlurVisualStyle,
    ) -> Option<RectI> {
        let output_bounds = RectI::new(0, 0, self.width, self.height)?;
        let padding = blur_style
            .radius
            .max(1)
            .saturating_mul(i32::from(blur_style.passes.max(1)))
            .saturating_mul(4);
        RectI::from_extents(
            clip.outer_rect.x.saturating_sub(padding),
            clip.outer_rect.y.saturating_sub(padding),
            clip.outer_rect.x2().saturating_add(padding),
            clip.outer_rect.y2().saturating_add(padding),
        )
        .and_then(|bounds| bounds.intersection(output_bounds))
    }

    fn build_blurred_backdrop(
        &self,
        framebuffer_object: gl::GLuint,
        target_height: i32,
        ring: crate::render::RoundedRing,
        blur_style: BlurVisualStyle,
        output_transform: OutputPresentationTransform,
    ) -> Option<(TextureFrameBuffer, RectI)> {
        let blur_bounds = self.blur_snapshot_bounds(ring.outer, blur_style)?;
        let copy_rect = framebuffer_rect_for_target(target_height, blur_bounds, output_transform)?;
        let blur_input = TextureFrameBuffer::new(blur_bounds.width, blur_bounds.height)?;
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer_object);
            gl::BindTexture(gl::TEXTURE_2D as _, blur_input.texture);
            gl::CopyTexSubImage2D(
                gl::TEXTURE_2D as _,
                0,
                0,
                0,
                copy_rect.x,
                copy_rect.y,
                copy_rect.width,
                copy_rect.height,
            );
            gl::BindTexture(gl::TEXTURE_2D as _, 0);
        }
        let mut scratch_a = TextureFrameBuffer::new(blur_bounds.width, blur_bounds.height)?;
        let mut scratch_b = TextureFrameBuffer::new(blur_bounds.width, blur_bounds.height)?;
        self.blur_texture(
            blur_input.texture,
            &mut scratch_a,
            &mut scratch_b,
            blur_bounds.width,
            blur_bounds.height,
            blur_style.radius.max(1) as f32,
            blur_style.passes.max(1),
            crate::transform::output_projection(
                blur_bounds.width as f32,
                blur_bounds.height as f32,
                output_transform,
            ),
        );
        Some((scratch_a, blur_bounds))
    }

    fn blur_texture(
        &self,
        input_texture: gl::GLuint,
        scratch_a: &mut TextureFrameBuffer,
        scratch_b: &mut TextureFrameBuffer,
        width: i32,
        height: i32,
        radius: f32,
        passes: u8,
        projection: Mat3,
    ) {
        let program = match self.blur_program() {
            Ok(program) => program,
            Err(error) => {
                log::error!("blur shader is unavailable: {error}");
                return;
            }
        };
        let vertices: [f32; 16] =
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let stride = (4 * std::mem::size_of::<f32>()) as i32;
        let transform =
            crate::transform::project_box(0.0, 0.0, width as f32, height as f32, projection)
                .to_gl_array();
        let tex_transform = Mat3::identity().to_gl_array();
        let sanitized_radius = radius.max(1.0);
        let pass_draws =
            blur_pass_draws(input_texture, scratch_a.texture, scratch_b.texture, passes);
        unsafe {
            gl::UseProgram(program.program);
            gl::Uniform1i(program.sampler_location, 0);
            gl::Uniform2f(program.texel_size_location, 1.0 / width as f32, 1.0 / height as f32);
            gl::Uniform1f(program.radius_location, sanitized_radius);
            gl::UniformMatrix3fv(program.transform_location, 1, 0, transform.as_ptr());
            gl::UniformMatrix3fv(program.tex_transform_location, 1, 0, tex_transform.as_ptr());
            gl::Uniform2f(program.item_size_location, width as f32, height as f32);
            gl::EnableVertexAttribArray(program.position_location);
            gl::EnableVertexAttribArray(program.texcoord_location);
            gl::VertexAttribPointer(
                program.position_location,
                2,
                gl::FLOAT as _,
                0,
                stride,
                vertices.as_ptr().cast(),
            );
            gl::VertexAttribPointer(
                program.texcoord_location,
                2,
                gl::FLOAT as _,
                0,
                stride,
                vertices.as_ptr().add(2).cast(),
            );
            gl::ActiveTexture(gl::TEXTURE0 as _);
        }
        for (source_texture, target_texture, direction) in &pass_draws {
            let target = if *target_texture == scratch_a.texture {
                scratch_a.framebuffer_object
            } else {
                scratch_b.framebuffer_object
            };
            unsafe {
                gl::BindFramebuffer(gl::FRAMEBUFFER as _, target);
                gl::Viewport(0, 0, width, height);
                gl::Uniform2f(program.direction_location, direction[0], direction[1]);
                gl::BindTexture(gl::TEXTURE_2D as _, *source_texture);
                gl::DrawArrays(gl::TRIANGLE_STRIP as _, 0, 4);
            }
        }
        unsafe {
            gl::BindTexture(gl::TEXTURE_2D as _, 0);
            gl::DisableVertexAttribArray(program.position_location);
            gl::DisableVertexAttribArray(program.texcoord_location);
            gl::UseProgram(0);
        }
        if pass_draws
            .last()
            .is_some_and(|(_, target_texture, _)| *target_texture == scratch_b.texture)
        {
            std::mem::swap(scratch_a, scratch_b);
        }
    }
}

fn blur_pass_draws(
    input_texture: gl::GLuint,
    scratch_a_texture: gl::GLuint,
    scratch_b_texture: gl::GLuint,
    passes: u8,
) -> Vec<(gl::GLuint, gl::GLuint, [f32; 2])> {
    let mut draws = Vec::with_capacity(usize::from(passes.max(1)) * 2);
    let mut source_texture = input_texture;
    for _ in 0..passes.max(1) {
        for (target_texture, direction) in
            [(scratch_a_texture, [1.0f32, 0.0f32]), (scratch_b_texture, [0.0f32, 1.0f32])]
        {
            draws.push((source_texture, target_texture, direction));
            source_texture = target_texture;
        }
    }
    draws
}

impl EglFrameBuffer {
    /// Draws `elements` into a new transparent texture. Coordinates are local to that texture.
    pub(crate) fn capture_transparent(
        &self,
        elements: &[FrameElement],
        width: i32,
        height: i32,
        presentation_transform: OutputPresentationTransform,
    ) -> Option<GlTexture> {
        let framebuffer = TextureFrameBuffer::new(width, height)?;
        let damage = DamageRegion::from_rect(RectI::new(0, 0, width, height)?);
        let projection = crate::transform::output_projection(
            width as f32,
            height as f32,
            presentation_transform,
        );
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer.framebuffer_object);
            gl::Viewport(0, 0, width, height);
            gl::Disable(gl::SCISSOR_TEST as _);
            gl::ClearColor(0.0, 0.0, 0.0, 0.0);
            gl::Clear(gl::COLOR_BUFFER_BIT as _);
        }
        self.render_frame_elements_to_target(
            framebuffer.framebuffer_object,
            width,
            height,
            elements,
            projection,
            &damage,
            presentation_transform,
        );
        Some(framebuffer.into_texture())
    }

    /// Draw a window into another framebuffer and clear that target to transparent first.
    ///
    /// The output framebuffer keeps its own pixels. FlipY matches [`EglFrameBuffer::read_top_left`].
    pub(crate) fn render_into_transparent(
        &self,
        framebuffer: gl::GLuint,
        width: i32,
        height: i32,
        recipe: &OutputFrameRecipe,
    ) {
        if width <= 0 || height <= 0 {
            return;
        }
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer);
            gl::Viewport(0, 0, width, height);
            gl::Disable(gl::SCISSOR_TEST as _);
            gl::ClearColor(0.0, 0.0, 0.0, 0.0);
            gl::Clear(gl::COLOR_BUFFER_BIT as _);
        }
        let Some(damage) = RectI::new(0, 0, width, height).map(DamageRegion::from_rect) else {
            return;
        };
        self.render_frame_elements_to_target(
            framebuffer,
            width,
            height,
            &recipe.elements,
            crate::transform::output_projection(
                width as f32,
                height as f32,
                OutputPresentationTransform::FlipY,
            ),
            &damage,
            OutputPresentationTransform::FlipY,
        );
    }

    pub(crate) fn read_top_left(
        &self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        dst_stride: usize,
        dst: &mut [u8],
    ) -> bool {
        read_flip_y_framebuffer(self.framebuffer_object, x, y, width, height, dst_stride, dst)
    }

    fn render_frame_elements_to_target(
        &self,
        framebuffer_object: gl::GLuint,
        target_width: i32,
        target_height: i32,
        elements: &[FrameElement],
        projection: Mat3,
        damage: &DamageRegion,
        output_transform: OutputPresentationTransform,
    ) {
        let mut next_index = 0usize;
        while next_index < elements.len() {
            match &elements[next_index] {
                FrameElement::Texture { .. } => {
                    let batch_start = next_index;
                    next_index += 1;
                    while next_index < elements.len()
                        && matches!(elements[next_index], FrameElement::Texture { .. })
                    {
                        next_index += 1;
                    }
                    let batch: Vec<_> = elements[batch_start..next_index]
                        .iter()
                        .filter_map(|element| match element {
                            FrameElement::Texture { element, .. } => Some(element.clone()),
                            FrameElement::Effect { .. } => None,
                        })
                        .collect();
                    self.render_texture_elements_to_target(
                        framebuffer_object,
                        target_width,
                        target_height,
                        &batch,
                        projection,
                        damage,
                        output_transform,
                    );
                }
                FrameElement::Effect { element: effect, .. } => {
                    next_index += 1;
                    match effect {
                        EffectElement::WindowBlurBackdrop { ring, style, alpha } => {
                            let Some((texture, blur_bounds)) = self.build_blurred_backdrop(
                                framebuffer_object,
                                target_height,
                                *ring,
                                *style,
                                output_transform,
                            ) else {
                                continue;
                            };
                            let item = RenderItem {
                                source: TextureSource::Texture,
                                texture: texture.texture,
                                x: blur_bounds.x,
                                y: blur_bounds.y,
                                width: blur_bounds.width,
                                height: blur_bounds.height,
                                tex_transform: Mat3::identity(),
                                damage: damage.clone(),
                                blend: true,
                                alpha: *alpha,
                                mask: RenderItemMask::FrameRing {
                                    ring: crate::render::RoundedRing {
                                        outer: RootSurfaceClip::new(
                                            ring.outer.outer_rect.translate(
                                                blur_bounds.x.saturating_neg(),
                                                blur_bounds.y.saturating_neg(),
                                            ),
                                            ring.outer.corner_radius,
                                            ring.outer.rounding_power,
                                        ),
                                        inner: RootSurfaceClip::new(
                                            ring.inner.outer_rect.translate(
                                                blur_bounds.x.saturating_neg(),
                                                blur_bounds.y.saturating_neg(),
                                            ),
                                            ring.inner.corner_radius,
                                            ring.inner.rounding_power,
                                        ),
                                    },
                                },
                            };
                            self.render_texture_elements_to_target(
                                framebuffer_object,
                                target_width,
                                target_height,
                                &[item],
                                projection,
                                damage,
                                output_transform,
                            );
                        }
                        EffectElement::WindowBorder { ring, gradient, alpha } => {
                            self.render_border_element_to_target(
                                framebuffer_object,
                                target_width,
                                target_height,
                                projection,
                                damage,
                                output_transform,
                                *ring,
                                *gradient,
                                *alpha,
                            );
                        }
                        EffectElement::WindowShadow { clip, style, alpha } => {
                            self.render_shadow_element_to_target(
                                framebuffer_object,
                                target_width,
                                target_height,
                                projection,
                                damage,
                                output_transform,
                                *clip,
                                *style,
                                *alpha,
                            );
                        }
                        EffectElement::DimOverlay { clip, alpha } => {
                            let Some(texture) = self.dim_texture() else {
                                continue;
                            };
                            let item = RenderItem {
                                source: TextureSource::Texture,
                                texture: texture.texture_id(),
                                x: clip.outer_rect.x,
                                y: clip.outer_rect.y,
                                width: clip.outer_rect.width,
                                height: clip.outer_rect.height,
                                tex_transform: Mat3::identity(),
                                damage: damage.clone(),
                                blend: true,
                                alpha: *alpha,
                                mask: RenderItemMask::RoundedClip {
                                    clip: RootSurfaceClip::new(
                                        RectI::new(
                                            0,
                                            0,
                                            clip.outer_rect.width,
                                            clip.outer_rect.height,
                                        )
                                        .expect("dim overlay bounds should stay valid"),
                                        clip.corner_radius,
                                        clip.rounding_power,
                                    ),
                                },
                            };
                            self.render_texture_elements_to_target(
                                framebuffer_object,
                                target_width,
                                target_height,
                                &[item],
                                projection,
                                damage,
                                output_transform,
                            );
                        }
                    }
                }
            }
        }
    }

    fn render_border_element_to_target(
        &self,
        framebuffer_object: gl::GLuint,
        target_width: i32,
        target_height: i32,
        projection: Mat3,
        damage: &DamageRegion,
        output_transform: OutputPresentationTransform,
        ring: crate::render::RoundedRing,
        gradient: crate::render::BorderGradient,
        alpha: u8,
    ) {
        let bounds = ring.outer.outer_rect;
        let mut clipped_damage = damage.clone();
        let Some(output_bounds) = RectI::new(0, 0, target_width, target_height) else {
            return;
        };
        clipped_damage.intersect_rect(output_bounds);
        clipped_damage.intersect_rect(bounds);
        if clipped_damage.is_empty() {
            return;
        }

        let program = match self.border_program() {
            Ok(program) => program,
            Err(error) => {
                log::error!("border shader is unavailable: {error}");
                return;
            }
        };
        let vertices: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let stride = (2 * std::mem::size_of::<f32>()) as i32;
        let transform = crate::transform::project_box(
            bounds.x as f32,
            bounds.y as f32,
            bounds.width as f32,
            bounds.height as f32,
            projection,
        )
        .to_gl_array();
        let start = gradient.start_color.to_premultiplied_rgba_bytes();
        let end = gradient.end_color.to_premultiplied_rgba_bytes();

        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer_object);
            gl::Viewport(0, 0, target_width, target_height);
            gl::UseProgram(program.program);
            gl::Enable(gl::BLEND as _);
            gl::BlendFunc(gl::ONE as _, gl::ONE_MINUS_SRC_ALPHA as _);
            gl::EnableVertexAttribArray(program.position_location);
            gl::UniformMatrix3fv(program.transform_location, 1, 0, transform.as_ptr());
            gl::Uniform2f(program.item_size_location, bounds.width as f32, bounds.height as f32);
            gl::Uniform4f(
                program.outer_rect_location,
                0.0,
                0.0,
                bounds.width as f32,
                bounds.height as f32,
            );
            gl::Uniform4f(
                program.inner_rect_location,
                ring.inner.outer_rect.x.saturating_sub(bounds.x) as f32,
                ring.inner.outer_rect.y.saturating_sub(bounds.y) as f32,
                ring.inner.outer_rect.width as f32,
                ring.inner.outer_rect.height as f32,
            );
            gl::Uniform1f(
                program.outer_corner_radius_location,
                ring.outer.corner_radius.max(0) as f32,
            );
            gl::Uniform1f(
                program.outer_rounding_power_location,
                ring.outer.rounding_power.get().max(0.0001),
            );
            gl::Uniform1f(
                program.inner_corner_radius_location,
                ring.inner.corner_radius.max(0) as f32,
            );
            gl::Uniform1f(
                program.inner_rounding_power_location,
                ring.inner.rounding_power.get().max(0.0001),
            );
            gl::Uniform4f(
                program.start_color_location,
                f32::from(start[0]) / 255.0,
                f32::from(start[1]) / 255.0,
                f32::from(start[2]) / 255.0,
                f32::from(start[3]) / 255.0,
            );
            gl::Uniform4f(
                program.end_color_location,
                f32::from(end[0]) / 255.0,
                f32::from(end[1]) / 255.0,
                f32::from(end[2]) / 255.0,
                f32::from(end[3]) / 255.0,
            );
            gl::Uniform1f(program.angle_location, gradient.angle_degrees as f32);
            gl::Uniform1f(program.alpha_location, f32::from(alpha) / 255.0);
            gl::VertexAttribPointer(
                program.position_location,
                2,
                gl::FLOAT as _,
                0,
                stride,
                vertices.as_ptr().cast(),
            );
            gl::Enable(gl::SCISSOR_TEST as _);
            for rect in clipped_damage.rects() {
                gl::Scissor(
                    rect.x,
                    self.scissor_y(target_height, *rect, output_transform),
                    rect.width,
                    rect.height,
                );
                gl::DrawArrays(gl::TRIANGLE_STRIP as _, 0, 4);
            }
            gl::Disable(gl::SCISSOR_TEST as _);
            gl::DisableVertexAttribArray(program.position_location);
            gl::Disable(gl::BLEND as _);
            gl::UseProgram(0);
        }
    }

    fn render_shadow_element_to_target(
        &self,
        framebuffer_object: gl::GLuint,
        target_width: i32,
        target_height: i32,
        projection: Mat3,
        damage: &DamageRegion,
        output_transform: OutputPresentationTransform,
        clip: crate::state::RootSurfaceClip,
        style: crate::render::ShadowVisualStyle,
        alpha: u8,
    ) {
        let Some(shadow_geometry) = shadow_geometry(clip, style) else {
            return;
        };
        let bounds = shadow_geometry.bounds;

        let mut clipped_damage = damage.clone();
        let Some(output_bounds) = RectI::new(0, 0, target_width, target_height) else {
            return;
        };
        clipped_damage.intersect_rect(output_bounds);
        clipped_damage.intersect_rect(bounds);
        if clipped_damage.is_empty() {
            return;
        }

        let program = match self.shadow_program() {
            Ok(program) => program,
            Err(error) => {
                log::error!("shadow shader is unavailable: {error}");
                return;
            }
        };
        let vertices: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let stride = (2 * std::mem::size_of::<f32>()) as i32;
        let transform = crate::transform::project_box(
            bounds.x as f32,
            bounds.y as f32,
            bounds.width as f32,
            bounds.height as f32,
            projection,
        )
        .to_gl_array();
        let color = shadow_color_with_alpha(style.color, alpha);

        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer_object);
            gl::Viewport(0, 0, target_width, target_height);
            gl::UseProgram(program.program);
            gl::Enable(gl::BLEND as _);
            gl::BlendFunc(gl::ONE as _, gl::ONE_MINUS_SRC_ALPHA as _);
            gl::EnableVertexAttribArray(program.position_location);
            gl::UniformMatrix3fv(program.transform_location, 1, 0, transform.as_ptr());
            gl::Uniform2f(program.item_size_location, bounds.width as f32, bounds.height as f32);
            gl::Uniform4f(program.shadow_color_location, color[0], color[1], color[2], color[3]);
            gl::Uniform4f(
                program.inner_rect_location,
                shadow_geometry.inner_rect.x as f32,
                shadow_geometry.inner_rect.y as f32,
                shadow_geometry.inner_rect.width as f32,
                shadow_geometry.inner_rect.height as f32,
            );
            gl::Uniform1f(program.corner_radius_location, clip.corner_radius.max(0) as f32);
            gl::Uniform1f(program.rounding_power_location, clip.rounding_power.get().max(0.0001));
            gl::Uniform1f(program.shadow_range_location, shadow_geometry.shadow_range as f32);
            gl::Uniform1f(program.shadow_power_location, style.power.get());
            gl::VertexAttribPointer(
                program.position_location,
                2,
                gl::FLOAT as _,
                0,
                stride,
                vertices.as_ptr().cast(),
            );
            gl::Enable(gl::SCISSOR_TEST as _);
            for rect in clipped_damage.rects() {
                gl::Scissor(
                    rect.x,
                    self.scissor_y(target_height, *rect, output_transform),
                    rect.width,
                    rect.height,
                );
                gl::DrawArrays(gl::TRIANGLE_STRIP as _, 0, 4);
            }
            gl::Disable(gl::SCISSOR_TEST as _);
            gl::DisableVertexAttribArray(program.position_location);
            gl::Disable(gl::BLEND as _);
            gl::UseProgram(0);
        }
    }
}

fn create_texture_program() -> Result<TextureProgram, String> {
    let vertex_source = include_str!("shaders/texture_clip.vert");
    let fragment_source = include_str!("shaders/texture_clip.frag");
    let vertex_shader = compile_shader(gl::VERTEX_SHADER as _, vertex_source)?;
    let fragment_shader = compile_shader(gl::FRAGMENT_SHADER as _, fragment_source)?;
    let program = link_program(vertex_shader, fragment_shader, "texture program")?;
    Ok(TextureProgram {
        program,
        position_location: unsafe {
            gl::GetAttribLocation(program, b"a_position\0".as_ptr().cast()) as gl::Uint
        },
        texcoord_location: unsafe {
            gl::GetAttribLocation(program, b"a_texcoord\0".as_ptr().cast()) as gl::Uint
        },
        sampler_location: unsafe {
            gl::GetUniformLocation(program, b"u_texture\0".as_ptr().cast())
        },
        transform_location: unsafe {
            gl::GetUniformLocation(program, b"u_transform\0".as_ptr().cast())
        },
        tex_transform_location: unsafe {
            gl::GetUniformLocation(program, b"u_tex_transform\0".as_ptr().cast())
        },
        item_size_location: unsafe {
            gl::GetUniformLocation(program, b"u_item_size\0".as_ptr().cast())
        },
        clip_mode_location: unsafe {
            gl::GetUniformLocation(program, b"u_clip_mode\0".as_ptr().cast())
        },
        clip_outer_rect_location: unsafe {
            gl::GetUniformLocation(program, b"u_clip_outer_rect\0".as_ptr().cast())
        },
        clip_inner_rect_location: unsafe {
            gl::GetUniformLocation(program, b"u_clip_inner_rect\0".as_ptr().cast())
        },
        outer_corner_radius_location: unsafe {
            gl::GetUniformLocation(program, b"u_clip_outer_corner_radius\0".as_ptr().cast())
        },
        outer_rounding_power_location: unsafe {
            gl::GetUniformLocation(program, b"u_clip_outer_rounding_power\0".as_ptr().cast())
        },
        inner_corner_radius_location: unsafe {
            gl::GetUniformLocation(program, b"u_clip_inner_corner_radius\0".as_ptr().cast())
        },
        inner_rounding_power_location: unsafe {
            gl::GetUniformLocation(program, b"u_clip_inner_rounding_power\0".as_ptr().cast())
        },
        alpha_location: unsafe { gl::GetUniformLocation(program, b"u_alpha\0".as_ptr().cast()) },
    })
}

fn create_border_program() -> Result<BorderProgram, String> {
    let vertex_source = include_str!("shaders/shadow.vert");
    let fragment_source = include_str!("shaders/border.frag");
    let vertex_shader = compile_shader(gl::VERTEX_SHADER as _, vertex_source)?;
    let fragment_shader = compile_shader(gl::FRAGMENT_SHADER as _, fragment_source)?;
    let program = link_program(vertex_shader, fragment_shader, "border program")?;

    Ok(BorderProgram {
        program,
        position_location: unsafe {
            gl::GetAttribLocation(program, b"a_position\0".as_ptr().cast()) as gl::Uint
        },
        transform_location: unsafe {
            gl::GetUniformLocation(program, b"u_transform\0".as_ptr().cast())
        },
        item_size_location: unsafe {
            gl::GetUniformLocation(program, b"u_item_size\0".as_ptr().cast())
        },
        outer_rect_location: unsafe {
            gl::GetUniformLocation(program, b"u_outer_rect\0".as_ptr().cast())
        },
        inner_rect_location: unsafe {
            gl::GetUniformLocation(program, b"u_inner_rect\0".as_ptr().cast())
        },
        outer_corner_radius_location: unsafe {
            gl::GetUniformLocation(program, b"u_outer_corner_radius\0".as_ptr().cast())
        },
        outer_rounding_power_location: unsafe {
            gl::GetUniformLocation(program, b"u_outer_rounding_power\0".as_ptr().cast())
        },
        inner_corner_radius_location: unsafe {
            gl::GetUniformLocation(program, b"u_inner_corner_radius\0".as_ptr().cast())
        },
        inner_rounding_power_location: unsafe {
            gl::GetUniformLocation(program, b"u_inner_rounding_power\0".as_ptr().cast())
        },
        start_color_location: unsafe {
            gl::GetUniformLocation(program, b"u_gradient_start\0".as_ptr().cast())
        },
        end_color_location: unsafe {
            gl::GetUniformLocation(program, b"u_gradient_end\0".as_ptr().cast())
        },
        angle_location: unsafe {
            gl::GetUniformLocation(program, b"u_gradient_angle_degrees\0".as_ptr().cast())
        },
        alpha_location: unsafe { gl::GetUniformLocation(program, b"u_alpha\0".as_ptr().cast()) },
    })
}

fn create_shadow_program() -> Result<ShadowProgram, String> {
    let vertex_source = include_str!("shaders/shadow.vert");
    let fragment_source = include_str!("shaders/shadow.frag");
    let vertex_shader = compile_shader(gl::VERTEX_SHADER as _, vertex_source)?;
    let fragment_shader = compile_shader(gl::FRAGMENT_SHADER as _, fragment_source)?;
    let program = link_program(vertex_shader, fragment_shader, "shadow program")?;

    Ok(ShadowProgram {
        program,
        position_location: unsafe {
            gl::GetAttribLocation(program, b"a_position\0".as_ptr().cast()) as gl::Uint
        },
        transform_location: unsafe {
            gl::GetUniformLocation(program, b"u_transform\0".as_ptr().cast())
        },
        item_size_location: unsafe {
            gl::GetUniformLocation(program, b"u_item_size\0".as_ptr().cast())
        },
        shadow_color_location: unsafe {
            gl::GetUniformLocation(program, b"u_shadow_color\0".as_ptr().cast())
        },
        inner_rect_location: unsafe {
            gl::GetUniformLocation(program, b"u_inner_rect\0".as_ptr().cast())
        },
        corner_radius_location: unsafe {
            gl::GetUniformLocation(program, b"u_corner_radius\0".as_ptr().cast())
        },
        rounding_power_location: unsafe {
            gl::GetUniformLocation(program, b"u_rounding_power\0".as_ptr().cast())
        },
        shadow_range_location: unsafe {
            gl::GetUniformLocation(program, b"u_shadow_range\0".as_ptr().cast())
        },
        shadow_power_location: unsafe {
            gl::GetUniformLocation(program, b"u_shadow_power\0".as_ptr().cast())
        },
    })
}

fn clear_framebuffer(framebuffer_object: gl::GLuint, width: i32, height: i32) {
    unsafe {
        gl::BindFramebuffer(gl::FRAMEBUFFER as _, framebuffer_object);
        gl::Viewport(0, 0, width, height);
        gl::Disable(gl::SCISSOR_TEST as _);
        gl::ClearColor(0.0, 0.0, 0.0, 0.0);
        gl::Clear(gl::COLOR_BUFFER_BIT as _);
        gl::BlendFunc(gl::ONE as _, gl::ONE_MINUS_SRC_ALPHA as _);
    }
}

fn create_blur_program() -> Result<BlurProgram, String> {
    let vertex_source = include_str!("shaders/blur.vert");
    let fragment_source = include_str!("shaders/blur.frag");
    let vertex_shader = compile_shader(gl::VERTEX_SHADER as _, vertex_source)?;
    let fragment_shader = compile_shader(gl::FRAGMENT_SHADER as _, fragment_source)?;
    let program = link_program(vertex_shader, fragment_shader, "blur program")?;
    Ok(BlurProgram {
        program,
        position_location: unsafe {
            gl::GetAttribLocation(program, b"a_position\0".as_ptr().cast()) as gl::Uint
        },
        texcoord_location: unsafe {
            gl::GetAttribLocation(program, b"a_texcoord\0".as_ptr().cast()) as gl::Uint
        },
        sampler_location: unsafe {
            gl::GetUniformLocation(program, b"u_texture\0".as_ptr().cast())
        },
        direction_location: unsafe {
            gl::GetUniformLocation(program, b"u_direction\0".as_ptr().cast())
        },
        texel_size_location: unsafe {
            gl::GetUniformLocation(program, b"u_texel_size\0".as_ptr().cast())
        },
        radius_location: unsafe { gl::GetUniformLocation(program, b"u_radius\0".as_ptr().cast()) },
        transform_location: unsafe {
            gl::GetUniformLocation(program, b"u_transform\0".as_ptr().cast())
        },
        tex_transform_location: unsafe {
            gl::GetUniformLocation(program, b"u_tex_transform\0".as_ptr().cast())
        },
        item_size_location: unsafe {
            gl::GetUniformLocation(program, b"u_item_size\0".as_ptr().cast())
        },
    })
}

fn compile_shader(kind: gl::Enum, source: &str) -> Result<gl::Uint, String> {
    let shader = unsafe { gl::CreateShader(kind) };
    // `string` and `length` are parallel arrays of `count` elements (here count == 1).
    let strings: [*const gl::GLchar; 1] = [source.as_ptr().cast::<gl::GLchar>()];
    let lengths: [gl::GLint; 1] = [source.len() as gl::GLint];

    unsafe {
        gl::ShaderSource(shader, 1, strings.as_ptr(), lengths.as_ptr());
        gl::CompileShader(shader);
    }

    let mut status = 0;
    unsafe { gl::GetShaderiv(shader, gl::COMPILE_STATUS as _, &mut status) };
    if status == 0 {
        unsafe { gl::DeleteShader(shader) };
        return Err("could not compile shader".into());
    }

    Ok(shader)
}

fn link_program(
    vertex_shader: gl::Uint,
    fragment_shader: gl::Uint,
    label: &str,
) -> Result<gl::Uint, String> {
    let program = unsafe { gl::CreateProgram() };
    unsafe {
        gl::AttachShader(program, vertex_shader);
        gl::AttachShader(program, fragment_shader);
        gl::LinkProgram(program);
        gl::DeleteShader(vertex_shader);
        gl::DeleteShader(fragment_shader);
    }

    let mut status = 0;
    unsafe { gl::GetProgramiv(program, gl::LINK_STATUS as _, &mut status) };
    if status == 0 {
        unsafe { gl::DeleteProgram(program) };
        return Err(format!("could not link {label}"));
    }
    Ok(program)
}

fn framebuffer_rect_for_target(
    target_height: i32,
    rect: RectI,
    output_transform: OutputPresentationTransform,
) -> Option<RectI> {
    RectI::new(
        rect.x,
        match output_transform {
            OutputPresentationTransform::Normal => target_height - rect.y - rect.height,
            OutputPresentationTransform::FlipY => rect.y,
        },
        rect.width,
        rect.height,
    )
}

fn shadow_color_with_alpha(color: crate::config::RgbaColor, alpha: u8) -> [f32; 4] {
    let premultiplied = color.to_premultiplied_rgba_bytes();
    let global_alpha = f32::from(alpha) / 255.0;
    [
        (f32::from(premultiplied[0]) / 255.0) * global_alpha,
        (f32::from(premultiplied[1]) / 255.0) * global_alpha,
        (f32::from(premultiplied[2]) / 255.0) * global_alpha,
        (f32::from(premultiplied[3]) / 255.0) * global_alpha,
    ]
}

#[cfg(test)]
mod tests {
    use {
        super::{blur_pass_draws, framebuffer_rect_for_target, shadow_color_with_alpha},
        crate::{config::RgbaColor, region::RectI, transform::OutputPresentationTransform},
    };

    #[test]
    fn blur_pass_draws_ping_pongs_previous_result_across_multiple_passes() {
        let draws = blur_pass_draws(10, 20, 30, 2);

        assert_eq!(
            draws,
            vec![
                (10, 20, [1.0, 0.0]),
                (20, 30, [0.0, 1.0]),
                (30, 20, [1.0, 0.0]),
                (20, 30, [0.0, 1.0]),
            ]
        );
    }

    #[test]
    fn blur_shader_uses_radius_in_sample_offsets() {
        let source = include_str!("shaders/blur.frag");

        assert!(source.contains("1.384615 * radius"));
        assert!(source.contains("3.230769 * radius"));
    }

    #[test]
    fn blur_backdrop_is_built_from_in_stream_framebuffer_snapshot() {
        let source = include_str!("egl.rs");

        assert!(source.contains("self.build_blurred_backdrop("));
        assert!(source.contains("gl::CopyTexSubImage2D("));
        assert!(source
            .contains("framebuffer_rect_for_target(target_height, blur_bounds, output_transform)"));
    }

    #[test]
    fn framebuffer_rect_helper_uses_the_active_render_target_height() {
        let rect = RectI::new(10, 20, 30, 40).unwrap();

        assert_eq!(
            framebuffer_rect_for_target(100, rect, OutputPresentationTransform::Normal),
            RectI::new(10, 40, 30, 40)
        );
        assert_eq!(
            framebuffer_rect_for_target(100, rect, OutputPresentationTransform::FlipY),
            RectI::new(10, 20, 30, 40)
        );
    }

    #[test]
    fn texture_and_border_shaders_use_rounded_inner_cutouts_without_texture_dim() {
        let texture_source = include_str!("shaders/texture_clip.frag");
        let border_source = include_str!("shaders/border.frag");

        assert!(texture_source.contains("pointInRoundedRect(point, u_clip_inner_rect"));
        assert!(!texture_source.contains("u_dim_amount"));
        assert!(!texture_source.contains("color.rgb *= (1.0 - u_dim_amount)"));
        assert!(border_source.contains("pointInRoundedRect(v_local_position, u_inner_rect"));
        assert!(!border_source.contains("pointInRect(v_local_position, u_inner_rect)"));
    }

    #[test]
    fn shadow_alpha_scales_premultiplied_rgb_and_alpha() {
        let color = shadow_color_with_alpha(RgbaColor::new(255, 128, 0, 128), 128);

        assert!((color[0] - 0.25196466).abs() < 0.0001);
        assert!((color[1] - 0.12598233).abs() < 0.0001);
        assert_eq!(color[2], 0.0);
        assert!((color[3] - 0.25196466).abs() < 0.0001);
    }
}

#[derive(Debug)]
pub struct EglContext {
    egl: EGL,
    display: egl::EGLDisplay,
    extensions: BTreeSet<String>,
    config: egl::EGLConfig,
    context: egl::EGLContext,
    pub gbm: GbmDevice,
    formats: HashMap<DrmFourcc, EglFormat>,
    pub opengl_formats: HashMap<DrmFourcc, OpenGlFormat>,
    opengl_extensions: BTreeSet<String>,
}

impl EglContext {
    pub fn bind(&self) {
        let _ = self
            .egl
            .make_current(self.display, None, None, Some(self.context));
    }

    pub fn make(egl: &EGL, gbm: GbmDevice) -> Result<Self, String> {
        let display = egl
            .get_platform_display(EGL_PLATFORM_GBM_KHR, gbm.gbm_device, &[egl::NONE as _])
            .map_err(|error| format!("platform display: {error:?}"))?;
        let (_major, _minor) = egl
            .initialize(display)
            .map_err(|error| format!("eglInitialize: {error:?}"))?;

        let Some(config) = Self::find_config_xrgb8888(egl, display) else {
            return Err("no XRGB8888 EGL config".into());
        };

        let extensions = egl
            .query_string(Some(display), egl::EXTENSIONS)
            .map_err(|error| format!("EGL extensions: {error:?}"))?;
        let extensions: BTreeSet<String> = extensions
            .to_string_lossy()
            .trim()
            .split(' ')
            .map(|s| s.trim().to_string())
            .collect();

        let context = egl
            .create_context(display, config, None, &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE])
            .map_err(|error| format!("eglCreateContext: {error:?}"))?;

        let formats = Self::query_formats(egl, &display);

        egl.make_current(display, None, None, Some(context))
            .map_err(|error| format!("eglMakeCurrent: {error:?}"))?;

        let opengl_extensions = get_opengl_extensions();
        let opengl_formats = {
            let supports_external_only = opengl_extensions.contains("GL_OES_EGL_image_external");
            formats
                .iter()
                .filter(|(_, f)| !(f.implicit_external_only && !supports_external_only))
                .filter_map(|(drm, egl_format)| {
                    let mut read_modifiers = BTreeSet::new();
                    let mut write_modifiers = BTreeSet::new();
                    for modifier in egl_format.modifiers.values() {
                        if modifier.external_only && !supports_external_only {
                            continue;
                        }
                        if !modifier.external_only {
                            write_modifiers.insert(modifier.modifier);
                        }
                        read_modifiers.insert(modifier.modifier);
                    }
                    if read_modifiers.is_empty() && write_modifiers.is_empty() {
                        return None;
                    }
                    let opengl_format =
                        OpenGlFormat { format: egl_format.format, read_modifiers, write_modifiers };
                    Some((*drm, opengl_format))
                })
                .collect::<HashMap<_, _>>()
        };

        let supports_native_render_fence = extensions.contains("EGL_KHR_fence_sync")
            && extensions.contains("EGL_ANDROID_native_fence_sync");
        log::info!("EGL native render fence support: {}", supports_native_render_fence);

        Ok(Self {
            egl: egl.clone(),
            display,
            extensions,
            config,
            context,
            gbm,
            formats,
            opengl_formats,
            opengl_extensions,
        })
    }

    pub fn import_dmabuf(&self, buf: &DmaBuf) -> EglImage {
        let egl_format = self.formats.get(&buf.format.drm).unwrap();
        let egl_modifier = egl_format.modifiers.get(&buf.modifier).unwrap();

        struct PlaneKey {
            fd: egl::Int,
            offset: egl::Int,
            pitch: egl::Int,
            modifier_lo: egl::Int,
            modifier_hi: egl::Int,
        }
        const PLANE_KEYS: [PlaneKey; 4] = [
            PlaneKey {
                fd: egl::DMA_BUF_PLANE0_FD_EXT,
                offset: egl::DMA_BUF_PLANE0_OFFSET_EXT,
                pitch: egl::DMA_BUF_PLANE0_PITCH_EXT,
                modifier_lo: egl::DMA_BUF_PLANE0_MODIFIER_LO_EXT,
                modifier_hi: egl::DMA_BUF_PLANE0_MODIFIER_HI_EXT,
            },
            PlaneKey {
                fd: egl::DMA_BUF_PLANE1_FD_EXT,
                offset: egl::DMA_BUF_PLANE1_OFFSET_EXT,
                pitch: egl::DMA_BUF_PLANE1_PITCH_EXT,
                modifier_lo: egl::DMA_BUF_PLANE1_MODIFIER_LO_EXT,
                modifier_hi: egl::DMA_BUF_PLANE1_MODIFIER_HI_EXT,
            },
            PlaneKey {
                fd: egl::DMA_BUF_PLANE2_FD_EXT,
                offset: egl::DMA_BUF_PLANE2_OFFSET_EXT,
                pitch: egl::DMA_BUF_PLANE2_PITCH_EXT,
                modifier_lo: egl::DMA_BUF_PLANE2_MODIFIER_LO_EXT,
                modifier_hi: egl::DMA_BUF_PLANE2_MODIFIER_HI_EXT,
            },
            PlaneKey {
                fd: egl::DMA_BUF_PLANE3_FD_EXT,
                offset: egl::DMA_BUF_PLANE3_OFFSET_EXT,
                pitch: egl::DMA_BUF_PLANE3_PITCH_EXT,
                modifier_lo: egl::DMA_BUF_PLANE3_MODIFIER_LO_EXT,
                modifier_hi: egl::DMA_BUF_PLANE3_MODIFIER_HI_EXT,
            },
        ];

        let mut attribs = Vec::<egl::Int>::with_capacity(128);
        attribs.extend([egl::WIDTH, buf.width]);
        attribs.extend([egl::HEIGHT, buf.height]);
        attribs.extend([egl::LINUX_DRM_FOURCC_EXT, buf.format.drm as i32]);
        attribs.extend([egl::IMAGE_PRESERVED_KHR, egl::TRUE as i32]);
        for (key, plane) in PLANE_KEYS.iter().zip(buf.planes.iter()) {
            attribs.extend([key.fd, plane.fd.as_raw_fd() as egl::Int]);
            attribs.extend([key.pitch, plane.stride as i32]);
            attribs.extend([key.offset, plane.offset as i32]);
            if buf.modifier != Modifier::INVALID {
                attribs.extend([key.modifier_lo, buf.modifier.low() as i32]);
                attribs.extend([key.modifier_hi, buf.modifier.high() as i32]);
            }
        }
        attribs.push(egl::NONE);

        let image = self
            .egl
            .create_image_khr(self.display, egl::LINUX_DMA_BUF_EXT as _, &attribs)
            .unwrap();

        EglImage {
            display: self.display,
            image,
            external_only: egl_modifier.external_only,
            destroyed: false,
        }
    }

    pub fn make_fb_from_image(&self, image: &EglImage, dmabuf: &DmaBuf) -> EglFrameBuffer {
        let render_buffer = self.make_render_buffer_from_image(image, dmabuf);
        render_buffer.make_framebuffer()
    }

    pub fn wait_on_native_fence(&self, fence_fd: OwnedFd) -> Result<(), egl::Error> {
        let raw_fd = fence_fd.into_raw_fd();
        let sync = self.egl.create_sync_khr(
            self.display,
            EGL_SYNC_NATIVE_FENCE_ANDROID,
            &[EGL_SYNC_NATIVE_FENCE_FD_ANDROID, raw_fd, egl::NONE],
        )?;
        let wait_result = self.egl.wait_sync_khr(self.display, sync, 0);
        let destroy_result = self.egl.destroy_sync_khr(self.display, sync);
        destroy_result?;
        wait_result
    }

    pub fn make_render_buffer_from_image(
        &self,
        image: &EglImage,
        dmabuf: &DmaBuf,
    ) -> EglRenderBuffer {
        let mut renderbuffer_object = 0;

        unsafe {
            gl::GenRenderbuffers(1, &mut renderbuffer_object);
            gl::BindRenderbuffer(gl::RENDERBUFFER as _, renderbuffer_object);
            self.egl
                .image_target_renderbuffer_storage_oes(gl::RENDERBUFFER as _, image.image)
                .unwrap();
            gl::BindRenderbuffer(gl::RENDERBUFFER as _, 0);
        }

        EglRenderBuffer {
            egl: self.egl.clone(),
            display: self.display,
            width: dmabuf.width,
            height: dmabuf.height,
            format: dmabuf.format,
            renderbuffer_object,
        }
    }

    fn query_formats(egl: &EGL, display: &egl::Display) -> HashMap<DrmFourcc, EglFormat> {
        let formats = egl.query_dma_buf_formats_ext(*display).unwrap();

        formats
            .iter()
            .filter_map(|fmt| {
                let format = FORMATS_MAP.get(&DrmFourcc::try_from(*fmt as u32).ok()?)?;
                let (modifiers, external_only) = Self::query_modifiers(egl, display, *fmt, format);

                Some((
                    format.drm,
                    EglFormat {
                        format: *format,
                        implicit_external_only: external_only,
                        modifiers,
                    },
                ))
            })
            .collect()
    }

    fn query_modifiers(
        egl: &EGL,
        display: &egl::Display,
        gl_format: egl::Int,
        format: &Format,
    ) -> (BTreeMap<Modifier, EglModifier>, bool) {
        const INVALID_MODIFIER: u64 = 0x00ff_ffff_ffff_ffff;

        let (modifiers, external_only) = egl
            .query_dma_buf_modifiers_ext(*display, gl_format)
            .unwrap();

        let mut map = modifiers
            .iter()
            .copied()
            .zip(external_only)
            .map(|(modifier, external_only)| {
                (modifier, EglModifier { modifier, external_only: external_only == egl::TRUE as _ })
            })
            .collect::<BTreeMap<_, _>>();

        let mut external_only = format.external_only_guess;
        if !modifiers.is_empty() {
            external_only = map.values().any(|m| m.external_only);
        }

        map.insert(Modifier::INVALID, EglModifier { modifier: Modifier::INVALID, external_only });

        (map, external_only)
    }

    fn find_config_xrgb8888(egl: &EGL, display: egl::Display) -> Option<egl::Config> {
        // let mut configs = Vec::with_capacity(egl.get_config_count(display).unwrap());
        let configs = egl.get_configs(display).unwrap();
        configs.into_iter().find(|config| {
            let attr = egl
                .get_config_attrib(display, *config, egl::NATIVE_VISUAL_ID)
                .unwrap();
            let drm = DrmFourcc::try_from(attr as u32).unwrap();
            drm == drm::buffer::DrmFourcc::Xrgb8888
        })
    }

    pub fn supports_native_render_fence(&self) -> bool {
        self.extensions.contains("EGL_KHR_fence_sync")
            && self.extensions.contains("EGL_ANDROID_native_fence_sync")
    }
}

impl Drop for EglContext {
    fn drop(&mut self) {
        // Context and display go away after the caller has dropped textures,
        // images, and framebuffers. The GBM device is destroyed by its own
        // field drop, which runs after this body.
        if self.display != egl::NO_DISPLAY {
            let _ = self.egl.make_current(self.display, None, None, None);
        }
        if self.context != egl::NO_CONTEXT && self.display != egl::NO_DISPLAY {
            unsafe { egl::DestroyContext(self.display, self.context) };
            self.context = egl::NO_CONTEXT;
        }
        if self.display != egl::NO_DISPLAY {
            unsafe { egl::Terminate(self.display) };
            self.display = egl::NO_DISPLAY;
        }
    }
}

#[allow(unused)]
pub fn make_display(
    drm_fd: &DrmFd,
    egl: &EGL,
    gbm: &GbmDevice,
    connector: &crate::drm::Connector,
    crtc: drm::control::crtc::Handle,
) {
    let display = egl
        .get_platform_display(EGL_PLATFORM_GBM_KHR, gbm.gbm_device, &[egl::NONE as _])
        .unwrap();

    let (major, minor) = egl.initialize(display).unwrap();

    // let mut configs = Vec::with_capacity(egl.get_config_count(display).unwrap());
    let configs = egl.get_configs(display).unwrap();

    dbg!(configs.len());

    let mut conf = None;

    for config in &configs {
        let a = egl
            .get_config_attrib(display, *config, egl::NATIVE_VISUAL_ID)
            .unwrap();
        let drm = DrmFourcc::try_from(a as u32).unwrap();

        if drm == drm::buffer::DrmFourcc::Xrgb8888 {
            conf = Some(*config);
            break;
        }
    }

    let config = conf.unwrap();

    let exts = egl.query_string(Some(display), egl::EXTENSIONS).unwrap();
    let exts: HashSet<String> = exts
        .to_string_lossy()
        .trim()
        .split(' ')
        .map(|s| s.trim().to_string())
        .collect();

    dbg!(exts);

    dbg!(display);
    dbg!(major, minor);

    let format = drm::buffer::DrmFourcc::Xrgb8888 as i32;

    let attrib_list = [
        // egl::NATIVE_VISUAL_ID, format,
        egl::SURFACE_TYPE,
        egl::EGLSurfaceTypeMask::WINDOW_BIT as _,
        // egl::RED_SIZE, 8,
        // egl::GREEN_SIZE, 8,
        // egl::BLUE_SIZE, 8,
        egl::RENDERABLE_TYPE,
        egl::EGLRenderableTypeMask::OPENGL_ES2_BIT as _,
        egl::NONE,
    ];

    // Get the number of matching configurations.
    // let count = egl.matching_config_count(display, &attrib_list).unwrap();
    // let mut configs = Vec::with_capacity(count);
    // egl.choose_config(display, &attrib_list, &mut configs).unwrap();
    // let config = egl.choose_first_config(display, &attrib_list).unwrap().unwrap();

    let egl_context = egl
        .create_context(display, config, None, &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE])
        .unwrap();

    // let egl_context = egl.create_context(display, configs[0], None, &[egl::NONE]).unwrap();
    let gbm_surface = gbm.create_surface(connector.preferred_mode.as_ref().unwrap());

    let egl_surface = egl
        .create_platform_window_surface(display, config, gbm_surface.raw(), &[egl::NONE as _])
        .unwrap();

    egl.make_current(display, Some(egl_surface), Some(egl_surface), Some(egl_context))
        .unwrap();

    let mut previous = None;

    for i in 0..100 {
        println!("I={:?}", i);

        let i = i as f32;
        unsafe {
            gl::ClearColor(0.5, i / 100.0, 0.0, 1.0);
            gl::Clear(gl::COLOR_BUFFER_BIT as _);
        }

        egl.swap_buffers(display, egl_surface).unwrap();

        dbg!(egl);
        dbg!(display);
        dbg!(egl_surface);
        dbg!(egl_context);

        let bo = gbm_surface.lock_front_buffer();

        // let mut buffer = Vec::new();
        // for i in 0..1280 {
        //     for _ in 0..720 {
        //         buffer.push(if i % 2 == 0 { 100 } else { 255 });
        //     }
        // }
        // eprintln!("A2");
        // bo.write(&buffer);
        // eprintln!("A3");

        // let handle = drm::buffer::Buffer::handle(&bo);
        // dbg!(handle);

        let fb = gbm.add_framebuffer(&bo, 24, 32).unwrap();
        let mode = connector.preferred_mode.unwrap();

        dbg!(&connector.crtcs);

        drm_fd
            .set_crtc(crtc, Some(fb), (0, 0), &[connector.handler], Some(mode))
            .unwrap();

        if let Some((fb, bo)) = previous.replace((fb, bo)) {
            gbm.destroy_framebuffer(fb).unwrap();
            gbm_surface.release_buffer(bo);
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}

fn get_opengl_extensions() -> BTreeSet<String> {
    let s = unsafe {
        let ptr = gl::GetString(gl::EXTENSIONS as _);
        assert!(!ptr.is_null());
        std::ffi::CStr::from_ptr(ptr as _)
    };
    s.to_string_lossy()
        .trim()
        .split(' ')
        .map(|s| s.trim().to_string())
        .collect()
}

// static void swap_buffers () {
// 	eglSwapBuffers (display, egl_surface);
// 	struct gbm_bo *bo = gbm_surface_lock_front_buffer (gbm_surface);
// 	uint32_t handle = gbm_bo_get_handle (bo).u32;
// 	uint32_t pitch = gbm_bo_get_stride (bo);
// 	uint32_t fb;
// 	drmModeAddFB (device, mode_info.hdisplay, mode_info.vdisplay, 24, 32, pitch, handle, &fb);
// 	drmModeSetCrtc (device, crtc->crtc_id, fb, 0, 0, &connector_id, 1, &mode_info);

// 	if (previous_bo) {
// 		drmModeRmFB (device, previous_fb);
// 		gbm_surface_release_buffer (gbm_surface, previous_bo);
// 	}
// 	previous_bo = bo;
// 	previous_fb = fb;
// }

// static void draw (float progress) {
// 	glClearColor (1.0f-progress, progress, 0.0, 1.0);
// 	glClear (GL_COLOR_BUFFER_BIT);
// 	swap_buffers ();
// }

#[derive(Copy, Clone, Debug)]
pub struct ImageKHR {
    ptr: *mut std::ffi::c_void,
}
