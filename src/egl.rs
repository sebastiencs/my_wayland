use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use {drm::buffer::DrmFourcc, egl::ATTRIB_NONE};
// use {
//     egli::{egl, query_extensions, query_version},
//     std::collections::BTreeSet,
// };
use {
    khronos_egl::{self as egl, Dynamic, Instance, EGL1_5},
    libloading::Library,
};

use drm::control::Device;

use crate::{
    dmabuf::DmaBuf,
    drm::{DrmFd, Format, Modifier, FORMATS_MAP},
    gbm::GbmDevice,
    utils,
};

const EGL_PLATFORM_GBM_KHR: u32 = 0x31D7;

pub type EglInstance = Arc<Instance<Dynamic<Library, EGL1_5>>>;

pub fn init() -> EglInstance {
    let egl = unsafe {
        egl::DynamicInstance::<EGL1_5>::load_required()
            .expect("Unable to load libEGL.so or libEGL.so.1")
    };

    let egl: EglInstance = Arc::new(egl);

    egl.bind_api(khronos_egl::OPENGL_ES_API)
        .expect("unable to select OpenGL API");

    egl

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

pub struct EglImage {
    image: ImageKHR,
    external_only: bool,
    destroyed: bool,
}

impl Drop for EglImage {
    fn drop(&mut self) {
        if !self.destroyed {
            // panic!("destroy image here !")
        }
    }
}

#[derive(Debug)]
pub struct EglRenderBuffer {
    width: i32,
    height: i32,
    format: &'static Format,
    pub renderbuffer_object: GLuint,
}

impl Drop for EglRenderBuffer {
    fn drop(&mut self) {
        unsafe {
            glDeleteRenderbuffers(1, &self.renderbuffer_object);
        }
    }
}

impl EglRenderBuffer {
    pub fn make_framebuffer(self) -> EglFrameBuffer {
        let mut framebuffer_object = 0;

        unsafe {
            glGenFramebuffers(1, &mut framebuffer_object);
            glBindFramebuffer(GL_FRAMEBUFFER, framebuffer_object);
            glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, self.renderbuffer_object);
            let status = glCheckFramebufferStatus(GL_FRAMEBUFFER);
            assert_eq!(status, GL_FRAMEBUFFER_COMPLETE);
            glBindFramebuffer(GL_FRAMEBUFFER, 0);
        }

        EglFrameBuffer {
            width: self.width,
            height: self.height,
            render_buffer: self,
            framebuffer_object,
        }
    }
}

pub struct EglFrameBuffer {
    pub width: i32,
    pub height: i32,
    render_buffer: EglRenderBuffer,
    pub framebuffer_object: GLuint,
}

impl Drop for EglFrameBuffer {
    fn drop(&mut self) {
        unsafe {
            glDeleteFramebuffers(1, &self.framebuffer_object);
        }
    }
}

#[derive(Debug)]
pub struct EglContext {
    egl: EglInstance,
    display: khronos_egl::Display,
    extensions: BTreeSet<String>,
    config: egl::Config,
    context: egl::Context,
    pub gbm: GbmDevice,
    formats: HashMap<DrmFourcc, EglFormat>,
    pub opengl_formats: HashMap<DrmFourcc, OpenGlFormat>,
    opengl_extensions: BTreeSet<String>,
}

impl EglContext {
    pub fn make(egl: &EglInstance, gbm: GbmDevice) -> Self {
        let display = unsafe {
            egl.get_platform_display(EGL_PLATFORM_GBM_KHR, gbm.gbm_device, &[ATTRIB_NONE])
                .unwrap()
        };
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
                fd: EGL_DMA_BUF_PLANE0_FD_EXT,
                offset: EGL_DMA_BUF_PLANE0_OFFSET_EXT,
                pitch: EGL_DMA_BUF_PLANE0_PITCH_EXT,
                modifier_lo: EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT,
                modifier_hi: EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT,
            },
            PlaneKey {
                fd: EGL_DMA_BUF_PLANE1_FD_EXT,
                offset: EGL_DMA_BUF_PLANE1_OFFSET_EXT,
                pitch: EGL_DMA_BUF_PLANE1_PITCH_EXT,
                modifier_lo: EGL_DMA_BUF_PLANE1_MODIFIER_LO_EXT,
                modifier_hi: EGL_DMA_BUF_PLANE1_MODIFIER_HI_EXT,
            },
            PlaneKey {
                fd: EGL_DMA_BUF_PLANE2_FD_EXT,
                offset: EGL_DMA_BUF_PLANE2_OFFSET_EXT,
                pitch: EGL_DMA_BUF_PLANE2_PITCH_EXT,
                modifier_lo: EGL_DMA_BUF_PLANE2_MODIFIER_LO_EXT,
                modifier_hi: EGL_DMA_BUF_PLANE2_MODIFIER_HI_EXT,
            },
            PlaneKey {
                fd: EGL_DMA_BUF_PLANE3_FD_EXT,
                offset: EGL_DMA_BUF_PLANE3_OFFSET_EXT,
                pitch: EGL_DMA_BUF_PLANE3_PITCH_EXT,
                modifier_lo: EGL_DMA_BUF_PLANE3_MODIFIER_LO_EXT,
                modifier_hi: EGL_DMA_BUF_PLANE3_MODIFIER_HI_EXT,
            },
        ];

        let mut attribs = Vec::<egl::Int>::with_capacity(128);
        attribs.extend([egl::WIDTH, buf.width]);
        attribs.extend([egl::HEIGHT, buf.height]);
        attribs.extend([EGL_LINUX_DRM_FOURCC_EXT, buf.format.drm as i32]);
        attribs.extend([EGL_IMAGE_PRESERVED_KHR, egl::TRUE as i32]);
        for (key, plane) in PLANE_KEYS.iter().zip(buf.planes.iter()) {
            attribs.extend([key.fd, *plane.fd]);
            attribs.extend([key.pitch, plane.stride as i32]);
            attribs.extend([key.offset, plane.offset as i32]);
            if buf.modifier != Modifier::INVALID {
                attribs.extend([key.modifier_lo, buf.modifier.low() as i32]);
                attribs.extend([key.modifier_hi, buf.modifier.high() as i32]);
            }
        }
        attribs.push(egl::NONE);

        let image = unsafe {
            create_image_khr(&self.egl, self.display, EGL_LINUX_DMA_BUF_EXT as _, &attribs).unwrap()
        };

        EglImage {
            image,
            external_only: egl_modifier.external_only,
            destroyed: false,
        }
    }

    pub fn make_fb_from_image(&self, image: &EglImage, dmabuf: &DmaBuf) -> EglFrameBuffer {
        let render_buffer = self.make_render_buffer_from_image(image, dmabuf);
        render_buffer.make_framebuffer()
    }

    pub fn make_render_buffer_from_image(&self, image: &EglImage, dmabuf: &DmaBuf) -> EglRenderBuffer {
        let mut renderbuffer_object = 0;

        unsafe {
            glGenRenderbuffers(1, &mut renderbuffer_object);
            glBindRenderbuffer(GL_RENDERBUFFER, renderbuffer_object);
            image_target_renderbuffer_storage_oes(&self.egl, GL_RENDERBUFFER, image.image.ptr);
            glBindRenderbuffer(GL_RENDERBUFFER, 0);
        }

        EglRenderBuffer {
            width: dmabuf.width,
            height: dmabuf.height,
            format: dmabuf.format,
            renderbuffer_object,
        }
    }

    fn query_formats(
        egl: &EglInstance,
        display: &khronos_egl::Display,
    ) -> HashMap<DrmFourcc, EglFormat> {
        let mut nformats = 0;
        unsafe {
            query_dma_buf_formats_ext(egl, *display, 0, &mut [], &mut nformats).unwrap();
        }
        let mut formats = vec![0i32; nformats as usize];
        unsafe {
            query_dma_buf_formats_ext(egl, *display, nformats, &mut formats, &mut nformats)
                .unwrap();
        }
        assert_eq!(nformats as usize, formats.len());

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
        egl: &EglInstance,
        display: &khronos_egl::Display,
        gl_format: egl::Int,
        format: &Format,
    ) -> (BTreeMap<Modifier, EglModifier>, bool) {
        const INVALID_MODIFIER: u64 = 0x00ff_ffff_ffff_ffff;

        let mut nmodifiers = 0;
        unsafe {
            query_dma_buf_modifiers_ext(
                egl,
                *display,
                gl_format,
                nmodifiers,
                &mut [],
                &mut [],
                &mut nmodifiers,
            )
            .unwrap();
        }
        let mut modifiers = vec![0u64; nmodifiers as usize];
        let mut external_only = vec![0; nmodifiers as usize];
        unsafe {
            query_dma_buf_modifiers_ext(
                egl,
                *display,
                gl_format,
                nmodifiers,
                &mut modifiers,
                &mut external_only,
                &mut nmodifiers,
            )
            .unwrap();
        }

        let mut map = modifiers
            .iter()
            .copied()
            .zip(external_only)
            .map(|(modifier, external_only)| {
                let modifier = Modifier(modifier);
                (modifier, EglModifier { modifier, external_only: external_only == egl::TRUE })
            })
            .collect::<BTreeMap<_, _>>();

        let mut external_only = format.external_only_guess;
        if !modifiers.is_empty() {
            external_only = map.values().any(|m| m.external_only);
        }

        map.insert(Modifier::INVALID, EglModifier { modifier: Modifier::INVALID, external_only });

        (map, external_only)
    }

    fn find_config_xrgb8888(
        egl: &EglInstance,
        display: khronos_egl::Display,
    ) -> Option<egl::Config> {
        let mut configs = Vec::with_capacity(egl.get_config_count(display).unwrap());
        egl.get_configs(display, &mut configs).unwrap();
        configs.into_iter().find(|config| {
            let attr = egl
                .get_config_attrib(display, *config, egl::NATIVE_VISUAL_ID)
                .unwrap();
            let drm = DrmFourcc::try_from(attr as u32).unwrap();
            drm == drm::buffer::DrmFourcc::Xrgb8888
        })
    }
}

pub fn make_display(
    drm_fd: &DrmFd,
    egl: &EglInstance,
    gbm: &GbmDevice,
    connector: &crate::drm::Connector,
    crtc: drm::control::crtc::Handle,
) {
    let display = unsafe {
        egl.get_platform_display(EGL_PLATFORM_GBM_KHR, gbm.gbm_device, &[ATTRIB_NONE])
            .unwrap()
    };

    let (major, minor) = egl.initialize(display).unwrap();

    let mut configs = Vec::with_capacity(egl.get_config_count(display).unwrap());
    egl.get_configs(display, &mut configs).unwrap();

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
        egl::WINDOW_BIT,
        // egl::RED_SIZE, 8,
        // egl::GREEN_SIZE, 8,
        // egl::BLUE_SIZE, 8,
        egl::RENDERABLE_TYPE,
        egl::OPENGL_ES2_BIT,
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

    let egl_surface = unsafe {
        egl.create_platform_window_surface(display, config, gbm_surface.raw(), &[ATTRIB_NONE])
            .unwrap()
    };

    egl.make_current(display, Some(egl_surface), Some(egl_surface), Some(egl_context))
        .unwrap();

    let mut previous = None;

    for i in 0..100 {
        println!("I={:?}", i);

        let i = i as f32;
        unsafe {
            glClearColor(0.5, i / 100.0, 0.0, 1.0);
            glClear(GL_COLOR_BUFFER_BIT);
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
        let ptr = glGetString(GL_EXTENSIONS);
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

pub type GLbitfield = libc::c_uint;
pub type GLfloat = libc::c_float;
pub type GLenum = libc::c_uint;
pub type GLsizei = libc::c_int;
pub type GLuint = libc::c_uint;

const GL_COLOR_BUFFER_BIT: GLbitfield = 0x00004000;
const GL_EXTENSIONS: GLenum = 0x1F03;
const GL_RENDERBUFFER: GLenum = 0x8D41;
const GL_FRAMEBUFFER: GLenum = 0x8D40;
const GL_COLOR_ATTACHMENT0: GLenum = 0x8CE0;
const GL_FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;

#[link(name = "GLESv2")]
extern "C" {
    pub fn glGetString(name: GLenum) -> *const u8;
    pub fn glClear(mask: GLbitfield);
    pub fn glClearColor(red: GLfloat, green: GLfloat, blue: GLfloat, alpha: GLfloat);
    pub fn glGenRenderbuffers(n: GLsizei, renderbuffers: *mut GLuint);
    pub fn glBindRenderbuffer(target: GLenum, renderbuffer: GLuint);
    pub fn glDeleteRenderbuffers(n: GLsizei, renderbuffers: *const GLuint);
    pub fn glGenFramebuffers(n: GLsizei, framebuffers: *mut GLuint);
    pub fn glBindFramebuffer(target: GLenum, framebuffer: GLuint);
    pub fn glFramebufferRenderbuffer(
        target: GLenum,
        attachment: GLenum,
        renderbuffertarget: GLenum,
        renderbuffer: GLuint,
    );
    pub fn glCheckFramebufferStatus(target: GLenum) -> GLenum;
    pub fn glDeleteFramebuffers(n: GLsizei, framebuffers: *const GLuint);
}

pub struct ImageKHR {
    ptr: *mut u8,
}

pub unsafe fn create_image_khr(
    egl: &EglInstance,
    display: egl::Display,
    target: egl::Enum,
    attrib_list: &[egl::Int],
) -> Result<ImageKHR, ()> {
    type FunSignature = extern "C" fn(
        egl::EGLDisplay,
        egl::EGLContext,
        egl::Enum,
        egl::EGLClientBuffer,
        *const egl::Int,
    ) -> *mut u8;
    let create_image_khr: FunSignature = cache!(FunSignature, {
        let ptr: extern "system" fn() = egl.get_proc_address("eglCreateImageKHR").unwrap();
        std::mem::transmute::<_, FunSignature>(ptr)
    });
    let attrib_ptr = utils::ptr_or_null(attrib_list);
    let ctx = std::ptr::null_mut();
    let buffer = std::ptr::null_mut();
    let ptr = create_image_khr(display.as_ptr(), ctx, target, buffer, attrib_ptr);
    if ptr.is_null() {
        panic!()
    }
    Ok(ImageKHR { ptr })
}

pub unsafe fn image_target_renderbuffer_storage_oes(
    egl: &EglInstance,
    target: egl::Enum,
    image: *mut u8,
) {
    type FunSignature = extern "C" fn(
        egl::Enum,
        *mut u8,
    );
    let image_target_renderbuffer_storage_oes: FunSignature = cache!(FunSignature, {
        let ptr: extern "system" fn() = egl.get_proc_address("glEGLImageTargetRenderbufferStorageOES").unwrap();
        std::mem::transmute::<_, FunSignature>(ptr)
    });
    image_target_renderbuffer_storage_oes(target, image);
}

pub unsafe fn query_dma_buf_formats_ext(
    egl: &EglInstance,
    display: egl::Display,
    max_formats: egl::Int,
    formats: &mut [egl::Int],
    num_formats: &mut egl::Int,
) -> Result<(), ()> {
    type FunSignature =
        extern "C" fn(egl::EGLDisplay, egl::Int, *mut egl::Int, *mut egl::Int) -> u32;
    let query_dma_buf_formats_ext: FunSignature = cache!(FunSignature, {
        let ptr: extern "system" fn() = egl.get_proc_address("eglQueryDmaBufFormatsEXT").unwrap();
        std::mem::transmute::<_, FunSignature>(ptr)
    });
    let ptr = utils::ptr_or_null_mut(formats);
    let result = query_dma_buf_formats_ext(display.as_ptr(), max_formats, ptr, num_formats);
    if result == egl::TRUE {
        Ok(())
    } else {
        Err(())
    }
}

pub unsafe fn query_dma_buf_modifiers_ext(
    egl: &EglInstance,
    display: egl::Display,
    format: egl::Int,
    max_modifiers: egl::Int,
    modifiers: &mut [u64],
    external_only: &mut [egl::Boolean],
    num_modifiers: &mut egl::Int,
) -> Result<(), ()> {
    type FunSignature = extern "C" fn(
        display: egl::EGLDisplay,
        format: egl::Int,
        max_modifiers: egl::Int,
        modifiers: *mut u64,
        external_only: *mut egl::Boolean,
        num_modifiers: *mut egl::Int,
    ) -> u32;
    let query_dma_buf_modifiers_ext: FunSignature = cache!(FunSignature, {
        let ptr: extern "system" fn() = egl.get_proc_address("eglQueryDmaBufModifiersEXT").unwrap();
        std::mem::transmute::<_, FunSignature>(ptr)
    });
    let ptr = utils::ptr_or_null_mut(modifiers);
    let external_only_ptr = utils::ptr_or_null_mut(external_only);
    let result = query_dma_buf_modifiers_ext(
        display.as_ptr(),
        format,
        max_modifiers,
        ptr,
        external_only_ptr,
        num_modifiers,
    );
    if result == egl::TRUE {
        Ok(())
    } else {
        Err(())
    }
}

pub const EGL_LINUX_DRM_FOURCC_EXT: egl::Int = 0x3271;
pub const EGL_DMA_BUF_PLANE0_FD_EXT: egl::Int = 0x3272;
pub const EGL_DMA_BUF_PLANE0_OFFSET_EXT: egl::Int = 0x3273;
pub const EGL_DMA_BUF_PLANE0_PITCH_EXT: egl::Int = 0x3274;
pub const EGL_DMA_BUF_PLANE1_FD_EXT: egl::Int = 0x3275;
pub const EGL_DMA_BUF_PLANE1_OFFSET_EXT: egl::Int = 0x3276;
pub const EGL_DMA_BUF_PLANE1_PITCH_EXT: egl::Int = 0x3277;
pub const EGL_DMA_BUF_PLANE2_FD_EXT: egl::Int = 0x3278;
pub const EGL_DMA_BUF_PLANE2_OFFSET_EXT: egl::Int = 0x3279;
pub const EGL_DMA_BUF_PLANE2_PITCH_EXT: egl::Int = 0x327A;
pub const EGL_DMA_BUF_PLANE3_FD_EXT: egl::Int = 0x3440;
pub const EGL_DMA_BUF_PLANE3_OFFSET_EXT: egl::Int = 0x3441;
pub const EGL_DMA_BUF_PLANE3_PITCH_EXT: egl::Int = 0x3442;
pub const EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: egl::Int = 0x3443;
pub const EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: egl::Int = 0x3444;
pub const EGL_DMA_BUF_PLANE1_MODIFIER_LO_EXT: egl::Int = 0x3445;
pub const EGL_DMA_BUF_PLANE1_MODIFIER_HI_EXT: egl::Int = 0x3446;
pub const EGL_DMA_BUF_PLANE2_MODIFIER_LO_EXT: egl::Int = 0x3447;
pub const EGL_DMA_BUF_PLANE2_MODIFIER_HI_EXT: egl::Int = 0x3448;
pub const EGL_DMA_BUF_PLANE3_MODIFIER_LO_EXT: egl::Int = 0x3449;
pub const EGL_DMA_BUF_PLANE3_MODIFIER_HI_EXT: egl::Int = 0x344A;
pub const EGL_IMAGE_PRESERVED_KHR: egl::Int = 0x30D2;
pub const EGL_LINUX_DMA_BUF_EXT: egl::Int = 0x3270;
