use std::{
    collections::BTreeMap,
    os::{
        fd::{AsRawFd, OwnedFd, RawFd},
        unix::prelude::OsStrExt,
    },
    path::PathBuf,
};

use nix::{
    fcntl::{
        self,
        FcntlArg::{self, F_GETFL},
        OFlag,
    },
    sys::stat::{major, minor},
};

use crate::{
    clients::ClientManagerAction,
    dbus::DbusAction,
    drm::{discard_backend_presentation_feedbacks, sync_output_nodes_for_backend, Drm, DrmReader},
    globals::Global,
    io_uring::{
        model::IoUringAction,
        ring::{PollCompleted, RingId},
    },
    libinput::LibinputAction,
    runner::{Dispatcher, Model, Redispatcher},
    state::{BackendOutputRef, DrmId, Output, OutputId, State},
    wayland_objects::{
        hyprland_ctm_control_manager_v1::HyprlandCtmControlManagerV1Global,
        wl_output::WlOutputGlobal,
        wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1Global,
        zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1Global,
    },
};

#[derive(Debug)]
pub struct Device {
    pub devnum: libc::dev_t,
    pub path_node: PathBuf,
    pub fd: RawFd,
}

impl Device {
    pub fn major(&self) -> u32 {
        major(self.devnum).try_into().unwrap()
    }

    pub fn minor(&self) -> u32 {
        minor(self.devnum).try_into().unwrap()
    }
}

enum PendingControl {
    Drm(udev::Device),
    Input(udev::Device),
}

pub struct Udev {
    udev_monitor: udev::MonitorSocket,
    pending: BTreeMap<libc::dev_t, PendingControl>,
    poll_op_id: Option<RingId>,
}

pub enum UdevAction {
    Start,
    Event,
    DeviceTaken { fd: OwnedFd, inactive: bool, devnum: libc::dev_t },
    FinalizeBackendRemoval { drm_id: DrmId },
    Stop,
}

impl Model for Udev {
    type Action = UdevAction;
    const START: Option<Self::Action> = Some(Self::Action::Start);
    const STOP: Option<Self::Action> = Some(Self::Action::Stop);

    fn process(
        &mut self,
        state: &mut State,
        action: Self::Action,
        dispatcher: &mut Dispatcher,
    ) -> Result<(), ()> {
        match action {
            UdevAction::Start => {
                self.dispatch_poll(dispatcher);
                self.enumerate(dispatcher).expect("Failed to enumerate");
            }
            UdevAction::Event => {
                // TODO: Remove `collect()`
                for event in self.udev_monitor.iter().collect::<Vec<_>>() {
                    self.handle_event(event, state, dispatcher)
                }
            }
            UdevAction::DeviceTaken { fd, inactive, devnum } => {
                match self.pending.remove(&devnum).unwrap() {
                    PendingControl::Drm(device) => {
                        self.on_drm_taken(device, fd, inactive, state, dispatcher)
                    }
                    PendingControl::Input(device) => {
                        self.on_input_taken(device, fd, inactive, devnum, dispatcher)
                    }
                }
                // self.on_taken(device, fd, inactive, devnum);
                // let Some(path_node) = device.devnode().map(PathBuf::from) else {
                //     return Ok(());
                // };
                // log::warn!("DEVICE TAKEN devum={:?} fd={:?} inactive={:?} {:?}", devnum, fd, inactive, path_node);
            }
            UdevAction::FinalizeBackendRemoval { drm_id } => {
                let _ = state.drm.remove(drm_id);
            }
            UdevAction::Stop => {
                if let Some(poll_op_id) = self.poll_op_id.take() {
                    dispatcher.dispatch(IoUringAction::PollRemove { target: poll_op_id });
                }
            }
        }
        Ok(())
    }
}

impl Udev {
    pub fn new() -> Result<Box<Udev>, std::io::Error> {
        let udev_monitor = udev::MonitorBuilder::new()?
            .match_subsystem("input")?
            .match_subsystem("drm")?
            .listen()?;
        let pending = BTreeMap::default();

        Ok(Box::new(Udev { udev_monitor, pending, poll_op_id: None }))
    }

    fn dispatch_poll(&mut self, dispatcher: &mut Dispatcher) {
        let poll_op_id = RingId::fresh();
        self.poll_op_id = Some(poll_op_id);
        dispatcher.dispatch(IoUringAction::Poll {
            id: Some(poll_op_id),
            fd: self.udev_monitor.as_raw_fd(),
            dispatch_to: Redispatcher::to::<Self>(|result: std::io::Result<PollCompleted>| {
                let PollCompleted {} = result.unwrap();
                UdevAction::Event
            }),
        });
    }

    fn enumerate(&mut self, dispatcher: &mut Dispatcher) -> std::io::Result<()> {
        let mut enumerator = udev::Enumerator::new()?;
        enumerator.match_subsystem("input")?;
        enumerator.match_subsystem("drm")?;
        for device in enumerator.scan_devices()? {
            self.handle_new_device(device, dispatcher);
        }
        Ok(())
    }

    fn handle_event(&mut self, event: udev::Event, state: &mut State, dispatcher: &mut Dispatcher) {
        use udev::EventType::*;

        match event.event_type() {
            Add => self.handle_new_device(event.device(), dispatcher),
            Change => self.handle_change_device(event.device(), state, dispatcher),
            Remove => self.handle_remove_device(event.device(), state, dispatcher),
            Bind | Unbind | Unknown => log::trace!("Ignored event {:?}", event.action()),
        }
    }

    fn find_drm_id_for_devnum(state: &State, devnum: libc::dev_t) -> Option<DrmId> {
        state
            .drm
            .iter()
            .find_map(|(drm_id, drm)| (drm.devnum() == Some(devnum)).then_some(drm_id))
    }

    fn handle_change_device(
        &mut self,
        device: udev::Device,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) {
        let Some(devnum) = device.devnum() else {
            return;
        };
        let Some(drm_id) = Self::find_drm_id_for_devnum(state, devnum) else {
            log::trace!("Ignored change event for unmanaged DRM device {:?}", device.devnode());
            return;
        };
        sync_output_nodes_for_backend(state, drm_id);
        for output_id in state.output_ids_for_backend(BackendOutputRef::new(drm_id)) {
            dispatcher.dispatch(ClientManagerAction::OutputStateChanged { output_id });
        }
        sync_color_control_globals(state, dispatcher);
    }

    fn handle_remove_device(
        &mut self,
        device: udev::Device,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) {
        let Some(devnum) = device.devnum() else {
            return;
        };
        let Some(drm_id) = Self::find_drm_id_for_devnum(state, devnum) else {
            log::trace!("Ignored remove event for unmanaged DRM device {:?}", device.devnode());
            return;
        };

        let backend = BackendOutputRef::new(drm_id);
        for output_id in state.output_ids_for_backend(backend) {
            self.remove_output_node(output_id, state, dispatcher);
        }

        if let Some(global_name) = state.backend_syncobj_globals.remove(&drm_id) {
            let _ = state.remove_global(global_name);
            dispatcher.dispatch(ClientManagerAction::RegistryGlobalRemoved {
                name: global_name.as_u32(),
            });
        }
        sync_color_control_globals(state, dispatcher);

        let client_ids = state
            .objects
            .iter()
            .filter_map(|(_, object)| {
                object
                    .downcast_ref::<crate::wayland_objects::wl_surface::WlSurface>()
                    .map(|surface| surface.client_id())
            })
            .collect::<std::collections::BTreeSet<_>>();
        for client_id in client_ids {
            dispatcher.dispatch_to(
                client_id,
                crate::clients::ClientAction::DiscardSurfacePresentationFeedbacksForBackend {
                    drm_id,
                },
            );
        }
        discard_backend_presentation_feedbacks(state, drm_id, dispatcher);
        dispatcher.stop_model(drm_id);
        dispatcher.dispatch(UdevAction::FinalizeBackendRemoval { drm_id });
    }

    fn handle_new_device(&mut self, device: udev::Device, dispatcher: &mut Dispatcher) {
        let Some(subsystem) = device.subsystem() else {
            return;
        };

        match subsystem.as_bytes() {
            b"input" => self.handle_new_input(device, dispatcher),
            b"drm" => self.handle_new_drm(device, dispatcher),
            _ => {}
        }
    }

    fn handle_new_input(&mut self, device: udev::Device, dispatcher: &mut Dispatcher) {
        if !device.is_initialized() {
            return;
        }
        let Some(devnum) = device.devnum() else {
            return;
        };
        dispatcher.dispatch(DbusAction::TakeDevice { devnum });
        self.pending.insert(devnum, PendingControl::Input(device));
    }

    fn handle_new_drm(&mut self, device: udev::Device, dispatcher: &mut Dispatcher) {
        if !device.is_initialized() {
            return;
        }
        let Some(devnum) = device.devnum() else {
            return;
        };

        // From xf86drm.h
        // #define DRM_DIR_NAME  "/dev/dri"
        // #define DRM_PRIMARY_MINOR_NAME  "card"
        // #define DRM_CONTROL_MINOR_NAME  "controlD"
        // #define DRM_RENDER_MINOR_NAME   "renderD"
        // #define DRM_PROC_NAME "/proc/dri/" /* For backward Linux compatibility */
        dispatcher.dispatch(DbusAction::TakeDevice { devnum });
        self.pending.insert(devnum, PendingControl::Drm(device));
    }

    fn on_input_taken(
        &mut self,
        device: udev::Device,
        fd: OwnedFd,
        inactive: bool,
        devnum: libc::dev_t,
        dispatcher: &mut Dispatcher,
    ) {
        if inactive {
            return;
        }
        let Some(devnode) = device.devnode().and_then(|devnode| devnode.to_str()) else {
            return;
        };
        let devnode = devnode.to_string();
        set_non_blocking(fd.as_raw_fd());

        dispatcher.dispatch(LibinputAction::OpenInput { devnode, fd, devnum });
    }

    fn on_drm_taken(
        &mut self,
        device: udev::Device,
        fd: OwnedFd,
        inactive: bool,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) {
        if inactive {
            return;
        }

        eprintln!("DRM TAKEN {:?}", device.devnode());

        let drm = Drm::create_drm(fd, device, &state.egl);
        let drm_id = state.drm.insert(drm);
        let drm_output_ids = state
            .drm
            .get(drm_id)
            .outputs
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let fd = state.drm.get(drm_id).fd();
        dispatcher.start_model(drm_id, DrmReader::new(drm_id, fd));
        for drm_output_id in drm_output_ids {
            let output =
                Output::new(BackendOutputRef::new(drm_id), drm_output_id, state.drm.get(drm_id));
            let output_id = state.insert_output(output);
            self.place_output_automatically(output_id, state);
            self.publish_output_node(output_id, state, dispatcher);
            let drm_model = state.drm.get(drm_id).make_output_model(drm_id, output_id);
            dispatcher.start_model(output_id, drm_model);
        }
        sync_output_nodes_for_backend(state, drm_id);
        if state.drm.get(drm_id).supports_linux_drm_syncobj() {
            let global = WpLinuxDrmSyncobjManagerV1Global::new(drm_id);
            let (interface, version) = global.interface();
            let name = state.insert_global(global);
            state.backend_syncobj_globals.insert(drm_id, name);
            dispatcher.dispatch(ClientManagerAction::RegistryGlobalAdded {
                name: name.as_u32(),
                interface: interface.to_string(),
                version,
            });
        }
        sync_color_control_globals(state, dispatcher);
    }

    fn publish_output_node(
        &self,
        output_id: OutputId,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) {
        let global = WlOutputGlobal::new(output_id);
        let (interface, version) = global.interface();
        let global_name = state.insert_global(global);
        if let Some(output) = state.outputs.try_get_mut(output_id) {
            output.global_name = Some(global_name);
        }
        dispatcher.dispatch(ClientManagerAction::RegistryGlobalAdded {
            name: global_name.as_u32(),
            interface: interface.to_string(),
            version,
        });
    }

    fn remove_output_node(
        &self,
        output_id: OutputId,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) {
        dispatcher.stop_model(output_id);
        dispatcher.dispatch(ClientManagerAction::OutputRemoved { output_id });
        let global_name = state
            .outputs
            .try_get(output_id)
            .and_then(|output| output.global_name);
        if let Some(global_name) = global_name {
            let _ = state.remove_global(global_name);
            dispatcher.dispatch(ClientManagerAction::RegistryGlobalRemoved {
                name: global_name.as_u32(),
            });
        }
        let _ = state.remove_output(output_id);
    }

    fn place_output_automatically(&self, output_id: OutputId, state: &mut State) {
        let next_x = state
            .outputs
            .iter()
            .filter(|(other_output_id, _)| *other_output_id != output_id)
            .filter_map(|(_, output)| output.logical_bounds().map(|bounds| bounds.x2()))
            .max()
            .unwrap_or(0);
        if let Some(output) = state.outputs.try_get_mut(output_id) {
            output.x = next_x;
            output.y = 0;
        }
    }
}

fn set_non_blocking(fd: RawFd) {
    let flags = fcntl::fcntl(fd, F_GETFL).unwrap();
    let flags = OFlag::from_bits_retain(flags);

    if !flags.contains(OFlag::O_NONBLOCK) {
        fcntl::fcntl(fd, FcntlArg::F_SETFL(OFlag::O_NONBLOCK | flags)).unwrap();
    }
}

fn sync_color_control_global(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    should_exist: bool,
    interface_name: &'static str,
    make_global: fn() -> Box<dyn Global>,
) {
    match (should_exist, state.globals.name_for_interface(interface_name)) {
        (true, None) => {
            let global = make_global();
            let (interface, version) = global.interface();
            let name = state.insert_global(global);
            dispatcher.dispatch(ClientManagerAction::RegistryGlobalAdded {
                name: name.as_u32(),
                interface: interface.to_string(),
                version,
            });
        }
        (false, Some(name)) => {
            let _ = state.remove_global(name);
            dispatcher.dispatch(ClientManagerAction::RegistryGlobalRemoved { name: name.as_u32() });
        }
        _ => {}
    }
}

fn sync_color_control_globals(state: &mut State, dispatcher: &mut Dispatcher) {
    sync_color_control_global(
        state,
        dispatcher,
        state.any_output_supports_gamma_control(),
        "zwlr_gamma_control_manager_v1",
        ZwlrGammaControlManagerV1Global::new,
    );
    sync_color_control_global(
        state,
        dispatcher,
        state.any_output_supports_ctm_control(),
        "hyprland_ctm_control_manager_v1",
        HyprlandCtmControlManagerV1Global::new,
    );
}
