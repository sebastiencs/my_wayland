use crate::{
    clients::{Client, ClientAction, ClientDispatcher, RawRequest},
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
        dispatcher: &mut ClientDispatcher,
    ) -> crate::object::RequestResult {
        use wl_data_device_manager::Request::*;

        match client.parse::<Self>(req).unwrap() {
            CreateDataSource { id } => {
                client
                    .insert_object(state, WlDataSource::new(id, client.client_id(), self.version));
            }
            GetDataDevice { id, seat } => {
                client.resolve_or_disconnect(seat, state, dispatcher)?;
                let _ = client.insert_object(
                    state,
                    WlDataDevice::new(WlDataDeviceId::from(id), seat, self.version),
                );
                let data_device_id = WlDataDeviceId::from(id);
                let data_device = client.resolve(data_device_id, state).unwrap();
                state.seat.register_data_device(data_device);
                if state
                    .seat
                    .keyboard_focus()
                    .is_some_and(|focus| focus.client_id() == client.client_id())
                {
                    dispatcher
                        .dispatch_to(client.client_id(), ClientAction::RefreshDataDeviceSelection);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {
        super::WlDataDeviceManager,
        crate::{
            clients::{Client, ClientAction, ClientId, ClientManagerAction, RawEvent, Resolved},
            egl::EGL,
            globals::Globals,
            object::Object,
            protocol_generated::{WlDataDeviceId, WlDataDeviceManagerId, WlDataSourceId, WlSeatId},
            runner::Dispatcher,
            seat::{set_keyboard_focus_from_surface, SurfaceFocus},
            state::State,
            wayland_objects::{
                wl_data_source::WlDataSource, wl_seat::WlSeat, wl_surface::WlSurface,
            },
            xkb::Xkb,
        },
    };

    fn make_state() -> State {
        let xkb = Xkb::new();
        let keyboard = xkb.create_keyboard_state();
        let mut globals = Globals::new();
        globals.initialize();
        State {
            objects: Default::default(),
            root_surfaces: Default::default(),
            layer_surface_objects: Default::default(),
            inputs: Default::default(),
            drm: Default::default(),
            outputs: Default::default(),
            active_output: None,
            workspace_groups: Default::default(),
            workspaces: Default::default(),
            output_workspace_groups: Default::default(),
            root_surface_workspaces: Default::default(),
            managed_root_surfaces: Default::default(),
            xdg_toplevel_roles: Default::default(),
            ctm_control_owner: None,
            data_offers_by_source: Default::default(),
            next_workspace_serial: 1,
            backend_syncobj_globals: Default::default(),
            globals,
            activation_tokens: Default::default(),
            egl: EGL::new(),
            config: Default::default(),
            seat: Default::default(),
            keyboard,
            xkb,
        }
    }

    fn get_data_device_request(
        manager_id: WlDataDeviceManagerId,
        data_device_id: WlDataDeviceId,
        seat_id: WlSeatId,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(manager_id.into(), 1, 2);
        event.id(data_device_id);
        event.id(seat_id);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn insert_surface(state: &mut State, client: &mut Client, surface_id: u32) {
        client.insert_object(state, WlSurface::new(surface_id.into(), client.client_id(), 6));
    }

    #[test]
    fn focused_client_get_data_device_queues_selection_refresh() {
        let mut state = make_state();
        let mut client = Client::new_for_tests(ClientId::from_raw(7));
        let mut dispatcher = Dispatcher::default();
        let seat_id = WlSeatId::from(2);
        let manager_id = WlDataDeviceManagerId::from(3);
        let mut manager = WlDataDeviceManager { id: manager_id, version: 3 };

        client.insert_object(&mut state, WlSeat::new(seat_id));
        insert_surface(&mut state, &mut client, 99);
        {
            let mut setup_dispatcher = client.test_dispatcher(&mut dispatcher);
            set_keyboard_focus_from_surface(
                &mut state,
                &mut setup_dispatcher,
                Some(SurfaceFocus::new(Resolved::new(
                    client.client_id(),
                    crate::protocol_generated::WlSurfaceId::from(99u32),
                    client.object_id(crate::protocol_generated::WlSurfaceId::from(99u32)),
                ))),
            );
        }
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        manager.on(
            &mut client,
            &mut state,
            get_data_device_request(manager_id, WlDataDeviceId::from(4), seat_id),
            &mut client_dispatcher,
        );

        assert!(dispatcher
            .test_has_queued_action_for::<ClientAction, ClientId>(client.client_id(), |action| {
                matches!(action, ClientAction::RefreshDataDeviceSelection)
            }));
    }

    #[test]
    fn get_data_device_rejects_non_seat_object() {
        let mut state = make_state();
        let mut client = Client::new_for_tests(ClientId::from_raw(7));
        let mut dispatcher = Dispatcher::default();
        let manager_id = WlDataDeviceManagerId::from(3);
        let mut manager = WlDataDeviceManager { id: manager_id, version: 3 };

        client.insert_object(
            &mut state,
            WlDataSource::new(WlDataSourceId::from(2), client.client_id(), 3),
        );
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client_dispatcher.set_requester_object_id_for_tests(manager_id.into());

        manager.on(
            &mut client,
            &mut state,
            get_data_device_request(manager_id, WlDataDeviceId::from(4), WlSeatId::from(2)),
            &mut client_dispatcher,
        );

        assert!(dispatcher.test_has_queued_action::<ClientManagerAction>(|action| {
            matches!(action, ClientManagerAction::Close { id } if *id == client.client_id())
        }));
    }
}
