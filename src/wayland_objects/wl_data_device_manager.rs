use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{
        wl_data_device_manager, Constants, NewId, WaylandId, WlDataDeviceId, WlDataDeviceManagerId,
    },
    state::State,
    wayland_objects::{wl_data_device::WlDataDevice, wl_data_source::WlDataSource},
};

#[derive(Debug)]
pub struct WlDataDeviceManagerGlobal;

impl WlDataDeviceManagerGlobal {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for WlDataDeviceManagerGlobal {
    fn interface(&self) -> (&'static str, u32) {
        (WlDataDeviceManager::INTERFACE_NAME, WlDataDeviceManager::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = WlDataDeviceManagerId::from(id);
        client.insert_object(state, WlDataDeviceManager { id, version });
    }
}

#[derive(Debug)]
pub struct WlDataDeviceManager {
    id: WlDataDeviceManagerId,
    version: u32,
}

impl Object for WlDataDeviceManager {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        _dispatcher: &mut ClientDispatcher,
    ) {
        use wl_data_device_manager::Request::*;

        match client.parse::<Self>(req).unwrap() {
            CreateDataSource { id } => {
                client.insert_object(state, WlDataSource::new(id, self.version));
            }
            GetDataDevice { id, seat } => {
                if !client.contains_object(seat, state) {
                    return;
                }
                client.insert_object(
                    state,
                    WlDataDevice::new(WlDataDeviceId::from(id), self.version),
                );
            }
        }
    }
}
