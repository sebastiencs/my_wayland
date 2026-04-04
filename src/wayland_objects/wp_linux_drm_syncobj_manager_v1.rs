use std::os::fd::{BorrowedFd, FromRawFd};

use drm::control::Device;

use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{
        wl_display, wp_linux_drm_syncobj_manager_v1, Constants, NewId, WaylandId,
        WpLinuxDrmSyncobjManagerV1Id,
    },
    state::{DrmId, State},
    sync::SyncTimeline,
    wayland_objects::{
        wl_surface::WlSurface, wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1,
        wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1,
    },
};

#[derive(Debug)]
pub struct WpLinuxDrmSyncobjManagerV1Global {
    drm_id: DrmId,
}

impl WpLinuxDrmSyncobjManagerV1Global {
    pub fn new(drm_id: DrmId) -> Box<dyn Global> {
        Box::new(Self { drm_id })
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
        client.insert_object(state, WpLinuxDrmSyncobjManagerV1 { id, import_drm_id: self.drm_id });
        log::debug!("advertised linux-drm-syncobj manager to client {:?}", client.client_id());
    }
}

#[derive(Debug)]
pub struct WpLinuxDrmSyncobjManagerV1 {
    id: WpLinuxDrmSyncobjManagerV1Id,
    import_drm_id: DrmId,
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
    ) -> crate::object::RequestResult {
        use wp_linux_drm_syncobj_manager_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetSurface { id, surface } => {
                let surface = client.resolve_or_disconnect(surface, state, dispatcher)?;
                {
                    let surface = surface.rref(state);
                    if surface.has_syncobj_surface_object() {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            wp_linux_drm_syncobj_manager_v1::Error::SurfaceExists as u32,
                            "surface already has a syncobj surface object",
                        );
                        return Ok(());
                    }
                }
                client.insert_object(state, WpLinuxDrmSyncobjSurfaceV1::new(id, surface));
                let object = client.resolve(id, state).unwrap();
                surface
                    .grab(state)
                    .set_syncobj_surface_object(object)
                    .expect("syncobj surface existence was checked before insertion");
            }
            ImportTimeline { id, fd } => {
                let Some(drm) = state.drm.try_get(self.import_drm_id) else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wp_linux_drm_syncobj_manager_v1::Error::InvalidTimeline as u32,
                        "cannot import syncobj timeline without an active DRM backend",
                    );
                    unsafe { libc::close(fd) };
                    return Ok(());
                };
                if !drm.supports_linux_drm_syncobj() {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wp_linux_drm_syncobj_manager_v1::Error::InvalidTimeline as u32,
                        "DRM backend does not support syncobj timelines",
                    );
                    unsafe { libc::close(fd) };
                    return Ok(());
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
                        return Ok(());
                    }
                };
                unsafe { libc::close(fd) };
                client.insert_object(
                    state,
                    WpLinuxDrmSyncobjTimelineV1::new(id, SyncTimeline::new(drm_fd, handle)),
                );
            }
        }
        Ok(())
    }
}
