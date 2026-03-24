use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ffi::CStr,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
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
    transform::Mat3,
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

fn load_methods() {
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
        load_gl::FramebufferRenderbuffer as _,
        load_gl::CheckFramebufferStatus as _,
        load_gl::DeleteFramebuffers as _,
        load_gl::GenRenderbuffers as _,
        load_gl::BindRenderbuffer as _,
        load_gl::EGLImageTargetTexture2DOES as _,
    ] {
        unsafe { loader(get_addr) }.expect("Failed to load method");
    }
}

#[derive(Clone, Debug)]
pub struct EGL {}

impl EGL {
    pub fn new() -> Self {
        load_methods();
        // gl::GenTextures(loader);
        // gl::load_glViewport(loader);
        // gl::load_glGenTexturesEXT(loader);
        // Self { dyns: load_methods() }
        Self {}
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
}

pub fn init() -> EGL {
    let egl = EGL::new();
    egl.bind_api(egl::OPENGL_ES_API).unwrap();
    egl

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
}

impl Drop for GlTexture {
    fn drop(&mut self) {
        if self.texture != 0 {
            unsafe {
                gl::DeleteTextures(1, &self.texture);
            }
            self.texture = 0;
        }
        self.image.take();
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RenderItem {
    pub texture: gl::Uint,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Future `wl_surface.set_buffer_transform` / scale support plugs in here.
    pub tex_transform: Mat3,
}

#[derive(Debug)]
pub struct RenderCompletion {
    pub in_fence_fd: Option<OwnedFd>,
    pub used_blocking_fallback: bool,
}

impl RenderCompletion {
    fn explicit(in_fence_fd: OwnedFd) -> Self {
        Self {
            in_fence_fd: Some(in_fence_fd),
            used_blocking_fallback: false,
        }
    }

    fn blocking_fallback() -> Self {
        Self { in_fence_fd: None, used_blocking_fallback: true }
    }
}

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
        Ok(GlTexture { image: Some(image), texture })
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
        Ok(Self { image: None, texture })
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
}

impl Drop for EglFrameBuffer {
    fn drop(&mut self) {
        unsafe {
            gl::DeleteFramebuffers(1, &self.framebuffer_object);
        }
    }
}

impl EglFrameBuffer {
    pub fn clear(&self) {
        // unsafe {
        //     gl::ClearColor(0.0, 0.0, 0.0, 0.0);
        //     gl::Clear(gl::COLOR_BUFFER_BIT as _);
        // }

        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, self.framebuffer_object);
            gl::Viewport(0, 0, self.width, self.height);
            // if let Some(c) = clear_color {
            gl::ClearColor(1.0, 0.0, 0.0, 1.0);
            gl::Clear(gl::COLOR_BUFFER_BIT as _);
            // }
            gl::BlendFunc(gl::ONE as _, gl::ONE_MINUS_SRC_ALPHA as _);
        }
        unsafe {
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
                    log::debug!(
                        "egl finish_render: exported explicit render fence fd={}",
                        in_fence_fd.as_raw_fd()
                    );
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
        items: &[RenderItem],
        projection: Mat3,
        render_sync_mode: RenderSyncMode,
    ) -> RenderCompletion {
        self.clear();

        if items.is_empty() {
            return self.finish_render(render_sync_mode);
        }

        let program = create_texture_program();
        let position_name = b"a_position\0";
        let texcoord_name = b"a_texcoord\0";
        let sampler_name = b"u_texture\0";
        let transform_name = b"u_transform\0";
        let tex_transform_name = b"u_tex_transform\0";
        let position_location =
            unsafe { gl::GetAttribLocation(program, position_name.as_ptr().cast()) } as gl::Uint;
        let texcoord_location =
            unsafe { gl::GetAttribLocation(program, texcoord_name.as_ptr().cast()) } as gl::Uint;
        let sampler_location =
            unsafe { gl::GetUniformLocation(program, sampler_name.as_ptr().cast()) };
        let transform_location =
            unsafe { gl::GetUniformLocation(program, transform_name.as_ptr().cast()) };
        let tex_transform_location =
            unsafe { gl::GetUniformLocation(program, tex_transform_name.as_ptr().cast()) };
        let vertices: [f32; 16] =
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let stride = (4 * std::mem::size_of::<f32>()) as i32;

        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER as _, self.framebuffer_object);
            gl::Viewport(0, 0, self.width, self.height);
            gl::UseProgram(program);
            gl::Enable(gl::BLEND as _);
            gl::BlendFunc(gl::ONE as _, gl::ONE_MINUS_SRC_ALPHA as _);
            gl::Uniform1i(sampler_location, 0);
            gl::ActiveTexture(gl::TEXTURE0 as _);
            gl::EnableVertexAttribArray(position_location);
            gl::EnableVertexAttribArray(texcoord_location);
        }

        for item in items {
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
                gl::BindTexture(gl::TEXTURE_2D as _, item.texture);
                gl::UniformMatrix3fv(transform_location, 1, 0, transform.as_ptr());
                gl::UniformMatrix3fv(tex_transform_location, 1, 0, tex_transform.as_ptr());
                gl::VertexAttribPointer(
                    position_location,
                    2,
                    gl::FLOAT as _,
                    0,
                    stride,
                    vertices.as_ptr().cast(),
                );
                gl::VertexAttribPointer(
                    texcoord_location,
                    2,
                    gl::FLOAT as _,
                    0,
                    stride,
                    vertices.as_ptr().add(2).cast(),
                );
                gl::DrawArrays(gl::TRIANGLE_STRIP as _, 0, 4);
            }
        }

        unsafe {
            gl::BindTexture(gl::TEXTURE_2D as _, 0);
            gl::DisableVertexAttribArray(position_location);
            gl::DisableVertexAttribArray(texcoord_location);
            gl::UseProgram(0);
            gl::DeleteProgram(program);
        }

        self.finish_render(render_sync_mode)
    }
}

fn create_texture_program() -> gl::Uint {
    let vertex_source = b"attribute vec2 a_position;\nattribute vec2 a_texcoord;\nuniform mat3 u_transform;\nuniform mat3 u_tex_transform;\nvarying vec2 v_texcoord;\nvoid main() {\n    vec3 position = u_transform * vec3(a_position, 1.0);\n    vec3 texcoord = u_tex_transform * vec3(a_texcoord, 1.0);\n    gl_Position = vec4(position.xy, 0.0, 1.0);\n    v_texcoord = texcoord.xy;\n}\n\0";
    let fragment_source = b"precision mediump float;\nvarying vec2 v_texcoord;\nuniform sampler2D u_texture;\nvoid main() {\n    gl_FragColor = texture2D(u_texture, v_texcoord);\n}\n\0";
    let vertex_shader = compile_shader(gl::VERTEX_SHADER as _, vertex_source);
    let fragment_shader = compile_shader(gl::FRAGMENT_SHADER as _, fragment_source);
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
        panic!("Could not link texture program");
    }

    program
}

fn compile_shader(kind: gl::Enum, source: &[u8]) -> gl::Uint {
    let shader = unsafe { gl::CreateShader(kind) };
    let source_ptr = source.as_ptr().cast();

    unsafe {
        gl::ShaderSource(shader, 1, &source_ptr, ptr::null());
        gl::CompileShader(shader);
    }

    let mut status = 0;
    unsafe { gl::GetShaderiv(shader, gl::COMPILE_STATUS as _, &mut status) };
    if status == 0 {
        panic!("Could not compile shader");
    }

    shader
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
    pub fn make(egl: &EGL, gbm: GbmDevice) -> Self {
        let display = egl
            .get_platform_display(EGL_PLATFORM_GBM_KHR, gbm.gbm_device, &[egl::NONE as _])
            .unwrap();
        let (_major, _minor) = egl.initialize(display).unwrap();

        let config = Self::find_config_xrgb8888(egl, display).unwrap();

        let extensions = egl.query_string(Some(display), egl::EXTENSIONS).unwrap();
        let extensions: BTreeSet<String> = extensions
            .to_string_lossy()
            .trim()
            .split(' ')
            .map(|s| s.trim().to_string())
            .collect();

        let context = egl
            .create_context(display, config, None, &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE])
            .unwrap();

        let formats = Self::query_formats(egl, &display);

        egl.make_current(display, None, None, Some(context))
            .unwrap();

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

        Self {
            egl: egl.clone(),
            display,
            extensions,
            config,
            context,
            gbm,
            formats,
            opengl_formats,
            opengl_extensions,
        }
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
