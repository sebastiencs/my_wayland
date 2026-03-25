use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{
        wl_display, zxdg_output_manager_v1, Constants, NewId, WaylandId, ZxdgOutputManagerV1Id,
    },
    state::State,
    wayland_objects::{wl_output::WlOutput, zxdg_output_v1::ZxdgOutputV1},
};

#[derive(Debug)]
pub struct ZxdgOutputManagerV1Global;

impl ZxdgOutputManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for ZxdgOutputManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (ZxdgOutputManagerV1::INTERFACE_NAME, ZxdgOutputManagerV1::INTERFACE_VERSION)
    }
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = ZxdgOutputManagerV1Id::from(id);
        client.insert_object(state, ZxdgOutputManagerV1 { id, version });
    }
}

#[derive(Debug)]
pub struct ZxdgOutputManagerV1 {
    id: ZxdgOutputManagerV1Id,
    version: u32,
}

impl ZxdgOutputManagerV1 {
    pub fn new(id: ZxdgOutputManagerV1Id, version: u32) -> Self {
        Self { id, version }
    }
}

impl Object for ZxdgOutputManagerV1 {
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
        use zxdg_output_manager_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetXdgOutput { id, output } => {
                let Some(output_oid) = client.try_object_id(output, state) else {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        output.into(),
                        wl_display::Error::InvalidObject as u32,
                        "invalid wl_output for zxdg_output_v1",
                    );
                    return;
                };
                let Some(wl_output) = state.objects.try_get_ref_of::<WlOutput>(output_oid) else {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        output.into(),
                        wl_display::Error::InvalidObject as u32,
                        "zxdg_output target is not a wl_output",
                    );
                    return;
                };

                let Some(output_id) = wl_output.output_id() else {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        output.into(),
                        wl_display::Error::InvalidObject as u32,
                        "wl_output is no longer associated with a live output node",
                    );
                    return;
                };

                let xdg_output = ZxdgOutputV1::new(id, output_id, self.version);
                xdg_output.send_logical_position(state, dispatcher);
                xdg_output.send_logical_size(state, dispatcher);
                xdg_output.send_name(state, dispatcher);
                xdg_output.send_description(state, dispatcher);
                xdg_output.send_done(dispatcher);
                wl_output.send_done(dispatcher);

                client.insert_object(state, xdg_output);
            }
        }
    }
}
