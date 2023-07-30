use std::{collections::HashSet, sync::Arc};

use {
    drm::buffer::DrmFourcc,
    egl::{Attrib, ATTRIB_NONE},
};
// use {
//     egli::{egl, query_extensions, query_version},
//     std::collections::BTreeSet,
// };
use {
    khronos_egl::{self as egl, Dynamic, Instance, EGL1_5},
    libloading::Library,
};

use drm::control::Device;

use crate::{drm::DrmFd, gbm::GbmDevice};

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

pub fn make_display(
    drm_fd: &DrmFd,
    egl: &EglInstance,
    gbm: &GbmDevice,
    connector: &crate::drm::Connector,
    crtc: drm::control::crtc::Handle,
) {
    let display = egl
        .get_platform_display(EGL_PLATFORM_GBM_KHR, gbm.gbm_device, &[ATTRIB_NONE])
        .unwrap();

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

    let egl_surface = egl
        .create_platform_window_surface(display, config, gbm_surface.raw(), &[ATTRIB_NONE])
        .unwrap();

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

        if let Some((fb, bo)) = previous.take() {
            gbm.destroy_framebuffer(fb).unwrap();
            gbm_surface.release_buffer(bo);
        }
        previous = Some((fb, bo));

        std::thread::sleep_ms(20);
    }
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

const GL_COLOR_BUFFER_BIT: GLbitfield = 0x00004000;

#[link(name = "GLESv2")]
extern "C" {
    pub fn glClear(mask: GLbitfield);
    pub fn glClearColor(red: GLfloat, green: GLfloat, blue: GLfloat, alpha: GLfloat);
}
