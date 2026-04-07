#![feature(c_variadic, iter_array_chunks, arbitrary_self_types)]
#![allow(dead_code)]

macro_rules! wayland_id {
    ($($name:ident,)+) => ($(
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, derive_more::From, derive_more::Into)]
        pub struct $name(pub(super) u32);
        impl From<WaylandId> for $name {
            fn from(value: WaylandId) -> Self {
                Self(value.0)
            }
        }
        impl From<NewId> for $name {
            fn from(value: NewId) -> Self {
                Self(value.0)
            }
        }
        impl From<$name> for WaylandId {
            fn from(value: $name) -> Self {
                Self(value.0)
            }
        }
        impl $name {
            pub fn null_or<T, F: FnOnce($name) -> T>(self, fun: F) -> Option<T> {
                if u32::from(self) == 0 {
                    None
                } else {
                    Some(fun(self))
                }
            }
            pub fn null_or_ok<T, E, F: FnOnce($name) -> Result<T, E>>(self, fun: F) -> Result<Option<T>, E> {
                if u32::from(self) == 0 {
                    Ok(None)
                } else {
                    Ok(Some(fun(self)?))
                }
            }
            pub fn to_opt(self) -> Option<Self> {
                if u32::from(self) == 0 {
                    None
                } else {
                    Some(self)
                }
            }
        }
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let Self(this) = self;
                // no vertical alignment
                f.write_fmt(format_args!("{}({})", stringify!($name), this))
            }
        }
    )+)
}

macro_rules! index_map_id {
    ($name:ident) => {
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name {
            index: u32,
            generation: u32,
        }
        impl $name {
            pub const fn from_parts(index: u32, generation: u32) -> Self {
                Self { index, generation }
            }

            pub const fn index(self) -> u32 {
                self.index
            }

            pub const fn generation(self) -> u32 {
                self.generation
            }

            pub const fn as_u64(self) -> u64 {
                ((self.generation as u64) << 32) | (self.index as u64)
            }
        }

        impl crate::state::IndexMapKey for $name {
            fn from_parts(index: u32, generation: u32) -> Self {
                Self { index, generation }
            }

            fn index(self) -> u32 {
                self.index
            }

            fn generation(self) -> u32 {
                self.generation
            }
        }

        #[cfg(test)]
        impl From<usize> for $name {
            fn from(value: usize) -> Self {
                Self::from_parts(value.try_into().unwrap(), 0)
            }
        }
    };
}

macro_rules! model_instance_id {
    ($vis:vis $name:ident => $model:path) => {
        #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, derive_more::From)]
        $vis struct $name(u64);

        impl crate::runner::ModelInstanceId for $name {
            type Model = $model;

            fn into_raw(self) -> u64 {
                self.0
            }
        }
    };
}

/// Cache any data
#[allow(unused)]
macro_rules! cache {
    ($F:ty, $compute:expr) => {{
        // Initializer is `const`.
        // Note that the macro `thread_local!` uses internally the
        // unstable `#[thread_local]`.
        // We use `ManuallyDrop` to avoid dealing with destructor state
        // so it's fast.
        // The destructor won't be run, but we don't care.
        //
        // See
        // https://github.com/rust-lang/rust/blob/635c4a5e612b0ee8af6615635599702d3dce9906/library/std/src/sys/common/thread_local/fast_local.rs#

        use std::cell::RefCell;
        use std::mem::ManuallyDrop;

        thread_local! {
            static CACHE: ManuallyDrop<RefCell<Option<Box<$F>>>> =
                const { ManuallyDrop::new(RefCell::new(Option::None)) };
        }

        CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if let Some(cached) = cache.as_ref() {
                return (**cached).clone();
            }
            let data = $compute;
            let _ = cache.insert(Box::new(data.clone()));
            data
        })
    }};
}

use std::time::Duration;

use nix::sys::timerfd::{self, TimerFd, TimerFlags, TimerSetTimeFlags};

use {globals::Globals, state::State};

use crate::{
    clients::ClientManager,
    io_uring::model::IoUringState,
    runner::{Dispatcher, Runner},
    udev::Udev,
};

mod clients;
mod config;
mod cursor;
mod dbus;
mod dmabuf;
mod drm;
mod egl;
mod gbm;
mod globals;
mod input;
mod io_uring;
mod libinput;
mod logger;
mod object;
mod protocol;
mod protocol_generated;
mod queue;
mod region;
mod render;
pub mod runner;
mod seat;
mod state;
mod sync;
#[cfg(test)]
mod test_support;
mod tiling;
mod transform;
mod udev;
mod utils;
mod wayland_objects;
mod xkb;

/// Master switch for incremental damage (surface commits, output damage, partial GL redraw,
/// `FB_DAMAGE_CLIPS`, buffer age, occlusion, etc.). When `false`, code paths should fall back
/// to full-frame invalidation and full redraws. Compile-time only; change and rebuild to toggle.
pub const ENABLE_DAMAGE: bool = true;

fn make_timer(duration: Duration) -> TimerFd {
    let timer = timerfd::TimerFd::new(
        timerfd::ClockId::CLOCK_MONOTONIC,
        TimerFlags::TFD_CLOEXEC | TimerFlags::TFD_NONBLOCK,
    )
    .unwrap();
    timer
        .set(timerfd::Expiration::OneShot(duration.into()), TimerSetTimeFlags::empty())
        .unwrap();
    timer
}

fn main() {
    // use std::any::Any;

    // dbus2::init().unwrap();

    // egl::init();

    // return;

    // use khronos_egl::api::EGL1_5;
    // let lib = unsafe {
    //     libloading::Library::new("libEGL.so.1").expect("unable to find libEGL.so.1")
    // };

    // const EGL_PLATFORM_GBM_KHR: u32 = 0x31D7;

    // let egl = unsafe {
    //     khronos_egl::DynamicInstance::<khronos_egl::EGL1_5>::load_required()
    //         .expect("Unable to load libEGL.so or libEGL.so.1")
    // };

    // let egl = Arc::new(egl);

    // // let egl2 = egl.clone();

    // egl.bind_api(khronos_egl::OPENGL_ES_API)
    //    .expect("unable to select OpenGL API");

    let egl = egl::init();

    // egl.get_platform_display(EGL_PLATFORM_GBM_KHR, native_display, &[]);

    // let get_platform_display_ext = egl.get_proc_address("eglGetPlatformDisplayEXT").unwrap();

    // eglCreatePlatformWindowSurface

    // return;

    // egl.

    // egl.

    // khronos_egl::Instance::new(khronos_egl::Static);

    logger::install();

    // egl::init();

    // let xkb = xkb::Xkb::new();

    // log::info!("hello");

    let xkb = xkb::Xkb::new();
    let keyboard = xkb.create_keyboard_state();
    let config = config::RuntimeConfig::load_initial(&xkb);

    let mut globals = Globals::new();
    globals.initialize();

    let mut state = State {
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
        egl,
        config,
        seat: Default::default(),
        keyboard,
        xkb,
    };
    let mut startup_dispatcher = Dispatcher::default();
    config::apply_general_settings(&mut state, &mut startup_dispatcher);

    let dbus = dbus::Dbus::new().unwrap();

    let udev = Udev::new().unwrap();
    let libinput = libinput::Libinput::new();

    // protocol::gen().unwrap();
    // return;

    let uring = Box::<IoUringState>::default();
    let client_manager = ClientManager::new();
    let config_watcher = Box::new(config::ConfigWatcher::new(&state.config));

    let mut runner = Runner::with_state(state);
    runner.add_model(uring);
    runner.add_model(dbus);
    runner.add_model(udev);
    runner.add_model(libinput);
    runner.add_model(client_manager);
    runner.add_model(config_watcher);
    runner.add_model(Box::new(drm::WindowAnimationManager::new()));

    let mut dispatcher = Dispatcher::default();
    dispatcher.start::<ClientManager>();
    dispatcher.start::<libinput::Libinput>();
    dispatcher.start::<config::ConfigWatcher>();
    dispatcher.start::<drm::WindowAnimationManager>();

    // let timer = make_timer(Duration::from_secs(10));

    // if !utils::is_x11() {
    //     dispatcher.dispatch(IoUringAction::Poll {
    //         id: None,
    //         fd: timer.as_fd().as_raw_fd(),
    //         dispatch_to: Redispatcher::to_any(|_res| {
    //             eprintln!("ON_TIMEOUT");
    //             std::process::abort();
    //         }),
    //     });

    //     std::thread::spawn(|| {
    //         std::thread::sleep(Duration::from_millis(500));
    //         let _output = std::process::Command::new("fish")
    //             .args(&[
    //                 "-c",
    //                 // "env -u DISPLAY env WAYLAND_DISPLAY=/var/run/user/1000/wayland-1 vlc",
    //                 // "env -u DISPLAY env WAYLAND_DISPLAY=/var/run/user/1000/wayland-1 alacritty",
    //                 // "env -u DISPLAY env WAYLAND_DISPLAY=/var/run/user/1000/wayland-1 /home/sebastien/github/client-toolkit/target/debug/examples/generic_simple_window",
    //                 // "env -u DISPLAY env WAYLAND_DISPLAY=/var/run/user/1000/my-wayland-0 /home/sebastien/github/wayland-rs/target/debug/examples/simple_window",
    //                 // "env -u DISPLAY env WAYLAND_DISPLAY=/var/run/user/1000/my-wayland-0 /usr/bin/wev",
    //                 "env -u DISPLAY env WAYLAND_DISPLAY=/var/run/user/1000/my-wayland-0 alacritty",
    //                 // "env -u DISPLAY env WAYLAND_DISPLAY=/var/run/user/1000/my-wayland-0 /home/sebastien/github/weston/build/clients/weston-simple-egl",
    //             ])
    //             .status()
    //             .unwrap();
    //         //     .piped()
    //         //     .output()
    //         //     .unwrap();
    //         // eprintln!("command stdout={}", String::from_utf8(output.stdout).unwrap());
    //         // eprintln!("command stderr={}", String::from_utf8(output.stderr).unwrap());
    //     });
    // }

    runner.run(dispatcher);

    // println!("Hello, world! {:?}", std::mem::size_of::<Box<dyn Any>>());
    // println!("Hello, world! {:?}", std::mem::size_of::<Box<()>>());
}
