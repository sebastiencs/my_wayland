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

use crate::gbm::GbmDevice;

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

pub fn make_display(egl: &EglInstance, gbm: &GbmDevice) {
    let display = egl
        .get_platform_display(EGL_PLATFORM_GBM_KHR, gbm.gbm_device, &[ATTRIB_NONE])
        .unwrap();

    let (major, minor) = egl.initialize(display).unwrap();

    let mut configs = Vec::with_capacity(egl.get_config_count(display).unwrap());
    egl.get_configs(display, &mut configs).unwrap();

    for config in &configs {
        let a = egl
            .get_config_attrib(display, *config, egl::NATIVE_VISUAL_ID)
            .unwrap();
        let drm = DrmFourcc::try_from(a as u32).unwrap();
    }

    let exts = egl.query_string(Some(display), egl::EXTENSIONS).unwrap();
    let exts: HashSet<String> = exts
        .to_string_lossy()
        .split(' ')
        .map(|s| s.trim().to_string())
        .collect();

    dbg!(exts);

    dbg!(display);
    dbg!(major, minor);
}
