use crate::{
    clients::{Client, ClientDispatcher},
    protocol_generated::NewId,
    state::{Grab, IndexMap, State},
    wayland_objects::{
        wl_compositor::WlCompositorGlobal, wl_data_device_manager::WlDataDeviceManagerGlobal,
        wl_seat::WlSeatGlobal, wl_shm::WlShmGlobal, wl_subcompositor::WlSubcompositorGlobal,
        wp_cursor_shape_manager_v1::WpCursorShapeManagerV1Global,
        wp_presentation::WpPresentationGlobal, wp_viewporter::WpViewporterGlobal,
        xdg_activation_v1::XdgActivationV1Global, xdg_wm_base::XdgWmBaseGlobal,
        zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1Global,
        zxdg_output_manager_v1::ZxdgOutputManagerV1Global,
    },
};

pub struct Globals {
    pub map: Box<IndexMap<GlobalName, Box<dyn Global>>>,
    current_name: u32,
}

index_map_id!(GlobalName);

impl GlobalName {
    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

pub trait Global {
    /// Returns `(interface_name, interface_version)`
    fn interface(&self) -> (&'static str, u32);
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        dispatcher: &mut ClientDispatcher,
    );
}

impl Globals {
    pub fn new() -> Self {
        Self { map: Box::default(), current_name: 1 }
    }

    pub fn initialize(&mut self) {
        for fun in [
            WlCompositorGlobal::new,
            WlDataDeviceManagerGlobal::new,
            WlShmGlobal::new,
            WlSubcompositorGlobal::new,
            WlSeatGlobal::new, // TODO: `WlSeatGlobal` should be added dynamically
            WpPresentationGlobal::new,
            WpViewporterGlobal::new,
            ZwpLinuxDmabufV1Global::new,
            WpCursorShapeManagerV1Global::new,
            XdgActivationV1Global::new,
            XdgWmBaseGlobal::new,
            ZxdgOutputManagerV1Global::new,
        ] {
            self.map.insert(fun());
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (GlobalName, &Box<dyn Global>)> {
        self.map.iter()
    }

    pub fn insert(&mut self, global: Box<dyn Global>) {
        self.map.insert(global);
    }

    pub fn contains_interface(&self, interface: &str) -> bool {
        self.map
            .iter()
            .any(|(_, global)| global.interface().0 == interface)
    }

    /// Take a global out of the registry for binding. Fails if `name` is not a live global slot.
    pub fn try_grab_global(&mut self, name: u32) -> Option<Grab<GlobalName, Box<dyn Global>>> {
        let key = GlobalName::from(name as usize);
        self.map.try_grab(key)
    }
}
