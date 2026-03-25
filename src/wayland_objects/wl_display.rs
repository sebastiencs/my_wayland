use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wl_display, WaylandId, WlDisplayId},
    state::State,
    wayland_objects::{wl_callback::WlCallback, wl_registry::WlRegistry},
};

fn monotonic_time_msec() -> u32 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
        return 0;
    }
    let millis = (ts.tv_sec as u64)
        .saturating_mul(1000)
        .saturating_add((ts.tv_nsec as u64) / 1_000_000);
    millis.min(u64::from(u32::MAX)) as u32
}

#[derive(Debug)]
pub struct WlDisplay {
    id: WlDisplayId,
}

impl Default for WlDisplay {
    fn default() -> Self {
        Self { id: 1.into() }
    }
}

impl Object for WlDisplay {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        request: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) {
        use wl_display::Request::*;

        match client.parse::<Self>(request).unwrap() {
            Sync { callback } => {
                let ((id, _), callback) = client.insert_object(state, WlCallback::new(callback));
                callback.done(monotonic_time_msec(), dispatcher);
                client.delete_id(self, state, dispatcher, id);
            }
            GetRegistry { registry } => {
                let registry = WlRegistry::new(registry);
                registry.list_global_objects(state, dispatcher);
                client.insert_object(state, registry);
            }
        }
    }
}
