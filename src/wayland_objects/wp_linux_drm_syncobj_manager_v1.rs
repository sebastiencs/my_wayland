use std::os::fd::{BorrowedFd, FromRawFd};

use drm::control::Device;

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{
        wp_linux_drm_syncobj_manager_v1, Constants, NewId, WaylandId, WpLinuxDrmSyncobjManagerV1Id,
    },
    state::State,
    sync::SyncTimeline,
    wayland_objects::{
        wl_surface::WlSurface, wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1,
        wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1,
    },
};

#[derive(Debug)]
pub struct WpLinuxDrmSyncobjManagerV1Global;

impl WpLinuxDrmSyncobjManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for WpLinuxDrmSyncobjManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (WpLinuxDrmSyncobjManagerV1::INTERFACE_NAME, WpLinuxDrmSyncobjManagerV1::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = WpLinuxDrmSyncobjManagerV1Id::from(id);
        client.insert_object(state, WpLinuxDrmSyncobjManagerV1 { id });
        log::debug!("advertised linux-drm-syncobj manager to client {:?}", client.client_id());
    }
}

#[derive(Debug)]
pub struct WpLinuxDrmSyncobjManagerV1 {
    id: WpLinuxDrmSyncobjManagerV1Id,
}

impl Object for WpLinuxDrmSyncobjManagerV1 {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) {
        use wp_linux_drm_syncobj_manager_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetSurface { id, surface } => {
                if !client.contains_object(surface, state) {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wp_linux_drm_syncobj_manager_v1::Error::SurfaceExists as u32,
                        "invalid wl_surface for syncobj surface",
                    );
                    return;
                }
                let surface_oid = client.object_id(surface);
                {
                    let mut surface_obj = state.objects.grab(surface_oid);
                    let surface = surface_obj.downcast_mut::<WlSurface>().unwrap();
                    if surface.set_syncobj_surface_object(id, dispatcher).is_err() {
                        return;
                    }
                }
                client.insert_object(state, WpLinuxDrmSyncobjSurfaceV1::new(id, surface_oid));
            }
            ImportTimeline { id, fd } => {
                let Some((_, drm)) = state.drm.iter().next() else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wp_linux_drm_syncobj_manager_v1::Error::InvalidTimeline as u32,
                        "cannot import syncobj timeline without an active DRM backend",
                    );
                    return;
                };
                if !drm.supports_syncobj_timeline() {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wp_linux_drm_syncobj_manager_v1::Error::InvalidTimeline as u32,
                        "DRM backend does not support syncobj timelines",
                    );
                    unsafe { libc::close(fd) };
                    return;
                }
                let drm_fd = drm.fd();
                let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
                let handle = match drm_fd.fd_to_syncobj(borrowed, false) {
                    Ok(handle) => handle,
                    Err(error) => {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            wp_linux_drm_syncobj_manager_v1::Error::InvalidTimeline as u32,
                            format!("failed to import DRM syncobj timeline: {error}"),
                        );
                        unsafe { libc::close(fd) };
                        return;
                    }
                };
                unsafe { libc::close(fd) };
                client.insert_object(
                    state,
                    WpLinuxDrmSyncobjTimelineV1::new(id, SyncTimeline::new(drm_fd, handle)),
                );
            }
        }
    }
}
