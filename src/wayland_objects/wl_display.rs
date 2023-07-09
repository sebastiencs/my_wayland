use crate::{
    clients::Client,
    object::Object,
    protocol_generated::{self, wl_display},
};

pub struct WlDisplay {
    pub id: protocol_generated::WlDisplayId,
}

impl Object for WlDisplay {
    fn on(&mut self, client: &mut Client, request: wl_display::Request) {
        use wl_display::Request::*;

        eprintln!("WlDisplay::on {:?}", request);
        match request {
            Sync { callback } => {}
            GetRegistry { registry } => {}
        }
    }
}
