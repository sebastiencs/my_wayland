use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_display, zwp_linux_dmabuf_v1, Constants, NewId, WaylandId, ZwpLinuxDmabufV1Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
};

#[derive(Debug)]
pub struct ZwpLinuxDmabufV1Global;

impl ZwpLinuxDmabufV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ZwpLinuxDmabufV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (ZwpLinuxDmabufV1::INTERFACE_NAME, 3)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let supported_formats = state
            .drm
            .iter()
            .next()
            .map(|(_, drm)| drm.supported_dmabuf_read_formats());
        let id = ZwpLinuxDmabufV1Id::from(id);
        let (_, dmabuf) = client.insert_object(state, ZwpLinuxDmabufV1 { id });

        let Some(supported_formats) = supported_formats else {
            log::warn!("linux-dmabuf bound before DRM backend became available");
            return;
        };
        for (format, modifiers) in supported_formats {
            dmabuf.send_event(
                zwp_linux_dmabuf_v1::Event::Format { format: format.drm as u32 },
                dispatcher,
            );
            for modifier in modifiers {
                dmabuf.send_event(
                    zwp_linux_dmabuf_v1::Event::Modifier {
                        format: format.drm as u32,
                        modifier_hi: modifier.high(),
                        modifier_lo: modifier.low(),
                    },
                    dispatcher,
                );
            }
        }
        log::debug!("advertised linux-dmabuf global to client {:?}", client.client_id());
    }
}

#[derive(Debug)]
pub struct ZwpLinuxDmabufV1 {
    id: ZwpLinuxDmabufV1Id,
}

impl Object for ZwpLinuxDmabufV1 {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        mut self: GrabbedObject<Self>,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        use zwp_linux_dmabuf_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            CreateParams { params_id } => {
                let params = ZwpLinuxBufferParamsV1::new(params_id);
                client.insert_object(state, params);
            }
            GetDefaultFeedback { .. } | GetSurfaceFeedback { .. } => {
                client.protocol_violation_disconnect(
                    dispatcher,
                    self.id.into(),
                    wl_display::Error::InvalidMethod as u32,
                    "linux-dmabuf feedback is not advertised by this compositor version",
                );
            }
        }
        Ok(())
    }
}
