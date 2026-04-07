use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwlr_gamma_control_manager_v1, Constants, NewId, WaylandId, WlOutputId,
        ZwlrGammaControlManagerV1Id, ZwlrGammaControlV1Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::{wl_display::WlDisplay, zwlr_gamma_control_v1::ZwlrGammaControlV1},
};

#[derive(Debug)]
pub struct ZwlrGammaControlManagerV1Global;

impl ZwlrGammaControlManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ZwlrGammaControlManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (ZwlrGammaControlManagerV1::INTERFACE_NAME, ZwlrGammaControlManagerV1::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = ZwlrGammaControlManagerV1Id::from(id);
        client.insert_object(state, ZwlrGammaControlManagerV1 { id, version });
    }
}

#[derive(Debug)]
pub struct ZwlrGammaControlManagerV1 {
    id: ZwlrGammaControlManagerV1Id,
    version: u32,
}

impl ZwlrGammaControlManagerV1 {
    fn create_gamma_control(
        &self,
        client: &mut Client,
        state: &mut State,
        id: ZwlrGammaControlV1Id,
        output: WlOutputId,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        let output = client.resolve_or_disconnect(output, state, dispatcher)?;
        let wl_output = output.rref(state);
        let target_output_id = wl_output.output_id();
        let valid_output_id = target_output_id.filter(|output_id| {
            state.output_supports_gamma_control(*output_id)
                && state.gamma_control_owner(*output_id).is_none()
        });
        let gamma_size = valid_output_id
            .and_then(|output_id| state.output_gamma_lut_size(output_id))
            .unwrap_or(0);

        let (gamma_handle, gamma_control) =
            client.insert_object(state, ZwlrGammaControlV1::new(id, valid_output_id, gamma_size));
        gamma_control.set_gamma_object(gamma_handle);

        if let Some(output_id) = valid_output_id {
            let _ = state.set_gamma_control_owner(output_id, gamma_handle);
            let gamma_control = gamma_handle.ref_mut(state);
            gamma_control.send_gamma_size(dispatcher);
        } else {
            let gamma_control = gamma_handle.ref_mut(state);
            gamma_control.send_failed(dispatcher);
        }
        Ok(())
    }
}

impl Object for ZwlrGammaControlManagerV1 {
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
        use zwlr_gamma_control_manager_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into()),
            GetGammaControl { id, output } => {
                let _ = self.version;
                self.create_gamma_control(client, state, id, output, dispatcher)?;
            }
        }
        Ok(())
    }
}
