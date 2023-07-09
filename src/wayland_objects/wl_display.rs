use crate::{
    clients::{Client, RawRequest},
    object::{Object, Parser},
    protocol_generated::{self, wl_display, WaylandId},
};

pub struct WlDisplay {
    pub id: protocol_generated::WlDisplayId,
}

impl WlDisplay {
    pub fn new() -> Self {
        Self { id: 1.into() }
    }
}

impl Object for WlDisplay {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, client: &mut Client, request: RawRequest) {
        use wl_display::Request::*;

        let request = Self::parse_request(request).unwrap();
        eprintln!("WlDisplay::on {:?}", request);
        match request {
            Sync { callback } => {}
            GetRegistry { registry } => {}
        }
    }
}
