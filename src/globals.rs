use crate::{
    clients::{Client, ClientDispatcher},
    protocol_generated::NewId,
    state::{Grab, IndexMap, State},
    wayland_objects::{
        ext_idle_notifier_v1::ExtIdleNotifierV1Global,
        ext_workspace_manager_v1::ExtWorkspaceManagerV1Global, wl_compositor::WlCompositorGlobal,
        wl_data_device_manager::WlDataDeviceManagerGlobal, wl_seat::WlSeatGlobal,
        wl_shm::WlShmGlobal, wl_subcompositor::WlSubcompositorGlobal,
        wp_cursor_shape_manager_v1::WpCursorShapeManagerV1Global,
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1Global,
        wp_presentation::WpPresentationGlobal, wp_viewporter::WpViewporterGlobal,
        xdg_activation_v1::XdgActivationV1Global, xdg_wm_base::XdgWmBaseGlobal,
        zwlr_layer_shell_v1::ZwlrLayerShellV1Global, zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1Global,
        zwp_pointer_constraints_v1::ZwpPointerConstraintsV1Global,
        zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1Global,
        zxdg_decoration_manager_v1::ZxdgDecorationManagerV1Global,
        zxdg_output_manager_v1::ZxdgOutputManagerV1Global,
    },
};

use std::collections::BTreeMap;

pub struct Globals {
    pub map: Box<IndexMap<GlobalSlotId, Box<dyn Global>>>,
    current_name: u32,
    names_by_slot: BTreeMap<GlobalSlotId, GlobalName>,
    slots_by_name: BTreeMap<GlobalName, GlobalSlotId>,
}

index_map_id!(GlobalSlotId);

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlobalName(u32);

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
        Self {
            map: Box::default(),
            current_name: 1,
            names_by_slot: BTreeMap::new(),
            slots_by_name: BTreeMap::new(),
        }
    }

    pub fn initialize(&mut self) {
        for fun in [
            WlCompositorGlobal::new,
            WlDataDeviceManagerGlobal::new,
            WlShmGlobal::new,
            WlSubcompositorGlobal::new,
            WlSeatGlobal::new, // TODO: `WlSeatGlobal` should be added dynamically
            WpPresentationGlobal::new,
            WpFractionalScaleManagerV1Global::new,
            WpViewporterGlobal::new,
            ZwpLinuxDmabufV1Global::new,
            WpCursorShapeManagerV1Global::new,
            ZwpRelativePointerManagerV1Global::new,
            ZwpPointerConstraintsV1Global::new,
            ExtIdleNotifierV1Global::new,
            XdgActivationV1Global::new,
            XdgWmBaseGlobal::new,
            ZxdgDecorationManagerV1Global::new,
            ZwlrLayerShellV1Global::new,
            ZxdgOutputManagerV1Global::new,
            ExtWorkspaceManagerV1Global::new,
        ] {
            self.insert(fun());
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (GlobalName, &Box<dyn Global>)> {
        self.map.iter().map(|(slot_id, global)| {
            let name = *self
                .names_by_slot
                .get(&slot_id)
                .expect("missing global wire name");
            (name, global)
        })
    }

    pub fn insert(&mut self, global: Box<dyn Global>) -> GlobalName {
        let slot_id = self.map.insert(global);
        let name = GlobalName(self.current_name);
        self.current_name = self
            .current_name
            .checked_add(1)
            .expect("global name space exhausted");
        assert!(self.names_by_slot.insert(slot_id, name).is_none());
        assert!(self.slots_by_name.insert(name, slot_id).is_none());
        name
    }

    pub fn remove(&mut self, name: GlobalName) -> Option<Box<dyn Global>> {
        let slot_id = self.slots_by_name.remove(&name)?;
        self.names_by_slot.remove(&slot_id);
        self.map.remove(slot_id)
    }

    pub fn contains_interface(&self, interface: &str) -> bool {
        self.map
            .iter()
            .any(|(_, global)| global.interface().0 == interface)
    }

    pub fn name_for_interface(&self, interface: &str) -> Option<GlobalName> {
        self.map.iter().find_map(|(slot_id, global)| {
            (global.interface().0 == interface).then(|| {
                *self
                    .names_by_slot
                    .get(&slot_id)
                    .expect("missing global wire name")
            })
        })
    }

    /// Take a global out of the registry for binding. Fails if `name` is not a live global slot.
    pub fn try_grab_global(&mut self, name: u32) -> Option<Grab<GlobalSlotId, Box<dyn Global>>> {
        let slot_id = *self.slots_by_name.get(&GlobalName(name))?;
        self.map.try_grab(slot_id)
    }
}
