use drm::control::Device;

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser},
    protocol_generated::{
        wp_linux_drm_syncobj_timeline_v1, WaylandId, WpLinuxDrmSyncobjTimelineV1Id,
    },
    state::State,
    sync::SyncTimeline,
};

#[derive(Debug)]
pub struct WpLinuxDrmSyncobjTimelineV1 {
    id: WpLinuxDrmSyncobjTimelineV1Id,
    timeline: SyncTimeline,
}

impl WpLinuxDrmSyncobjTimelineV1 {
    pub fn new(id: WpLinuxDrmSyncobjTimelineV1Id, timeline: SyncTimeline) -> Self {
        Self { id, timeline }
    }

    pub fn timeline(&self) -> &SyncTimeline {
        &self.timeline
    }
}

impl Drop for WpLinuxDrmSyncobjTimelineV1 {
    fn drop(&mut self) {
        if let Err(error) = self.timeline.drm().destroy_syncobj(self.timeline.handle()) {
            log::warn!("failed to destroy imported syncobj timeline {:?}: {:?}", self.id, error);
        }
    }
}

impl Object for WpLinuxDrmSyncobjTimelineV1 {
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
        use wp_linux_drm_syncobj_timeline_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
    }
}
