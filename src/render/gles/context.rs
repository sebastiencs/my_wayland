use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    os::fd::{AsRawFd, IntoRawFd, OwnedFd},
    rc::Rc,
    time::Duration,
};

use {
    drm::buffer::DrmFourcc,
    khronos_types::{egl, gl},
};

use drm::control::Device;

use {
    super::{
        import::{EglFormat, EglImage, EglModifier, OpenGlFormat},
        loader::{EGL, EGL_PLATFORM_GBM_KHR},
        resources::{EglFrameBuffer, EglRenderBuffer, GlesPrograms},
        sync::{EGL_SYNC_NATIVE_FENCE_ANDROID, EGL_SYNC_NATIVE_FENCE_FD_ANDROID},
    },
    crate::{
        dmabuf::DmaBuf,
        drm::{DrmFd, Format, Modifier, FORMATS_MAP},
        gbm::GbmDevice,
        render::RenderDeviceId,
    },
};

#[derive(Debug)]
pub struct EglContext {
    egl: EGL,
    display: egl::EGLDisplay,
    extensions: BTreeSet<String>,
    config: egl::EGLConfig,
    context: egl::EGLContext,
    formats: HashMap<DrmFourcc, EglFormat>,
    pub opengl_formats: HashMap<DrmFourcc, OpenGlFormat>,
    opengl_extensions: BTreeSet<String>,
    device_id: RenderDeviceId,
    pub(super) programs: Rc<GlesPrograms>,
}

impl EglContext {
    pub fn device_id(&self) -> RenderDeviceId {
        self.device_id
    }

    pub fn bind(&self) {
        let _ = self
            .egl
            .make_current(self.display, None, None, Some(self.context));
    }

    pub fn make(egl: &EGL, gbm: &GbmDevice) -> Result<Self, String> {
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
            formats,
            opengl_formats,
            opengl_extensions,
            device_id: RenderDeviceId::next(),
            programs: Rc::new(GlesPrograms::new()),
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
        render_buffer.make_framebuffer(Rc::clone(&self.programs))
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
            device_id: self.device_id,
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
        // images, and framebuffers. Shared programs are deleted while this
        // context is still current. The GBM device that created the display
        // outlives this context and is destroyed by its owner afterward.
        if self.display != egl::NO_DISPLAY && self.context != egl::NO_CONTEXT {
            let _ = self
                .egl
                .make_current(self.display, None, None, Some(self.context));
            if let Some(programs) = Rc::get_mut(&mut self.programs) {
                programs.destroy_gl();
            }
        }
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
