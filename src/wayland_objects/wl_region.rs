use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wl_display, wl_region, WaylandId, WlRegionId},
    region::{RectI, RegionSpec, RegionValue},
    state::State,
};

#[derive(Debug)]
pub struct WlRegion {
    id: WlRegionId,
    spec: RegionSpec,
}

impl WlRegion {
    pub fn new(id: WlRegionId) -> Self {
        Self { id, spec: RegionSpec::default() }
    }

    pub fn snapshot(&self) -> RegionValue {
        RegionValue::from_spec(self.spec.clone())
    }

    fn delete_self(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        use crate::wayland_objects::wl_display::WlDisplay;
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }

    fn reject_negative_extents(&self, dispatcher: &mut ClientDispatcher) {
        dispatcher.send_wl_display_error(
            self.id.into(),
            wl_display::Error::InvalidMethod as u32,
            "wl_region rectangles must have non-negative width and height",
        );
    }
}

impl Object for WlRegion {
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
        use wl_region::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => self.delete_self(client, state, dispatcher),
            Add { x, y, width, height } => {
                let Some(rect) = RectI::new(x, y, width, height) else {
                    self.reject_negative_extents(dispatcher);
                    return;
                };
                self.spec.add_rect(rect);
            }
            Subtract { x, y, width, height } => {
                let Some(rect) = RectI::new(x, y, width, height) else {
                    self.reject_negative_extents(dispatcher);
                    return;
                };
                self.spec.subtract_rect(rect);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::WlRegion,
        crate::{
            protocol_generated::WlRegionId,
            region::{RectI, RegionValue},
        },
    };

    #[test]
    fn snapshot_has_copy_semantics() {
        let mut region = WlRegion::new(WlRegionId::from(1));
        region.spec.add_rect(RectI::new(0, 0, 20, 20).unwrap());
        let snapshot = region.snapshot();

        region.spec.subtract_rect(RectI::new(0, 0, 20, 20).unwrap());

        assert!(snapshot.contains(5, 5));
        assert!(!region.snapshot().contains(5, 5));
    }

    #[test]
    fn empty_region_snapshot_stays_empty() {
        let region = WlRegion::new(WlRegionId::from(2));
        assert_eq!(region.snapshot(), RegionValue::empty());
    }
}
