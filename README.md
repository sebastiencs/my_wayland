# my_wayland

A Wayland compositor written from scratch in Rust, with tiling inspired by
[bspwm](https://github.com/baskerville/bspwm) (X11). It speaks the Wayland
protocol itself, drives the display
through DRM/KMS + GBM, renders with **OpenGL ES** or **Vulkan**, and runs its
entire event loop (clients, input, udev, D-Bus, DRM) on a single
io_uring-driven action dispatcher.

## Status

**Work in progress.** The core is implemented and the compositor runs real
clients (terminals, bars, browsers, screen capture tools), but the project is
young: expect rough edges, missing polish, and breaking changes.  
The config format is still evolving.

The essentials are done and usable today:

- Tiling, floating and fullscreen windows
- Multi-output with hotplug
- Layer-shell panels and bars
- Screen capture / sharing
- Clipboard managers and idle daemons
- Session locking

## Features

### Rendering

- Hardware-accelerated rendering with **two backends: OpenGL ES and Vulkan**
- Renderer selectable per GPU device (`opengl`, `vulkan`, or `auto`)
- Direct DRM/KMS atomic modesetting, GBM buffer allocation, dmabuf import
- Explicit synchronization (`linux-drm-syncobj`) and incremental damage
  tracking (`FB_DAMAGE_CLIPS`, buffer age, occlusion)

### Effects & animations

- Cool animations: custom bezier curves, popin/slide/fade styles for
  windows and workspace transitions
- Blur, shadows, gradient borders with corner rounding
- Inactive-window dimming and opacity

### Platform

- libinput + xkbcommon, XCursor themes
- logind session management, VT switching, udev hotplug
- Hot-reloading TOML configuration (reload on save)

## Build & run

Requires a Rust **nightly** toolchain and `libshaderc_shared.so`
(the `shaderc` package on most distros).

    cargo build --release

Run from a TTY inside a logind session:

    ./target/release/my-wayland

Clients connect on the `my-wayland-0` Wayland socket. Spawned keybind
commands inherit `WAYLAND_DISPLAY` automatically.

## Configuration

The compositor reads `~/.config/my_wayland/config.toml` and reloads it on
save.  
See [config.toml.example](config.toml.example) for all options

## License

Licensed under the [BSD 3-Clause License](LICENSE).
