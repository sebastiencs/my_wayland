use std::os::fd::{AsRawFd, FromRawFd};

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    drm::request_output,
    object::{Object, Parser, RequestResult},
    protocol_generated::{zwlr_gamma_control_v1, Constants, WaylandId, ZwlrGammaControlV1Id},
    state::{GrabbedObject, OutputId, State},
    wayland_objects::wl_display::WlDisplay,
};

fn set_nonblocking(fd: i32) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error());
    }
    if (flags & libc::O_NONBLOCK) != 0 {
        return Ok(());
    }
    let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if result == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[derive(Debug)]
pub struct ZwlrGammaControlV1 {
    id: ZwlrGammaControlV1Id,
    gamma_object: Option<Resolved<ZwlrGammaControlV1Id>>,
    output_id: Option<OutputId>,
    gamma_size: u32,
}

impl ZwlrGammaControlV1 {
    pub fn new(id: ZwlrGammaControlV1Id, output_id: Option<OutputId>, gamma_size: u32) -> Self {
        Self { id, gamma_object: None, output_id, gamma_size }
    }

    pub fn set_gamma_object(&mut self, gamma_object: Resolved<ZwlrGammaControlV1Id>) {
        self.gamma_object = Some(gamma_object);
    }

    pub fn send_gamma_size(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(
            zwlr_gamma_control_v1::Event::GammaSize { size: self.gamma_size },
            dispatcher,
        );
    }

    pub fn send_failed(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwlr_gamma_control_v1::Event::Failed, dispatcher);
    }

    pub fn controls_output(&self, output_id: OutputId) -> bool {
        self.output_id == Some(output_id)
    }

    fn release_gamma(&mut self, state: &mut State) -> Option<OutputId> {
        let Some(output_id) = self.output_id.take() else {
            return None;
        };
        let reset_applied = state.set_output_gamma_lut(output_id, None).is_ok();
        if let Some(gamma_object) = self.gamma_object {
            state.clear_gamma_control_owner(output_id, gamma_object);
        }
        reset_applied.then_some(output_id)
    }

    fn release_gamma_and_schedule(&mut self, state: &mut State, dispatcher: &mut ClientDispatcher) {
        let Some(output_id) = self.release_gamma(state) else {
            return;
        };
        request_output(state, output_id, dispatcher);
    }

    pub fn invalidate(&mut self, state: &mut State, dispatcher: &mut ClientDispatcher) {
        self.release_gamma_and_schedule(state, dispatcher);
        self.send_failed(dispatcher);
    }

    fn set_gamma_from_fd(
        &mut self,
        client: &mut Client,
        state: &mut State,
        fd: i32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        if set_nonblocking(file.as_raw_fd()).is_err() {
            self.invalidate(state, dispatcher);
            return;
        }
        let Some(output_id) = self.output_id else {
            self.send_failed(dispatcher);
            return;
        };
        let expected_entries = self.gamma_size as usize * 3;
        let expected_bytes = expected_entries * std::mem::size_of::<u16>();
        let mut bytes = vec![0u8; expected_bytes];
        let n_read =
            unsafe { libc::pread(file.as_raw_fd(), bytes.as_mut_ptr().cast(), expected_bytes, 0) };
        if n_read < 0 {
            self.invalidate(state, dispatcher);
            return;
        }
        if n_read as usize != expected_bytes {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                zwlr_gamma_control_v1::Error::InvalidGamma as u32,
                "gamma table size does not match advertised gamma size",
            );
            return;
        }
        let mut extra = [0u8; 1];
        let extra_read = unsafe {
            libc::pread(
                file.as_raw_fd(),
                extra.as_mut_ptr().cast(),
                extra.len(),
                expected_bytes as libc::off_t,
            )
        };
        if extra_read < 0 {
            self.invalidate(state, dispatcher);
            return;
        }
        if extra_read != 0 {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                zwlr_gamma_control_v1::Error::InvalidGamma as u32,
                "gamma table size does not match advertised gamma size",
            );
            return;
        }

        let raw = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_ne_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        let ramp_size = self.gamma_size as usize;
        let (red, rest) = raw.split_at(ramp_size);
        let (green, blue) = rest.split_at(ramp_size);
        let mut interleaved = Vec::with_capacity(raw.len());
        for index in 0..ramp_size {
            interleaved.push(red[index]);
            interleaved.push(green[index]);
            interleaved.push(blue[index]);
        }

        if state
            .set_output_gamma_lut(output_id, Some(interleaved))
            .is_err()
        {
            self.invalidate(state, dispatcher);
            return;
        }

        request_output(state, output_id, dispatcher);
    }
}

impl Object for ZwlrGammaControlV1 {
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
        use zwlr_gamma_control_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                self.release_gamma_and_schedule(state, dispatcher);
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            SetGamma { fd } => self.set_gamma_from_fd(client, state, fd, dispatcher),
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        _client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.release_gamma_and_schedule(state, dispatcher);
    }
}
