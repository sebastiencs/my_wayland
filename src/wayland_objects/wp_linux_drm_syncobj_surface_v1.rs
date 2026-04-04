use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    object::{Object, Parser},
    protocol_generated::{
        wp_linux_drm_syncobj_surface_v1, WaylandId, WlSurfaceId, WpLinuxDrmSyncobjSurfaceV1Id,
    },
    state::State,
    sync::{AcquireSync, ReleasePoint},
    wayland_objects::{
        wl_surface::WlSurface, wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1,
    },
};

#[derive(Debug)]
pub struct WpLinuxDrmSyncobjSurfaceV1 {
    id: WpLinuxDrmSyncobjSurfaceV1Id,
    surface: Resolved<WlSurfaceId>,
}

impl WpLinuxDrmSyncobjSurfaceV1 {
    pub fn new(id: WpLinuxDrmSyncobjSurfaceV1Id, surface: Resolved<WlSurfaceId>) -> Self {
        Self { id, surface }
    }
}

impl Object for WpLinuxDrmSyncobjSurfaceV1 {
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
        use wp_linux_drm_syncobj_surface_v1::Request::*;

        let Some(mut surface) = self.surface.try_grab(state) else {
            client.send_protocol_error(
                dispatcher,
                self.id.into(),
                wp_linux_drm_syncobj_surface_v1::Error::NoSurface as u32,
                "associated wl_surface no longer exists",
            );
            return Ok(());
        };

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                surface.clear_syncobj_surface_object(Resolved::new(
                    self.surface.client_id(),
                    self.id,
                    client.object_id(self.id),
                ));
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            SetAcquirePoint { timeline, point_hi, point_lo } => {
                let Some(timeline) = client.resolve(timeline, state) else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wp_linux_drm_syncobj_surface_v1::Error::NoSurface as u32,
                        "invalid syncobj timeline for acquire point",
                    );
                    return Ok(());
                };
                let timeline: &mut WpLinuxDrmSyncobjTimelineV1 = timeline.ref_mut(state);
                let point = ((point_hi as u64) << 32) | point_lo as u64;
                surface.pending_sync_mut().acquire =
                    AcquireSync::TimelinePoint { timeline: timeline.timeline().clone(), point };
            }
            SetReleasePoint { timeline, point_hi, point_lo } => {
                let Some(timeline) = client.resolve(timeline, state) else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wp_linux_drm_syncobj_surface_v1::Error::NoSurface as u32,
                        "invalid syncobj timeline for release point",
                    );
                    return Ok(());
                };
                let timeline: &mut WpLinuxDrmSyncobjTimelineV1 = timeline.ref_mut(state);
                let point = ((point_hi as u64) << 32) | point_lo as u64;
                surface.pending_sync_mut().release =
                    ReleasePoint::TimelinePoint { timeline: timeline.timeline().clone(), point };
            }
        }
        Ok(())
    }
}
