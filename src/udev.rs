use std::{
    collections::{BTreeMap, BTreeSet},
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
    drm::{
        discard_backend_presentation_feedbacks, quiesce_backend, resume_drm_backend,
        sync_output_nodes_for_backend, Drm, DrmReader,
    },
    globals::Global,
    io_uring::{
        model::IoUringAction,
        ring::{PollCompleted, RingId},
    },
    libinput::LibinputAction,
    runner::{
        session_shutdown, Dispatcher, Model, ModelFailure, Redispatcher, SessionFault,
        ShutdownReason,
    },
    seat,
    state::{BackendOutputRef, DrmId, Output, OutputId, State},
    wayland_objects::{
        hyprland_ctm_control_manager_v1::HyprlandCtmControlManagerV1Global,
        wl_output::WlOutputGlobal,
        wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1Global,
        zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1Global,
    },
};

#[derive(Debug, PartialEq, Eq)]
enum Bringup {
    Continue,
    WaitForDevice,
    FatalNoOutput,
}

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
    /// DRM devices from the initial scan that have not finished TakeDevice.
    initial_drm: BTreeSet<libc::dev_t>,
    /// Devnodes kept so a paused input device can be opened again.
    retained_inputs: BTreeMap<libc::dev_t, String>,
    /// TakeDevice results for a session that was not active yet.
    parked: BTreeMap<libc::dev_t, ParkedSessionDevice>,
    /// Resume fds kept until the session is active and the device can be restored.
    resumed_fds: BTreeMap<libc::dev_t, OwnedFd>,
    /// Live or parked DRM devices waiting for a resume fd before they are modeset.
    awaiting_drm_resume: BTreeSet<libc::dev_t>,
    poll_op_id: Option<RingId>,
}

struct ParkedSessionDevice {
    kind: ParkedKind,
    fd: Option<OwnedFd>,
}

enum ParkedKind {
    Input,
    Drm(udev::Device),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevicePauseKind {
    /// Stop using the device, then call PauseDeviceComplete.
    Pause,
    /// Stop using the device immediately. Logind does not wait for an ack.
    Force,
    /// The device was removed. Logind does not wait for an ack.
    Gone,
}

pub enum UdevAction {
    Start,
    Event,
    DeviceTaken {
        fd: OwnedFd,
        inactive: bool,
        devnum: libc::dev_t,
    },
    DeviceTakeFailed {
        devnum: libc::dev_t,
    },
    PollFailed {
        message: String,
    },
    FinalizeBackendRemoval {
        drm_id: DrmId,
    },
    /// Logind asked this session to stop using `devnum`.
    PauseDevice {
        devnum: libc::dev_t,
        kind: DevicePauseKind,
    },
    /// Logind handed back a device fd. The fd is owned by this action exactly once.
    ResumeDevice {
        devnum: libc::dev_t,
        fd: OwnedFd,
    },
    /// The session is active, so parked input devices may be opened.
    SessionActive,
    /// path_add_device failed. Drop this input device and keep the session.
    InputLost {
        devnum: libc::dev_t,
    },
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
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        match action {
            UdevAction::Start => {
                self.dispatch_poll(dispatcher);
                if let Err(error) = self.enumerate(dispatcher) {
                    return Err(ModelFailure::Shutdown(ShutdownReason::Fatal {
                        source: "udev",
                        message: format!("device enumeration failed: {error}"),
                    }));
                }
                self.shutdown_if_no_output()?;
            }
            UdevAction::Event => {
                for event in self.udev_monitor.iter().collect::<Vec<_>>() {
                    self.handle_event(event, state, dispatcher)?;
                }
            }
            UdevAction::DeviceTaken { fd, inactive, devnum } => {
                let Some(pending) = self.pending.remove(&devnum) else {
                    log::warn!("TakeDevice reply for an unknown device {devnum}");
                    return Ok(());
                };
                match pending {
                    PendingControl::Drm(device) => {
                        self.on_drm_taken(device, fd, inactive, false, devnum, state, dispatcher)?
                    }
                    PendingControl::Input(device) => {
                        self.on_input_taken(device, fd, inactive, devnum, dispatcher)
                    }
                }
            }
            UdevAction::DeviceTakeFailed { devnum } => {
                self.pending.remove(&devnum);
                self.settle_initial_drm(devnum, state)?;
            }
            UdevAction::PollFailed { message } => {
                return Err(ModelFailure::Shutdown(ShutdownReason::Fatal {
                    source: "udev",
                    message: format!("monitor poll failed: {message}"),
                }));
            }
            UdevAction::FinalizeBackendRemoval { drm_id } => {
                let _ = state.drm.remove(drm_id);
            }
            UdevAction::PauseDevice { devnum, kind } => {
                self.resumed_fds.remove(&devnum);
                match kind {
                    DevicePauseKind::Gone => {
                        self.pending.remove(&devnum);
                        self.retained_inputs.remove(&devnum);
                        self.parked.remove(&devnum);
                        self.awaiting_drm_resume.remove(&devnum);
                        dispatcher.dispatch(LibinputAction::Quiesce { devnum: devnum as u64 });
                        if let Some(drm_id) = Self::find_drm_id_for_devnum(state, devnum) {
                            self.remove_drm_backend(drm_id, state, dispatcher);
                            self.fatal_if_no_output(state)?;
                        }
                    }
                    DevicePauseKind::Pause | DevicePauseKind::Force => {
                        let drm_id = Self::find_drm_id_for_devnum(state, devnum);
                        let parked_drm = self
                            .parked
                            .get(&devnum)
                            .is_some_and(|parked| matches!(parked.kind, ParkedKind::Drm(_)));
                        if let Some(drm_id) = drm_id {
                            log::info!(
                                "pausing primary DRM device {devnum}: stopping KMS submissions; allocator {} stays open",
                                state.drm.get(drm_id).describe_allocator()
                            );
                        } else if parked_drm {
                            log::info!(
                                "pausing parked primary DRM device {devnum}; its allocator is not open"
                            );
                        }
                        self.quiesce_device(devnum, state, dispatcher);
                        if drm_id.is_some() || parked_drm {
                            self.awaiting_drm_resume.insert(devnum);
                        }
                        dispatcher.dispatch(DbusAction::DevicePauseQuiesced { devnum });
                    }
                }
            }
            UdevAction::ResumeDevice { devnum, fd } => {
                if dispatcher.shutdown_requested() {
                    log::info!(
                        "cancelling resume of device {devnum} because shutdown already started"
                    );
                    drop(fd);
                    return Ok(());
                }
                self.resumed_fds.insert(devnum, fd);
                if state.session_active {
                    if self.retained_inputs.contains_key(&devnum) {
                        self.restore_ready_input(devnum, dispatcher);
                    } else {
                        self.restore_ready_drm(devnum, state, dispatcher)?;
                    }
                }
            }
            UdevAction::SessionActive => {
                if state.session_active && !dispatcher.shutdown_requested() {
                    let devnums: Vec<_> = self.retained_inputs.keys().copied().collect();
                    for devnum in devnums {
                        self.restore_ready_input(devnum, dispatcher);
                    }
                    let mut drm_devnums: Vec<_> = self
                        .parked
                        .iter()
                        .filter(|(_, parked)| matches!(parked.kind, ParkedKind::Drm(_)))
                        .map(|(devnum, _)| *devnum)
                        .collect();
                    for (_, drm) in state.drm.iter() {
                        if let Some(devnum) = drm.devnum() {
                            if self.awaiting_drm_resume.contains(&devnum)
                                || self.resumed_fds.contains_key(&devnum)
                            {
                                drm_devnums.push(devnum);
                            }
                        }
                    }
                    drm_devnums.sort_unstable();
                    drm_devnums.dedup();
                    for devnum in drm_devnums {
                        self.restore_ready_drm(devnum, state, dispatcher)?;
                    }
                    self.confirm_ready_outputs(state, dispatcher);
                }
            }
            UdevAction::InputLost { devnum } => {
                self.retained_inputs.remove(&devnum);
                self.resumed_fds.remove(&devnum);
                if self
                    .parked
                    .get(&devnum)
                    .is_some_and(|parked| matches!(parked.kind, ParkedKind::Input))
                {
                    self.parked.remove(&devnum);
                }
            }
            UdevAction::Stop => self.shutdown_backends(state, dispatcher),
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

        Ok(Box::new(Udev {
            udev_monitor,
            pending,
            initial_drm: BTreeSet::new(),
            retained_inputs: BTreeMap::new(),
            parked: BTreeMap::new(),
            resumed_fds: BTreeMap::new(),
            awaiting_drm_resume: BTreeSet::new(),
            poll_op_id: None,
        }))
    }

    fn dispatch_poll(&mut self, dispatcher: &mut Dispatcher) {
        let poll_op_id = RingId::fresh();
        self.poll_op_id = Some(poll_op_id);
        dispatcher.dispatch(IoUringAction::Poll {
            id: Some(poll_op_id),
            fd: self.udev_monitor.as_raw_fd(),
            dispatch_to: Redispatcher::to::<Self>(|result: std::io::Result<PollCompleted>| {
                match result {
                    Ok(PollCompleted {}) => UdevAction::Event,
                    Err(error) => UdevAction::PollFailed { message: error.to_string() },
                }
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
        self.initial_drm = self
            .pending
            .iter()
            .filter_map(|(devnum, pending)| {
                matches!(pending, PendingControl::Drm(_)).then_some(*devnum)
            })
            .collect();
        Ok(())
    }

    fn handle_event(
        &mut self,
        event: udev::Event,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), ModelFailure> {
        use udev::EventType::*;

        match event.event_type() {
            Add => self.handle_new_device(event.device(), dispatcher),
            Change => self.handle_change_device(event.device(), state, dispatcher),
            Remove => return self.handle_remove_device(event.device(), state, dispatcher),
            Bind | Unbind | Unknown => log::trace!("Ignored event {:?}", event.action()),
        }
        Ok(())
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
            dispatcher.dispatch_to(
                crate::scheduler::output_scheduler_id(output_id),
                crate::scheduler::OutputSchedulerAction::ModeUpdated,
            );
            dispatcher.dispatch(ClientManagerAction::OutputStateChanged { output_id });
        }
        sync_color_control_globals(state, dispatcher);
    }

    fn handle_remove_device(
        &mut self,
        device: udev::Device,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), ModelFailure> {
        let Some(devnum) = device.devnum() else {
            return Ok(());
        };
        let Some(drm_id) = Self::find_drm_id_for_devnum(state, devnum) else {
            log::trace!("Ignored remove event for unmanaged DRM device {:?}", device.devnode());
            return Ok(());
        };
        self.remove_drm_backend(drm_id, state, dispatcher);
        self.fatal_if_no_output(state)
    }

    fn shutdown_backends(&mut self, state: &mut State, dispatcher: &mut Dispatcher) {
        if let Some(poll_op_id) = self.poll_op_id.take() {
            dispatcher.dispatch(IoUringAction::PollRemove { target: poll_op_id });
        }
        let drm_ids: Vec<_> = state.drm.iter().map(|(drm_id, _)| drm_id).collect();
        for drm_id in drm_ids {
            self.remove_drm_backend(drm_id, state, dispatcher);
        }
    }

    /// Disable the backend, then follow the hot-remove order: stop schedulers,
    /// discard presentation feedback, remove globals, stop output and reader
    /// models, release the logind device, and drop the backend.
    fn remove_drm_backend(&self, drm_id: DrmId, state: &mut State, dispatcher: &mut Dispatcher) {
        if state.drm.try_get(drm_id).is_none() {
            return;
        }
        let devnum = state.drm.try_get(drm_id).and_then(|drm| drm.devnum());
        if let Some(drm) = state.drm.try_get_mut(drm_id) {
            drm.disable_outputs();
        }

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

        dispatcher.dispatch(ClientManagerAction::DiscardSurfacePresentationFeedbacksForBackend {
            drm_id,
        });
        discard_backend_presentation_feedbacks(state, drm_id, dispatcher);
        dispatcher.stop_model(drm_id);
        if let Some(devnum) = devnum {
            log::info!(
                "releasing logind primary device {devnum}; the render-node allocator closes with the backend"
            );
            dispatcher.dispatch(DbusAction::ReleaseDevice { devnum });
        }
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

        let Some(sysname) = device.sysname().to_str() else {
            log::debug!("ignoring DRM device {devnum}: sysname is not UTF-8");
            return;
        };
        if !crate::drm::node::logind_takes_drm_node(sysname) {
            log::debug!(
                "ignoring DRM node {sysname} ({devnum}); logind TakeDevice is limited to cardN"
            );
            return;
        }
        log::info!("requesting logind TakeDevice for primary DRM node {sysname} ({devnum})");
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
        let Some(devnode) = device.devnode().and_then(|devnode| devnode.to_str()) else {
            log::warn!("input device {devnum} has no devnode");
            self.parked
                .insert(devnum, ParkedSessionDevice { kind: ParkedKind::Input, fd: Some(fd) });
            return;
        };
        let devnode = devnode.to_string();
        self.retained_inputs.insert(devnum, devnode.clone());
        if inactive {
            log::info!("parking inactive input device {devnode}");
            self.parked
                .insert(devnum, ParkedSessionDevice { kind: ParkedKind::Input, fd: Some(fd) });
            return;
        }
        set_non_blocking(fd.as_raw_fd());
        dispatcher.dispatch(LibinputAction::OpenInput { devnode, fd, devnum: devnum as u64 });
    }

    /// Stop using one device before logind is told the pause is complete.
    fn quiesce_device(
        &mut self,
        devnum: libc::dev_t,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) {
        if let Some(parked) = self.parked.get_mut(&devnum) {
            parked.fd.take();
        }
        if self.retained_inputs.contains_key(&devnum) {
            dispatcher.dispatch(LibinputAction::Quiesce { devnum: devnum as u64 });
        }
        if let Some(drm_id) = Self::find_drm_id_for_devnum(state, devnum) {
            quiesce_backend(state, dispatcher, drm_id);
            for output_id in state.output_ids_for_backend(BackendOutputRef::new(drm_id)) {
                dispatcher.dispatch_to(
                    crate::scheduler::output_scheduler_id(output_id),
                    crate::scheduler::OutputSchedulerAction::Disarm,
                );
            }
        }
        seat::quiesce_for_session_pause(state, dispatcher);
    }

    /// Open one retained input device with its newest session fd.
    fn restore_ready_input(&mut self, devnum: libc::dev_t, dispatcher: &mut Dispatcher) {
        let Some(devnode) = self.retained_inputs.get(&devnum).cloned() else {
            return;
        };
        let Some(fd) = self.take_input_fd(devnum) else {
            return;
        };
        set_non_blocking(fd.as_raw_fd());
        dispatcher.dispatch(LibinputAction::Quiesce { devnum: devnum as u64 });
        dispatcher.dispatch(LibinputAction::OpenInput { devnode, fd, devnum: devnum as u64 });
    }

    /// ResumeDevice replaces a parked TakeDevice fd. The returned fd is consumed once.
    fn take_input_fd(&mut self, devnum: libc::dev_t) -> Option<OwnedFd> {
        let resumed = self.resumed_fds.remove(&devnum);
        let parked = self.parked.get_mut(&devnum).and_then(|parked| {
            matches!(parked.kind, ParkedKind::Input)
                .then(|| parked.fd.take())
                .flatten()
        });
        match (resumed, parked) {
            (Some(resumed), parked) => {
                drop(parked);
                Some(resumed)
            }
            (None, parked) => parked,
        }
    }

    /// Modeset a live DRM backend, or open a parked one, once a resume fd is available.
    fn restore_ready_drm(
        &mut self,
        devnum: libc::dev_t,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), ModelFailure> {
        if let Some(drm_id) = Self::find_drm_id_for_devnum(state, devnum) {
            let Some(fd) = self.resumed_fds.remove(&devnum) else {
                self.awaiting_drm_resume.insert(devnum);
                return Ok(());
            };
            self.awaiting_drm_resume.remove(&devnum);
            {
                // The resume fd is the temporary master. Closing it, then
                // SET_MASTER on the original primary fd, leaves the renderer and
                // allocator file description untouched.
                let drm = state.drm.get(drm_id);
                let allocator = drm.describe_allocator();
                drop(fd);
                if let Err(error) = drm.reclaim_master() {
                    log::warn!(
                        "primary DRM device {devnum} kept its fd, setmaster failed: {error}; allocator {allocator} remains open"
                    );
                } else {
                    log::info!(
                        "resumed primary DRM device {devnum}: reclaimed master on the primary fd; allocator {allocator} remains open"
                    );
                }
            }
            let failed = resume_drm_backend(state, dispatcher, drm_id);
            for crtc_id in failed {
                if let Some(output_id) = state.output_id_for_drm_crtc(drm_id, crtc_id) {
                    self.remove_output_node(output_id, state, dispatcher);
                }
            }
            if state
                .output_ids_for_backend(BackendOutputRef::new(drm_id))
                .is_empty()
            {
                self.remove_drm_backend(drm_id, state, dispatcher);
            }
            self.confirm_ready_outputs(state, dispatcher);
            return self.finish_drm_restore(state);
        }

        let parked_drm = self
            .parked
            .get(&devnum)
            .is_some_and(|parked| matches!(parked.kind, ParkedKind::Drm(_)));
        if !parked_drm {
            return Ok(());
        }
        let Some(fd) = self.take_drm_fd(devnum) else {
            self.awaiting_drm_resume.insert(devnum);
            return Ok(());
        };
        let Some(parked) = self.parked.remove(&devnum) else {
            return Ok(());
        };
        let ParkedKind::Drm(device) = parked.kind else {
            return Ok(());
        };
        self.awaiting_drm_resume.remove(&devnum);
        self.on_drm_taken(device, fd, false, true, devnum, state, dispatcher)?;
        self.confirm_ready_outputs(state, dispatcher);
        self.finish_drm_restore(state)
    }

    /// Let schedulers paint outputs whose DRM device is no longer waiting for a resume fd.
    fn confirm_ready_outputs(&self, state: &State, dispatcher: &mut Dispatcher) {
        for (output_id, output) in state.outputs.iter() {
            let waiting = state
                .drm
                .try_get(output.backend.drm_id)
                .and_then(|drm| drm.devnum())
                .is_some_and(|devnum| self.awaiting_drm_resume.contains(&devnum));
            if waiting {
                continue;
            }
            dispatcher.dispatch_to(
                crate::scheduler::output_scheduler_id(output_id),
                crate::scheduler::OutputSchedulerAction::Restored,
            );
        }
    }

    fn take_drm_fd(&mut self, devnum: libc::dev_t) -> Option<OwnedFd> {
        let resumed = self.resumed_fds.remove(&devnum);
        let parked = self.parked.get_mut(&devnum).and_then(|parked| {
            matches!(parked.kind, ParkedKind::Drm(_))
                .then(|| parked.fd.take())
                .flatten()
        });
        match (resumed, parked) {
            (Some(resumed), parked) => {
                drop(parked);
                Some(resumed)
            }
            (None, parked) => parked,
        }
    }

    fn finish_drm_restore(&self, state: &State) -> std::result::Result<(), ModelFailure> {
        if self.drm_restore_pending() {
            Ok(())
        } else {
            self.fatal_if_no_output(state)
        }
    }

    fn drm_restore_pending(&self) -> bool {
        !self.initial_drm.is_empty()
            || !self.awaiting_drm_resume.is_empty()
            || self
                .parked
                .values()
                .any(|parked| matches!(parked.kind, ParkedKind::Drm(_)))
    }

    fn on_drm_taken(
        &mut self,
        device: udev::Device,
        fd: OwnedFd,
        inactive: bool,
        from_resume: bool,
        devnum: libc::dev_t,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), ModelFailure> {
        if inactive {
            log::info!("parking inactive DRM device {devnum}");
            self.parked.insert(
                devnum,
                ParkedSessionDevice { kind: ParkedKind::Drm(device), fd: Some(fd) },
            );
            return Ok(());
        }

        eprintln!("DRM TAKEN {:?}", device.devnode());

        let drm = match Drm::create_drm(fd, device, state.config.general.renderer) {
            Ok(drm) => drm,
            Err(error) => {
                if from_resume {
                    crate::runner::isolate_session_fault(
                        crate::runner::SessionFault::UnusableResumeFd,
                        format!("DRM device {devnum} failed: {error}"),
                    );
                } else {
                    log::error!("DRM device {devnum} failed: {error}");
                }
                dispatcher.dispatch(DbusAction::ReleaseDevice { devnum });
                return self.settle_initial_drm(devnum, state);
            }
        };
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
            let output_scheduler = state
                .drm
                .get(drm_id)
                .make_output_scheduler(drm_id, output_id);
            let drm_model = state.drm.get(drm_id).make_output_model(drm_id, output_id);
            dispatcher
                .start_model(crate::scheduler::output_scheduler_id(output_id), output_scheduler);
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
        self.settle_initial_drm(devnum, state)
    }

    /// One failed connector is skipped. The session ends only when every
    /// initial DRM device has settled and none of them produced an output.
    fn bringup_decision(ready_outputs: usize, pending_devices: usize) -> Bringup {
        if ready_outputs > 0 {
            Bringup::Continue
        } else if pending_devices > 0 {
            Bringup::WaitForDevice
        } else {
            Bringup::FatalNoOutput
        }
    }

    fn shutdown_if_no_output(&self) -> std::result::Result<(), ModelFailure> {
        Self::fatal_when(Self::bringup_decision(0, self.initial_drm.len()))
    }

    fn settle_initial_drm(
        &mut self,
        devnum: libc::dev_t,
        state: &State,
    ) -> std::result::Result<(), ModelFailure> {
        if !self.initial_drm.remove(&devnum) {
            return Ok(());
        }
        self.fatal_if_no_output(state)
    }

    fn fatal_if_no_output(&self, state: &State) -> std::result::Result<(), ModelFailure> {
        let pending = usize::from(self.drm_restore_pending());
        Self::fatal_when(Self::bringup_decision(state.outputs.iter().count(), pending))
    }

    fn fatal_when(decision: Bringup) -> std::result::Result<(), ModelFailure> {
        match decision {
            Bringup::FatalNoOutput => Err(ModelFailure::Shutdown(session_shutdown(
                SessionFault::NoUsableOutput,
                "no usable output",
            ))),
            Bringup::Continue | Bringup::WaitForDevice => Ok(()),
        }
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
        dispatcher.stop_model(crate::scheduler::output_scheduler_id(output_id));
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
        let _ = state.remove_output(output_id, dispatcher);
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

#[cfg(test)]
impl Udev {
    pub(crate) fn test_retain_input(&mut self, devnum: libc::dev_t, devnode: impl Into<String>) {
        self.retained_inputs.insert(devnum, devnode.into());
    }

    pub(crate) fn test_has_retained_input(&self, devnum: libc::dev_t) -> bool {
        self.retained_inputs.contains_key(&devnum)
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

#[cfg(test)]
mod tests {
    use super::{Bringup, Udev};

    #[test]
    fn bringup_keeps_a_session_that_has_one_working_output() {
        assert_eq!(Udev::bringup_decision(1, 0), Bringup::Continue);
        assert_eq!(Udev::bringup_decision(1, 3), Bringup::Continue);
    }

    #[test]
    fn bringup_waits_while_an_initial_device_is_still_pending() {
        assert_eq!(Udev::bringup_decision(0, 2), Bringup::WaitForDevice);
    }

    #[test]
    fn bringup_is_fatal_when_every_initial_device_failed() {
        assert_eq!(Udev::bringup_decision(0, 0), Bringup::FatalNoOutput);
    }

    #[test]
    fn a_monitor_poll_failure_shuts_the_session_down() {
        use crate::{
            runner::{Model, ModelFailure, ShutdownReason},
            test_support::state_for_tests,
        };

        let mut udev = Udev::new().expect("udev monitor");
        let mut state = state_for_tests();
        let mut dispatcher = crate::runner::Dispatcher::default();
        let error = udev
            .process(
                &mut state,
                super::UdevAction::PollFailed { message: "monitor closed".into() },
                &mut dispatcher,
            )
            .expect_err("poll failure is fatal");
        assert!(matches!(
            error,
            ModelFailure::Shutdown(ShutdownReason::Fatal { source: "udev", .. })
        ));
    }

    #[test]
    fn the_last_failed_device_with_no_output_is_fatal() {
        use crate::{
            runner::{Model, ModelFailure, ShutdownReason},
            test_support::state_for_tests,
        };

        let mut udev = Udev::new().expect("udev monitor");
        udev.initial_drm.insert(7);
        udev.initial_drm.insert(8);
        let mut state = state_for_tests();
        let mut dispatcher = crate::runner::Dispatcher::default();
        udev.process(
            &mut state,
            super::UdevAction::DeviceTakeFailed { devnum: 7 },
            &mut dispatcher,
        )
        .expect("another initial device is still pending");
        let error = udev
            .process(&mut state, super::UdevAction::DeviceTakeFailed { devnum: 8 }, &mut dispatcher)
            .expect_err("no output remains");
        assert!(matches!(
            error,
            ModelFailure::Shutdown(ShutdownReason::Fatal { source: "drm", .. })
        ));
    }

    #[test]
    fn pause_confirms_once_per_request_and_resume_keeps_the_latest_fd() {
        use std::os::fd::AsRawFd;

        use crate::{dbus::DbusAction, runner::Model, test_support::state_for_tests};

        let mut udev = Udev::new().expect("udev monitor");
        let mut state = state_for_tests();
        let mut dispatcher = crate::runner::Dispatcher::default();
        udev.process(
            &mut state,
            super::UdevAction::PauseDevice { devnum: 4, kind: super::DevicePauseKind::Pause },
            &mut dispatcher,
        )
        .expect("pause");
        assert_eq!(
            dispatcher.test_count_queued_action(|action: &DbusAction| {
                matches!(action, DbusAction::DevicePauseQuiesced { devnum: 4 })
            }),
            1
        );

        let first = std::fs::File::open("/dev/null").unwrap().into();
        let second = std::fs::File::open("/dev/null").unwrap().into();
        let second_raw = AsRawFd::as_raw_fd(&second);
        udev.process(
            &mut state,
            super::UdevAction::ResumeDevice { devnum: 4, fd: first },
            &mut dispatcher,
        )
        .expect("first resume");
        udev.process(
            &mut state,
            super::UdevAction::ResumeDevice { devnum: 4, fd: second },
            &mut dispatcher,
        )
        .expect("second resume");
        assert_eq!(udev.resumed_fds.get(&4).unwrap().as_raw_fd(), second_raw);

        udev.process(
            &mut state,
            super::UdevAction::PauseDevice { devnum: 9, kind: super::DevicePauseKind::Gone },
            &mut dispatcher,
        )
        .expect("gone without a backend");
        assert!(!dispatcher.shutdown_requested());
        assert!(udev.resumed_fds.get(&4).is_some());
    }

    #[test]
    fn an_inactive_take_is_parked_until_pause_quiesces_it() {
        use std::os::fd::AsRawFd;

        use crate::{dbus::DbusAction, libinput::LibinputAction, runner::Model};

        let input = first_device("input");
        let drm = first_device("drm");
        let input_devnum = input.devnum().expect("input devnum");
        let drm_devnum = drm.devnum().expect("drm devnum");
        let mut udev = Udev::new().expect("udev monitor");
        udev.pending
            .insert(input_devnum, super::PendingControl::Input(input));
        udev.pending
            .insert(drm_devnum, super::PendingControl::Drm(drm));
        udev.initial_drm.insert(drm_devnum);

        let mut state = crate::test_support::state_for_tests();
        let mut dispatcher = crate::runner::Dispatcher::default();
        let input_fd: std::os::fd::OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        let input_raw = input_fd.as_raw_fd();
        udev.process(
            &mut state,
            super::UdevAction::DeviceTaken { fd: input_fd, inactive: true, devnum: input_devnum },
            &mut dispatcher,
        )
        .expect("park input");
        let drm_fd: std::os::fd::OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        let drm_raw = drm_fd.as_raw_fd();
        udev.process(
            &mut state,
            super::UdevAction::DeviceTaken { fd: drm_fd, inactive: true, devnum: drm_devnum },
            &mut dispatcher,
        )
        .expect("park drm");

        assert!(udev.initial_drm.contains(&drm_devnum));
        assert!(!dispatcher.shutdown_requested());
        assert!(!dispatcher.test_has_queued_action(|action: &DbusAction| {
            matches!(action, DbusAction::ReleaseDevice { .. })
        }));
        assert!(!dispatcher.test_has_queued_action(|action: &LibinputAction| {
            matches!(action, LibinputAction::OpenInput { .. })
        }));
        assert_eq!(
            udev.parked
                .get(&input_devnum)
                .unwrap()
                .fd
                .as_ref()
                .unwrap()
                .as_raw_fd(),
            input_raw
        );
        assert_eq!(
            udev.parked
                .get(&drm_devnum)
                .unwrap()
                .fd
                .as_ref()
                .unwrap()
                .as_raw_fd(),
            drm_raw
        );
        assert!(udev.retained_inputs.contains_key(&input_devnum));

        state.keyboard.update_key(30, true);
        udev.process(
            &mut state,
            super::UdevAction::PauseDevice {
                devnum: input_devnum,
                kind: super::DevicePauseKind::Pause,
            },
            &mut dispatcher,
        )
        .expect("pause input");
        assert!(udev.parked.get(&input_devnum).unwrap().fd.is_none());
        assert!(udev.retained_inputs.contains_key(&input_devnum));
        assert!(state.keyboard.pressed_keys().0.is_empty());
        assert!(dispatcher.test_has_queued_action(|action: &LibinputAction| {
            matches!(action, LibinputAction::Quiesce { devnum } if *devnum == input_devnum as u64)
        }));
        assert!(dispatcher.test_has_queued_action(|action: &DbusAction| {
            matches!(action, DbusAction::DevicePauseQuiesced { devnum } if *devnum == input_devnum)
        }));
        assert!(matches!(udev.parked.get(&drm_devnum).unwrap().kind, super::ParkedKind::Drm(_)));
    }

    #[test]
    fn resume_opens_a_retained_input_once_the_session_is_active() {
        use std::os::fd::AsRawFd;

        use crate::{libinput::LibinputAction, runner::Model};

        let mut udev = Udev::new().expect("udev monitor");
        let mut state = crate::test_support::state_for_tests();
        state.session_active = false;
        let mut dispatcher = crate::runner::Dispatcher::default();
        let devnum = 42;
        udev.retained_inputs
            .insert(devnum, "/dev/input/event0".into());

        let first: std::os::fd::OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        let second: std::os::fd::OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        let second_raw = second.as_raw_fd();
        udev.process(
            &mut state,
            super::UdevAction::ResumeDevice { devnum, fd: first },
            &mut dispatcher,
        )
        .expect("resume before active");
        udev.process(
            &mut state,
            super::UdevAction::ResumeDevice { devnum, fd: second },
            &mut dispatcher,
        )
        .expect("duplicate resume");
        assert!(!dispatcher.test_has_queued_action(|action: &LibinputAction| {
            matches!(action, LibinputAction::OpenInput { .. })
        }));
        assert_eq!(udev.resumed_fds.get(&devnum).unwrap().as_raw_fd(), second_raw);
        udev.process(&mut state, super::UdevAction::Stop, &mut dispatcher)
            .expect("stop while the resume fd is still parked");
        assert_eq!(udev.resumed_fds.get(&devnum).unwrap().as_raw_fd(), second_raw);
        assert!(udev.retained_inputs.contains_key(&devnum));

        state.session_active = true;
        udev.process(&mut state, super::UdevAction::SessionActive, &mut dispatcher)
            .expect("active after resume");
        assert!(udev.resumed_fds.get(&devnum).is_none());
        assert_eq!(
            dispatcher.test_count_queued_action(|action: &LibinputAction| {
                matches!(action, LibinputAction::OpenInput { fd, .. } if fd.as_raw_fd() == second_raw)
            }),
            1
        );

        let mut dispatcher = crate::runner::Dispatcher::default();
        state.session_active = true;
        udev.process(&mut state, super::UdevAction::SessionActive, &mut dispatcher)
            .expect("active before the next resume");
        assert!(!dispatcher.test_has_queued_action(|action: &LibinputAction| {
            matches!(action, LibinputAction::OpenInput { .. })
        }));
        let third: std::os::fd::OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        let third_raw = third.as_raw_fd();
        udev.process(
            &mut state,
            super::UdevAction::ResumeDevice { devnum, fd: third },
            &mut dispatcher,
        )
        .expect("resume after active");
        assert!(udev.resumed_fds.get(&devnum).is_none());
        assert!(dispatcher.test_has_queued_action(|action: &LibinputAction| {
            matches!(
                action,
                LibinputAction::OpenInput { fd, .. } if fd.as_raw_fd() == third_raw
            )
        }));

        udev.process(
            &mut state,
            super::UdevAction::PauseDevice { devnum, kind: super::DevicePauseKind::Pause },
            &mut dispatcher,
        )
        .expect("pause again");
        assert!(udev.retained_inputs.contains_key(&devnum));
        let fourth: std::os::fd::OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        let fourth_raw = fourth.as_raw_fd();
        udev.process(
            &mut state,
            super::UdevAction::ResumeDevice { devnum, fd: fourth },
            &mut dispatcher,
        )
        .expect("resume again");
        assert!(dispatcher.test_has_queued_action(|action: &LibinputAction| {
            matches!(action, LibinputAction::OpenInput { fd, .. } if fd.as_raw_fd() == fourth_raw)
        }));
    }

    #[test]
    fn a_drm_resume_fd_stays_stored_until_modeset() {
        use std::os::fd::AsRawFd;

        use crate::{libinput::LibinputAction, runner::Model};

        let mut udev = Udev::new().expect("udev monitor");
        let mut state = crate::test_support::state_for_tests();
        let mut dispatcher = crate::runner::Dispatcher::default();
        let devnum = 7;
        let fd: std::os::fd::OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        let raw = fd.as_raw_fd();
        udev.process(&mut state, super::UdevAction::ResumeDevice { devnum, fd }, &mut dispatcher)
            .expect("drm resume");
        assert_eq!(udev.resumed_fds.get(&devnum).unwrap().as_raw_fd(), raw);
        assert!(!dispatcher.test_has_queued_action(|action: &LibinputAction| {
            matches!(action, LibinputAction::OpenInput { .. })
        }));
    }

    #[test]
    fn a_parked_drm_device_waits_for_a_resume_fd_then_fails_closed() {
        use crate::{
            dbus::DbusAction,
            runner::{Model, ModelFailure, ShutdownReason},
        };

        let drm = first_device("drm");
        let devnum = drm.devnum().expect("drm devnum");
        let mut udev = Udev::new().expect("udev monitor");
        udev.parked.insert(
            devnum,
            super::ParkedSessionDevice { kind: super::ParkedKind::Drm(drm), fd: None },
        );
        let mut state = crate::test_support::state_for_tests();
        state.session_active = true;
        let mut dispatcher = crate::runner::Dispatcher::default();
        udev.process(&mut state, super::UdevAction::SessionActive, &mut dispatcher)
            .expect("active before the resume fd");
        assert!(udev.awaiting_drm_resume.contains(&devnum));
        assert!(!dispatcher.shutdown_requested());
        assert!(udev.parked.contains_key(&devnum));

        let fd: std::os::fd::OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        let error = udev
            .process(&mut state, super::UdevAction::ResumeDevice { devnum, fd }, &mut dispatcher)
            .expect_err("the parked fd cannot be restored");
        assert!(matches!(
            error,
            ModelFailure::Shutdown(ShutdownReason::Fatal { source: "drm", .. })
        ));
        assert!(udev.resumed_fds.get(&devnum).is_none());
        assert!(dispatcher.test_has_queued_action(|action: &DbusAction| {
            matches!(action, DbusAction::ReleaseDevice { .. })
        }));
    }

    #[test]
    fn a_failed_drm_resume_leaves_another_output_in_place() {
        use crate::{
            dbus::DbusAction, runner::Model, state::BackendOutputRef, test_support::test_output,
        };

        let drm = first_device("drm");
        let devnum = drm.devnum().expect("drm devnum");
        let mut udev = Udev::new().expect("udev monitor");
        udev.parked.insert(
            devnum,
            super::ParkedSessionDevice { kind: super::ParkedKind::Drm(drm), fd: None },
        );
        let mut state = crate::test_support::state_for_tests();
        state.session_active = true;
        let surviving = state.insert_output(test_output(
            BackendOutputRef::new(crate::state::DrmId::from(0usize)),
            1,
            0,
            0,
            1920,
            1080,
            "surviving",
        ));
        let mut dispatcher = crate::runner::Dispatcher::default();
        let fd: std::os::fd::OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        udev.process(&mut state, super::UdevAction::ResumeDevice { devnum, fd }, &mut dispatcher)
            .expect("the other output keeps the session up");
        assert!(!dispatcher.shutdown_requested());
        assert_eq!(state.outputs.try_get(surviving).unwrap().name, "surviving");
        assert!(dispatcher.test_has_queued_action(|action: &DbusAction| {
            matches!(action, DbusAction::ReleaseDevice { .. })
        }));
    }

    #[test]
    fn shutdown_cancels_a_stored_input_resume() {
        use crate::{
            libinput::LibinputAction,
            runner::{Model, ShutdownReason},
        };

        let mut udev = Udev::new().expect("udev monitor");
        let mut state = crate::test_support::state_for_tests();
        state.session_active = true;
        let mut dispatcher = crate::runner::Dispatcher::default();
        let devnum = 42;
        udev.retained_inputs
            .insert(devnum, "/dev/input/event0".into());
        dispatcher.request_shutdown(ShutdownReason::Quit);
        let fd: std::os::fd::OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        udev.process(&mut state, super::UdevAction::ResumeDevice { devnum, fd }, &mut dispatcher)
            .expect("resume during shutdown");
        assert!(udev.resumed_fds.get(&devnum).is_none());
        assert!(!dispatcher.test_has_queued_action(|action: &LibinputAction| {
            matches!(action, LibinputAction::OpenInput { .. })
        }));

        state.session_active = false;
        let fd: std::os::fd::OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        let mut dispatcher = crate::runner::Dispatcher::default();
        udev.process(&mut state, super::UdevAction::ResumeDevice { devnum, fd }, &mut dispatcher)
            .expect("store a resume fd");
        assert!(udev.resumed_fds.get(&devnum).is_some());
        state.session_active = true;
        dispatcher.request_shutdown(ShutdownReason::Signal(libc::SIGTERM));
        udev.process(&mut state, super::UdevAction::SessionActive, &mut dispatcher)
            .expect("active during shutdown");
        assert!(udev.resumed_fds.get(&devnum).is_some());
        assert!(!dispatcher.test_has_queued_action(|action: &LibinputAction| {
            matches!(action, LibinputAction::OpenInput { .. })
        }));
    }

    fn first_device(subsystem: &str) -> udev::Device {
        let mut enumerator = udev::Enumerator::new().expect("udev enumerator");
        enumerator.match_subsystem(subsystem).expect("subsystem");
        enumerator
            .scan_devices()
            .expect("scan")
            .find(|device| device.devnum().is_some() && device.devnode().is_some())
            .unwrap_or_else(|| panic!("missing {subsystem} device"))
    }
}
