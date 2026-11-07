use crate::{
    clients::{Client, ClientDispatcher, Resolved},
    protocol_generated::{NewId, WlRegistryId},
    state::{Grab, IndexMap, State},
    wayland_objects::{
        ext_data_control_manager_v1::ExtDataControlManagerV1Global,
        ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1Global,
        ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1Global,
        ext_idle_notifier_v1::ExtIdleNotifierV1Global,
        ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1Global,
        ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1Global,
        ext_session_lock_manager_v1::ExtSessionLockManagerV1Global,
        ext_workspace_manager_v1::ExtWorkspaceManagerV1Global, wl_compositor::WlCompositorGlobal,
        wl_data_device_manager::WlDataDeviceManagerGlobal, wl_fixes::WlFixesGlobal,
        wl_seat::WlSeatGlobal, wl_shm::WlShmGlobal, wl_subcompositor::WlSubcompositorGlobal,
        wp_cursor_shape_manager_v1::WpCursorShapeManagerV1Global,
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1Global,
        wp_presentation::WpPresentationGlobal, wp_viewporter::WpViewporterGlobal,
        xdg_activation_v1::XdgActivationV1Global, xdg_wm_base::XdgWmBaseGlobal,
        zwlr_data_control_manager_v1::ZwlrDataControlManagerV1Global,
        zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1Global,
        zwlr_layer_shell_v1::ZwlrLayerShellV1Global,
        zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1Global,
        zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1Global,
        zwp_input_method_manager_v2::ZwpInputMethodManagerV2Global,
        zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1Global,
        zwp_pointer_constraints_v1::ZwpPointerConstraintsV1Global,
        zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1Global,
        zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1Global,
        zwp_text_input_manager_v3::ZwpTextInputManagerV3Global,
        zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1Global,
        zxdg_decoration_manager_v1::ZxdgDecorationManagerV1Global,
        zxdg_output_manager_v1::ZxdgOutputManagerV1Global,
    },
};

use std::collections::{BTreeMap, BTreeSet};

pub struct Globals {
    pub map: Box<IndexMap<GlobalSlotId, Box<dyn Global>>>,
    current_name: u32,
    names_by_slot: BTreeMap<GlobalSlotId, GlobalName>,
    slots_by_name: BTreeMap<GlobalName, GlobalSlotId>,
    /// Globals that have sent `wl_registry.global_remove` and stay bindable until every
    /// registry that was offered them acknowledges removal or is destroyed.
    removed: BTreeSet<GlobalName>,
    /// Registries that were sent `wl_registry.global` for a name. Matches libwayland's
    /// per-registry offer list: a client that never calls `ack_global_remove` pins the
    /// global until that registry object is destroyed.
    offers: BTreeMap<GlobalName, BTreeSet<Resolved<WlRegistryId>>>,
}

index_map_id!(GlobalSlotId);

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlobalName(u32);

impl GlobalName {
    pub fn from_raw(name: u32) -> Self {
        Self(name)
    }

    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalRemoveAckError {
    /// This registry was never told about the name.
    Unknown,
    /// The name exists but `wl_registry.global_remove` has not been sent.
    NotRemoved,
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
            removed: BTreeSet::new(),
            offers: BTreeMap::new(),
        }
    }

    pub fn initialize(&mut self) {
        for fun in [
            WlCompositorGlobal::new,
            WlDataDeviceManagerGlobal::new,
            ZwlrDataControlManagerV1Global::new,
            ExtDataControlManagerV1Global::new,
            ZwpPrimarySelectionDeviceManagerV1Global::new,
            WlFixesGlobal::new,
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
            ExtSessionLockManagerV1Global::new,
            ZwlrScreencopyManagerV1Global::new,
            ExtImageCopyCaptureManagerV1Global::new,
            ExtOutputImageCaptureSourceManagerV1Global::new,
            ExtForeignToplevelImageCaptureSourceManagerV1Global::new,
            ExtForeignToplevelListV1Global::new,
            ZwlrForeignToplevelManagerV1Global::new,
            ZwpIdleInhibitManagerV1Global::new,
            ZwpInputMethodManagerV2Global::new,
            ZwpTextInputManagerV3Global::new,
            ZwpVirtualKeyboardManagerV1Global::new,
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
        self.map.iter().filter_map(|(slot_id, global)| {
            let name = *self
                .names_by_slot
                .get(&slot_id)
                .expect("missing global wire name");
            (!self.removed.contains(&name)).then_some((name, global))
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
        self.removed.remove(&name);
        self.offers.remove(&name);
        self.map.remove(slot_id)
    }

    /// Stop advertising `name` but keep it bindable until every registry that saw it
    /// acknowledges the removal or is destroyed. Returns the global only when nothing
    /// is left to wait for.
    pub fn withdraw(&mut self, name: GlobalName) -> Option<Box<dyn Global>> {
        if !self.slots_by_name.contains_key(&name) || !self.removed.insert(name) {
            return None;
        }
        self.finish_if_withdrawn(name)
    }

    pub fn note_offer(&mut self, name: GlobalName, registry: Resolved<WlRegistryId>) {
        if self.removed.contains(&name) || !self.slots_by_name.contains_key(&name) {
            return;
        }
        self.offers.entry(name).or_default().insert(registry);
    }

    pub fn was_offered(&self, name: GlobalName, registry: Resolved<WlRegistryId>) -> bool {
        self.offers
            .get(&name)
            .is_some_and(|registries| registries.contains(&registry))
    }

    pub fn ack_global_remove(
        &mut self,
        name: GlobalName,
        registry: Resolved<WlRegistryId>,
    ) -> Result<Option<Box<dyn Global>>, GlobalRemoveAckError> {
        if !self.was_offered(name, registry) {
            return Err(GlobalRemoveAckError::Unknown);
        }
        if !self.removed.contains(&name) {
            return Err(GlobalRemoveAckError::NotRemoved);
        }
        self.take_offer(name, registry);
        Ok(self.finish_if_withdrawn(name))
    }

    pub fn release_registry(&mut self, registry: Resolved<WlRegistryId>) {
        let names: Vec<GlobalName> = self.offers.keys().copied().collect();
        for name in names {
            if self.take_offer(name, registry) {
                let _ = self.finish_if_withdrawn(name);
            }
        }
    }

    fn take_offer(&mut self, name: GlobalName, registry: Resolved<WlRegistryId>) -> bool {
        let Some(registries) = self.offers.get_mut(&name) else {
            return false;
        };
        let removed = registries.remove(&registry);
        if registries.is_empty() {
            self.offers.remove(&name);
        }
        removed
    }

    fn finish_if_withdrawn(&mut self, name: GlobalName) -> Option<Box<dyn Global>> {
        if !self.removed.contains(&name) || self.offers.contains_key(&name) {
            return None;
        }
        self.remove(name)
    }

    pub fn contains_interface(&self, interface: &str) -> bool {
        self.iter()
            .any(|(_, global)| global.interface().0 == interface)
    }

    pub fn name_for_interface(&self, interface: &str) -> Option<GlobalName> {
        self.iter()
            .find_map(|(name, global)| (global.interface().0 == interface).then_some(name))
    }

    /// Take a global out of the registry for binding. Fails if `name` is not a live global slot.
    pub fn try_grab_global(&mut self, name: u32) -> Option<Grab<GlobalSlotId, Box<dyn Global>>> {
        let slot_id = *self.slots_by_name.get(&GlobalName(name))?;
        self.map.try_grab(slot_id)
    }
}
