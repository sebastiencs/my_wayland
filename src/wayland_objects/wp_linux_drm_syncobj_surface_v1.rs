use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser},
    protocol_generated::{
        wp_linux_drm_syncobj_surface_v1, WaylandId, WpLinuxDrmSyncobjSurfaceV1Id,
    },
    state::{ObjectId, State},
    sync::{AcquireSync, ReleasePoint},
    wayland_objects::{
        wl_surface::WlSurface, wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1,
    },
};

#[derive(Debug)]
pub struct WpLinuxDrmSyncobjSurfaceV1 {
    id: WpLinuxDrmSyncobjSurfaceV1Id,
    surface_object_id: ObjectId,
}

impl WpLinuxDrmSyncobjSurfaceV1 {
    pub fn new(id: WpLinuxDrmSyncobjSurfaceV1Id, surface_object_id: ObjectId) -> Self {
        Self { id, surface_object_id }
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
    ) {
        use wp_linux_drm_syncobj_surface_v1::Request::*;

        let Some(mut surface) = state
            .objects
            .try_grab_object::<WlSurface>(self.surface_object_id)
        else {
            client.send_protocol_error(
                dispatcher,
                self.id.into(),
                wp_linux_drm_syncobj_surface_v1::Error::NoSurface as u32,
                "associated wl_surface no longer exists",
            );
            return;
        };

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                surface.clear_syncobj_surface_object(self.id);
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            SetAcquirePoint { timeline, point_hi, point_lo } => {
                if !client.object_is(timeline, state) {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wp_linux_drm_syncobj_surface_v1::Error::NoSurface as u32,
                        "invalid syncobj timeline for acquire point",
                    );
                    return;
                }
                let timeline: &mut WpLinuxDrmSyncobjTimelineV1 = client.get_object(timeline, state);
                let point = ((point_hi as u64) << 32) | point_lo as u64;
                surface.pending_sync_mut().acquire =
                    AcquireSync::TimelinePoint { timeline: timeline.timeline().clone(), point };
            }
            SetReleasePoint { timeline, point_hi, point_lo } => {
                if !client.object_is(timeline, state) {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wp_linux_drm_syncobj_surface_v1::Error::NoSurface as u32,
                        "invalid syncobj timeline for release point",
                    );
                    return;
                }
                let timeline: &mut WpLinuxDrmSyncobjTimelineV1 = client.get_object(timeline, state);
                let point = ((point_hi as u64) << 32) | point_lo as u64;
                surface.pending_sync_mut().release =
                    ReleasePoint::TimelinePoint { timeline: timeline.timeline().clone(), point };
            }
        }
    }
}
