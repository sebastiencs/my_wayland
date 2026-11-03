use std::ffi::CStr;

use khronos_types::{egl, gl};

use crate::drm::Modifier;
pub(super) const EGL_PLATFORM_GBM_KHR: u32 = 0x31D7;

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

    pub(super) fn error_on<T: PartialEq>(result: T, error_value: T) -> Result<T, egl::Error> {
        if result == error_value {
            Err(Self::last_error().unwrap_err())
        } else {
            Ok(result)
        }
    }

    pub(super) fn error_if(is_error: bool) -> Result<(), egl::Error> {
        if is_error {
            Err(Self::last_error().unwrap_err())
        } else {
            Ok(())
        }
    }

    pub(super) fn ok_on<T: PartialEq>(result: T, ok_value: T) -> Result<T, egl::Error> {
        if result == ok_value {
            Ok(result)
        } else {
            Err(Self::last_error().unwrap_err())
        }
    }

    pub(super) fn last_error() -> Result<(), egl::Error> {
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

    pub(super) fn get_platform_display(
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

    pub(super) fn initialize(&self, display: egl::Display) -> Result<(i32, i32), egl::Error> {
        let (mut major, mut minor) = (0, 0);
        let result = unsafe { egl::Initialize(display, &mut major, &mut minor) };
        Self::ok_on(result, egl::TRUE as _)?;
        Ok((major, minor))
    }

    pub(super) fn query_string(
        &self,
        display: Option<egl::Display>,
        name: egl::Int,
    ) -> Result<&'static CStr, egl::Error> {
        let display = display.unwrap_or(egl::NO_DISPLAY);
        let s = unsafe { egl::QueryString(display, name) };
        Self::error_if(s.is_null())?;
        Ok(unsafe { CStr::from_ptr(s) })
    }

    pub(super) fn create_context(
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

    pub(super) fn make_current(
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

    pub(super) fn assert_attribs_with_none(list: &[egl::Attrib]) {
        assert_eq!(*list.last().unwrap(), egl::NONE as _);
    }
    pub(super) fn assert_ends_with_none(list: &[egl::Int]) {
        assert_eq!(*list.last().unwrap(), egl::NONE);
    }

    pub(super) fn create_platform_window_surface(
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

    pub(super) fn swap_buffers(
        &self,
        display: egl::Display,
        surface: egl::Surface,
    ) -> Result<(), egl::Error> {
        let result = unsafe { egl::SwapBuffers(display, surface) };
        Self::ok_on(result, egl::TRUE as _)?;
        Ok(())
    }

    pub(super) fn create_image_khr(
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

    pub(super) fn image_target_renderbuffer_storage_oes(
        &self,
        target: egl::Enum,
        image: ImageKHR,
    ) -> Result<(), egl::Error> {
        unsafe { gl::EGLImageTargetRenderbufferStorageOES(target, image.ptr) };
        Self::last_error()
    }

    pub(super) fn query_dma_buf_formats_ext(
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

    pub(super) fn query_dma_buf_modifiers_ext(
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

#[derive(Copy, Clone, Debug)]
pub struct ImageKHR {
    pub(super) ptr: *mut std::ffi::c_void,
}
