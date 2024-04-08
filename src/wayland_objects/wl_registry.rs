use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wl_registry, WaylandId, WlRegistryId},
    state::State,
};

#[derive(Debug)]
pub struct WlRegistry {
    pub id: WlRegistryId,
}

impl WlRegistry {
    pub fn new(id: WlRegistryId) -> Self {
        Self { id }
    }

    pub fn list_global_objects(&self, state: &mut State, dispatcher: &mut ClientDispatcher) {
        for (name, global) in state.globals.iter() {
            let (interface_name, interface_version) = global.interface();

            let global = wl_registry::Event::Global {
                name: name.as_u32(),
                interface: interface_name.to_string(),
                version: interface_version,
            };

            self.send_event(global, dispatcher);
        }
    }

    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 1, interface: "wl_compositor", version: 6)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 2, interface: "wl_shm", version: 2)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 3, interface: "wl_subcompositor", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 4, interface: "xdg_wm_base", version: 6)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 5, interface: "wl_data_device_manager", version: 3)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 6, interface: "zxdg_decoration_manager_v1", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 7, interface: "org_kde_kwin_server_decoration_manager", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 23, interface: "xdg_activation_v1", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 12, interface: "zwlr_screencopy_manager_v1", version: 3)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 9, interface: "zwlr_layer_shell_v1", version: 4)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 25, interface: "zwp_idle_inhibit_manager_v1", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 19, interface: "wp_tearing_control_manager_v1", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 10, interface: "zxdg_output_manager_v1", version: 3)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 15, interface: "wp_viewporter", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 31, interface: "wl_drm", version: 2)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 8, interface: "zwp_primary_selection_device_manager_v1", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 22, interface: "wp_content_type_manager_v1", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 13, interface: "zwp_relative_pointer_manager_v1", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 16, interface: "wp_fractional_scale_manager_v1", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 29, interface: "wp_presentation", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 17, interface: "zwp_pointer_constraints_v1", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 32, interface: "zwp_linux_dmabuf_v1", version: 5)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 20, interface: "wp_single_pixel_buffer_manager_v1", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 21, interface: "wp_cursor_shape_manager_v1", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 27, interface: "xdg_toplevel_drag_manager_v1", version: 1)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 30, interface: "wl_seat", version: 9)
    // [INFO  jay::client] Client 1 [OUT] wl_registry@2.global(name: 33, interface: "wl_output", version: 4)

    // pub fn done(&self, dispatcher: &mut ClientDispatcher) {
    //     let callback_data = self.id.into();
    //     self.send_event(Event::Done { callback_data }, dispatcher);
    // }
}

impl Object for WlRegistry {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, _: &mut Client, _: &mut State, req: RawRequest, _: &mut ClientDispatcher) {
        match Self::parse(req).unwrap() {
            wl_registry::Request::Bind { name, id } => todo!(),
        }
    }
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
