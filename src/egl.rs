use {
    egli::{egl, query_extensions, query_version},
    std::collections::BTreeSet,
};

pub fn init() {
    dbg!(query_extensions().unwrap());
    dbg!(query_version().unwrap());

    egl::bind_api(egl::EGL_OPENGL_ES_API).expect("EGL bind_api failed");

    let extensions: BTreeSet<&'static str> = query_extensions().unwrap().split(' ').collect();
    dbg!(extensions);
}
