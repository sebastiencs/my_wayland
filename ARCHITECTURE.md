# Tema architecture notes

This document is a practical map of the repository and of the Linux graphics stack it sits on top of.

It is written for "future me after a long break", not for a graphics specialist. The goal is to make it easy to answer:

- What kind of project is this?
- Which layers exist between a Wayland client and the monitor?
- What does each acronym do?
- How do the modules in this repository map to those layers?

This is not a full graphics textbook. It focuses on the pieces that matter for understanding this codebase.

## 1. What This Repository Is

This repository is a small Wayland compositor built fairly close to the metal.

At a high level, it does four things:

1. Accepts Wayland clients and speaks the Wayland protocol.
2. Tracks compositor state such as objects, globals, devices, and buffers.
3. Talks to Linux graphics and input subsystems.
4. Sets up display output through DRM/KMS and GBM, then draws with OpenGL ES or Vulkan.

Another useful way to say it:

- The top half is "Wayland server and compositor logic".
- The bottom half is "Linux graphics stack and hardware-facing pieces".

## 2. The Big Layer Cake

From top to bottom, the stack looks like this:

```text
Applications
  GTK / Qt / SDL / custom Wayland clients

Wayland protocol
  wl_display, wl_registry, wl_surface, xdg_surface, wl_shm, etc.

This compositor
  Clients, Wayland objects, surface state, roles, buffer handling

Rendering layer
  OpenGL ES or Vulkan

Buffer allocation / sharing layer
  GBM + dma-buf

Kernel graphics / display control
  DRM / KMS + GEM

Hardware
  GPU, display engine, connectors, monitor
```

That same stack can also be read as a pipeline:

1. A client creates a buffer.
2. The compositor receives that buffer through Wayland.
3. The compositor imports or uploads the buffer into GPU-usable form.
4. The compositor renders the final scene into an output buffer.
5. DRM/KMS tells the display engine to scan out that buffer to a connector.
6. The monitor shows the image.

## 3. How This Repository Maps to the Layers

These files are the most important anchors:

- `src/main.rs`: startup and wiring.
- `src/runner.rs`: the event/action engine.
- `src/state.rs`: shared compositor state.
- `src/clients.rs`: Wayland server socket, client connections, protocol dispatch.
- `src/wayland_objects/*`: concrete Wayland object implementations.
- `src/input.rs`: libinput integration.
- `src/udev.rs`: device discovery and hotplug.
- `src/dbus.rs`: session and device access through logind.
- `src/drm.rs`: display resources, modesetting, planes, framebuffers.
- `src/gbm/*`: GBM buffer allocation.
- `src/render/`: renderer selection and the shared frame recipe.
- `src/render/gles/`: OpenGL ES backend, including its EGL display.
- `src/render/vulkan/`: Vulkan backend.
- `src/dmabuf.rs`: shared dma-buf description.

### Repository mental model

The center of the architecture is:

- `State`: the world.
- `Runner`: the scheduler.
- `Model`s: active subsystems.
- `Object`s: Wayland protocol objects.

The code is structured as a small action-driven runtime:

- `io_uring` produces completions.
- completions are turned into actions.
- actions are dispatched to models.
- models mutate `State` and schedule more work.

So this is not just "a compositor that happens to use async I/O". It is closer to "a compositor runtime whose subsystems communicate through typed actions".

## 4. The Main Runtime Flow

At startup:

1. `main.rs` initializes EGL, XKB, globals, and shared `State`.
2. It creates models for:
   - `IoUringState`
   - `Dbus`
   - `Udev`
   - `Libinput`
   - `Clients`
3. `ClientManagerAction::Start` starts the Wayland socket accept loop and also starts D-Bus and udev.

At runtime:

- `Runner` continuously drains queued actions.
- When there are no queued actions, it waits for `io_uring` completions.
- Those completions become new actions and wake the appropriate model.

That means the Wayland server, input, hotplug, D-Bus, and DRM event reading all flow through the same core dispatch mechanism.

## 5. Wayland Layer: What Happens Above Graphics

The Wayland side of the repository is the protocol-facing half.

Important concepts:

- A Wayland client connects over a Unix socket.
- It sends protocol messages addressed to object IDs.
- The compositor looks up the object and calls its handler.
- Objects represent protocol instances like `wl_display`, `wl_registry`, `wl_surface`, `wl_buffer`, and `xdg_surface`.

In this repository:

- `src/clients.rs` manages client sockets, reads messages, parses them, and dispatches requests.
- `src/wayland_objects/*` implements the behavior of each protocol object.

Examples:

- `wl_display.get_registry` creates a registry object and advertises globals.
- `wl_compositor.create_surface` creates a `wl_surface`.
- `xdg_wm_base.get_xdg_surface` gives a surface an xdg role.
- `wl_shm.create_pool` and `wl_shm_pool.create_buffer` create CPU-shared pixel buffers.
- `wl_surface.attach` + `commit` moves a pending buffer onto the visible surface state.

The Wayland layer does not directly put pixels on the monitor. It defines:

- who owns which surface
- which buffer is attached
- which role a surface has
- when a state change becomes visible

The lower graphics layers take over after that.

## 5.1 Input methods

Text entry is three protocols, all advertised to every client:

- `zwp_text_input_v3` is the application's side. Tema advertises it at version 1. A client enables it on the focused surface and sends the surrounding text, the content type, and the cursor rectangle. `done.serial` counts that object's commits.
- `zwp_input_method_v2` is the input method's side. One live input method per seat. A second `get_input_method` is created inert and sent `unavailable` immediately.
- `zwp_virtual_keyboard_v1` injects keys. `wtype`, fcitx5, and on-screen keyboards use it for keysyms the hardware keymap does not have. Those keys do not enter the hardware key path, so they cannot loop back into the input-method grab.

The relay lives in `src/input/ime.rs`. An enable on the focused surface sends `activate`, the current state, and `done` to the input method. Later commits send only the fields that changed. The input method's `commit` must echo the last `done` serial; a mismatch drops the pending strings. A match is forwarded to the active text input as delete, preedit, commit string, then `done`. A focus change clears the preedit, deactivates the input method, leaves the old client's text inputs, and enters every text input of the new client. Requests the client sends after `leave` are still applied, so its following `disable` is a real edge.

Each client has its own keymap. The hardware keymap is id 0. A virtual keyboard compiles the keymap string it sends and switches the focused client to that blob: if keys are held, the client gets `leave`, the new keymap, `enter` with those keys, then modifiers. The next hardware key or focus change switches back to the hardware keymap. Replacing or destroying a virtual keyboard releases the keys it still has down.

`zwp_input_method_keyboard_grab_v2` is consulted after compositor key bindings and before the focused client. A press goes to the grab and is recorded. Its release stays with the grab, and is swallowed if the grab has already gone. Presses that happened before the grab, or while the session is locked, still reach the client. The lock screen bypasses the grab, and input-method popups are not drawn on it.

`zwp_input_popup_surface_v2` is the candidate window. It is placed below the cursor rectangle on the output that contains the caret, flipped above when it would leave that output, then clamped inside it. `text_input_rectangle` tells the input method where the caret sits in popup-local coordinates. The popup is not a pointer target and never takes keyboard focus.

An on-screen keyboard is a layer-shell surface, usually with `keyboard_interactivity = none`, plus a virtual keyboard. A click on that layer leaves keyboard focus where it is, so the tap does not deactivate the input method. `on_demand` and `exclusive` layers still take focus. Other focus requests, including an explicit `set_keyboard_focus` of a `none` layer, still go through the normal focus normalization.

## 6. Linux Graphics Stack: The Lower Half

This is the part that usually feels like a wall of acronyms:

- dma-buf
- GBM
- EGL
- OpenGL ES / GL
- DRM / KMS
- GEM
- connector / CRTC / plane

The easiest way to understand them is to group them by job.

### 6.1 Memory sharing: dma-buf

`dma-buf` is the standard Linux mechanism for sharing image buffers across subsystems and processes without copying the pixels.

It is not a renderer and not a display API.

It is basically:

- a file descriptor
- plus metadata
- describing an image stored in memory

That metadata includes:

- width
- height
- pixel format
- per-plane stride
- per-plane offset
- modifier

In this repository, that description lives in `src/dmabuf.rs`.

Practical meaning:

- a client, decoder, compositor, or GPU component can all refer to the same underlying image
- if they all understand the format and modifier

### 6.2 Buffer allocation: GBM

`GBM` is the Generic Buffer Manager.

Its job is to allocate buffer objects that are suitable for graphics use, especially:

- GPU rendering
- display scanout
- exporting as dma-buf

Think of GBM as: "please give me a buffer the driver and hardware will actually like".

In this repository:

- `src/gbm/gbm.rs` creates a GBM device from the DRM device
- GBM allocates buffer objects using usage flags and modifiers
- those GBM buffer objects are exported as `DmaBuf`

GBM sits between high-level rendering code and low-level driver memory management.

### 6.3 Rendering API glue: EGL

`EGL` is the bridge between native platform graphics objects and rendering APIs like OpenGL ES.

It does several jobs:

- create a rendering display from GBM
- choose a config
- create a GL context
- make that context current
- import dma-bufs as `EGLImage`

EGL is not the display engine and not the renderer itself.

It is the integration layer that makes "GL on Linux native buffers" work.

In this repository, `src/render/gles/` is where:

- the EGL display is created from GBM
- supported dma-buf formats/modifiers are queried
- dma-bufs are imported into EGL images
- those images are turned into renderbuffers, framebuffers, or textures

Vulkan does not use EGL. Its device, images, and sync files live in `src/render/vulkan/`.

### 6.4 Rendering API: OpenGL ES / GL

`OpenGL ES` is one of the two rendering APIs. Vulkan is the other, and section 6.5 is how a device chooses.

For this repository, GL is mostly about:

- taking imported surface data
- treating it as textures or image-backed render targets
- drawing into output buffers

Useful terms here:

- render: generate pixels into a destination
- texture: image resource sampled by shaders
- blend: combine source pixels with destination pixels
- viewport: map normalized output coordinates into a target rectangle
- scissor: clip writes to a rectangular area

OpenGL ES operates above buffer allocation and below compositor scene logic.

### 6.5 Choosing the renderer

Each DRM device builds one renderer at startup from `general.renderer`.

- An omitted key, or `opengl`, uses OpenGL ES.
- `auto` tries OpenGL ES on that device, then Vulkan.
- `vulkan` uses Vulkan. If that device cannot match the DRM node or lacks a required extension, startup fails for that device. The compositor does not switch it to OpenGL.

A running compositor keeps its renderer. Changing the setting needs a restart.

When the renderer can export a sync file and the primary plane has `IN_FENCE_FD`, KMS waits on that fence. Otherwise the renderer waits for the GPU to finish before `render` returns. The DRM log for that second path is `Falling back to blocking GL sync`.

### 6.6 Kernel display control: DRM/KMS

`DRM` is the Linux kernel graphics subsystem.

`KMS` means Kernel Mode Setting, the display-control side of DRM.

This is the part that answers:

- which monitor is connected?
- which mode is active?
- which image is being shown?
- which plane/CRTC/connector relationships are active?

In this repository, `src/drm.rs`:

- enables atomic modesetting
- enumerates connectors, CRTCs, encoders, and planes
- chooses a usable connector and primary plane
- creates DRM framebuffers from dma-bufs
- commits display state through atomic KMS

This is the layer closest to "actually putting pixels on screen".

### 6.7 Kernel graphics memory objects: GEM

`GEM` stands for Graphics Execution Manager.

In practice, a GEM object is a kernel/driver-managed graphics memory object.

A helpful distinction:

- dma-buf fd: shareable cross-subsystem handle
- GEM handle/object: DRM-driver-side graphics memory object
- framebuffer: KMS object describing how to interpret memory for scanout

Your GBM buffers are backed by driver-managed memory. When KMS needs to scan them out, that memory is represented in DRM terms through GEM-backed handles.

## 7. Display Hardware Objects: Connector, CRTC, Plane

These are the most important KMS concepts.

### Connector

A connector is an output endpoint, such as:

- HDMI-A-1
- DP-1
- eDP-1

It represents the place where a monitor is attached.

The connector knows things like:

- connected or disconnected state
- supported modes
- EDID-derived monitor information

### CRTC

A CRTC is the scanout and timing engine.

Historically the name came from old display hardware, but in modern Linux graphics it roughly means:

- the hardware pipeline that reads image data
- applies display timing
- feeds the output path

You can think of the CRTC as the part that is actively driving the frame to the display.

### Plane

A plane is a hardware image layer.

Typical kinds are:

- primary plane
- cursor plane
- overlay plane

The primary plane usually carries the main desktop image.
Cursor and overlay planes can be used for hardware composition.

### Relationship

One useful simplified model is:

- connector: where the image goes
- CRTC: the engine that drives the image
- plane: which image layer is scanned

And underneath the plane sits a framebuffer object pointing at image memory.

## 8. Scanout, Page Flip, and Direct Scanout

### Scanout

To scan out a buffer means:

- the display engine reads pixels from memory
- and streams them to the monitor

So rendering writes pixels into memory, while scanout reads pixels from memory for display.

### Page flip

A page flip is when the display system switches from one framebuffer to another for scanout.

Typical double-buffering idea:

- one buffer is currently visible
- another buffer is rendered offscreen
- then the display switches to the new buffer at a safe point

That switch is the page flip.

In modern KMS this often appears as part of an atomic commit rather than an older standalone flip API, but the concept is the same.

### Direct scanout

Normally the compositor imports client buffers and composites them into a new output buffer.

Direct scanout is the optimization where the compositor skips that extra composition step and lets the display hardware scan out a client buffer directly.

This is only possible when many constraints are satisfied, for example:

- format is supported
- modifier is supported
- geometry is suitable
- no overlapping composition is needed

## 9. Format and Modifier

These two are easy to confuse.

### Format

The format tells you what the pixels mean.

Examples:

- `XRGB8888`
- `ARGB8888`

It answers:

- how many channels exist?
- in what order?
- how many bits per channel?
- is there alpha?

### Modifier

The modifier tells you how those pixels are laid out in memory.

It may describe:

- linear layout
- tiled layout
- compressed tiled layout
- vendor-specific swizzles or compression

So:

- format = what the pixels are
- modifier = how the pixels are physically stored

This matters because two components can only share a buffer safely if they agree on both.

That is why the repository intersects modifier support between the rendering side and the KMS plane side.

## 10. Linear, Tiled, and Compressed Tiled Memory

### Linear memory

Pixels are stored row by row in straightforward order.

This is the easiest layout to understand, but not always the fastest for hardware.

### Tiled GPU memory

Pixels are stored in hardware-friendly tiles rather than simple rows.

This usually improves:

- cache locality
- memory bandwidth usage
- rendering efficiency

### Compressed tiled memory

The image is tiled and also stored in a hardware-understood compressed form.

This can save bandwidth and power, but it requires that all users of the image understand that layout.

This is one major reason modifiers are so important.

## 11. Texture, EGLImage, and Framebuffer

These concepts sit in the rendering half.

### Texture

A texture is a GL image resource that shaders can sample from.

For a compositor, a client surface often becomes a texture so it can be drawn onto the final output.

### EGLImage

An `EGLImage` is an EGL/native image object used for interop.

It is often created from an external resource such as a dma-buf and then attached to GL objects.

The shortest distinction is:

- texture: GL-side sampled image object
- EGLImage: native/interoperable image bridge object

### Framebuffer

A framebuffer is the rendering destination.

It is the thing GL renders into.

Separately, on the DRM/KMS side, a framebuffer object is also the KMS description of how image memory should be interpreted for scanout.

The word "framebuffer" is overloaded, so context matters:

- GL framebuffer: rendering target
- DRM framebuffer: scanout description for KMS

## 12. A Concrete End-to-End Story

This is the shortest realistic "one frame" narrative for this repository:

1. A Wayland client creates a `wl_surface`.
2. The client creates a buffer, for example through `wl_shm`.
3. The client attaches that buffer and commits the surface.
4. The compositor records that the surface now has a new front buffer.
5. The buffer data is imported or uploaded into a GL-usable form.
6. The compositor renders the scene into an output buffer allocated through GBM.
7. That output buffer is represented as a dma-buf with a format and modifier.
8. DRM/KMS creates a framebuffer object for that memory.
9. An atomic commit binds that framebuffer to a plane on a CRTC attached to a connector.
10. The display engine scans out the pixels to the monitor.

That is the entire stack in motion.

## 13. Practical Reading Order for This Repository

If coming back after a long time, this order should restore the mental model quickly:

1. `src/main.rs`
   Understand startup and which subsystems exist.
2. `src/runner.rs`
   Understand the action-driven runtime.
3. `src/state.rs`
   See what global state the compositor owns.
4. `src/clients.rs`
   Rebuild the Wayland server/client mental model.
5. `src/wayland_objects/wl_display.rs`
   Remember how the protocol bootstrap starts.
6. `src/wayland_objects/wl_registry.rs`
   Remember how globals are advertised.
7. `src/wayland_objects/wl_surface.rs`
   Rebuild the attach/commit model.
8. `src/wayland_objects/wl_buffer.rs` and `src/wayland_objects/wl_shm*.rs`
   See where client image data enters the compositor.
9. `src/render/`
   See how a DRM device chooses OpenGL ES or Vulkan, then how that backend imports a buffer.
10. `src/drm.rs`
   Rebuild KMS resources, framebuffers, and display output.
11. `src/gbm/gbm.rs`
   Understand output buffer allocation.
12. `src/input.rs`, `src/udev.rs`, `src/dbus.rs`
   Rebuild device discovery and session/device access.

## 14. Vocabulary

### Wayland

- compositor: the server that clients talk to and that decides what appears on screen
- client: an application using the Wayland protocol
- global: an advertised protocol capability such as `wl_compositor` or `wl_shm`
- surface: a client-visible drawable object
- role: the semantic meaning of a surface, such as xdg toplevel
- buffer: image data attached to a surface
- attach/commit: Wayland's "pending state then apply atomically" model

### Rendering

- render: generate pixels into a destination
- texture: image resource sampled during rendering
- shader: program run on the GPU during drawing
- blend: combine source pixels with destination pixels
- blend state: the rules controlling how that combination happens
- viewport: map normalized output coordinates into a target rectangle
- scissor: rectangular clip on writes
- GL context: the current OpenGL ES stateful execution environment
- rendering context: general term for that stateful rendering environment

### EGL / GL

- OpenGL ES: one rendering API used to draw images
- Vulkan: the other rendering API. It imports the same dma-bufs and does not use EGL
- GL: shorthand for the OpenGL API family or its function set
- EGL: bridge between native graphics resources and GL
- EGLImage: EGL-side interoperable image object, often imported from dma-buf

### Buffer / memory

- dma-buf: Linux shared-buffer mechanism based on file descriptors plus image metadata
- GBM: Generic Buffer Manager, used to allocate graphics/display-friendly buffers
- GEM object: kernel/driver graphics memory object
- format: what the pixels mean
- modifier: how the pixels are laid out in memory
- linear: simple row-by-row memory layout
- tiled: hardware-optimized non-linear memory layout
- compressed tiled: tiled layout with hardware-understood compression

### Display / KMS

- DRM: Linux kernel graphics subsystem
- KMS: kernel mode setting, the display-control side of DRM
- framebuffer: image memory description used for rendering or scanout, depending on context
- scanout: display engine reading pixels from memory to drive the monitor
- page flip: switching scanout from one framebuffer to another
- direct scanout: scanning out a client buffer directly without an extra composition step
- connector: output endpoint such as HDMI or DP
- CRTC: display timing and scanout engine
- plane: hardware image layer attached to a CRTC

## 15. Final Mental Shortcut

When this repository feels too abstract, reduce it to this:

- Wayland decides which client buffer should be visible.
- The renderer, OpenGL ES or Vulkan, draws those buffers into an output image.
- GBM allocates the buffers the GPU and display hardware want.
- dma-buf lets those buffers be shared without copying.
- DRM/KMS tells the display hardware what to show.
- connector, CRTC, and plane are the hardware display graph.

That is the whole system.
