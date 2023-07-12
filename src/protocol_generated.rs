#![allow(dead_code, unused_imports, unused_mut, clippy::let_and_return)]
#![allow(clippy::enum_variant_names)]

use {
    crate::{
        clients::{EventWriter, RawEvent, RawRequest},
        object::{Parser, ParserError},
    },
    std::os::unix::io::RawFd,
};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, derive_more::From, derive_more::Into)]
pub struct WaylandId(u32);
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, derive_more::From, derive_more::Into)]
pub struct NewId(u32);
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, derive_more::From, derive_more::Into)]
pub struct Array;

wayland_id! {
  WlDisplayId,
  WlRegistryId,
  WlCallbackId,
  WlCompositorId,
  WlShmPoolId,
  WlShmId,
  WlBufferId,
  WlDataOfferId,
  WlDataSourceId,
  WlDataDeviceId,
  WlDataDeviceManagerId,
  WlShellId,
  WlShellSurfaceId,
  WlSurfaceId,
  WlSeatId,
  WlPointerId,
  WlKeyboardId,
  WlTouchId,
  WlOutputId,
  WlRegionId,
  WlSubcompositorId,
  WlSubsurfaceId,
  WpPresentationId,
  WpPresentationFeedbackId,
  WpViewporterId,
  WpViewportId,
  XdgWmBaseId,
  XdgPositionerId,
  XdgSurfaceId,
  XdgToplevelId,
  XdgPopupId,
  WpContentTypeManagerV1Id,
  WpContentTypeV1Id,
  WpDrmLeaseDeviceV1Id,
  WpDrmLeaseConnectorV1Id,
  WpDrmLeaseRequestV1Id,
  WpDrmLeaseV1Id,
  ExtIdleNotifierV1Id,
  ExtIdleNotificationV1Id,
  ExtSessionLockManagerV1Id,
  ExtSessionLockV1Id,
  ExtSessionLockSurfaceV1Id,
  WpFractionalScaleManagerV1Id,
  WpFractionalScaleV1Id,
  WpSinglePixelBufferManagerV1Id,
  WpTearingControlManagerV1Id,
  WpTearingControlV1Id,
  XdgActivationV1Id,
  XdgActivationTokenV1Id,
  XwaylandShellV1Id,
  XwaylandSurfaceV1Id,
}

/// Core global object
///
/// The core global object.  This is a special singleton object.  It
/// is used for internal Wayland protocol features.
pub mod wl_display {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Asynchronous roundtrip
        ///
        /// The sync request asks the server to emit the 'done' event
        /// on the returned wl_callback object.  Since requests are
        /// handled in-order and events are delivered in-order, this can
        /// be used as a barrier to ensure all previous requests and the
        /// resulting events have been handled.
        ///
        /// The object returned by this request will be destroyed by the
        /// compositor after the callback is fired and as such the client must not
        /// attempt to use it after that point.
        ///
        /// The callback_data passed in the callback is the event serial.
        Sync {
            /// Callback object for the sync request,
            callback: WlCallbackId,
        },
        /// Get global registry object
        ///
        /// This request creates a registry object that allows the client
        /// to list and bind the global objects available from the
        /// compositor.
        ///
        /// It should be noted that the server side resources consumed in
        /// response to a get_registry request can only be released when the
        /// client disconnects, not when the client side proxy is destroyed.
        /// Therefore, clients should invoke get_registry as infrequently as
        /// possible to avoid wasting memory.
        GetRegistry {
            /// Global registry object,
            registry: WlRegistryId,
        },
    }
    impl Parser for crate::wayland_objects::wl_display::WlDisplay {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Sync { callback: req.id()? },
                1 => GetRegistry { registry: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Fatal error event
        ///
        /// The error event is sent out when a fatal (non-recoverable)
        /// error has occurred.  The object_id argument is the object
        /// where the error occurred, most often in response to a request
        /// to that object.  The code identifies the error and is defined
        /// by the object interface.  As such, each interface defines its
        /// own set of error codes.  The message is a brief description
        /// of the error, for (debugging) convenience.
        Error {
            /// Object where the error occurred,
            object_id: WaylandId,
            /// Error code,
            code: u32,
            /// Error description,
            message: String,
        },
        /// Acknowledge object ID deletion
        ///
        /// This event is used internally by the object ID management
        /// logic. When a client deletes an object that it had created,
        /// the server will send this event to acknowledge that it has
        /// seen the delete request. When the client receives this event,
        /// it will know that it can safely reuse the object ID.
        DeleteId {
            /// Deleted object ID,
            id: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_display::WlDisplay {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Error { object_id, code, message } => {
                    let mut raw = RawEvent::new(self.id(), 0, 3);
                    raw.id(object_id);
                    raw.uint(code);
                    raw.string(message);
                    raw
                }
                DeleteId { id } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.uint(id);
                    raw
                }
            }
        }
    }
    /// Global error values
    ///
    /// These errors are global and can be emitted in response to any
    /// server request.
    #[derive(Debug)]
    pub enum Error {
        /// Server couldn't find object,
        InvalidObject = 0,
        /// Method doesn't exist on the specified interface or malformed request,
        InvalidMethod = 1,
        /// Server is out of memory,
        NoMemory = 2,
        /// Implementation error in compositor,
        Implementation = 3,
    }
}

/// Global registry object
///
/// The singleton global registry object.  The server has a number of
/// global objects that are available to all clients.  These objects
/// typically represent an actual object in the server (for example,
/// an input device) or they are singleton objects that provide
/// extension functionality.
///
/// When a client creates a registry object, the registry object
/// will emit a global event for each global currently in the
/// registry.  Globals come and go as a result of device or
/// monitor hotplugs, reconfiguration or other events, and the
/// registry will send out global and global_remove events to
/// keep the client up to date with the changes.  To mark the end
/// of the initial burst of events, the client can use the
/// wl_display.sync request immediately after calling
/// wl_display.get_registry.
///
/// A client can bind to a global object by using the bind
/// request.  This creates a client-side handle that lets the object
/// emit events to the client and lets the client invoke requests on
/// the object.
pub mod wl_registry {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Bind an object to the display
        ///
        /// Binds a new, client-created object to the server using the
        /// specified name as the identifier.
        Bind {
            /// Unique numeric name of the object,
            name: u32,
            /// Bounded object,
            id: NewId,
        },
    }
    impl Parser for crate::wayland_objects::wl_registry::WlRegistry {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Bind { name: req.uint()?, id: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Announce global object
        ///
        /// Notify the client of global objects.
        ///
        /// The event notifies the client that a global object with
        /// the given name is now available, and it implements the
        /// given version of the given interface.
        Global {
            /// Numeric name of the global object,
            name: u32,
            /// Interface implemented by the object,
            interface: String,
            /// Interface version,
            version: u32,
        },
        /// Announce removal of global object
        ///
        /// Notify the client of removed global objects.
        ///
        /// This event notifies the client that the global identified
        /// by name is no longer available.  If the client bound to
        /// the global using the bind request, the client should now
        /// destroy that object.
        ///
        /// The object remains valid and requests to the object will be
        /// ignored until the client destroys it, to avoid races between
        /// the global going away and a client sending a request to it.
        GlobalRemove {
            /// Numeric name of the global object,
            name: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_registry::WlRegistry {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Global { name, interface, version } => {
                    let mut raw = RawEvent::new(self.id(), 0, 3);
                    raw.uint(name);
                    raw.string(interface);
                    raw.uint(version);
                    raw
                }
                GlobalRemove { name } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.uint(name);
                    raw
                }
            }
        }
    }
}

/// Callback object
///
/// Clients can handle the 'done' event to get notified when
/// the related request is done.
///
/// Note, because wl_callback objects are created from multiple independent
/// factory interfaces, the wl_callback interface is frozen at version 1.
pub mod wl_callback {
    use super::*;
    #[derive(Debug)]
    pub enum Event {
        /// Done event
        ///
        /// Notify the client when the related request is done.
        Done {
            /// Request-specific data for the callback,
            callback_data: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_callback::WlCallback {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Done { callback_data } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(callback_data);
                    raw
                }
            }
        }
    }
}

/// The compositor singleton
///
/// A compositor.  This object is a singleton global.  The
/// compositor is in charge of combining the contents of multiple
/// surfaces into one displayable output.
pub mod wl_compositor {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Create new surface
        ///
        /// Ask the compositor to create a new surface.
        CreateSurface {
            /// The new surface,
            id: WlSurfaceId,
        },
        /// Create new region
        ///
        /// Ask the compositor to create a new region.
        CreateRegion {
            /// The new region,
            id: WlRegionId,
        },
    }
    impl Parser for crate::wayland_objects::wl_compositor::WlCompositor {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => CreateSurface { id: req.id()? },
                1 => CreateRegion { id: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// A shared memory pool
///
/// The wl_shm_pool object encapsulates a piece of memory shared
/// between the compositor and client.  Through the wl_shm_pool
/// object, the client can allocate shared memory wl_buffer objects.
/// All objects created through the same pool share the same
/// underlying mapped memory. Reusing the mapped memory avoids the
/// setup/teardown overhead and is useful when interactively resizing
/// a surface or for many small buffers.
pub mod wl_shm_pool {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Create a buffer from the pool
        ///
        /// Create a wl_buffer object from the pool.
        ///
        /// The buffer is created offset bytes into the pool and has
        /// width and height as specified.  The stride argument specifies
        /// the number of bytes from the beginning of one row to the beginning
        /// of the next.  The format is the pixel format of the buffer and
        /// must be one of those advertised through the wl_shm.format event.
        ///
        /// A buffer will keep a reference to the pool it was created from
        /// so it is valid to destroy the pool immediately after creating
        /// a buffer from it.
        CreateBuffer {
            /// Buffer to create,
            id: WlBufferId,
            /// Buffer byte offset within the pool,
            offset: i32,
            /// Buffer width, in pixels,
            width: i32,
            /// Buffer height, in pixels,
            height: i32,
            /// Number of bytes from the beginning of one row to the beginning of the next row,
            stride: i32,
            /// Buffer pixel format,
            format: u32,
        },
        /// Destroy the pool
        ///
        /// Destroy the shared memory pool.
        ///
        /// The mmapped memory will be released when all
        /// buffers that have been created from this pool
        /// are gone.
        Destroy,
        /// Change the size of the pool mapping
        ///
        /// This request will cause the server to remap the backing memory
        /// for the pool from the file descriptor passed when the pool was
        /// created, but using the new size.  This request can only be
        /// used to make the pool bigger.
        ///
        /// This request only changes the amount of bytes that are mmapped
        /// by the server and does not touch the file corresponding to the
        /// file descriptor passed at creation time. It is the client's
        /// responsibility to ensure that the file is at least as big as
        /// the new pool size.
        Resize {
            /// New size of the pool, in bytes,
            size: i32,
        },
    }
    impl Parser for crate::wayland_objects::wl_shm_pool::WlShmPool {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => CreateBuffer {
                    id: req.id()?,
                    offset: req.int()?,
                    width: req.int()?,
                    height: req.int()?,
                    stride: req.int()?,
                    format: req.uint()?,
                },
                1 => Destroy,
                2 => Resize { size: req.int()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// Shared memory support
///
/// A singleton global object that provides support for shared
/// memory.
///
/// Clients can create wl_shm_pool objects using the create_pool
/// request.
///
/// On binding the wl_shm object one or more format events
/// are emitted to inform clients about the valid pixel formats
/// that can be used for buffers.
pub mod wl_shm {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Create a shm pool
        ///
        /// Create a new wl_shm_pool object.
        ///
        /// The pool can be used to create shared memory based buffer
        /// objects.  The server will mmap size bytes of the passed file
        /// descriptor, to use as backing memory for the pool.
        CreatePool {
            /// Pool to create,
            id: WlShmPoolId,
            /// File descriptor for the pool,
            fd: RawFd,
            /// Pool size, in bytes,
            size: i32,
        },
    }
    impl Parser for crate::wayland_objects::wl_shm::WlShm {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => CreatePool { id: req.id()?, fd: req.fd()?, size: req.int()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Pixel format description
        ///
        /// Informs the client about a valid pixel format that
        /// can be used for buffers. Known formats include
        /// argb8888 and xrgb8888.
        Format {
            /// Buffer pixel format,
            format: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_shm::WlShm {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Format { format } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(format);
                    raw
                }
            }
        }
    }
    /// Wl_shm error values
    ///
    /// These errors can be emitted in response to wl_shm requests.
    #[derive(Debug)]
    pub enum Error {
        /// Buffer format is not known,
        InvalidFormat = 0,
        /// Invalid size or stride during pool or buffer creation,
        InvalidStride = 1,
        /// Mmapping the file descriptor failed,
        InvalidFd = 2,
    }
    /// Pixel formats
    ///
    /// This describes the memory layout of an individual pixel.
    ///
    /// All renderers should support argb8888 and xrgb8888 but any other
    /// formats are optional and may not be supported by the particular
    /// renderer in use.
    ///
    /// The drm format codes match the macros defined in drm_fourcc.h, except
    /// argb8888 and xrgb8888. The formats actually supported by the compositor
    /// will be reported by the format event.
    ///
    /// For all wl_shm formats and unless specified in another protocol
    /// extension, pre-multiplied alpha is used for pixel values.
    #[derive(Debug)]
    pub enum Format {
        /// 32-bit ARGB format, [31:0] A:R:G:B 8:8:8:8 little endian,
        Argb8888 = 0,
        /// 32-bit RGB format, [31:0] x:R:G:B 8:8:8:8 little endian,
        Xrgb8888 = 1,
        /// 8-bit color index format, [7:0] C,
        C8 = 538982467,
        /// 8-bit RGB format, [7:0] R:G:B 3:3:2,
        Rgb332 = 943867730,
        /// 8-bit BGR format, [7:0] B:G:R 2:3:3,
        Bgr233 = 944916290,
        /// 16-bit xRGB format, [15:0] x:R:G:B 4:4:4:4 little endian,
        Xrgb4444 = 842093144,
        /// 16-bit xBGR format, [15:0] x:B:G:R 4:4:4:4 little endian,
        Xbgr4444 = 842089048,
        /// 16-bit RGBx format, [15:0] R:G:B:x 4:4:4:4 little endian,
        Rgbx4444 = 842094674,
        /// 16-bit BGRx format, [15:0] B:G:R:x 4:4:4:4 little endian,
        Bgrx4444 = 842094658,
        /// 16-bit ARGB format, [15:0] A:R:G:B 4:4:4:4 little endian,
        Argb4444 = 842093121,
        /// 16-bit ABGR format, [15:0] A:B:G:R 4:4:4:4 little endian,
        Abgr4444 = 842089025,
        /// 16-bit RBGA format, [15:0] R:G:B:A 4:4:4:4 little endian,
        Rgba4444 = 842088786,
        /// 16-bit BGRA format, [15:0] B:G:R:A 4:4:4:4 little endian,
        Bgra4444 = 842088770,
        /// 16-bit xRGB format, [15:0] x:R:G:B 1:5:5:5 little endian,
        Xrgb1555 = 892424792,
        /// 16-bit xBGR 1555 format, [15:0] x:B:G:R 1:5:5:5 little endian,
        Xbgr1555 = 892420696,
        /// 16-bit RGBx 5551 format, [15:0] R:G:B:x 5:5:5:1 little endian,
        Rgbx5551 = 892426322,
        /// 16-bit BGRx 5551 format, [15:0] B:G:R:x 5:5:5:1 little endian,
        Bgrx5551 = 892426306,
        /// 16-bit ARGB 1555 format, [15:0] A:R:G:B 1:5:5:5 little endian,
        Argb1555 = 892424769,
        /// 16-bit ABGR 1555 format, [15:0] A:B:G:R 1:5:5:5 little endian,
        Abgr1555 = 892420673,
        /// 16-bit RGBA 5551 format, [15:0] R:G:B:A 5:5:5:1 little endian,
        Rgba5551 = 892420434,
        /// 16-bit BGRA 5551 format, [15:0] B:G:R:A 5:5:5:1 little endian,
        Bgra5551 = 892420418,
        /// 16-bit RGB 565 format, [15:0] R:G:B 5:6:5 little endian,
        Rgb565 = 909199186,
        /// 16-bit BGR 565 format, [15:0] B:G:R 5:6:5 little endian,
        Bgr565 = 909199170,
        /// 24-bit RGB format, [23:0] R:G:B little endian,
        Rgb888 = 875710290,
        /// 24-bit BGR format, [23:0] B:G:R little endian,
        Bgr888 = 875710274,
        /// 32-bit xBGR format, [31:0] x:B:G:R 8:8:8:8 little endian,
        Xbgr8888 = 875709016,
        /// 32-bit RGBx format, [31:0] R:G:B:x 8:8:8:8 little endian,
        Rgbx8888 = 875714642,
        /// 32-bit BGRx format, [31:0] B:G:R:x 8:8:8:8 little endian,
        Bgrx8888 = 875714626,
        /// 32-bit ABGR format, [31:0] A:B:G:R 8:8:8:8 little endian,
        Abgr8888 = 875708993,
        /// 32-bit RGBA format, [31:0] R:G:B:A 8:8:8:8 little endian,
        Rgba8888 = 875708754,
        /// 32-bit BGRA format, [31:0] B:G:R:A 8:8:8:8 little endian,
        Bgra8888 = 875708738,
        /// 32-bit xRGB format, [31:0] x:R:G:B 2:10:10:10 little endian,
        Xrgb2101010 = 808669784,
        /// 32-bit xBGR format, [31:0] x:B:G:R 2:10:10:10 little endian,
        Xbgr2101010 = 808665688,
        /// 32-bit RGBx format, [31:0] R:G:B:x 10:10:10:2 little endian,
        Rgbx1010102 = 808671314,
        /// 32-bit BGRx format, [31:0] B:G:R:x 10:10:10:2 little endian,
        Bgrx1010102 = 808671298,
        /// 32-bit ARGB format, [31:0] A:R:G:B 2:10:10:10 little endian,
        Argb2101010 = 808669761,
        /// 32-bit ABGR format, [31:0] A:B:G:R 2:10:10:10 little endian,
        Abgr2101010 = 808665665,
        /// 32-bit RGBA format, [31:0] R:G:B:A 10:10:10:2 little endian,
        Rgba1010102 = 808665426,
        /// 32-bit BGRA format, [31:0] B:G:R:A 10:10:10:2 little endian,
        Bgra1010102 = 808665410,
        /// Packed YCbCr format, [31:0] Cr0:Y1:Cb0:Y0 8:8:8:8 little endian,
        Yuyv = 1448695129,
        /// Packed YCbCr format, [31:0] Cb0:Y1:Cr0:Y0 8:8:8:8 little endian,
        Yvyu = 1431918169,
        /// Packed YCbCr format, [31:0] Y1:Cr0:Y0:Cb0 8:8:8:8 little endian,
        Uyvy = 1498831189,
        /// Packed YCbCr format, [31:0] Y1:Cb0:Y0:Cr0 8:8:8:8 little endian,
        Vyuy = 1498765654,
        /// Packed AYCbCr format, [31:0] A:Y:Cb:Cr 8:8:8:8 little endian,
        Ayuv = 1448433985,
        /// 2 plane YCbCr Cr:Cb format, 2x2 subsampled Cr:Cb plane,
        Nv12 = 842094158,
        /// 2 plane YCbCr Cb:Cr format, 2x2 subsampled Cb:Cr plane,
        Nv21 = 825382478,
        /// 2 plane YCbCr Cr:Cb format, 2x1 subsampled Cr:Cb plane,
        Nv16 = 909203022,
        /// 2 plane YCbCr Cb:Cr format, 2x1 subsampled Cb:Cr plane,
        Nv61 = 825644622,
        /// 3 plane YCbCr format, 4x4 subsampled Cb (1) and Cr (2) planes,
        Yuv410 = 961959257,
        /// 3 plane YCbCr format, 4x4 subsampled Cr (1) and Cb (2) planes,
        Yvu410 = 961893977,
        /// 3 plane YCbCr format, 4x1 subsampled Cb (1) and Cr (2) planes,
        Yuv411 = 825316697,
        /// 3 plane YCbCr format, 4x1 subsampled Cr (1) and Cb (2) planes,
        Yvu411 = 825316953,
        /// 3 plane YCbCr format, 2x2 subsampled Cb (1) and Cr (2) planes,
        Yuv420 = 842093913,
        /// 3 plane YCbCr format, 2x2 subsampled Cr (1) and Cb (2) planes,
        Yvu420 = 842094169,
        /// 3 plane YCbCr format, 2x1 subsampled Cb (1) and Cr (2) planes,
        Yuv422 = 909202777,
        /// 3 plane YCbCr format, 2x1 subsampled Cr (1) and Cb (2) planes,
        Yvu422 = 909203033,
        /// 3 plane YCbCr format, non-subsampled Cb (1) and Cr (2) planes,
        Yuv444 = 875713881,
        /// 3 plane YCbCr format, non-subsampled Cr (1) and Cb (2) planes,
        Yvu444 = 875714137,
        /// [7:0] R,
        R8 = 538982482,
        /// [15:0] R little endian,
        R16 = 540422482,
        /// [15:0] R:G 8:8 little endian,
        Rg88 = 943212370,
        /// [15:0] G:R 8:8 little endian,
        Gr88 = 943215175,
        /// [31:0] R:G 16:16 little endian,
        Rg1616 = 842221394,
        /// [31:0] G:R 16:16 little endian,
        Gr1616 = 842224199,
        /// [63:0] x:R:G:B 16:16:16:16 little endian,
        Xrgb16161616f = 1211388504,
        /// [63:0] x:B:G:R 16:16:16:16 little endian,
        Xbgr16161616f = 1211384408,
        /// [63:0] A:R:G:B 16:16:16:16 little endian,
        Argb16161616f = 1211388481,
        /// [63:0] A:B:G:R 16:16:16:16 little endian,
        Abgr16161616f = 1211384385,
        /// [31:0] X:Y:Cb:Cr 8:8:8:8 little endian,
        Xyuv8888 = 1448434008,
        /// [23:0] Cr:Cb:Y 8:8:8 little endian,
        Vuy888 = 875713878,
        /// Y followed by U then V, 10:10:10. Non-linear modifier only,
        Vuy101010 = 808670550,
        /// [63:0] Cr0:0:Y1:0:Cb0:0:Y0:0 10:6:10:6:10:6:10:6 little endian per 2 Y pixels,
        Y210 = 808530521,
        /// [63:0] Cr0:0:Y1:0:Cb0:0:Y0:0 12:4:12:4:12:4:12:4 little endian per 2 Y pixels,
        Y212 = 842084953,
        /// [63:0] Cr0:Y1:Cb0:Y0 16:16:16:16 little endian per 2 Y pixels,
        Y216 = 909193817,
        /// [31:0] A:Cr:Y:Cb 2:10:10:10 little endian,
        Y410 = 808531033,
        /// [63:0] A:0:Cr:0:Y:0:Cb:0 12:4:12:4:12:4:12:4 little endian,
        Y412 = 842085465,
        /// [63:0] A:Cr:Y:Cb 16:16:16:16 little endian,
        Y416 = 909194329,
        /// [31:0] X:Cr:Y:Cb 2:10:10:10 little endian,
        Xvyu2101010 = 808670808,
        /// [63:0] X:0:Cr:0:Y:0:Cb:0 12:4:12:4:12:4:12:4 little endian,
        Xvyu1216161616 = 909334104,
        /// [63:0] X:Cr:Y:Cb 16:16:16:16 little endian,
        Xvyu16161616 = 942954072,
        /// [63:0] A3:A2:Y3:0:Cr0:0:Y2:0:A1:A0:Y1:0:Cb0:0:Y0:0 1:1:8:2:8:2:8:2:1:1:8:2:8:2:8:2 little endian,
        Y0l0 = 810299481,
        /// [63:0] X3:X2:Y3:0:Cr0:0:Y2:0:X1:X0:Y1:0:Cb0:0:Y0:0 1:1:8:2:8:2:8:2:1:1:8:2:8:2:8:2 little endian,
        X0l0 = 810299480,
        /// [63:0] A3:A2:Y3:Cr0:Y2:A1:A0:Y1:Cb0:Y0 1:1:10:10:10:1:1:10:10:10 little endian,
        Y0l2 = 843853913,
        /// [63:0] X3:X2:Y3:Cr0:Y2:X1:X0:Y1:Cb0:Y0 1:1:10:10:10:1:1:10:10:10 little endian,
        X0l2 = 843853912,
        Yuv4208bit = 942691673,
        Yuv42010bit = 808539481,
        Xrgb8888A8 = 943805016,
        Xbgr8888A8 = 943800920,
        Rgbx8888A8 = 943806546,
        Bgrx8888A8 = 943806530,
        Rgb888A8 = 943798354,
        Bgr888A8 = 943798338,
        Rgb565A8 = 943797586,
        Bgr565A8 = 943797570,
        /// Non-subsampled Cr:Cb plane,
        Nv24 = 875714126,
        /// Non-subsampled Cb:Cr plane,
        Nv42 = 842290766,
        /// 2x1 subsampled Cr:Cb plane, 10 bit per channel,
        P210 = 808530512,
        /// 2x2 subsampled Cr:Cb plane 10 bits per channel,
        P010 = 808530000,
        /// 2x2 subsampled Cr:Cb plane 12 bits per channel,
        P012 = 842084432,
        /// 2x2 subsampled Cr:Cb plane 16 bits per channel,
        P016 = 909193296,
        /// [63:0] A:x:B:x:G:x:R:x 10:6:10:6:10:6:10:6 little endian,
        Axbxgxrx106106106106 = 808534593,
        /// 2x2 subsampled Cr:Cb plane,
        Nv15 = 892425806,
        Q410 = 808531025,
        Q401 = 825242705,
        /// [63:0] x:R:G:B 16:16:16:16 little endian,
        Xrgb16161616 = 942953048,
        /// [63:0] x:B:G:R 16:16:16:16 little endian,
        Xbgr16161616 = 942948952,
        /// [63:0] A:R:G:B 16:16:16:16 little endian,
        Argb16161616 = 942953025,
        /// [63:0] A:B:G:R 16:16:16:16 little endian,
        Abgr16161616 = 942948929,
    }
}

/// Content for a wl_surface
///
/// A buffer provides the content for a wl_surface. Buffers are
/// created through factory interfaces such as wl_shm, wp_linux_buffer_params
/// (from the linux-dmabuf protocol extension) or similar. It has a width and
/// a height and can be attached to a wl_surface, but the mechanism by which a
/// client provides and updates the contents is defined by the buffer factory
/// interface.
///
/// If the buffer uses a format that has an alpha channel, the alpha channel
/// is assumed to be premultiplied in the color channels unless otherwise
/// specified.
///
/// Note, because wl_buffer objects are created from multiple independent
/// factory interfaces, the wl_buffer interface is frozen at version 1.
pub mod wl_buffer {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy a buffer
        ///
        /// Destroy a buffer. If and how you need to release the backing
        /// storage is defined by the buffer factory interface.
        ///
        /// For possible side-effects to a surface, see wl_surface.attach.
        Destroy,
    }
    impl Parser for crate::wayland_objects::wl_buffer::WlBuffer {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Compositor releases buffer
        ///
        /// Sent when this wl_buffer is no longer used by the compositor.
        /// The client is now free to reuse or destroy this buffer and its
        /// backing storage.
        ///
        /// If a client receives a release event before the frame callback
        /// requested in the same wl_surface.commit that attaches this
        /// wl_buffer to a surface, then the client is immediately free to
        /// reuse the buffer and its backing storage, and does not need a
        /// second buffer for the next surface content update. Typically
        /// this is possible, when the compositor maintains a copy of the
        /// wl_surface contents, e.g. as a GL texture. This is an important
        /// optimization for GL(ES) compositors with wl_shm clients.
        Release,
    }
    impl EventWriter for crate::wayland_objects::wl_buffer::WlBuffer {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Release {} => {
                    let mut raw = RawEvent::new(self.id(), 0, 0);
                    raw
                }
            }
        }
    }
}

/// Offer to transfer data
///
/// A wl_data_offer represents a piece of data offered for transfer
/// by another client (the source client).  It is used by the
/// copy-and-paste and drag-and-drop mechanisms.  The offer
/// describes the different mime types that the data can be
/// converted to and provides the mechanism for transferring the
/// data directly from the source client.
pub mod wl_data_offer {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Accept one of the offered mime types
        ///
        /// Indicate that the client can accept the given mime type, or
        /// NULL for not accepted.
        ///
        /// For objects of version 2 or older, this request is used by the
        /// client to give feedback whether the client can receive the given
        /// mime type, or NULL if none is accepted; the feedback does not
        /// determine whether the drag-and-drop operation succeeds or not.
        ///
        /// For objects of version 3 or newer, this request determines the
        /// final result of the drag-and-drop operation. If the end result
        /// is that no mime types were accepted, the drag-and-drop operation
        /// will be cancelled and the corresponding drag source will receive
        /// wl_data_source.cancelled. Clients may still use this event in
        /// conjunction with wl_data_source.action for feedback.
        Accept {
            /// Serial number of the accept request,
            serial: u32,
            /// Mime type accepted by the client,
            mime_type: String,
        },
        /// Request that the data is transferred
        ///
        /// To transfer the offered data, the client issues this request
        /// and indicates the mime type it wants to receive.  The transfer
        /// happens through the passed file descriptor (typically created
        /// with the pipe system call).  The source client writes the data
        /// in the mime type representation requested and then closes the
        /// file descriptor.
        ///
        /// The receiving client reads from the read end of the pipe until
        /// EOF and then closes its end, at which point the transfer is
        /// complete.
        ///
        /// This request may happen multiple times for different mime types,
        /// both before and after wl_data_device.drop. Drag-and-drop destination
        /// clients may preemptively fetch data or examine it more closely to
        /// determine acceptance.
        Receive {
            /// Mime type desired by receiver,
            mime_type: String,
            /// File descriptor for data transfer,
            fd: RawFd,
        },
        /// Destroy data offer
        ///
        /// Destroy the data offer.
        Destroy,
        /// The offer will no longer be used
        ///
        /// Notifies the compositor that the drag destination successfully
        /// finished the drag-and-drop operation.
        ///
        /// Upon receiving this request, the compositor will emit
        /// wl_data_source.dnd_finished on the drag source client.
        ///
        /// It is a client error to perform other requests than
        /// wl_data_offer.destroy after this one. It is also an error to perform
        /// this request after a NULL mime type has been set in
        /// wl_data_offer.accept or no action was received through
        /// wl_data_offer.action.
        ///
        /// If wl_data_offer.finish request is received for a non drag and drop
        /// operation, the invalid_finish protocol error is raised.
        Finish,
        /// Set the available/preferred drag-and-drop actions
        ///
        /// Sets the actions that the destination side client supports for
        /// this operation. This request may trigger the emission of
        /// wl_data_source.action and wl_data_offer.action events if the compositor
        /// needs to change the selected action.
        ///
        /// This request can be called multiple times throughout the
        /// drag-and-drop operation, typically in response to wl_data_device.enter
        /// or wl_data_device.motion events.
        ///
        /// This request determines the final result of the drag-and-drop
        /// operation. If the end result is that no action is accepted,
        /// the drag source will receive wl_data_source.cancelled.
        ///
        /// The dnd_actions argument must contain only values expressed in the
        /// wl_data_device_manager.dnd_actions enum, and the preferred_action
        /// argument must only contain one of those values set, otherwise it
        /// will result in a protocol error.
        ///
        /// While managing an "ask" action, the destination drag-and-drop client
        /// may perform further wl_data_offer.receive requests, and is expected
        /// to perform one last wl_data_offer.set_actions request with a preferred
        /// action other than "ask" (and optionally wl_data_offer.accept) before
        /// requesting wl_data_offer.finish, in order to convey the action selected
        /// by the user. If the preferred action is not in the
        /// wl_data_offer.source_actions mask, an error will be raised.
        ///
        /// If the "ask" action is dismissed (e.g. user cancellation), the client
        /// is expected to perform wl_data_offer.destroy right away.
        ///
        /// This request can only be made on drag-and-drop offers, a protocol error
        /// will be raised otherwise.
        SetActions {
            /// Actions supported by the destination client,
            dnd_actions: u32,
            /// Action preferred by the destination client,
            preferred_action: u32,
        },
    }
    impl Parser for crate::wayland_objects::wl_data_offer::WlDataOffer {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Accept { serial: req.uint()?, mime_type: req.string()? },
                1 => Receive { mime_type: req.string()?, fd: req.fd()? },
                2 => Destroy,
                3 => Finish,
                4 => SetActions { dnd_actions: req.uint()?, preferred_action: req.uint()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Advertise offered mime type
        ///
        /// Sent immediately after creating the wl_data_offer object.  One
        /// event per offered mime type.
        Offer {
            /// Offered mime type,
            mime_type: String,
        },
        /// Notify the source-side available actions
        ///
        /// This event indicates the actions offered by the data source. It
        /// will be sent immediately after creating the wl_data_offer object,
        /// or anytime the source side changes its offered actions through
        /// wl_data_source.set_actions.
        SourceActions {
            /// Actions offered by the data source,
            source_actions: u32,
        },
        /// Notify the selected action
        ///
        /// This event indicates the action selected by the compositor after
        /// matching the source/destination side actions. Only one action (or
        /// none) will be offered here.
        ///
        /// This event can be emitted multiple times during the drag-and-drop
        /// operation in response to destination side action changes through
        /// wl_data_offer.set_actions.
        ///
        /// This event will no longer be emitted after wl_data_device.drop
        /// happened on the drag-and-drop destination, the client must
        /// honor the last action received, or the last preferred one set
        /// through wl_data_offer.set_actions when handling an "ask" action.
        ///
        /// Compositors may also change the selected action on the fly, mainly
        /// in response to keyboard modifier changes during the drag-and-drop
        /// operation.
        ///
        /// The most recent action received is always the valid one. Prior to
        /// receiving wl_data_device.drop, the chosen action may change (e.g.
        /// due to keyboard modifiers being pressed). At the time of receiving
        /// wl_data_device.drop the drag-and-drop destination must honor the
        /// last action received.
        ///
        /// Action changes may still happen after wl_data_device.drop,
        /// especially on "ask" actions, where the drag-and-drop destination
        /// may choose another action afterwards. Action changes happening
        /// at this stage are always the result of inter-client negotiation, the
        /// compositor shall no longer be able to induce a different action.
        ///
        /// Upon "ask" actions, it is expected that the drag-and-drop destination
        /// may potentially choose a different action and/or mime type,
        /// based on wl_data_offer.source_actions and finally chosen by the
        /// user (e.g. popping up a menu with the available options). The
        /// final wl_data_offer.set_actions and wl_data_offer.accept requests
        /// must happen before the call to wl_data_offer.finish.
        Action {
            /// Action selected by the compositor,
            dnd_action: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_data_offer::WlDataOffer {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Offer { mime_type } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.string(mime_type);
                    raw
                }
                SourceActions { source_actions } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.uint(source_actions);
                    raw
                }
                Action { dnd_action } => {
                    let mut raw = RawEvent::new(self.id(), 2, 1);
                    raw.uint(dnd_action);
                    raw
                }
            }
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Finish request was called untimely,
        InvalidFinish = 0,
        /// Action mask contains invalid values,
        InvalidActionMask = 1,
        /// Action argument has an invalid value,
        InvalidAction = 2,
        /// Offer doesn't accept this request,
        InvalidOffer = 3,
    }
}

/// Offer to transfer data
///
/// The wl_data_source object is the source side of a wl_data_offer.
/// It is created by the source client in a data transfer and
/// provides a way to describe the offered data and a way to respond
/// to requests to transfer the data.
pub mod wl_data_source {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Add an offered mime type
        ///
        /// This request adds a mime type to the set of mime types
        /// advertised to targets.  Can be called several times to offer
        /// multiple types.
        Offer {
            /// Mime type offered by the data source,
            mime_type: String,
        },
        /// Destroy the data source
        ///
        /// Destroy the data source.
        Destroy,
        /// Set the available drag-and-drop actions
        ///
        /// Sets the actions that the source side client supports for this
        /// operation. This request may trigger wl_data_source.action and
        /// wl_data_offer.action events if the compositor needs to change the
        /// selected action.
        ///
        /// The dnd_actions argument must contain only values expressed in the
        /// wl_data_device_manager.dnd_actions enum, otherwise it will result
        /// in a protocol error.
        ///
        /// This request must be made once only, and can only be made on sources
        /// used in drag-and-drop, so it must be performed before
        /// wl_data_device.start_drag. Attempting to use the source other than
        /// for drag-and-drop will raise a protocol error.
        SetActions {
            /// Actions supported by the data source,
            dnd_actions: u32,
        },
    }
    impl Parser for crate::wayland_objects::wl_data_source::WlDataSource {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Offer { mime_type: req.string()? },
                1 => Destroy,
                2 => SetActions { dnd_actions: req.uint()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// A target accepts an offered mime type
        ///
        /// Sent when a target accepts pointer_focus or motion events.  If
        /// a target does not accept any of the offered types, type is NULL.
        ///
        /// Used for feedback during drag-and-drop.
        Target {
            /// Mime type accepted by the target,
            mime_type: String,
        },
        /// Send the data
        ///
        /// Request for data from the client.  Send the data as the
        /// specified mime type over the passed file descriptor, then
        /// close it.
        Send {
            /// Mime type for the data,
            mime_type: String,
            /// File descriptor for the data,
            fd: RawFd,
        },
        /// Selection was cancelled
        ///
        /// This data source is no longer valid. There are several reasons why
        /// this could happen:
        ///
        /// - The data source has been replaced by another data source.
        /// - The drag-and-drop operation was performed, but the drop destination
        /// did not accept any of the mime types offered through
        /// wl_data_source.target.
        /// - The drag-and-drop operation was performed, but the drop destination
        /// did not select any of the actions present in the mask offered through
        /// wl_data_source.action.
        /// - The drag-and-drop operation was performed but didn't happen over a
        /// surface.
        /// - The compositor cancelled the drag-and-drop operation (e.g. compositor
        /// dependent timeouts to avoid stale drag-and-drop transfers).
        ///
        /// The client should clean up and destroy this data source.
        ///
        /// For objects of version 2 or older, wl_data_source.cancelled will
        /// only be emitted if the data source was replaced by another data
        /// source.
        Cancelled,
        /// The drag-and-drop operation physically finished
        ///
        /// The user performed the drop action. This event does not indicate
        /// acceptance, wl_data_source.cancelled may still be emitted afterwards
        /// if the drop destination does not accept any mime type.
        ///
        /// However, this event might however not be received if the compositor
        /// cancelled the drag-and-drop operation before this event could happen.
        ///
        /// Note that the data_source may still be used in the future and should
        /// not be destroyed here.
        DndDropPerformed,
        /// The drag-and-drop operation concluded
        ///
        /// The drop destination finished interoperating with this data
        /// source, so the client is now free to destroy this data source and
        /// free all associated data.
        ///
        /// If the action used to perform the operation was "move", the
        /// source can now delete the transferred data.
        DndFinished,
        /// Notify the selected action
        ///
        /// This event indicates the action selected by the compositor after
        /// matching the source/destination side actions. Only one action (or
        /// none) will be offered here.
        ///
        /// This event can be emitted multiple times during the drag-and-drop
        /// operation, mainly in response to destination side changes through
        /// wl_data_offer.set_actions, and as the data device enters/leaves
        /// surfaces.
        ///
        /// It is only possible to receive this event after
        /// wl_data_source.dnd_drop_performed if the drag-and-drop operation
        /// ended in an "ask" action, in which case the final wl_data_source.action
        /// event will happen immediately before wl_data_source.dnd_finished.
        ///
        /// Compositors may also change the selected action on the fly, mainly
        /// in response to keyboard modifier changes during the drag-and-drop
        /// operation.
        ///
        /// The most recent action received is always the valid one. The chosen
        /// action may change alongside negotiation (e.g. an "ask" action can turn
        /// into a "move" operation), so the effects of the final action must
        /// always be applied in wl_data_offer.dnd_finished.
        ///
        /// Clients can trigger cursor surface changes from this point, so
        /// they reflect the current action.
        Action {
            /// Action selected by the compositor,
            dnd_action: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_data_source::WlDataSource {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Target { mime_type } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.string(mime_type);
                    raw
                }
                Send { mime_type, fd } => {
                    let mut raw = RawEvent::new(self.id(), 1, 2);
                    raw.string(mime_type);
                    raw.fd(fd);
                    raw
                }
                Cancelled {} => {
                    let mut raw = RawEvent::new(self.id(), 2, 0);
                    raw
                }
                DndDropPerformed {} => {
                    let mut raw = RawEvent::new(self.id(), 3, 0);
                    raw
                }
                DndFinished {} => {
                    let mut raw = RawEvent::new(self.id(), 4, 0);
                    raw
                }
                Action { dnd_action } => {
                    let mut raw = RawEvent::new(self.id(), 5, 1);
                    raw.uint(dnd_action);
                    raw
                }
            }
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Action mask contains invalid values,
        InvalidActionMask = 0,
        /// Source doesn't accept this request,
        InvalidSource = 1,
    }
}

/// Data transfer device
///
/// There is one wl_data_device per seat which can be obtained
/// from the global wl_data_device_manager singleton.
///
/// A wl_data_device provides access to inter-client data transfer
/// mechanisms such as copy-and-paste and drag-and-drop.
pub mod wl_data_device {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Start drag-and-drop operation
        ///
        /// This request asks the compositor to start a drag-and-drop
        /// operation on behalf of the client.
        ///
        /// The source argument is the data source that provides the data
        /// for the eventual data transfer. If source is NULL, enter, leave
        /// and motion events are sent only to the client that initiated the
        /// drag and the client is expected to handle the data passing
        /// internally. If source is destroyed, the drag-and-drop session will be
        /// cancelled.
        ///
        /// The origin surface is the surface where the drag originates and
        /// the client must have an active implicit grab that matches the
        /// serial.
        ///
        /// The icon surface is an optional (can be NULL) surface that
        /// provides an icon to be moved around with the cursor.  Initially,
        /// the top-left corner of the icon surface is placed at the cursor
        /// hotspot, but subsequent wl_surface.attach request can move the
        /// relative position. Attach requests must be confirmed with
        /// wl_surface.commit as usual. The icon surface is given the role of
        /// a drag-and-drop icon. If the icon surface already has another role,
        /// it raises a protocol error.
        ///
        /// The input region is ignored for wl_surfaces with the role of a
        /// drag-and-drop icon.
        StartDrag {
            /// Data source for the eventual transfer,
            source: WlDataSourceId,
            /// Surface where the drag originates,
            origin: WlSurfaceId,
            /// Drag-and-drop icon surface,
            icon: WlSurfaceId,
            /// Serial number of the implicit grab on the origin,
            serial: u32,
        },
        /// Copy data to the selection
        ///
        /// This request asks the compositor to set the selection
        /// to the data from the source on behalf of the client.
        ///
        /// To unset the selection, set the source to NULL.
        SetSelection {
            /// Data source for the selection,
            source: WlDataSourceId,
            /// Serial number of the event that triggered this request,
            serial: u32,
        },
        /// Destroy data device
        ///
        /// This request destroys the data device.
        Release,
    }
    impl Parser for crate::wayland_objects::wl_data_device::WlDataDevice {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => StartDrag {
                    source: req.id()?,
                    origin: req.id()?,
                    icon: req.id()?,
                    serial: req.uint()?,
                },
                1 => SetSelection { source: req.id()?, serial: req.uint()? },
                2 => Release,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Introduce a new wl_data_offer
        ///
        /// The data_offer event introduces a new wl_data_offer object,
        /// which will subsequently be used in either the
        /// data_device.enter event (for drag-and-drop) or the
        /// data_device.selection event (for selections).  Immediately
        /// following the data_device.data_offer event, the new data_offer
        /// object will send out data_offer.offer events to describe the
        /// mime types it offers.
        DataOffer {
            /// The new data_offer object,
            id: WlDataOfferId,
        },
        /// Initiate drag-and-drop session
        ///
        /// This event is sent when an active drag-and-drop pointer enters
        /// a surface owned by the client.  The position of the pointer at
        /// enter time is provided by the x and y arguments, in surface-local
        /// coordinates.
        Enter {
            /// Serial number of the enter event,
            serial: u32,
            /// Client surface entered,
            surface: WlSurfaceId,
            /// Surface-local x coordinate,
            x: f32,
            /// Surface-local y coordinate,
            y: f32,
            /// Source data_offer object,
            id: WlDataOfferId,
        },
        /// End drag-and-drop session
        ///
        /// This event is sent when the drag-and-drop pointer leaves the
        /// surface and the session ends.  The client must destroy the
        /// wl_data_offer introduced at enter time at this point.
        Leave,
        /// Drag-and-drop session motion
        ///
        /// This event is sent when the drag-and-drop pointer moves within
        /// the currently focused surface. The new position of the pointer
        /// is provided by the x and y arguments, in surface-local
        /// coordinates.
        Motion {
            /// Timestamp with millisecond granularity,
            time: u32,
            /// Surface-local x coordinate,
            x: f32,
            /// Surface-local y coordinate,
            y: f32,
        },
        /// End drag-and-drop session successfully
        ///
        /// The event is sent when a drag-and-drop operation is ended
        /// because the implicit grab is removed.
        ///
        /// The drag-and-drop destination is expected to honor the last action
        /// received through wl_data_offer.action, if the resulting action is
        /// "copy" or "move", the destination can still perform
        /// wl_data_offer.receive requests, and is expected to end all
        /// transfers with a wl_data_offer.finish request.
        ///
        /// If the resulting action is "ask", the action will not be considered
        /// final. The drag-and-drop destination is expected to perform one last
        /// wl_data_offer.set_actions request, or wl_data_offer.destroy in order
        /// to cancel the operation.
        Drop,
        /// Advertise new selection
        ///
        /// The selection event is sent out to notify the client of a new
        /// wl_data_offer for the selection for this device.  The
        /// data_device.data_offer and the data_offer.offer events are
        /// sent out immediately before this event to introduce the data
        /// offer object.  The selection event is sent to a client
        /// immediately before receiving keyboard focus and when a new
        /// selection is set while the client has keyboard focus.  The
        /// data_offer is valid until a new data_offer or NULL is received
        /// or until the client loses keyboard focus.  Switching surface with
        /// keyboard focus within the same client doesn't mean a new selection
        /// will be sent.  The client must destroy the previous selection
        /// data_offer, if any, upon receiving this event.
        Selection {
            /// Selection data_offer object,
            id: WlDataOfferId,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_data_device::WlDataDevice {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                DataOffer { id } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.id(id);
                    raw
                }
                Enter { serial, surface, x, y, id } => {
                    let mut raw = RawEvent::new(self.id(), 1, 5);
                    raw.uint(serial);
                    raw.id(surface);
                    raw.fixed(x);
                    raw.fixed(y);
                    raw.id(id);
                    raw
                }
                Leave {} => {
                    let mut raw = RawEvent::new(self.id(), 2, 0);
                    raw
                }
                Motion { time, x, y } => {
                    let mut raw = RawEvent::new(self.id(), 3, 3);
                    raw.uint(time);
                    raw.fixed(x);
                    raw.fixed(y);
                    raw
                }
                Drop {} => {
                    let mut raw = RawEvent::new(self.id(), 4, 0);
                    raw
                }
                Selection { id } => {
                    let mut raw = RawEvent::new(self.id(), 5, 1);
                    raw.id(id);
                    raw
                }
            }
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Given wl_surface has another role,
        Role = 0,
    }
}

/// Data transfer interface
///
/// The wl_data_device_manager is a singleton global object that
/// provides access to inter-client data transfer mechanisms such as
/// copy-and-paste and drag-and-drop.  These mechanisms are tied to
/// a wl_seat and this interface lets a client get a wl_data_device
/// corresponding to a wl_seat.
///
/// Depending on the version bound, the objects created from the bound
/// wl_data_device_manager object will have different requirements for
/// functioning properly. See wl_data_source.set_actions,
/// wl_data_offer.accept and wl_data_offer.finish for details.
pub mod wl_data_device_manager {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Create a new data source
        ///
        /// Create a new data source.
        CreateDataSource {
            /// Data source to create,
            id: WlDataSourceId,
        },
        /// Create a new data device
        ///
        /// Create a new data device for a given seat.
        GetDataDevice {
            /// Data device to create,
            id: WlDataDeviceId,
            /// Seat associated with the data device,
            seat: WlSeatId,
        },
    }
    impl Parser for crate::wayland_objects::wl_data_device_manager::WlDataDeviceManager {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => CreateDataSource { id: req.id()? },
                1 => GetDataDevice { id: req.id()?, seat: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    /// Drag and drop actions
    ///
    /// This is a bitmask of the available/preferred actions in a
    /// drag-and-drop operation.
    ///
    /// In the compositor, the selected action is a result of matching the
    /// actions offered by the source and destination sides.  "action" events
    /// with a "none" action will be sent to both source and destination if
    /// there is no match. All further checks will effectively happen on
    /// (source actions ∩ destination actions).
    ///
    /// In addition, compositors may also pick different actions in
    /// reaction to key modifiers being pressed. One common design that
    /// is used in major toolkits (and the behavior recommended for
    /// compositors) is:
    ///
    /// - If no modifiers are pressed, the first match (in bit order)
    /// will be used.
    /// - Pressing Shift selects "move", if enabled in the mask.
    /// - Pressing Control selects "copy", if enabled in the mask.
    ///
    /// Behavior beyond that is considered implementation-dependent.
    /// Compositors may for example bind other modifiers (like Alt/Meta)
    /// or drags initiated with other buttons than BTN_LEFT to specific
    /// actions (e.g. "ask").
    #[derive(Debug)]
    pub enum DndAction {
        /// No action,
        None = 0,
        /// Copy action,
        Copy = 1,
        /// Move action,
        Move = 2,
        /// Ask action,
        Ask = 4,
    }
}

/// Create desktop-style surfaces
///
/// This interface is implemented by servers that provide
/// desktop-style user interfaces.
///
/// It allows clients to associate a wl_shell_surface with
/// a basic surface.
///
/// Note! This protocol is deprecated and not intended for production use.
/// For desktop-style user interfaces, use xdg_shell. Compositors and clients
/// should not implement this interface.
pub mod wl_shell {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Create a shell surface from a surface
        ///
        /// Create a shell surface for an existing surface. This gives
        /// the wl_surface the role of a shell surface. If the wl_surface
        /// already has another role, it raises a protocol error.
        ///
        /// Only one shell surface can be associated with a given surface.
        GetShellSurface {
            /// Shell surface to create,
            id: WlShellSurfaceId,
            /// Surface to be given the shell surface role,
            surface: WlSurfaceId,
        },
    }
    impl Parser for crate::wayland_objects::wl_shell::WlShell {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => GetShellSurface { id: req.id()?, surface: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Given wl_surface has another role,
        Role = 0,
    }
}

/// Desktop-style metadata interface
///
/// An interface that may be implemented by a wl_surface, for
/// implementations that provide a desktop-style user interface.
///
/// It provides requests to treat surfaces like toplevel, fullscreen
/// or popup windows, move, resize or maximize them, associate
/// metadata like title and class, etc.
///
/// On the server side the object is automatically destroyed when
/// the related wl_surface is destroyed. On the client side,
/// wl_shell_surface_destroy() must be called before destroying
/// the wl_surface object.
pub mod wl_shell_surface {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Respond to a ping event
        ///
        /// A client must respond to a ping event with a pong request or
        /// the client may be deemed unresponsive.
        Pong {
            /// Serial number of the ping event,
            serial: u32,
        },
        /// Start an interactive move
        ///
        /// Start a pointer-driven move of the surface.
        ///
        /// This request must be used in response to a button press event.
        /// The server may ignore move requests depending on the state of
        /// the surface (e.g. fullscreen or maximized).
        Move {
            /// Seat whose pointer is used,
            seat: WlSeatId,
            /// Serial number of the implicit grab on the pointer,
            serial: u32,
        },
        /// Start an interactive resize
        ///
        /// Start a pointer-driven resizing of the surface.
        ///
        /// This request must be used in response to a button press event.
        /// The server may ignore resize requests depending on the state of
        /// the surface (e.g. fullscreen or maximized).
        Resize {
            /// Seat whose pointer is used,
            seat: WlSeatId,
            /// Serial number of the implicit grab on the pointer,
            serial: u32,
            /// Which edge or corner is being dragged,
            edges: u32,
        },
        /// Make the surface a toplevel surface
        ///
        /// Map the surface as a toplevel surface.
        ///
        /// A toplevel surface is not fullscreen, maximized or transient.
        SetToplevel,
        /// Make the surface a transient surface
        ///
        /// Map the surface relative to an existing surface.
        ///
        /// The x and y arguments specify the location of the upper left
        /// corner of the surface relative to the upper left corner of the
        /// parent surface, in surface-local coordinates.
        ///
        /// The flags argument controls details of the transient behaviour.
        SetTransient {
            /// Parent surface,
            parent: WlSurfaceId,
            /// Surface-local x coordinate,
            x: i32,
            /// Surface-local y coordinate,
            y: i32,
            /// Transient surface behavior,
            flags: u32,
        },
        /// Make the surface a fullscreen surface
        ///
        /// Map the surface as a fullscreen surface.
        ///
        /// If an output parameter is given then the surface will be made
        /// fullscreen on that output. If the client does not specify the
        /// output then the compositor will apply its policy - usually
        /// choosing the output on which the surface has the biggest surface
        /// area.
        ///
        /// The client may specify a method to resolve a size conflict
        /// between the output size and the surface size - this is provided
        /// through the method parameter.
        ///
        /// The framerate parameter is used only when the method is set
        /// to "driver", to indicate the preferred framerate. A value of 0
        /// indicates that the client does not care about framerate.  The
        /// framerate is specified in mHz, that is framerate of 60000 is 60Hz.
        ///
        /// A method of "scale" or "driver" implies a scaling operation of
        /// the surface, either via a direct scaling operation or a change of
        /// the output mode. This will override any kind of output scaling, so
        /// that mapping a surface with a buffer size equal to the mode can
        /// fill the screen independent of buffer_scale.
        ///
        /// A method of "fill" means we don't scale up the buffer, however
        /// any output scale is applied. This means that you may run into
        /// an edge case where the application maps a buffer with the same
        /// size of the output mode but buffer_scale 1 (thus making a
        /// surface larger than the output). In this case it is allowed to
        /// downscale the results to fit the screen.
        ///
        /// The compositor must reply to this request with a configure event
        /// with the dimensions for the output on which the surface will
        /// be made fullscreen.
        SetFullscreen {
            /// Method for resolving size conflict,
            method: u32,
            /// Framerate in mHz,
            framerate: u32,
            /// Output on which the surface is to be fullscreen,
            output: WlOutputId,
        },
        /// Make the surface a popup surface
        ///
        /// Map the surface as a popup.
        ///
        /// A popup surface is a transient surface with an added pointer
        /// grab.
        ///
        /// An existing implicit grab will be changed to owner-events mode,
        /// and the popup grab will continue after the implicit grab ends
        /// (i.e. releasing the mouse button does not cause the popup to
        /// be unmapped).
        ///
        /// The popup grab continues until the window is destroyed or a
        /// mouse button is pressed in any other client's window. A click
        /// in any of the client's surfaces is reported as normal, however,
        /// clicks in other clients' surfaces will be discarded and trigger
        /// the callback.
        ///
        /// The x and y arguments specify the location of the upper left
        /// corner of the surface relative to the upper left corner of the
        /// parent surface, in surface-local coordinates.
        SetPopup {
            /// Seat whose pointer is used,
            seat: WlSeatId,
            /// Serial number of the implicit grab on the pointer,
            serial: u32,
            /// Parent surface,
            parent: WlSurfaceId,
            /// Surface-local x coordinate,
            x: i32,
            /// Surface-local y coordinate,
            y: i32,
            /// Transient surface behavior,
            flags: u32,
        },
        /// Make the surface a maximized surface
        ///
        /// Map the surface as a maximized surface.
        ///
        /// If an output parameter is given then the surface will be
        /// maximized on that output. If the client does not specify the
        /// output then the compositor will apply its policy - usually
        /// choosing the output on which the surface has the biggest surface
        /// area.
        ///
        /// The compositor will reply with a configure event telling
        /// the expected new surface size. The operation is completed
        /// on the next buffer attach to this surface.
        ///
        /// A maximized surface typically fills the entire output it is
        /// bound to, except for desktop elements such as panels. This is
        /// the main difference between a maximized shell surface and a
        /// fullscreen shell surface.
        ///
        /// The details depend on the compositor implementation.
        SetMaximized {
            /// Output on which the surface is to be maximized,
            output: WlOutputId,
        },
        /// Set surface title
        ///
        /// Set a short title for the surface.
        ///
        /// This string may be used to identify the surface in a task bar,
        /// window list, or other user interface elements provided by the
        /// compositor.
        ///
        /// The string must be encoded in UTF-8.
        SetTitle {
            /// Surface title,
            title: String,
        },
        /// Set surface class
        ///
        /// Set a class for the surface.
        ///
        /// The surface class identifies the general class of applications
        /// to which the surface belongs. A common convention is to use the
        /// file name (or the full path if it is a non-standard location) of
        /// the application's .desktop file as the class.
        SetClass {
            /// Surface class,
            class_: String,
        },
    }
    impl Parser for crate::wayland_objects::wl_shell_surface::WlShellSurface {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Pong { serial: req.uint()? },
                1 => Move { seat: req.id()?, serial: req.uint()? },
                2 => Resize { seat: req.id()?, serial: req.uint()?, edges: req.uint()? },
                3 => SetToplevel,
                4 => SetTransient {
                    parent: req.id()?,
                    x: req.int()?,
                    y: req.int()?,
                    flags: req.uint()?,
                },
                5 => SetFullscreen {
                    method: req.uint()?,
                    framerate: req.uint()?,
                    output: req.id()?,
                },
                6 => SetPopup {
                    seat: req.id()?,
                    serial: req.uint()?,
                    parent: req.id()?,
                    x: req.int()?,
                    y: req.int()?,
                    flags: req.uint()?,
                },
                7 => SetMaximized { output: req.id()? },
                8 => SetTitle { title: req.string()? },
                9 => SetClass { class_: req.string()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Ping client
        ///
        /// Ping a client to check if it is receiving events and sending
        /// requests. A client is expected to reply with a pong request.
        Ping {
            /// Serial number of the ping,
            serial: u32,
        },
        /// Suggest resize
        ///
        /// The configure event asks the client to resize its surface.
        ///
        /// The size is a hint, in the sense that the client is free to
        /// ignore it if it doesn't resize, pick a smaller size (to
        /// satisfy aspect ratio or resize in steps of NxM pixels).
        ///
        /// The edges parameter provides a hint about how the surface
        /// was resized. The client may use this information to decide
        /// how to adjust its content to the new size (e.g. a scrolling
        /// area might adjust its content position to leave the viewable
        /// content unmoved).
        ///
        /// The client is free to dismiss all but the last configure
        /// event it received.
        ///
        /// The width and height arguments specify the size of the window
        /// in surface-local coordinates.
        Configure {
            /// How the surface was resized,
            edges: u32,
            /// New width of the surface,
            width: i32,
            /// New height of the surface,
            height: i32,
        },
        /// Popup interaction is done
        ///
        /// The popup_done event is sent out when a popup grab is broken,
        /// that is, when the user clicks a surface that doesn't belong
        /// to the client owning the popup surface.
        PopupDone,
    }
    impl EventWriter for crate::wayland_objects::wl_shell_surface::WlShellSurface {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Ping { serial } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(serial);
                    raw
                }
                Configure { edges, width, height } => {
                    let mut raw = RawEvent::new(self.id(), 1, 3);
                    raw.uint(edges);
                    raw.int(width);
                    raw.int(height);
                    raw
                }
                PopupDone {} => {
                    let mut raw = RawEvent::new(self.id(), 2, 0);
                    raw
                }
            }
        }
    }
    /// Edge values for resizing
    ///
    /// These values are used to indicate which edge of a surface
    /// is being dragged in a resize operation. The server may
    /// use this information to adapt its behavior, e.g. choose
    /// an appropriate cursor image.
    #[derive(Debug)]
    pub enum Resize {
        /// No edge,
        None = 0,
        /// Top edge,
        Top = 1,
        /// Bottom edge,
        Bottom = 2,
        /// Left edge,
        Left = 4,
        /// Top and left edges,
        TopLeft = 5,
        /// Bottom and left edges,
        BottomLeft = 6,
        /// Right edge,
        Right = 8,
        /// Top and right edges,
        TopRight = 9,
        /// Bottom and right edges,
        BottomRight = 10,
    }
    /// Details of transient behaviour
    ///
    /// These flags specify details of the expected behaviour
    /// of transient surfaces. Used in the set_transient request.
    #[derive(Debug)]
    pub enum Transient {
        /// Do not set keyboard focus,
        Inactive = 1,
    }
    /// Different method to set the surface fullscreen
    ///
    /// Hints to indicate to the compositor how to deal with a conflict
    /// between the dimensions of the surface and the dimensions of the
    /// output. The compositor is free to ignore this parameter.
    #[derive(Debug)]
    pub enum FullscreenMethod {
        /// No preference, apply default policy,
        Default = 0,
        /// Scale, preserve the surface's aspect ratio and center on output,
        Scale = 1,
        /// Switch output mode to the smallest mode that can fit the surface, add black borders to compensate size mismatch,
        Driver = 2,
        /// No upscaling, center on output and add black borders to compensate size mismatch,
        Fill = 3,
    }
}

/// An onscreen surface
///
/// A surface is a rectangular area that may be displayed on zero
/// or more outputs, and shown any number of times at the compositor's
/// discretion. They can present wl_buffers, receive user input, and
/// define a local coordinate system.
///
/// The size of a surface (and relative positions on it) is described
/// in surface-local coordinates, which may differ from the buffer
/// coordinates of the pixel content, in case a buffer_transform
/// or a buffer_scale is used.
///
/// A surface without a "role" is fairly useless: a compositor does
/// not know where, when or how to present it. The role is the
/// purpose of a wl_surface. Examples of roles are a cursor for a
/// pointer (as set by wl_pointer.set_cursor), a drag icon
/// (wl_data_device.start_drag), a sub-surface
/// (wl_subcompositor.get_subsurface), and a window as defined by a
/// shell protocol (e.g. wl_shell.get_shell_surface).
///
/// A surface can have only one role at a time. Initially a
/// wl_surface does not have a role. Once a wl_surface is given a
/// role, it is set permanently for the whole lifetime of the
/// wl_surface object. Giving the current role again is allowed,
/// unless explicitly forbidden by the relevant interface
/// specification.
///
/// Surface roles are given by requests in other interfaces such as
/// wl_pointer.set_cursor. The request should explicitly mention
/// that this request gives a role to a wl_surface. Often, this
/// request also creates a new protocol object that represents the
/// role and adds additional functionality to wl_surface. When a
/// client wants to destroy a wl_surface, they must destroy this role
/// object before the wl_surface, otherwise a defunct_role_object error is
/// sent.
///
/// Destroying the role object does not remove the role from the
/// wl_surface, but it may stop the wl_surface from "playing the role".
/// For instance, if a wl_subsurface object is destroyed, the wl_surface
/// it was created for will be unmapped and forget its position and
/// z-order. It is allowed to create a wl_subsurface for the same
/// wl_surface again, but it is not allowed to use the wl_surface as
/// a cursor (cursor is a different role than sub-surface, and role
/// switching is not allowed).
pub mod wl_surface {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Delete surface
        ///
        /// Deletes the surface and invalidates its object ID.
        Destroy,
        /// Set the surface contents
        ///
        /// Set a buffer as the content of this surface.
        ///
        /// The new size of the surface is calculated based on the buffer
        /// size transformed by the inverse buffer_transform and the
        /// inverse buffer_scale. This means that at commit time the supplied
        /// buffer size must be an integer multiple of the buffer_scale. If
        /// that's not the case, an invalid_size error is sent.
        ///
        /// The x and y arguments specify the location of the new pending
        /// buffer's upper left corner, relative to the current buffer's upper
        /// left corner, in surface-local coordinates. In other words, the
        /// x and y, combined with the new surface size define in which
        /// directions the surface's size changes. Setting anything other than 0
        /// as x and y arguments is discouraged, and should instead be replaced
        /// with using the separate wl_surface.offset request.
        ///
        /// When the bound wl_surface version is 5 or higher, passing any
        /// non-zero x or y is a protocol violation, and will result in an
        /// 'invalid_offset' error being raised. The x and y arguments are ignored
        /// and do not change the pending state. To achieve equivalent semantics,
        /// use wl_surface.offset.
        ///
        /// Surface contents are double-buffered state, see wl_surface.commit.
        ///
        /// The initial surface contents are void; there is no content.
        /// wl_surface.attach assigns the given wl_buffer as the pending
        /// wl_buffer. wl_surface.commit makes the pending wl_buffer the new
        /// surface contents, and the size of the surface becomes the size
        /// calculated from the wl_buffer, as described above. After commit,
        /// there is no pending buffer until the next attach.
        ///
        /// Committing a pending wl_buffer allows the compositor to read the
        /// pixels in the wl_buffer. The compositor may access the pixels at
        /// any time after the wl_surface.commit request. When the compositor
        /// will not access the pixels anymore, it will send the
        /// wl_buffer.release event. Only after receiving wl_buffer.release,
        /// the client may reuse the wl_buffer. A wl_buffer that has been
        /// attached and then replaced by another attach instead of committed
        /// will not receive a release event, and is not used by the
        /// compositor.
        ///
        /// If a pending wl_buffer has been committed to more than one wl_surface,
        /// the delivery of wl_buffer.release events becomes undefined. A well
        /// behaved client should not rely on wl_buffer.release events in this
        /// case. Alternatively, a client could create multiple wl_buffer objects
        /// from the same backing storage or use wp_linux_buffer_release.
        ///
        /// Destroying the wl_buffer after wl_buffer.release does not change
        /// the surface contents. Destroying the wl_buffer before wl_buffer.release
        /// is allowed as long as the underlying buffer storage isn't re-used (this
        /// can happen e.g. on client process termination). However, if the client
        /// destroys the wl_buffer before receiving the wl_buffer.release event and
        /// mutates the underlying buffer storage, the surface contents become
        /// undefined immediately.
        ///
        /// If wl_surface.attach is sent with a NULL wl_buffer, the
        /// following wl_surface.commit will remove the surface content.
        Attach {
            /// Buffer of surface contents,
            buffer: WlBufferId,
            /// Surface-local x coordinate,
            x: i32,
            /// Surface-local y coordinate,
            y: i32,
        },
        /// Mark part of the surface damaged
        ///
        /// This request is used to describe the regions where the pending
        /// buffer is different from the current surface contents, and where
        /// the surface therefore needs to be repainted. The compositor
        /// ignores the parts of the damage that fall outside of the surface.
        ///
        /// Damage is double-buffered state, see wl_surface.commit.
        ///
        /// The damage rectangle is specified in surface-local coordinates,
        /// where x and y specify the upper left corner of the damage rectangle.
        ///
        /// The initial value for pending damage is empty: no damage.
        /// wl_surface.damage adds pending damage: the new pending damage
        /// is the union of old pending damage and the given rectangle.
        ///
        /// wl_surface.commit assigns pending damage as the current damage,
        /// and clears pending damage. The server will clear the current
        /// damage as it repaints the surface.
        ///
        /// Note! New clients should not use this request. Instead damage can be
        /// posted with wl_surface.damage_buffer which uses buffer coordinates
        /// instead of surface coordinates.
        Damage {
            /// Surface-local x coordinate,
            x: i32,
            /// Surface-local y coordinate,
            y: i32,
            /// Width of damage rectangle,
            width: i32,
            /// Height of damage rectangle,
            height: i32,
        },
        /// Request a frame throttling hint
        ///
        /// Request a notification when it is a good time to start drawing a new
        /// frame, by creating a frame callback. This is useful for throttling
        /// redrawing operations, and driving animations.
        ///
        /// When a client is animating on a wl_surface, it can use the 'frame'
        /// request to get notified when it is a good time to draw and commit the
        /// next frame of animation. If the client commits an update earlier than
        /// that, it is likely that some updates will not make it to the display,
        /// and the client is wasting resources by drawing too often.
        ///
        /// The frame request will take effect on the next wl_surface.commit.
        /// The notification will only be posted for one frame unless
        /// requested again. For a wl_surface, the notifications are posted in
        /// the order the frame requests were committed.
        ///
        /// The server must send the notifications so that a client
        /// will not send excessive updates, while still allowing
        /// the highest possible update rate for clients that wait for the reply
        /// before drawing again. The server should give some time for the client
        /// to draw and commit after sending the frame callback events to let it
        /// hit the next output refresh.
        ///
        /// A server should avoid signaling the frame callbacks if the
        /// surface is not visible in any way, e.g. the surface is off-screen,
        /// or completely obscured by other opaque surfaces.
        ///
        /// The object returned by this request will be destroyed by the
        /// compositor after the callback is fired and as such the client must not
        /// attempt to use it after that point.
        ///
        /// The callback_data passed in the callback is the current time, in
        /// milliseconds, with an undefined base.
        Frame {
            /// Callback object for the frame request,
            callback: WlCallbackId,
        },
        /// Set opaque region
        ///
        /// This request sets the region of the surface that contains
        /// opaque content.
        ///
        /// The opaque region is an optimization hint for the compositor
        /// that lets it optimize the redrawing of content behind opaque
        /// regions.  Setting an opaque region is not required for correct
        /// behaviour, but marking transparent content as opaque will result
        /// in repaint artifacts.
        ///
        /// The opaque region is specified in surface-local coordinates.
        ///
        /// The compositor ignores the parts of the opaque region that fall
        /// outside of the surface.
        ///
        /// Opaque region is double-buffered state, see wl_surface.commit.
        ///
        /// wl_surface.set_opaque_region changes the pending opaque region.
        /// wl_surface.commit copies the pending region to the current region.
        /// Otherwise, the pending and current regions are never changed.
        ///
        /// The initial value for an opaque region is empty. Setting the pending
        /// opaque region has copy semantics, and the wl_region object can be
        /// destroyed immediately. A NULL wl_region causes the pending opaque
        /// region to be set to empty.
        SetOpaqueRegion {
            /// Opaque region of the surface,
            region: WlRegionId,
        },
        /// Set input region
        ///
        /// This request sets the region of the surface that can receive
        /// pointer and touch events.
        ///
        /// Input events happening outside of this region will try the next
        /// surface in the server surface stack. The compositor ignores the
        /// parts of the input region that fall outside of the surface.
        ///
        /// The input region is specified in surface-local coordinates.
        ///
        /// Input region is double-buffered state, see wl_surface.commit.
        ///
        /// wl_surface.set_input_region changes the pending input region.
        /// wl_surface.commit copies the pending region to the current region.
        /// Otherwise the pending and current regions are never changed,
        /// except cursor and icon surfaces are special cases, see
        /// wl_pointer.set_cursor and wl_data_device.start_drag.
        ///
        /// The initial value for an input region is infinite. That means the
        /// whole surface will accept input. Setting the pending input region
        /// has copy semantics, and the wl_region object can be destroyed
        /// immediately. A NULL wl_region causes the input region to be set
        /// to infinite.
        SetInputRegion {
            /// Input region of the surface,
            region: WlRegionId,
        },
        /// Commit pending surface state
        ///
        /// Surface state (input, opaque, and damage regions, attached buffers,
        /// etc.) is double-buffered. Protocol requests modify the pending state,
        /// as opposed to the current state in use by the compositor. A commit
        /// request atomically applies all pending state, replacing the current
        /// state. After commit, the new pending state is as documented for each
        /// related request.
        ///
        /// On commit, a pending wl_buffer is applied first, and all other state
        /// second. This means that all coordinates in double-buffered state are
        /// relative to the new wl_buffer coming into use, except for
        /// wl_surface.attach itself. If there is no pending wl_buffer, the
        /// coordinates are relative to the current surface contents.
        ///
        /// All requests that need a commit to become effective are documented
        /// to affect double-buffered state.
        ///
        /// Other interfaces may add further double-buffered surface state.
        Commit,
        /// Sets the buffer transformation
        ///
        /// This request sets an optional transformation on how the compositor
        /// interprets the contents of the buffer attached to the surface. The
        /// accepted values for the transform parameter are the values for
        /// wl_output.transform.
        ///
        /// Buffer transform is double-buffered state, see wl_surface.commit.
        ///
        /// A newly created surface has its buffer transformation set to normal.
        ///
        /// wl_surface.set_buffer_transform changes the pending buffer
        /// transformation. wl_surface.commit copies the pending buffer
        /// transformation to the current one. Otherwise, the pending and current
        /// values are never changed.
        ///
        /// The purpose of this request is to allow clients to render content
        /// according to the output transform, thus permitting the compositor to
        /// use certain optimizations even if the display is rotated. Using
        /// hardware overlays and scanning out a client buffer for fullscreen
        /// surfaces are examples of such optimizations. Those optimizations are
        /// highly dependent on the compositor implementation, so the use of this
        /// request should be considered on a case-by-case basis.
        ///
        /// Note that if the transform value includes 90 or 270 degree rotation,
        /// the width of the buffer will become the surface height and the height
        /// of the buffer will become the surface width.
        ///
        /// If transform is not one of the values from the
        /// wl_output.transform enum the invalid_transform protocol error
        /// is raised.
        SetBufferTransform {
            /// Transform for interpreting buffer contents,
            transform: i32,
        },
        /// Sets the buffer scaling factor
        ///
        /// This request sets an optional scaling factor on how the compositor
        /// interprets the contents of the buffer attached to the window.
        ///
        /// Buffer scale is double-buffered state, see wl_surface.commit.
        ///
        /// A newly created surface has its buffer scale set to 1.
        ///
        /// wl_surface.set_buffer_scale changes the pending buffer scale.
        /// wl_surface.commit copies the pending buffer scale to the current one.
        /// Otherwise, the pending and current values are never changed.
        ///
        /// The purpose of this request is to allow clients to supply higher
        /// resolution buffer data for use on high resolution outputs. It is
        /// intended that you pick the same buffer scale as the scale of the
        /// output that the surface is displayed on. This means the compositor
        /// can avoid scaling when rendering the surface on that output.
        ///
        /// Note that if the scale is larger than 1, then you have to attach
        /// a buffer that is larger (by a factor of scale in each dimension)
        /// than the desired surface size.
        ///
        /// If scale is not positive the invalid_scale protocol error is
        /// raised.
        SetBufferScale {
            /// Positive scale for interpreting buffer contents,
            scale: i32,
        },
        /// Mark part of the surface damaged using buffer coordinates
        ///
        /// This request is used to describe the regions where the pending
        /// buffer is different from the current surface contents, and where
        /// the surface therefore needs to be repainted. The compositor
        /// ignores the parts of the damage that fall outside of the surface.
        ///
        /// Damage is double-buffered state, see wl_surface.commit.
        ///
        /// The damage rectangle is specified in buffer coordinates,
        /// where x and y specify the upper left corner of the damage rectangle.
        ///
        /// The initial value for pending damage is empty: no damage.
        /// wl_surface.damage_buffer adds pending damage: the new pending
        /// damage is the union of old pending damage and the given rectangle.
        ///
        /// wl_surface.commit assigns pending damage as the current damage,
        /// and clears pending damage. The server will clear the current
        /// damage as it repaints the surface.
        ///
        /// This request differs from wl_surface.damage in only one way - it
        /// takes damage in buffer coordinates instead of surface-local
        /// coordinates. While this generally is more intuitive than surface
        /// coordinates, it is especially desirable when using wp_viewport
        /// or when a drawing library (like EGL) is unaware of buffer scale
        /// and buffer transform.
        ///
        /// Note: Because buffer transformation changes and damage requests may
        /// be interleaved in the protocol stream, it is impossible to determine
        /// the actual mapping between surface and buffer damage until
        /// wl_surface.commit time. Therefore, compositors wishing to take both
        /// kinds of damage into account will have to accumulate damage from the
        /// two requests separately and only transform from one to the other
        /// after receiving the wl_surface.commit.
        DamageBuffer {
            /// Buffer-local x coordinate,
            x: i32,
            /// Buffer-local y coordinate,
            y: i32,
            /// Width of damage rectangle,
            width: i32,
            /// Height of damage rectangle,
            height: i32,
        },
        /// Set the surface contents offset
        ///
        /// The x and y arguments specify the location of the new pending
        /// buffer's upper left corner, relative to the current buffer's upper
        /// left corner, in surface-local coordinates. In other words, the
        /// x and y, combined with the new surface size define in which
        /// directions the surface's size changes.
        ///
        /// Surface location offset is double-buffered state, see
        /// wl_surface.commit.
        ///
        /// This request is semantically equivalent to and the replaces the x and y
        /// arguments in the wl_surface.attach request in wl_surface versions prior
        /// to 5. See wl_surface.attach for details.
        Offset {
            /// Surface-local x coordinate,
            x: i32,
            /// Surface-local y coordinate,
            y: i32,
        },
    }
    impl Parser for crate::wayland_objects::wl_surface::WlSurface {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => Attach { buffer: req.id()?, x: req.int()?, y: req.int()? },
                2 => Damage {
                    x: req.int()?,
                    y: req.int()?,
                    width: req.int()?,
                    height: req.int()?,
                },
                3 => Frame { callback: req.id()? },
                4 => SetOpaqueRegion { region: req.id()? },
                5 => SetInputRegion { region: req.id()? },
                6 => Commit,
                7 => SetBufferTransform { transform: req.int()? },
                8 => SetBufferScale { scale: req.int()? },
                9 => DamageBuffer {
                    x: req.int()?,
                    y: req.int()?,
                    width: req.int()?,
                    height: req.int()?,
                },
                10 => Offset { x: req.int()?, y: req.int()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Surface enters an output
        ///
        /// This is emitted whenever a surface's creation, movement, or resizing
        /// results in some part of it being within the scanout region of an
        /// output.
        ///
        /// Note that a surface may be overlapping with zero or more outputs.
        Enter {
            /// Output entered by the surface,
            output: WlOutputId,
        },
        /// Surface leaves an output
        ///
        /// This is emitted whenever a surface's creation, movement, or resizing
        /// results in it no longer having any part of it within the scanout region
        /// of an output.
        ///
        /// Clients should not use the number of outputs the surface is on for frame
        /// throttling purposes. The surface might be hidden even if no leave event
        /// has been sent, and the compositor might expect new surface content
        /// updates even if no enter event has been sent. The frame event should be
        /// used instead.
        Leave {
            /// Output left by the surface,
            output: WlOutputId,
        },
        /// Preferred buffer scale for the surface
        ///
        /// This event indicates the preferred buffer scale for this surface. It is
        /// sent whenever the compositor's preference changes.
        ///
        /// It is intended that scaling aware clients use this event to scale their
        /// content and use wl_surface.set_buffer_scale to indicate the scale they
        /// have rendered with. This allows clients to supply a higher detail
        /// buffer.
        PreferredBufferScale {
            /// Preferred scaling factor,
            factor: i32,
        },
        /// Preferred buffer transform for the surface
        ///
        /// This event indicates the preferred buffer transform for this surface.
        /// It is sent whenever the compositor's preference changes.
        ///
        /// It is intended that transform aware clients use this event to apply the
        /// transform to their content and use wl_surface.set_buffer_transform to
        /// indicate the transform they have rendered with.
        PreferredBufferTransform {
            /// Preferred transform,
            transform: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_surface::WlSurface {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Enter { output } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.id(output);
                    raw
                }
                Leave { output } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.id(output);
                    raw
                }
                PreferredBufferScale { factor } => {
                    let mut raw = RawEvent::new(self.id(), 2, 1);
                    raw.int(factor);
                    raw
                }
                PreferredBufferTransform { transform } => {
                    let mut raw = RawEvent::new(self.id(), 3, 1);
                    raw.uint(transform);
                    raw
                }
            }
        }
    }
    /// Wl_surface error values
    ///
    /// These errors can be emitted in response to wl_surface requests.
    #[derive(Debug)]
    pub enum Error {
        /// Buffer scale value is invalid,
        InvalidScale = 0,
        /// Buffer transform value is invalid,
        InvalidTransform = 1,
        /// Buffer size is invalid,
        InvalidSize = 2,
        /// Buffer offset is invalid,
        InvalidOffset = 3,
        /// Surface was destroyed before its role object,
        DefunctRoleObject = 4,
    }
}

/// Group of input devices
///
/// A seat is a group of keyboards, pointer and touch devices. This
/// object is published as a global during start up, or when such a
/// device is hot plugged.  A seat typically has a pointer and
/// maintains a keyboard focus and a pointer focus.
pub mod wl_seat {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Return pointer object
        ///
        /// The ID provided will be initialized to the wl_pointer interface
        /// for this seat.
        ///
        /// This request only takes effect if the seat has the pointer
        /// capability, or has had the pointer capability in the past.
        /// It is a protocol violation to issue this request on a seat that has
        /// never had the pointer capability. The missing_capability error will
        /// be sent in this case.
        GetPointer {
            /// Seat pointer,
            id: WlPointerId,
        },
        /// Return keyboard object
        ///
        /// The ID provided will be initialized to the wl_keyboard interface
        /// for this seat.
        ///
        /// This request only takes effect if the seat has the keyboard
        /// capability, or has had the keyboard capability in the past.
        /// It is a protocol violation to issue this request on a seat that has
        /// never had the keyboard capability. The missing_capability error will
        /// be sent in this case.
        GetKeyboard {
            /// Seat keyboard,
            id: WlKeyboardId,
        },
        /// Return touch object
        ///
        /// The ID provided will be initialized to the wl_touch interface
        /// for this seat.
        ///
        /// This request only takes effect if the seat has the touch
        /// capability, or has had the touch capability in the past.
        /// It is a protocol violation to issue this request on a seat that has
        /// never had the touch capability. The missing_capability error will
        /// be sent in this case.
        GetTouch {
            /// Seat touch interface,
            id: WlTouchId,
        },
        /// Release the seat object
        ///
        /// Using this request a client can tell the server that it is not going to
        /// use the seat object anymore.
        Release,
    }
    impl Parser for crate::wayland_objects::wl_seat::WlSeat {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => GetPointer { id: req.id()? },
                1 => GetKeyboard { id: req.id()? },
                2 => GetTouch { id: req.id()? },
                3 => Release,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Seat capabilities changed
        ///
        /// This is emitted whenever a seat gains or loses the pointer,
        /// keyboard or touch capabilities.  The argument is a capability
        /// enum containing the complete set of capabilities this seat has.
        ///
        /// When the pointer capability is added, a client may create a
        /// wl_pointer object using the wl_seat.get_pointer request. This object
        /// will receive pointer events until the capability is removed in the
        /// future.
        ///
        /// When the pointer capability is removed, a client should destroy the
        /// wl_pointer objects associated with the seat where the capability was
        /// removed, using the wl_pointer.release request. No further pointer
        /// events will be received on these objects.
        ///
        /// In some compositors, if a seat regains the pointer capability and a
        /// client has a previously obtained wl_pointer object of version 4 or
        /// less, that object may start sending pointer events again. This
        /// behavior is considered a misinterpretation of the intended behavior
        /// and must not be relied upon by the client. wl_pointer objects of
        /// version 5 or later must not send events if created before the most
        /// recent event notifying the client of an added pointer capability.
        ///
        /// The above behavior also applies to wl_keyboard and wl_touch with the
        /// keyboard and touch capabilities, respectively.
        Capabilities {
            /// Capabilities of the seat,
            capabilities: u32,
        },
        /// Unique identifier for this seat
        ///
        /// In a multi-seat configuration the seat name can be used by clients to
        /// help identify which physical devices the seat represents.
        ///
        /// The seat name is a UTF-8 string with no convention defined for its
        /// contents. Each name is unique among all wl_seat globals. The name is
        /// only guaranteed to be unique for the current compositor instance.
        ///
        /// The same seat names are used for all clients. Thus, the name can be
        /// shared across processes to refer to a specific wl_seat global.
        ///
        /// The name event is sent after binding to the seat global. This event is
        /// only sent once per seat object, and the name does not change over the
        /// lifetime of the wl_seat global.
        ///
        /// Compositors may re-use the same seat name if the wl_seat global is
        /// destroyed and re-created later.
        Name {
            /// Seat identifier,
            name: String,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_seat::WlSeat {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Capabilities { capabilities } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(capabilities);
                    raw
                }
                Name { name } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.string(name);
                    raw
                }
            }
        }
    }
    /// Seat capability bitmask
    ///
    /// This is a bitmask of capabilities this seat has; if a member is
    /// set, then it is present on the seat.
    #[derive(Debug)]
    pub enum Capability {
        /// The seat has pointer devices,
        Pointer = 1,
        /// The seat has one or more keyboards,
        Keyboard = 2,
        /// The seat has touch devices,
        Touch = 4,
    }
    /// Wl_seat error values
    ///
    /// These errors can be emitted in response to wl_seat requests.
    #[derive(Debug)]
    pub enum Error {
        /// Get_pointer, get_keyboard or get_touch called on seat without the matching capability,
        MissingCapability = 0,
    }
}

/// Pointer input device
///
/// The wl_pointer interface represents one or more input devices,
/// such as mice, which control the pointer location and pointer_focus
/// of a seat.
///
/// The wl_pointer interface generates motion, enter and leave
/// events for the surfaces that the pointer is located over,
/// and button and axis events for button presses, button releases
/// and scrolling.
pub mod wl_pointer {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Set the pointer surface
        ///
        /// Set the pointer surface, i.e., the surface that contains the
        /// pointer image (cursor). This request gives the surface the role
        /// of a cursor. If the surface already has another role, it raises
        /// a protocol error.
        ///
        /// The cursor actually changes only if the pointer
        /// focus for this device is one of the requesting client's surfaces
        /// or the surface parameter is the current pointer surface. If
        /// there was a previous surface set with this request it is
        /// replaced. If surface is NULL, the pointer image is hidden.
        ///
        /// The parameters hotspot_x and hotspot_y define the position of
        /// the pointer surface relative to the pointer location. Its
        /// top-left corner is always at (x, y) - (hotspot_x, hotspot_y),
        /// where (x, y) are the coordinates of the pointer location, in
        /// surface-local coordinates.
        ///
        /// On surface.attach requests to the pointer surface, hotspot_x
        /// and hotspot_y are decremented by the x and y parameters
        /// passed to the request. Attach must be confirmed by
        /// wl_surface.commit as usual.
        ///
        /// The hotspot can also be updated by passing the currently set
        /// pointer surface to this request with new values for hotspot_x
        /// and hotspot_y.
        ///
        /// The input region is ignored for wl_surfaces with the role of
        /// a cursor. When the use as a cursor ends, the wl_surface is
        /// unmapped.
        ///
        /// The serial parameter must match the latest wl_pointer.enter
        /// serial number sent to the client. Otherwise the request will be
        /// ignored.
        SetCursor {
            /// Serial number of the enter event,
            serial: u32,
            /// Pointer surface,
            surface: WlSurfaceId,
            /// Surface-local x coordinate,
            hotspot_x: i32,
            /// Surface-local y coordinate,
            hotspot_y: i32,
        },
        /// Release the pointer object
        ///
        /// Using this request a client can tell the server that it is not going to
        /// use the pointer object anymore.
        ///
        /// This request destroys the pointer proxy object, so clients must not call
        /// wl_pointer_destroy() after using this request.
        Release,
    }
    impl Parser for crate::wayland_objects::wl_pointer::WlPointer {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetCursor {
                    serial: req.uint()?,
                    surface: req.id()?,
                    hotspot_x: req.int()?,
                    hotspot_y: req.int()?,
                },
                1 => Release,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Enter event
        ///
        /// Notification that this seat's pointer is focused on a certain
        /// surface.
        ///
        /// When a seat's focus enters a surface, the pointer image
        /// is undefined and a client should respond to this event by setting
        /// an appropriate pointer image with the set_cursor request.
        Enter {
            /// Serial number of the enter event,
            serial: u32,
            /// Surface entered by the pointer,
            surface: WlSurfaceId,
            /// Surface-local x coordinate,
            surface_x: f32,
            /// Surface-local y coordinate,
            surface_y: f32,
        },
        /// Leave event
        ///
        /// Notification that this seat's pointer is no longer focused on
        /// a certain surface.
        ///
        /// The leave notification is sent before the enter notification
        /// for the new focus.
        Leave {
            /// Serial number of the leave event,
            serial: u32,
            /// Surface left by the pointer,
            surface: WlSurfaceId,
        },
        /// Pointer motion event
        ///
        /// Notification of pointer location change. The arguments
        /// surface_x and surface_y are the location relative to the
        /// focused surface.
        Motion {
            /// Timestamp with millisecond granularity,
            time: u32,
            /// Surface-local x coordinate,
            surface_x: f32,
            /// Surface-local y coordinate,
            surface_y: f32,
        },
        /// Pointer button event
        ///
        /// Mouse button click and release notifications.
        ///
        /// The location of the click is given by the last motion or
        /// enter event.
        /// The time argument is a timestamp with millisecond
        /// granularity, with an undefined base.
        ///
        /// The button is a button code as defined in the Linux kernel's
        /// linux/input-event-codes.h header file, e.g. BTN_LEFT.
        ///
        /// Any 16-bit button code value is reserved for future additions to the
        /// kernel's event code list. All other button codes above 0xFFFF are
        /// currently undefined but may be used in future versions of this
        /// protocol.
        Button {
            /// Serial number of the button event,
            serial: u32,
            /// Timestamp with millisecond granularity,
            time: u32,
            /// Button that produced the event,
            button: u32,
            /// Physical state of the button,
            state: u32,
        },
        /// Axis event
        ///
        /// Scroll and other axis notifications.
        ///
        /// For scroll events (vertical and horizontal scroll axes), the
        /// value parameter is the length of a vector along the specified
        /// axis in a coordinate space identical to those of motion events,
        /// representing a relative movement along the specified axis.
        ///
        /// For devices that support movements non-parallel to axes multiple
        /// axis events will be emitted.
        ///
        /// When applicable, for example for touch pads, the server can
        /// choose to emit scroll events where the motion vector is
        /// equivalent to a motion event vector.
        ///
        /// When applicable, a client can transform its content relative to the
        /// scroll distance.
        Axis {
            /// Timestamp with millisecond granularity,
            time: u32,
            /// Axis type,
            axis: u32,
            /// Length of vector in surface-local coordinate space,
            value: f32,
        },
        /// End of a pointer event sequence
        ///
        /// Indicates the end of a set of events that logically belong together.
        /// A client is expected to accumulate the data in all events within the
        /// frame before proceeding.
        ///
        /// All wl_pointer events before a wl_pointer.frame event belong
        /// logically together. For example, in a diagonal scroll motion the
        /// compositor will send an optional wl_pointer.axis_source event, two
        /// wl_pointer.axis events (horizontal and vertical) and finally a
        /// wl_pointer.frame event. The client may use this information to
        /// calculate a diagonal vector for scrolling.
        ///
        /// When multiple wl_pointer.axis events occur within the same frame,
        /// the motion vector is the combined motion of all events.
        /// When a wl_pointer.axis and a wl_pointer.axis_stop event occur within
        /// the same frame, this indicates that axis movement in one axis has
        /// stopped but continues in the other axis.
        /// When multiple wl_pointer.axis_stop events occur within the same
        /// frame, this indicates that these axes stopped in the same instance.
        ///
        /// A wl_pointer.frame event is sent for every logical event group,
        /// even if the group only contains a single wl_pointer event.
        /// Specifically, a client may get a sequence: motion, frame, button,
        /// frame, axis, frame, axis_stop, frame.
        ///
        /// The wl_pointer.enter and wl_pointer.leave events are logical events
        /// generated by the compositor and not the hardware. These events are
        /// also grouped by a wl_pointer.frame. When a pointer moves from one
        /// surface to another, a compositor should group the
        /// wl_pointer.leave event within the same wl_pointer.frame.
        /// However, a client must not rely on wl_pointer.leave and
        /// wl_pointer.enter being in the same wl_pointer.frame.
        /// Compositor-specific policies may require the wl_pointer.leave and
        /// wl_pointer.enter event being split across multiple wl_pointer.frame
        /// groups.
        Frame,
        /// Axis source event
        ///
        /// Source information for scroll and other axes.
        ///
        /// This event does not occur on its own. It is sent before a
        /// wl_pointer.frame event and carries the source information for
        /// all events within that frame.
        ///
        /// The source specifies how this event was generated. If the source is
        /// wl_pointer.axis_source.finger, a wl_pointer.axis_stop event will be
        /// sent when the user lifts the finger off the device.
        ///
        /// If the source is wl_pointer.axis_source.wheel,
        /// wl_pointer.axis_source.wheel_tilt or
        /// wl_pointer.axis_source.continuous, a wl_pointer.axis_stop event may
        /// or may not be sent. Whether a compositor sends an axis_stop event
        /// for these sources is hardware-specific and implementation-dependent;
        /// clients must not rely on receiving an axis_stop event for these
        /// scroll sources and should treat scroll sequences from these scroll
        /// sources as unterminated by default.
        ///
        /// This event is optional. If the source is unknown for a particular
        /// axis event sequence, no event is sent.
        /// Only one wl_pointer.axis_source event is permitted per frame.
        ///
        /// The order of wl_pointer.axis_discrete and wl_pointer.axis_source is
        /// not guaranteed.
        AxisSource {
            /// Source of the axis event,
            axis_source: u32,
        },
        /// Axis stop event
        ///
        /// Stop notification for scroll and other axes.
        ///
        /// For some wl_pointer.axis_source types, a wl_pointer.axis_stop event
        /// is sent to notify a client that the axis sequence has terminated.
        /// This enables the client to implement kinetic scrolling.
        /// See the wl_pointer.axis_source documentation for information on when
        /// this event may be generated.
        ///
        /// Any wl_pointer.axis events with the same axis_source after this
        /// event should be considered as the start of a new axis motion.
        ///
        /// The timestamp is to be interpreted identical to the timestamp in the
        /// wl_pointer.axis event. The timestamp value may be the same as a
        /// preceding wl_pointer.axis event.
        AxisStop {
            /// Timestamp with millisecond granularity,
            time: u32,
            /// The axis stopped with this event,
            axis: u32,
        },
        /// Axis click event
        ///
        /// Discrete step information for scroll and other axes.
        ///
        /// This event carries the axis value of the wl_pointer.axis event in
        /// discrete steps (e.g. mouse wheel clicks).
        ///
        /// This event is deprecated with wl_pointer version 8 - this event is not
        /// sent to clients supporting version 8 or later.
        ///
        /// This event does not occur on its own, it is coupled with a
        /// wl_pointer.axis event that represents this axis value on a
        /// continuous scale. The protocol guarantees that each axis_discrete
        /// event is always followed by exactly one axis event with the same
        /// axis number within the same wl_pointer.frame. Note that the protocol
        /// allows for other events to occur between the axis_discrete and
        /// its coupled axis event, including other axis_discrete or axis
        /// events. A wl_pointer.frame must not contain more than one axis_discrete
        /// event per axis type.
        ///
        /// This event is optional; continuous scrolling devices
        /// like two-finger scrolling on touchpads do not have discrete
        /// steps and do not generate this event.
        ///
        /// The discrete value carries the directional information. e.g. a value
        /// of -2 is two steps towards the negative direction of this axis.
        ///
        /// The axis number is identical to the axis number in the associated
        /// axis event.
        ///
        /// The order of wl_pointer.axis_discrete and wl_pointer.axis_source is
        /// not guaranteed.
        AxisDiscrete {
            /// Axis type,
            axis: u32,
            /// Number of steps,
            discrete: i32,
        },
        /// Axis high-resolution scroll event
        ///
        /// Discrete high-resolution scroll information.
        ///
        /// This event carries high-resolution wheel scroll information,
        /// with each multiple of 120 representing one logical scroll step
        /// (a wheel detent). For example, an axis_value120 of 30 is one quarter of
        /// a logical scroll step in the positive direction, a value120 of
        /// -240 are two logical scroll steps in the negative direction within the
        /// same hardware event.
        /// Clients that rely on discrete scrolling should accumulate the
        /// value120 to multiples of 120 before processing the event.
        ///
        /// The value120 must not be zero.
        ///
        /// This event replaces the wl_pointer.axis_discrete event in clients
        /// supporting wl_pointer version 8 or later.
        ///
        /// Where a wl_pointer.axis_source event occurs in the same
        /// wl_pointer.frame, the axis source applies to this event.
        ///
        /// The order of wl_pointer.axis_value120 and wl_pointer.axis_source is
        /// not guaranteed.
        AxisValue120 {
            /// Axis type,
            axis: u32,
            /// Scroll distance as fraction of 120,
            value120: i32,
        },
        /// Axis relative physical direction event
        ///
        /// Relative directional information of the entity causing the axis
        /// motion.
        ///
        /// For a wl_pointer.axis event, the wl_pointer.axis_relative_direction
        /// event specifies the movement direction of the entity causing the
        /// wl_pointer.axis event. For example:
        /// - if a user's fingers on a touchpad move down and this
        /// causes a wl_pointer.axis vertical_scroll down event, the physical
        /// direction is 'identical'
        /// - if a user's fingers on a touchpad move down and this causes a
        /// wl_pointer.axis vertical_scroll up scroll up event ('natural
        /// scrolling'), the physical direction is 'inverted'.
        ///
        /// A client may use this information to adjust scroll motion of
        /// components. Specifically, enabling natural scrolling causes the
        /// content to change direction compared to traditional scrolling.
        /// Some widgets like volume control sliders should usually match the
        /// physical direction regardless of whether natural scrolling is
        /// active. This event enables clients to match the scroll direction of
        /// a widget to the physical direction.
        ///
        /// This event does not occur on its own, it is coupled with a
        /// wl_pointer.axis event that represents this axis value.
        /// The protocol guarantees that each axis_relative_direction event is
        /// always followed by exactly one axis event with the same
        /// axis number within the same wl_pointer.frame. Note that the protocol
        /// allows for other events to occur between the axis_relative_direction
        /// and its coupled axis event.
        ///
        /// The axis number is identical to the axis number in the associated
        /// axis event.
        ///
        /// The order of wl_pointer.axis_relative_direction,
        /// wl_pointer.axis_discrete and wl_pointer.axis_source is not
        /// guaranteed.
        AxisRelativeDirection {
            /// Axis type,
            axis: u32,
            /// Physical direction relative to axis motion,
            direction: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_pointer::WlPointer {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Enter { serial, surface, surface_x, surface_y } => {
                    let mut raw = RawEvent::new(self.id(), 0, 4);
                    raw.uint(serial);
                    raw.id(surface);
                    raw.fixed(surface_x);
                    raw.fixed(surface_y);
                    raw
                }
                Leave { serial, surface } => {
                    let mut raw = RawEvent::new(self.id(), 1, 2);
                    raw.uint(serial);
                    raw.id(surface);
                    raw
                }
                Motion { time, surface_x, surface_y } => {
                    let mut raw = RawEvent::new(self.id(), 2, 3);
                    raw.uint(time);
                    raw.fixed(surface_x);
                    raw.fixed(surface_y);
                    raw
                }
                Button { serial, time, button, state } => {
                    let mut raw = RawEvent::new(self.id(), 3, 4);
                    raw.uint(serial);
                    raw.uint(time);
                    raw.uint(button);
                    raw.uint(state);
                    raw
                }
                Axis { time, axis, value } => {
                    let mut raw = RawEvent::new(self.id(), 4, 3);
                    raw.uint(time);
                    raw.uint(axis);
                    raw.fixed(value);
                    raw
                }
                Frame {} => {
                    let mut raw = RawEvent::new(self.id(), 5, 0);
                    raw
                }
                AxisSource { axis_source } => {
                    let mut raw = RawEvent::new(self.id(), 6, 1);
                    raw.uint(axis_source);
                    raw
                }
                AxisStop { time, axis } => {
                    let mut raw = RawEvent::new(self.id(), 7, 2);
                    raw.uint(time);
                    raw.uint(axis);
                    raw
                }
                AxisDiscrete { axis, discrete } => {
                    let mut raw = RawEvent::new(self.id(), 8, 2);
                    raw.uint(axis);
                    raw.int(discrete);
                    raw
                }
                AxisValue120 { axis, value120 } => {
                    let mut raw = RawEvent::new(self.id(), 9, 2);
                    raw.uint(axis);
                    raw.int(value120);
                    raw
                }
                AxisRelativeDirection { axis, direction } => {
                    let mut raw = RawEvent::new(self.id(), 10, 2);
                    raw.uint(axis);
                    raw.uint(direction);
                    raw
                }
            }
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Given wl_surface has another role,
        Role = 0,
    }
    /// Physical button state
    ///
    /// Describes the physical state of a button that produced the button
    /// event.
    #[derive(Debug)]
    pub enum ButtonState {
        /// The button is not pressed,
        Released = 0,
        /// The button is pressed,
        Pressed = 1,
    }
    /// Axis types
    ///
    /// Describes the axis types of scroll events.
    #[derive(Debug)]
    pub enum Axis {
        /// Vertical axis,
        VerticalScroll = 0,
        /// Horizontal axis,
        HorizontalScroll = 1,
    }
    /// Axis source types
    ///
    /// Describes the source types for axis events. This indicates to the
    /// client how an axis event was physically generated; a client may
    /// adjust the user interface accordingly. For example, scroll events
    /// from a "finger" source may be in a smooth coordinate space with
    /// kinetic scrolling whereas a "wheel" source may be in discrete steps
    /// of a number of lines.
    ///
    /// The "continuous" axis source is a device generating events in a
    /// continuous coordinate space, but using something other than a
    /// finger. One example for this source is button-based scrolling where
    /// the vertical motion of a device is converted to scroll events while
    /// a button is held down.
    ///
    /// The "wheel tilt" axis source indicates that the actual device is a
    /// wheel but the scroll event is not caused by a rotation but a
    /// (usually sideways) tilt of the wheel.
    #[derive(Debug)]
    pub enum AxisSource {
        /// A physical wheel rotation,
        Wheel = 0,
        /// Finger on a touch surface,
        Finger = 1,
        /// Continuous coordinate space,
        Continuous = 2,
        /// A physical wheel tilt,
        WheelTilt = 3,
    }
    /// Axis relative direction
    ///
    /// This specifies the direction of the physical motion that caused a
    /// wl_pointer.axis event, relative to the wl_pointer.axis direction.
    #[derive(Debug)]
    pub enum AxisRelativeDirection {
        /// Physical motion matches axis direction,
        Identical = 0,
        /// Physical motion is the inverse of the axis direction,
        Inverted = 1,
    }
}

/// Keyboard input device
///
/// The wl_keyboard interface represents one or more keyboards
/// associated with a seat.
pub mod wl_keyboard {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Release the keyboard object
        ///
        ///
        Release,
    }
    impl Parser for crate::wayland_objects::wl_keyboard::WlKeyboard {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Release,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Keyboard mapping
        ///
        /// This event provides a file descriptor to the client which can be
        /// memory-mapped in read-only mode to provide a keyboard mapping
        /// description.
        ///
        /// From version 7 onwards, the fd must be mapped with MAP_PRIVATE by
        /// the recipient, as MAP_SHARED may fail.
        Keymap {
            /// Keymap format,
            format: u32,
            /// Keymap file descriptor,
            fd: RawFd,
            /// Keymap size, in bytes,
            size: u32,
        },
        /// Enter event
        ///
        /// Notification that this seat's keyboard focus is on a certain
        /// surface.
        ///
        /// The compositor must send the wl_keyboard.modifiers event after this
        /// event.
        Enter {
            /// Serial number of the enter event,
            serial: u32,
            /// Surface gaining keyboard focus,
            surface: WlSurfaceId,
            /// The currently pressed keys,
            keys: Array,
        },
        /// Leave event
        ///
        /// Notification that this seat's keyboard focus is no longer on
        /// a certain surface.
        ///
        /// The leave notification is sent before the enter notification
        /// for the new focus.
        ///
        /// After this event client must assume that all keys, including modifiers,
        /// are lifted and also it must stop key repeating if there's some going on.
        Leave {
            /// Serial number of the leave event,
            serial: u32,
            /// Surface that lost keyboard focus,
            surface: WlSurfaceId,
        },
        /// Key event
        ///
        /// A key was pressed or released.
        /// The time argument is a timestamp with millisecond
        /// granularity, with an undefined base.
        ///
        /// The key is a platform-specific key code that can be interpreted
        /// by feeding it to the keyboard mapping (see the keymap event).
        ///
        /// If this event produces a change in modifiers, then the resulting
        /// wl_keyboard.modifiers event must be sent after this event.
        Key {
            /// Serial number of the key event,
            serial: u32,
            /// Timestamp with millisecond granularity,
            time: u32,
            /// Key that produced the event,
            key: u32,
            /// Physical state of the key,
            state: u32,
        },
        /// Modifier and group state
        ///
        /// Notifies clients that the modifier and/or group state has
        /// changed, and it should update its local state.
        Modifiers {
            /// Serial number of the modifiers event,
            serial: u32,
            /// Depressed modifiers,
            mods_depressed: u32,
            /// Latched modifiers,
            mods_latched: u32,
            /// Locked modifiers,
            mods_locked: u32,
            /// Keyboard layout,
            group: u32,
        },
        /// Repeat rate and delay
        ///
        /// Informs the client about the keyboard's repeat rate and delay.
        ///
        /// This event is sent as soon as the wl_keyboard object has been created,
        /// and is guaranteed to be received by the client before any key press
        /// event.
        ///
        /// Negative values for either rate or delay are illegal. A rate of zero
        /// will disable any repeating (regardless of the value of delay).
        ///
        /// This event can be sent later on as well with a new value if necessary,
        /// so clients should continue listening for the event past the creation
        /// of wl_keyboard.
        RepeatInfo {
            /// The rate of repeating keys in characters per second,
            rate: i32,
            /// Delay in milliseconds since key down until repeating starts,
            delay: i32,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_keyboard::WlKeyboard {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Keymap { format, fd, size } => {
                    let mut raw = RawEvent::new(self.id(), 0, 3);
                    raw.uint(format);
                    raw.fd(fd);
                    raw.uint(size);
                    raw
                }
                Enter { serial, surface, keys } => {
                    let mut raw = RawEvent::new(self.id(), 1, 3);
                    raw.uint(serial);
                    raw.id(surface);
                    raw.array(keys);
                    raw
                }
                Leave { serial, surface } => {
                    let mut raw = RawEvent::new(self.id(), 2, 2);
                    raw.uint(serial);
                    raw.id(surface);
                    raw
                }
                Key { serial, time, key, state } => {
                    let mut raw = RawEvent::new(self.id(), 3, 4);
                    raw.uint(serial);
                    raw.uint(time);
                    raw.uint(key);
                    raw.uint(state);
                    raw
                }
                Modifiers { serial, mods_depressed, mods_latched, mods_locked, group } => {
                    let mut raw = RawEvent::new(self.id(), 4, 5);
                    raw.uint(serial);
                    raw.uint(mods_depressed);
                    raw.uint(mods_latched);
                    raw.uint(mods_locked);
                    raw.uint(group);
                    raw
                }
                RepeatInfo { rate, delay } => {
                    let mut raw = RawEvent::new(self.id(), 5, 2);
                    raw.int(rate);
                    raw.int(delay);
                    raw
                }
            }
        }
    }
    /// Keyboard mapping format
    ///
    /// This specifies the format of the keymap provided to the
    /// client with the wl_keyboard.keymap event.
    #[derive(Debug)]
    pub enum KeymapFormat {
        /// No keymap; client must understand how to interpret the raw keycode,
        NoKeymap = 0,
        /// Libxkbcommon compatible, null-terminated string; to determine the xkb keycode, clients must add 8 to the key event keycode,
        XkbV1 = 1,
    }
    /// Physical key state
    ///
    /// Describes the physical state of a key that produced the key event.
    #[derive(Debug)]
    pub enum KeyState {
        /// Key is not pressed,
        Released = 0,
        /// Key is pressed,
        Pressed = 1,
    }
}

/// Touchscreen input device
///
/// The wl_touch interface represents a touchscreen
/// associated with a seat.
///
/// Touch interactions can consist of one or more contacts.
/// For each contact, a series of events is generated, starting
/// with a down event, followed by zero or more motion events,
/// and ending with an up event. Events relating to the same
/// contact point can be identified by the ID of the sequence.
pub mod wl_touch {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Release the touch object
        ///
        ///
        Release,
    }
    impl Parser for crate::wayland_objects::wl_touch::WlTouch {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Release,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Touch down event and beginning of a touch sequence
        ///
        /// A new touch point has appeared on the surface. This touch point is
        /// assigned a unique ID. Future events from this touch point reference
        /// this ID. The ID ceases to be valid after a touch up event and may be
        /// reused in the future.
        Down {
            /// Serial number of the touch down event,
            serial: u32,
            /// Timestamp with millisecond granularity,
            time: u32,
            /// Surface touched,
            surface: WlSurfaceId,
            /// The unique ID of this touch point,
            id: i32,
            /// Surface-local x coordinate,
            x: f32,
            /// Surface-local y coordinate,
            y: f32,
        },
        /// End of a touch event sequence
        ///
        /// The touch point has disappeared. No further events will be sent for
        /// this touch point and the touch point's ID is released and may be
        /// reused in a future touch down event.
        Up {
            /// Serial number of the touch up event,
            serial: u32,
            /// Timestamp with millisecond granularity,
            time: u32,
            /// The unique ID of this touch point,
            id: i32,
        },
        /// Update of touch point coordinates
        ///
        /// A touch point has changed coordinates.
        Motion {
            /// Timestamp with millisecond granularity,
            time: u32,
            /// The unique ID of this touch point,
            id: i32,
            /// Surface-local x coordinate,
            x: f32,
            /// Surface-local y coordinate,
            y: f32,
        },
        /// End of touch frame event
        ///
        /// Indicates the end of a set of events that logically belong together.
        /// A client is expected to accumulate the data in all events within the
        /// frame before proceeding.
        ///
        /// A wl_touch.frame terminates at least one event but otherwise no
        /// guarantee is provided about the set of events within a frame. A client
        /// must assume that any state not updated in a frame is unchanged from the
        /// previously known state.
        Frame,
        /// Touch session cancelled
        ///
        /// Sent if the compositor decides the touch stream is a global
        /// gesture. No further events are sent to the clients from that
        /// particular gesture. Touch cancellation applies to all touch points
        /// currently active on this client's surface. The client is
        /// responsible for finalizing the touch points, future touch points on
        /// this surface may reuse the touch point ID.
        Cancel,
        /// Update shape of touch point
        ///
        /// Sent when a touchpoint has changed its shape.
        ///
        /// This event does not occur on its own. It is sent before a
        /// wl_touch.frame event and carries the new shape information for
        /// any previously reported, or new touch points of that frame.
        ///
        /// Other events describing the touch point such as wl_touch.down,
        /// wl_touch.motion or wl_touch.orientation may be sent within the
        /// same wl_touch.frame. A client should treat these events as a single
        /// logical touch point update. The order of wl_touch.shape,
        /// wl_touch.orientation and wl_touch.motion is not guaranteed.
        /// A wl_touch.down event is guaranteed to occur before the first
        /// wl_touch.shape event for this touch ID but both events may occur within
        /// the same wl_touch.frame.
        ///
        /// A touchpoint shape is approximated by an ellipse through the major and
        /// minor axis length. The major axis length describes the longer diameter
        /// of the ellipse, while the minor axis length describes the shorter
        /// diameter. Major and minor are orthogonal and both are specified in
        /// surface-local coordinates. The center of the ellipse is always at the
        /// touchpoint location as reported by wl_touch.down or wl_touch.move.
        ///
        /// This event is only sent by the compositor if the touch device supports
        /// shape reports. The client has to make reasonable assumptions about the
        /// shape if it did not receive this event.
        Shape {
            /// The unique ID of this touch point,
            id: i32,
            /// Length of the major axis in surface-local coordinates,
            major: f32,
            /// Length of the minor axis in surface-local coordinates,
            minor: f32,
        },
        /// Update orientation of touch point
        ///
        /// Sent when a touchpoint has changed its orientation.
        ///
        /// This event does not occur on its own. It is sent before a
        /// wl_touch.frame event and carries the new shape information for
        /// any previously reported, or new touch points of that frame.
        ///
        /// Other events describing the touch point such as wl_touch.down,
        /// wl_touch.motion or wl_touch.shape may be sent within the
        /// same wl_touch.frame. A client should treat these events as a single
        /// logical touch point update. The order of wl_touch.shape,
        /// wl_touch.orientation and wl_touch.motion is not guaranteed.
        /// A wl_touch.down event is guaranteed to occur before the first
        /// wl_touch.orientation event for this touch ID but both events may occur
        /// within the same wl_touch.frame.
        ///
        /// The orientation describes the clockwise angle of a touchpoint's major
        /// axis to the positive surface y-axis and is normalized to the -180 to
        /// +180 degree range. The granularity of orientation depends on the touch
        /// device, some devices only support binary rotation values between 0 and
        /// 90 degrees.
        ///
        /// This event is only sent by the compositor if the touch device supports
        /// orientation reports.
        Orientation {
            /// The unique ID of this touch point,
            id: i32,
            /// Angle between major axis and positive surface y-axis in degrees,
            orientation: f32,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_touch::WlTouch {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Down { serial, time, surface, id, x, y } => {
                    let mut raw = RawEvent::new(self.id(), 0, 6);
                    raw.uint(serial);
                    raw.uint(time);
                    raw.id(surface);
                    raw.int(id);
                    raw.fixed(x);
                    raw.fixed(y);
                    raw
                }
                Up { serial, time, id } => {
                    let mut raw = RawEvent::new(self.id(), 1, 3);
                    raw.uint(serial);
                    raw.uint(time);
                    raw.int(id);
                    raw
                }
                Motion { time, id, x, y } => {
                    let mut raw = RawEvent::new(self.id(), 2, 4);
                    raw.uint(time);
                    raw.int(id);
                    raw.fixed(x);
                    raw.fixed(y);
                    raw
                }
                Frame {} => {
                    let mut raw = RawEvent::new(self.id(), 3, 0);
                    raw
                }
                Cancel {} => {
                    let mut raw = RawEvent::new(self.id(), 4, 0);
                    raw
                }
                Shape { id, major, minor } => {
                    let mut raw = RawEvent::new(self.id(), 5, 3);
                    raw.int(id);
                    raw.fixed(major);
                    raw.fixed(minor);
                    raw
                }
                Orientation { id, orientation } => {
                    let mut raw = RawEvent::new(self.id(), 6, 2);
                    raw.int(id);
                    raw.fixed(orientation);
                    raw
                }
            }
        }
    }
}

/// Compositor output region
///
/// An output describes part of the compositor geometry.  The
/// compositor works in the 'compositor coordinate system' and an
/// output corresponds to a rectangular area in that space that is
/// actually visible.  This typically corresponds to a monitor that
/// displays part of the compositor space.  This object is published
/// as global during start up, or when a monitor is hotplugged.
pub mod wl_output {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Release the output object
        ///
        /// Using this request a client can tell the server that it is not going to
        /// use the output object anymore.
        Release,
    }
    impl Parser for crate::wayland_objects::wl_output::WlOutput {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Release,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Properties of the output
        ///
        /// The geometry event describes geometric properties of the output.
        /// The event is sent when binding to the output object and whenever
        /// any of the properties change.
        ///
        /// The physical size can be set to zero if it doesn't make sense for this
        /// output (e.g. for projectors or virtual outputs).
        ///
        /// The geometry event will be followed by a done event (starting from
        /// version 2).
        ///
        /// Note: wl_output only advertises partial information about the output
        /// position and identification. Some compositors, for instance those not
        /// implementing a desktop-style output layout or those exposing virtual
        /// outputs, might fake this information. Instead of using x and y, clients
        /// should use xdg_output.logical_position. Instead of using make and model,
        /// clients should use name and description.
        Geometry {
            /// X position within the global compositor space,
            x: i32,
            /// Y position within the global compositor space,
            y: i32,
            /// Width in millimeters of the output,
            physical_width: i32,
            /// Height in millimeters of the output,
            physical_height: i32,
            /// Subpixel orientation of the output,
            subpixel: i32,
            /// Textual description of the manufacturer,
            make: String,
            /// Textual description of the model,
            model: String,
            /// Transform that maps framebuffer to output,
            transform: i32,
        },
        /// Advertise available modes for the output
        ///
        /// The mode event describes an available mode for the output.
        ///
        /// The event is sent when binding to the output object and there
        /// will always be one mode, the current mode.  The event is sent
        /// again if an output changes mode, for the mode that is now
        /// current.  In other words, the current mode is always the last
        /// mode that was received with the current flag set.
        ///
        /// Non-current modes are deprecated. A compositor can decide to only
        /// advertise the current mode and never send other modes. Clients
        /// should not rely on non-current modes.
        ///
        /// The size of a mode is given in physical hardware units of
        /// the output device. This is not necessarily the same as
        /// the output size in the global compositor space. For instance,
        /// the output may be scaled, as described in wl_output.scale,
        /// or transformed, as described in wl_output.transform. Clients
        /// willing to retrieve the output size in the global compositor
        /// space should use xdg_output.logical_size instead.
        ///
        /// The vertical refresh rate can be set to zero if it doesn't make
        /// sense for this output (e.g. for virtual outputs).
        ///
        /// The mode event will be followed by a done event (starting from
        /// version 2).
        ///
        /// Clients should not use the refresh rate to schedule frames. Instead,
        /// they should use the wl_surface.frame event or the presentation-time
        /// protocol.
        ///
        /// Note: this information is not always meaningful for all outputs. Some
        /// compositors, such as those exposing virtual outputs, might fake the
        /// refresh rate or the size.
        Mode {
            /// Bitfield of mode flags,
            flags: u32,
            /// Width of the mode in hardware units,
            width: i32,
            /// Height of the mode in hardware units,
            height: i32,
            /// Vertical refresh rate in mHz,
            refresh: i32,
        },
        /// Sent all information about output
        ///
        /// This event is sent after all other properties have been
        /// sent after binding to the output object and after any
        /// other property changes done after that. This allows
        /// changes to the output properties to be seen as
        /// atomic, even if they happen via multiple events.
        Done,
        /// Output scaling properties
        ///
        /// This event contains scaling geometry information
        /// that is not in the geometry event. It may be sent after
        /// binding the output object or if the output scale changes
        /// later. If it is not sent, the client should assume a
        /// scale of 1.
        ///
        /// A scale larger than 1 means that the compositor will
        /// automatically scale surface buffers by this amount
        /// when rendering. This is used for very high resolution
        /// displays where applications rendering at the native
        /// resolution would be too small to be legible.
        ///
        /// It is intended that scaling aware clients track the
        /// current output of a surface, and if it is on a scaled
        /// output it should use wl_surface.set_buffer_scale with
        /// the scale of the output. That way the compositor can
        /// avoid scaling the surface, and the client can supply
        /// a higher detail image.
        ///
        /// The scale event will be followed by a done event.
        Scale {
            /// Scaling factor of output,
            factor: i32,
        },
        /// Name of this output
        ///
        /// Many compositors will assign user-friendly names to their outputs, show
        /// them to the user, allow the user to refer to an output, etc. The client
        /// may wish to know this name as well to offer the user similar behaviors.
        ///
        /// The name is a UTF-8 string with no convention defined for its contents.
        /// Each name is unique among all wl_output globals. The name is only
        /// guaranteed to be unique for the compositor instance.
        ///
        /// The same output name is used for all clients for a given wl_output
        /// global. Thus, the name can be shared across processes to refer to a
        /// specific wl_output global.
        ///
        /// The name is not guaranteed to be persistent across sessions, thus cannot
        /// be used to reliably identify an output in e.g. configuration files.
        ///
        /// Examples of names include 'HDMI-A-1', 'WL-1', 'X11-1', etc. However, do
        /// not assume that the name is a reflection of an underlying DRM connector,
        /// X11 connection, etc.
        ///
        /// The name event is sent after binding the output object. This event is
        /// only sent once per output object, and the name does not change over the
        /// lifetime of the wl_output global.
        ///
        /// Compositors may re-use the same output name if the wl_output global is
        /// destroyed and re-created later. Compositors should avoid re-using the
        /// same name if possible.
        ///
        /// The name event will be followed by a done event.
        Name {
            /// Output name,
            name: String,
        },
        /// Human-readable description of this output
        ///
        /// Many compositors can produce human-readable descriptions of their
        /// outputs. The client may wish to know this description as well, e.g. for
        /// output selection purposes.
        ///
        /// The description is a UTF-8 string with no convention defined for its
        /// contents. The description is not guaranteed to be unique among all
        /// wl_output globals. Examples might include 'Foocorp 11" Display' or
        /// 'Virtual X11 output via :1'.
        ///
        /// The description event is sent after binding the output object and
        /// whenever the description changes. The description is optional, and may
        /// not be sent at all.
        ///
        /// The description event will be followed by a done event.
        Description {
            /// Output description,
            description: String,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_output::WlOutput {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Geometry {
                    x,
                    y,
                    physical_width,
                    physical_height,
                    subpixel,
                    make,
                    model,
                    transform,
                } => {
                    let mut raw = RawEvent::new(self.id(), 0, 8);
                    raw.int(x);
                    raw.int(y);
                    raw.int(physical_width);
                    raw.int(physical_height);
                    raw.int(subpixel);
                    raw.string(make);
                    raw.string(model);
                    raw.int(transform);
                    raw
                }
                Mode { flags, width, height, refresh } => {
                    let mut raw = RawEvent::new(self.id(), 1, 4);
                    raw.uint(flags);
                    raw.int(width);
                    raw.int(height);
                    raw.int(refresh);
                    raw
                }
                Done {} => {
                    let mut raw = RawEvent::new(self.id(), 2, 0);
                    raw
                }
                Scale { factor } => {
                    let mut raw = RawEvent::new(self.id(), 3, 1);
                    raw.int(factor);
                    raw
                }
                Name { name } => {
                    let mut raw = RawEvent::new(self.id(), 4, 1);
                    raw.string(name);
                    raw
                }
                Description { description } => {
                    let mut raw = RawEvent::new(self.id(), 5, 1);
                    raw.string(description);
                    raw
                }
            }
        }
    }
    /// Subpixel geometry information
    ///
    /// This enumeration describes how the physical
    /// pixels on an output are laid out.
    #[derive(Debug)]
    pub enum Subpixel {
        /// Unknown geometry,
        Unknown = 0,
        /// No geometry,
        None = 1,
        /// Horizontal RGB,
        HorizontalRgb = 2,
        /// Horizontal BGR,
        HorizontalBgr = 3,
        /// Vertical RGB,
        VerticalRgb = 4,
        /// Vertical BGR,
        VerticalBgr = 5,
    }
    /// Transform from framebuffer to output
    ///
    /// This describes the transform that a compositor will apply to a
    /// surface to compensate for the rotation or mirroring of an
    /// output device.
    ///
    /// The flipped values correspond to an initial flip around a
    /// vertical axis followed by rotation.
    ///
    /// The purpose is mainly to allow clients to render accordingly and
    /// tell the compositor, so that for fullscreen surfaces, the
    /// compositor will still be able to scan out directly from client
    /// surfaces.
    #[derive(Debug)]
    pub enum Transform {
        /// No transform,
        Normal = 0,
        /// 90 degrees counter-clockwise,
        _90 = 1,
        /// 180 degrees counter-clockwise,
        _180 = 2,
        /// 270 degrees counter-clockwise,
        _270 = 3,
        /// 180 degree flip around a vertical axis,
        Flipped = 4,
        /// Flip and rotate 90 degrees counter-clockwise,
        Flipped90 = 5,
        /// Flip and rotate 180 degrees counter-clockwise,
        Flipped180 = 6,
        /// Flip and rotate 270 degrees counter-clockwise,
        Flipped270 = 7,
    }
    /// Mode information
    ///
    /// These flags describe properties of an output mode.
    /// They are used in the flags bitfield of the mode event.
    #[derive(Debug)]
    pub enum Mode {
        /// Indicates this is the current mode,
        Current = 1,
        /// Indicates this is the preferred mode,
        Preferred = 2,
    }
}

/// Region interface
///
/// A region object describes an area.
///
/// Region objects are used to describe the opaque and input
/// regions of a surface.
pub mod wl_region {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy region
        ///
        /// Destroy the region.  This will invalidate the object ID.
        Destroy,
        /// Add rectangle to region
        ///
        /// Add the specified rectangle to the region.
        Add {
            /// Region-local x coordinate,
            x: i32,
            /// Region-local y coordinate,
            y: i32,
            /// Rectangle width,
            width: i32,
            /// Rectangle height,
            height: i32,
        },
        /// Subtract rectangle from region
        ///
        /// Subtract the specified rectangle from the region.
        Subtract {
            /// Region-local x coordinate,
            x: i32,
            /// Region-local y coordinate,
            y: i32,
            /// Rectangle width,
            width: i32,
            /// Rectangle height,
            height: i32,
        },
    }
    impl Parser for crate::wayland_objects::wl_region::WlRegion {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => Add {
                    x: req.int()?,
                    y: req.int()?,
                    width: req.int()?,
                    height: req.int()?,
                },
                2 => Subtract {
                    x: req.int()?,
                    y: req.int()?,
                    width: req.int()?,
                    height: req.int()?,
                },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// Sub-surface compositing
///
/// The global interface exposing sub-surface compositing capabilities.
/// A wl_surface, that has sub-surfaces associated, is called the
/// parent surface. Sub-surfaces can be arbitrarily nested and create
/// a tree of sub-surfaces.
///
/// The root surface in a tree of sub-surfaces is the main
/// surface. The main surface cannot be a sub-surface, because
/// sub-surfaces must always have a parent.
///
/// A main surface with its sub-surfaces forms a (compound) window.
/// For window management purposes, this set of wl_surface objects is
/// to be considered as a single window, and it should also behave as
/// such.
///
/// The aim of sub-surfaces is to offload some of the compositing work
/// within a window from clients to the compositor. A prime example is
/// a video player with decorations and video in separate wl_surface
/// objects. This should allow the compositor to pass YUV video buffer
/// processing to dedicated overlay hardware when possible.
pub mod wl_subcompositor {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Unbind from the subcompositor interface
        ///
        /// Informs the server that the client will not be using this
        /// protocol object anymore. This does not affect any other
        /// objects, wl_subsurface objects included.
        Destroy,
        /// Give a surface the role sub-surface
        ///
        /// Create a sub-surface interface for the given surface, and
        /// associate it with the given parent surface. This turns a
        /// plain wl_surface into a sub-surface.
        ///
        /// The to-be sub-surface must not already have another role, and it
        /// must not have an existing wl_subsurface object. Otherwise the
        /// bad_surface protocol error is raised.
        ///
        /// Adding sub-surfaces to a parent is a double-buffered operation on the
        /// parent (see wl_surface.commit). The effect of adding a sub-surface
        /// becomes visible on the next time the state of the parent surface is
        /// applied.
        ///
        /// The parent surface must not be one of the child surface's descendants,
        /// and the parent must be different from the child surface, otherwise the
        /// bad_parent protocol error is raised.
        ///
        /// This request modifies the behaviour of wl_surface.commit request on
        /// the sub-surface, see the documentation on wl_subsurface interface.
        GetSubsurface {
            /// The new sub-surface object ID,
            id: WlSubsurfaceId,
            /// The surface to be turned into a sub-surface,
            surface: WlSurfaceId,
            /// The parent surface,
            parent: WlSurfaceId,
        },
    }
    impl Parser for crate::wayland_objects::wl_subcompositor::WlSubcompositor {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetSubsurface { id: req.id()?, surface: req.id()?, parent: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// The to-be sub-surface is invalid,
        BadSurface = 0,
        /// The to-be sub-surface parent is invalid,
        BadParent = 1,
    }
}

/// Sub-surface interface to a wl_surface
///
/// An additional interface to a wl_surface object, which has been
/// made a sub-surface. A sub-surface has one parent surface. A
/// sub-surface's size and position are not limited to that of the parent.
/// Particularly, a sub-surface is not automatically clipped to its
/// parent's area.
///
/// A sub-surface becomes mapped, when a non-NULL wl_buffer is applied
/// and the parent surface is mapped. The order of which one happens
/// first is irrelevant. A sub-surface is hidden if the parent becomes
/// hidden, or if a NULL wl_buffer is applied. These rules apply
/// recursively through the tree of surfaces.
///
/// The behaviour of a wl_surface.commit request on a sub-surface
/// depends on the sub-surface's mode. The possible modes are
/// synchronized and desynchronized, see methods
/// wl_subsurface.set_sync and wl_subsurface.set_desync. Synchronized
/// mode caches the wl_surface state to be applied when the parent's
/// state gets applied, and desynchronized mode applies the pending
/// wl_surface state directly. A sub-surface is initially in the
/// synchronized mode.
///
/// Sub-surfaces also have another kind of state, which is managed by
/// wl_subsurface requests, as opposed to wl_surface requests. This
/// state includes the sub-surface position relative to the parent
/// surface (wl_subsurface.set_position), and the stacking order of
/// the parent and its sub-surfaces (wl_subsurface.place_above and
/// .place_below). This state is applied when the parent surface's
/// wl_surface state is applied, regardless of the sub-surface's mode.
/// As the exception, set_sync and set_desync are effective immediately.
///
/// The main surface can be thought to be always in desynchronized mode,
/// since it does not have a parent in the sub-surfaces sense.
///
/// Even if a sub-surface is in desynchronized mode, it will behave as
/// in synchronized mode, if its parent surface behaves as in
/// synchronized mode. This rule is applied recursively throughout the
/// tree of surfaces. This means, that one can set a sub-surface into
/// synchronized mode, and then assume that all its child and grand-child
/// sub-surfaces are synchronized, too, without explicitly setting them.
///
/// Destroying a sub-surface takes effect immediately. If you need to
/// synchronize the removal of a sub-surface to the parent surface update,
/// unmap the sub-surface first by attaching a NULL wl_buffer, update parent,
/// and then destroy the sub-surface.
///
/// If the parent wl_surface object is destroyed, the sub-surface is
/// unmapped.
pub mod wl_subsurface {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Remove sub-surface interface
        ///
        /// The sub-surface interface is removed from the wl_surface object
        /// that was turned into a sub-surface with a
        /// wl_subcompositor.get_subsurface request. The wl_surface's association
        /// to the parent is deleted. The wl_surface is unmapped immediately.
        Destroy,
        /// Reposition the sub-surface
        ///
        /// This schedules a sub-surface position change.
        /// The sub-surface will be moved so that its origin (top left
        /// corner pixel) will be at the location x, y of the parent surface
        /// coordinate system. The coordinates are not restricted to the parent
        /// surface area. Negative values are allowed.
        ///
        /// The scheduled coordinates will take effect whenever the state of the
        /// parent surface is applied. When this happens depends on whether the
        /// parent surface is in synchronized mode or not. See
        /// wl_subsurface.set_sync and wl_subsurface.set_desync for details.
        ///
        /// If more than one set_position request is invoked by the client before
        /// the commit of the parent surface, the position of a new request always
        /// replaces the scheduled position from any previous request.
        ///
        /// The initial position is 0, 0.
        SetPosition {
            /// X coordinate in the parent surface,
            x: i32,
            /// Y coordinate in the parent surface,
            y: i32,
        },
        /// Restack the sub-surface
        ///
        /// This sub-surface is taken from the stack, and put back just
        /// above the reference surface, changing the z-order of the sub-surfaces.
        /// The reference surface must be one of the sibling surfaces, or the
        /// parent surface. Using any other surface, including this sub-surface,
        /// will cause a protocol error.
        ///
        /// The z-order is double-buffered. Requests are handled in order and
        /// applied immediately to a pending state. The final pending state is
        /// copied to the active state the next time the state of the parent
        /// surface is applied. When this happens depends on whether the parent
        /// surface is in synchronized mode or not. See wl_subsurface.set_sync and
        /// wl_subsurface.set_desync for details.
        ///
        /// A new sub-surface is initially added as the top-most in the stack
        /// of its siblings and parent.
        PlaceAbove {
            /// The reference surface,
            sibling: WlSurfaceId,
        },
        /// Restack the sub-surface
        ///
        /// The sub-surface is placed just below the reference surface.
        /// See wl_subsurface.place_above.
        PlaceBelow {
            /// The reference surface,
            sibling: WlSurfaceId,
        },
        /// Set sub-surface to synchronized mode
        ///
        /// Change the commit behaviour of the sub-surface to synchronized
        /// mode, also described as the parent dependent mode.
        ///
        /// In synchronized mode, wl_surface.commit on a sub-surface will
        /// accumulate the committed state in a cache, but the state will
        /// not be applied and hence will not change the compositor output.
        /// The cached state is applied to the sub-surface immediately after
        /// the parent surface's state is applied. This ensures atomic
        /// updates of the parent and all its synchronized sub-surfaces.
        /// Applying the cached state will invalidate the cache, so further
        /// parent surface commits do not (re-)apply old state.
        ///
        /// See wl_subsurface for the recursive effect of this mode.
        SetSync,
        /// Set sub-surface to desynchronized mode
        ///
        /// Change the commit behaviour of the sub-surface to desynchronized
        /// mode, also described as independent or freely running mode.
        ///
        /// In desynchronized mode, wl_surface.commit on a sub-surface will
        /// apply the pending state directly, without caching, as happens
        /// normally with a wl_surface. Calling wl_surface.commit on the
        /// parent surface has no effect on the sub-surface's wl_surface
        /// state. This mode allows a sub-surface to be updated on its own.
        ///
        /// If cached state exists when wl_surface.commit is called in
        /// desynchronized mode, the pending state is added to the cached
        /// state, and applied as a whole. This invalidates the cache.
        ///
        /// Note: even if a sub-surface is set to desynchronized, a parent
        /// sub-surface may override it to behave as synchronized. For details,
        /// see wl_subsurface.
        ///
        /// If a surface's parent surface behaves as desynchronized, then
        /// the cached state is applied on set_desync.
        SetDesync,
    }
    impl Parser for crate::wayland_objects::wl_subsurface::WlSubsurface {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => SetPosition { x: req.int()?, y: req.int()? },
                2 => PlaceAbove { sibling: req.id()? },
                3 => PlaceBelow { sibling: req.id()? },
                4 => SetSync,
                5 => SetDesync,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Wl_surface is not a sibling or the parent,
        BadSurface = 0,
    }
}

/// Timed presentation related wl_surface requests
///
/// The main feature of this interface is accurate presentation
/// timing feedback to ensure smooth video playback while maintaining
/// audio/video synchronization. Some features use the concept of a
/// presentation clock, which is defined in the
/// presentation.clock_id event.
///
/// A content update for a wl_surface is submitted by a
/// wl_surface.commit request. Request 'feedback' associates with
/// the wl_surface.commit and provides feedback on the content
/// update, particularly the final realized presentation time.
///
/// When the final realized presentation time is available, e.g.
/// after a framebuffer flip completes, the requested
/// presentation_feedback.presented events are sent. The final
/// presentation time can differ from the compositor's predicted
/// display update time and the update's target time, especially
/// when the compositor misses its target vertical blanking period.
pub mod wp_presentation {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Unbind from the presentation interface
        ///
        /// Informs the server that the client will no longer be using
        /// this protocol object. Existing objects created by this object
        /// are not affected.
        Destroy,
        /// Request presentation feedback information
        ///
        /// Request presentation feedback for the current content submission
        /// on the given surface. This creates a new presentation_feedback
        /// object, which will deliver the feedback information once. If
        /// multiple presentation_feedback objects are created for the same
        /// submission, they will all deliver the same information.
        ///
        /// For details on what information is returned, see the
        /// presentation_feedback interface.
        Feedback {
            /// Target surface,
            surface: WlSurfaceId,
            /// New feedback object,
            callback: WpPresentationFeedbackId,
        },
    }
    impl Parser for crate::wayland_objects::wp_presentation::WpPresentation {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => Feedback { surface: req.id()?, callback: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Clock ID for timestamps
        ///
        /// This event tells the client in which clock domain the
        /// compositor interprets the timestamps used by the presentation
        /// extension. This clock is called the presentation clock.
        ///
        /// The compositor sends this event when the client binds to the
        /// presentation interface. The presentation clock does not change
        /// during the lifetime of the client connection.
        ///
        /// The clock identifier is platform dependent. On Linux/glibc,
        /// the identifier value is one of the clockid_t values accepted
        /// by clock_gettime(). clock_gettime() is defined by
        /// POSIX.1-2001.
        ///
        /// Timestamps in this clock domain are expressed as tv_sec_hi,
        /// tv_sec_lo, tv_nsec triples, each component being an unsigned
        /// 32-bit value. Whole seconds are in tv_sec which is a 64-bit
        /// value combined from tv_sec_hi and tv_sec_lo, and the
        /// additional fractional part in tv_nsec as nanoseconds. Hence,
        /// for valid timestamps tv_nsec must be in [0, 999999999].
        ///
        /// Note that clock_id applies only to the presentation clock,
        /// and implies nothing about e.g. the timestamps used in the
        /// Wayland core protocol input events.
        ///
        /// Compositors should prefer a clock which does not jump and is
        /// not slewed e.g. by NTP. The absolute value of the clock is
        /// irrelevant. Precision of one millisecond or better is
        /// recommended. Clients must be able to query the current clock
        /// value directly, not by asking the compositor.
        ClockId {
            /// Platform clock identifier,
            clk_id: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::wp_presentation::WpPresentation {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                ClockId { clk_id } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(clk_id);
                    raw
                }
            }
        }
    }
    /// Fatal presentation errors
    ///
    /// These fatal protocol errors may be emitted in response to
    /// illegal presentation requests.
    #[derive(Debug)]
    pub enum Error {
        /// Invalid value in tv_nsec,
        InvalidTimestamp = 0,
        /// Invalid flag,
        InvalidFlag = 1,
    }
}

/// Presentation time feedback event
///
/// A presentation_feedback object returns an indication that a
/// wl_surface content update has become visible to the user.
/// One object corresponds to one content update submission
/// (wl_surface.commit). There are two possible outcomes: the
/// content update is presented to the user, and a presentation
/// timestamp delivered; or, the user did not see the content
/// update because it was superseded or its surface destroyed,
/// and the content update is discarded.
///
/// Once a presentation_feedback object has delivered a 'presented'
/// or 'discarded' event it is automatically destroyed.
pub mod wp_presentation_feedback {
    use super::*;
    #[derive(Debug)]
    pub enum Event {
        /// Presentation synchronized to this output
        ///
        /// As presentation can be synchronized to only one output at a
        /// time, this event tells which output it was. This event is only
        /// sent prior to the presented event.
        ///
        /// As clients may bind to the same global wl_output multiple
        /// times, this event is sent for each bound instance that matches
        /// the synchronized output. If a client has not bound to the
        /// right wl_output global at all, this event is not sent.
        SyncOutput {
            /// Presentation output,
            output: WlOutputId,
        },
        /// The content update was displayed
        ///
        /// The associated content update was displayed to the user at the
        /// indicated time (tv_sec_hi/lo, tv_nsec). For the interpretation of
        /// the timestamp, see presentation.clock_id event.
        ///
        /// The timestamp corresponds to the time when the content update
        /// turned into light the first time on the surface's main output.
        /// Compositors may approximate this from the framebuffer flip
        /// completion events from the system, and the latency of the
        /// physical display path if known.
        ///
        /// This event is preceded by all related sync_output events
        /// telling which output's refresh cycle the feedback corresponds
        /// to, i.e. the main output for the surface. Compositors are
        /// recommended to choose the output containing the largest part
        /// of the wl_surface, or keeping the output they previously
        /// chose. Having a stable presentation output association helps
        /// clients predict future output refreshes (vblank).
        ///
        /// The 'refresh' argument gives the compositor's prediction of how
        /// many nanoseconds after tv_sec, tv_nsec the very next output
        /// refresh may occur. This is to further aid clients in
        /// predicting future refreshes, i.e., estimating the timestamps
        /// targeting the next few vblanks. If such prediction cannot
        /// usefully be done, the argument is zero.
        ///
        /// If the output does not have a constant refresh rate, explicit
        /// video mode switches excluded, then the refresh argument must
        /// be zero.
        ///
        /// The 64-bit value combined from seq_hi and seq_lo is the value
        /// of the output's vertical retrace counter when the content
        /// update was first scanned out to the display. This value must
        /// be compatible with the definition of MSC in
        /// GLX_OML_sync_control specification. Note, that if the display
        /// path has a non-zero latency, the time instant specified by
        /// this counter may differ from the timestamp's.
        ///
        /// If the output does not have a concept of vertical retrace or a
        /// refresh cycle, or the output device is self-refreshing without
        /// a way to query the refresh count, then the arguments seq_hi
        /// and seq_lo must be zero.
        Presented {
            /// High 32 bits of the seconds part of the presentation timestamp,
            tv_sec_hi: u32,
            /// Low 32 bits of the seconds part of the presentation timestamp,
            tv_sec_lo: u32,
            /// Nanoseconds part of the presentation timestamp,
            tv_nsec: u32,
            /// Nanoseconds till next refresh,
            refresh: u32,
            /// High 32 bits of refresh counter,
            seq_hi: u32,
            /// Low 32 bits of refresh counter,
            seq_lo: u32,
            /// Combination of 'kind' values,
            flags: u32,
        },
        /// The content update was not displayed
        ///
        /// The content update was never displayed to the user.
        Discarded,
    }
    impl EventWriter for crate::wayland_objects::wp_presentation_feedback::WpPresentationFeedback {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                SyncOutput { output } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.id(output);
                    raw
                }
                Presented {
                    tv_sec_hi,
                    tv_sec_lo,
                    tv_nsec,
                    refresh,
                    seq_hi,
                    seq_lo,
                    flags,
                } => {
                    let mut raw = RawEvent::new(self.id(), 1, 7);
                    raw.uint(tv_sec_hi);
                    raw.uint(tv_sec_lo);
                    raw.uint(tv_nsec);
                    raw.uint(refresh);
                    raw.uint(seq_hi);
                    raw.uint(seq_lo);
                    raw.uint(flags);
                    raw
                }
                Discarded {} => {
                    let mut raw = RawEvent::new(self.id(), 2, 0);
                    raw
                }
            }
        }
    }
    /// Bitmask of flags in presented event
    ///
    /// These flags provide information about how the presentation of
    /// the related content update was done. The intent is to help
    /// clients assess the reliability of the feedback and the visual
    /// quality with respect to possible tearing and timings.
    #[derive(Debug)]
    pub enum Kind {
        /// Presentation was vsync'd
        ///
        /// The presentation was synchronized to the "vertical retrace" by
        /// the display hardware such that tearing does not happen.
        /// Relying on software scheduling is not acceptable for this
        /// flag. If presentation is done by a copy to the active
        /// frontbuffer, then it must guarantee that tearing cannot
        /// happen.
        Vsync = 1,
        /// Hardware provided the presentation timestamp
        ///
        /// The display hardware provided measurements that the hardware
        /// driver converted into a presentation timestamp. Sampling a
        /// clock in software is not acceptable for this flag.
        HwClock = 2,
        /// Hardware signalled the start of the presentation
        ///
        /// The display hardware signalled that it started using the new
        /// image content. The opposite of this is e.g. a timer being used
        /// to guess when the display hardware has switched to the new
        /// image content.
        HwCompletion = 4,
        /// Presentation was done zero-copy
        ///
        /// The presentation of this update was done zero-copy. This means
        /// the buffer from the client was given to display hardware as
        /// is, without copying it. Compositing with OpenGL counts as
        /// copying, even if textured directly from the client buffer.
        /// Possible zero-copy cases include direct scanout of a
        /// fullscreen surface and a surface on a hardware overlay.
        ZeroCopy = 8,
    }
}

/// Surface cropping and scaling
///
/// The global interface exposing surface cropping and scaling
/// capabilities is used to instantiate an interface extension for a
/// wl_surface object. This extended interface will then allow
/// cropping and scaling the surface contents, effectively
/// disconnecting the direct relationship between the buffer and the
/// surface size.
pub mod wp_viewporter {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Unbind from the cropping and scaling interface
        ///
        /// Informs the server that the client will not be using this
        /// protocol object anymore. This does not affect any other objects,
        /// wp_viewport objects included.
        Destroy,
        /// Extend surface interface for crop and scale
        ///
        /// Instantiate an interface extension for the given wl_surface to
        /// crop and scale its content. If the given wl_surface already has
        /// a wp_viewport object associated, the viewport_exists
        /// protocol error is raised.
        GetViewport {
            /// The new viewport interface id,
            id: WpViewportId,
            /// The surface,
            surface: WlSurfaceId,
        },
    }
    impl Parser for crate::wayland_objects::wp_viewporter::WpViewporter {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetViewport { id: req.id()?, surface: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// The surface already has a viewport object associated,
        ViewportExists = 0,
    }
}

/// Crop and scale interface to a wl_surface
///
/// An additional interface to a wl_surface object, which allows the
/// client to specify the cropping and scaling of the surface
/// contents.
///
/// This interface works with two concepts: the source rectangle (src_x,
/// src_y, src_width, src_height), and the destination size (dst_width,
/// dst_height). The contents of the source rectangle are scaled to the
/// destination size, and content outside the source rectangle is ignored.
/// This state is double-buffered, and is applied on the next
/// wl_surface.commit.
///
/// The two parts of crop and scale state are independent: the source
/// rectangle, and the destination size. Initially both are unset, that
/// is, no scaling is applied. The whole of the current wl_buffer is
/// used as the source, and the surface size is as defined in
/// wl_surface.attach.
///
/// If the destination size is set, it causes the surface size to become
/// dst_width, dst_height. The source (rectangle) is scaled to exactly
/// this size. This overrides whatever the attached wl_buffer size is,
/// unless the wl_buffer is NULL. If the wl_buffer is NULL, the surface
/// has no content and therefore no size. Otherwise, the size is always
/// at least 1x1 in surface local coordinates.
///
/// If the source rectangle is set, it defines what area of the wl_buffer is
/// taken as the source. If the source rectangle is set and the destination
/// size is not set, then src_width and src_height must be integers, and the
/// surface size becomes the source rectangle size. This results in cropping
/// without scaling. If src_width or src_height are not integers and
/// destination size is not set, the bad_size protocol error is raised when
/// the surface state is applied.
///
/// The coordinate transformations from buffer pixel coordinates up to
/// the surface-local coordinates happen in the following order:
/// 1. buffer_transform (wl_surface.set_buffer_transform)
/// 2. buffer_scale (wl_surface.set_buffer_scale)
/// 3. crop and scale (wp_viewport.set*)
/// This means, that the source rectangle coordinates of crop and scale
/// are given in the coordinates after the buffer transform and scale,
/// i.e. in the coordinates that would be the surface-local coordinates
/// if the crop and scale was not applied.
///
/// If src_x or src_y are negative, the bad_value protocol error is raised.
/// Otherwise, if the source rectangle is partially or completely outside of
/// the non-NULL wl_buffer, then the out_of_buffer protocol error is raised
/// when the surface state is applied. A NULL wl_buffer does not raise the
/// out_of_buffer error.
///
/// If the wl_surface associated with the wp_viewport is destroyed,
/// all wp_viewport requests except 'destroy' raise the protocol error
/// no_surface.
///
/// If the wp_viewport object is destroyed, the crop and scale
/// state is removed from the wl_surface. The change will be applied
/// on the next wl_surface.commit.
pub mod wp_viewport {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Remove scaling and cropping from the surface
        ///
        /// The associated wl_surface's crop and scale state is removed.
        /// The change is applied on the next wl_surface.commit.
        Destroy,
        /// Set the source rectangle for cropping
        ///
        /// Set the source rectangle of the associated wl_surface. See
        /// wp_viewport for the description, and relation to the wl_buffer
        /// size.
        ///
        /// If all of x, y, width and height are -1.0, the source rectangle is
        /// unset instead. Any other set of values where width or height are zero
        /// or negative, or x or y are negative, raise the bad_value protocol
        /// error.
        ///
        /// The crop and scale state is double-buffered state, and will be
        /// applied on the next wl_surface.commit.
        SetSource {
            /// Source rectangle x,
            x: f32,
            /// Source rectangle y,
            y: f32,
            /// Source rectangle width,
            width: f32,
            /// Source rectangle height,
            height: f32,
        },
        /// Set the surface size for scaling
        ///
        /// Set the destination size of the associated wl_surface. See
        /// wp_viewport for the description, and relation to the wl_buffer
        /// size.
        ///
        /// If width is -1 and height is -1, the destination size is unset
        /// instead. Any other pair of values for width and height that
        /// contains zero or negative values raises the bad_value protocol
        /// error.
        ///
        /// The crop and scale state is double-buffered state, and will be
        /// applied on the next wl_surface.commit.
        SetDestination {
            /// Surface width,
            width: i32,
            /// Surface height,
            height: i32,
        },
    }
    impl Parser for crate::wayland_objects::wp_viewport::WpViewport {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => SetSource {
                    x: req.fixed()?,
                    y: req.fixed()?,
                    width: req.fixed()?,
                    height: req.fixed()?,
                },
                2 => SetDestination { width: req.int()?, height: req.int()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Negative or zero values in width or height,
        BadValue = 0,
        /// Destination size is not integer,
        BadSize = 1,
        /// Source rectangle extends outside of the content area,
        OutOfBuffer = 2,
        /// The wl_surface was destroyed,
        NoSurface = 3,
    }
}

/// Create desktop-style surfaces
///
/// The xdg_wm_base interface is exposed as a global object enabling clients
/// to turn their wl_surfaces into windows in a desktop environment. It
/// defines the basic functionality needed for clients and the compositor to
/// create windows that can be dragged, resized, maximized, etc, as well as
/// creating transient windows such as popup menus.
pub mod xdg_wm_base {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy xdg_wm_base
        ///
        /// Destroy this xdg_wm_base object.
        ///
        /// Destroying a bound xdg_wm_base object while there are surfaces
        /// still alive created by this xdg_wm_base object instance is illegal
        /// and will result in a defunct_surfaces error.
        Destroy,
        /// Create a positioner object
        ///
        /// Create a positioner object. A positioner object is used to position
        /// surfaces relative to some parent surface. See the interface description
        /// and xdg_surface.get_popup for details.
        CreatePositioner { id: XdgPositionerId },
        /// Create a shell surface from a surface
        ///
        /// This creates an xdg_surface for the given surface. While xdg_surface
        /// itself is not a role, the corresponding surface may only be assigned
        /// a role extending xdg_surface, such as xdg_toplevel or xdg_popup. It is
        /// illegal to create an xdg_surface for a wl_surface which already has an
        /// assigned role and this will result in a role error.
        ///
        /// This creates an xdg_surface for the given surface. An xdg_surface is
        /// used as basis to define a role to a given surface, such as xdg_toplevel
        /// or xdg_popup. It also manages functionality shared between xdg_surface
        /// based surface roles.
        ///
        /// See the documentation of xdg_surface for more details about what an
        /// xdg_surface is and how it is used.
        GetXdgSurface { id: XdgSurfaceId, surface: WlSurfaceId },
        /// Respond to a ping event
        ///
        /// A client must respond to a ping event with a pong request or
        /// the client may be deemed unresponsive. See xdg_wm_base.ping
        /// and xdg_wm_base.error.unresponsive.
        Pong {
            /// Serial of the ping event,
            serial: u32,
        },
    }
    impl Parser for crate::wayland_objects::xdg_wm_base::XdgWmBase {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => CreatePositioner { id: req.id()? },
                2 => GetXdgSurface { id: req.id()?, surface: req.id()? },
                3 => Pong { serial: req.uint()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Check if the client is alive
        ///
        /// The ping event asks the client if it's still alive. Pass the
        /// serial specified in the event back to the compositor by sending
        /// a "pong" request back with the specified serial. See xdg_wm_base.pong.
        ///
        /// Compositors can use this to determine if the client is still
        /// alive. It's unspecified what will happen if the client doesn't
        /// respond to the ping request, or in what timeframe. Clients should
        /// try to respond in a reasonable amount of time. The “unresponsive”
        /// error is provided for compositors that wish to disconnect unresponsive
        /// clients.
        ///
        /// A compositor is free to ping in any way it wants, but a client must
        /// always respond to any xdg_wm_base object it created.
        Ping {
            /// Pass this to the pong request,
            serial: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::xdg_wm_base::XdgWmBase {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Ping { serial } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(serial);
                    raw
                }
            }
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Given wl_surface has another role,
        Role = 0,
        /// Xdg_wm_base was destroyed before children,
        DefunctSurfaces = 1,
        /// The client tried to map or destroy a non-topmost popup,
        NotTheTopmostPopup = 2,
        /// The client specified an invalid popup parent surface,
        InvalidPopupParent = 3,
        /// The client provided an invalid surface state,
        InvalidSurfaceState = 4,
        /// The client provided an invalid positioner,
        InvalidPositioner = 5,
        /// The client didn’t respond to a ping event in time,
        Unresponsive = 6,
    }
}

/// Child surface positioner
///
/// The xdg_positioner provides a collection of rules for the placement of a
/// child surface relative to a parent surface. Rules can be defined to ensure
/// the child surface remains within the visible area's borders, and to
/// specify how the child surface changes its position, such as sliding along
/// an axis, or flipping around a rectangle. These positioner-created rules are
/// constrained by the requirement that a child surface must intersect with or
/// be at least partially adjacent to its parent surface.
///
/// See the various requests for details about possible rules.
///
/// At the time of the request, the compositor makes a copy of the rules
/// specified by the xdg_positioner. Thus, after the request is complete the
/// xdg_positioner object can be destroyed or reused; further changes to the
/// object will have no effect on previous usages.
///
/// For an xdg_positioner object to be considered complete, it must have a
/// non-zero size set by set_size, and a non-zero anchor rectangle set by
/// set_anchor_rect. Passing an incomplete xdg_positioner object when
/// positioning a surface raises an invalid_positioner error.
pub mod xdg_positioner {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the xdg_positioner object
        ///
        /// Notify the compositor that the xdg_positioner will no longer be used.
        Destroy,
        /// Set the size of the to-be positioned rectangle
        ///
        /// Set the size of the surface that is to be positioned with the positioner
        /// object. The size is in surface-local coordinates and corresponds to the
        /// window geometry. See xdg_surface.set_window_geometry.
        ///
        /// If a zero or negative size is set the invalid_input error is raised.
        SetSize {
            /// Width of positioned rectangle,
            width: i32,
            /// Height of positioned rectangle,
            height: i32,
        },
        /// Set the anchor rectangle within the parent surface
        ///
        /// Specify the anchor rectangle within the parent surface that the child
        /// surface will be placed relative to. The rectangle is relative to the
        /// window geometry as defined by xdg_surface.set_window_geometry of the
        /// parent surface.
        ///
        /// When the xdg_positioner object is used to position a child surface, the
        /// anchor rectangle may not extend outside the window geometry of the
        /// positioned child's parent surface.
        ///
        /// If a negative size is set the invalid_input error is raised.
        SetAnchorRect {
            /// X position of anchor rectangle,
            x: i32,
            /// Y position of anchor rectangle,
            y: i32,
            /// Width of anchor rectangle,
            width: i32,
            /// Height of anchor rectangle,
            height: i32,
        },
        /// Set anchor rectangle anchor
        ///
        /// Defines the anchor point for the anchor rectangle. The specified anchor
        /// is used derive an anchor point that the child surface will be
        /// positioned relative to. If a corner anchor is set (e.g. 'top_left' or
        /// 'bottom_right'), the anchor point will be at the specified corner;
        /// otherwise, the derived anchor point will be centered on the specified
        /// edge, or in the center of the anchor rectangle if no edge is specified.
        SetAnchor {
            /// Anchor,
            anchor: u32,
        },
        /// Set child surface gravity
        ///
        /// Defines in what direction a surface should be positioned, relative to
        /// the anchor point of the parent surface. If a corner gravity is
        /// specified (e.g. 'bottom_right' or 'top_left'), then the child surface
        /// will be placed towards the specified gravity; otherwise, the child
        /// surface will be centered over the anchor point on any axis that had no
        /// gravity specified. If the gravity is not in the ‘gravity’ enum, an
        /// invalid_input error is raised.
        SetGravity {
            /// Gravity direction,
            gravity: u32,
        },
        /// Set the adjustment to be done when constrained
        ///
        /// Specify how the window should be positioned if the originally intended
        /// position caused the surface to be constrained, meaning at least
        /// partially outside positioning boundaries set by the compositor. The
        /// adjustment is set by constructing a bitmask describing the adjustment to
        /// be made when the surface is constrained on that axis.
        ///
        /// If no bit for one axis is set, the compositor will assume that the child
        /// surface should not change its position on that axis when constrained.
        ///
        /// If more than one bit for one axis is set, the order of how adjustments
        /// are applied is specified in the corresponding adjustment descriptions.
        ///
        /// The default adjustment is none.
        SetConstraintAdjustment {
            /// Bit mask of constraint adjustments,
            constraint_adjustment: u32,
        },
        /// Set surface position offset
        ///
        /// Specify the surface position offset relative to the position of the
        /// anchor on the anchor rectangle and the anchor on the surface. For
        /// example if the anchor of the anchor rectangle is at (x, y), the surface
        /// has the gravity bottom|right, and the offset is (ox, oy), the calculated
        /// surface position will be (x + ox, y + oy). The offset position of the
        /// surface is the one used for constraint testing. See
        /// set_constraint_adjustment.
        ///
        /// An example use case is placing a popup menu on top of a user interface
        /// element, while aligning the user interface element of the parent surface
        /// with some user interface element placed somewhere in the popup surface.
        SetOffset {
            /// Surface position x offset,
            x: i32,
            /// Surface position y offset,
            y: i32,
        },
        /// Continuously reconstrain the surface
        ///
        /// When set reactive, the surface is reconstrained if the conditions used
        /// for constraining changed, e.g. the parent window moved.
        ///
        /// If the conditions changed and the popup was reconstrained, an
        /// xdg_popup.configure event is sent with updated geometry, followed by an
        /// xdg_surface.configure event.
        SetReactive,
        ///
        ///
        /// Set the parent window geometry the compositor should use when
        /// positioning the popup. The compositor may use this information to
        /// determine the future state the popup should be constrained using. If
        /// this doesn't match the dimension of the parent the popup is eventually
        /// positioned against, the behavior is undefined.
        ///
        /// The arguments are given in the surface-local coordinate space.
        SetParentSize {
            /// Future window geometry width of parent,
            parent_width: i32,
            /// Future window geometry height of parent,
            parent_height: i32,
        },
        /// Set parent configure this is a response to
        ///
        /// Set the serial of an xdg_surface.configure event this positioner will be
        /// used in response to. The compositor may use this information together
        /// with set_parent_size to determine what future state the popup should be
        /// constrained using.
        SetParentConfigure {
            /// Serial of parent configure event,
            serial: u32,
        },
    }
    impl Parser for crate::wayland_objects::xdg_positioner::XdgPositioner {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => SetSize { width: req.int()?, height: req.int()? },
                2 => SetAnchorRect {
                    x: req.int()?,
                    y: req.int()?,
                    width: req.int()?,
                    height: req.int()?,
                },
                3 => SetAnchor { anchor: req.uint()? },
                4 => SetGravity { gravity: req.uint()? },
                5 => SetConstraintAdjustment { constraint_adjustment: req.uint()? },
                6 => SetOffset { x: req.int()?, y: req.int()? },
                7 => SetReactive,
                8 => SetParentSize { parent_width: req.int()?, parent_height: req.int()? },
                9 => SetParentConfigure { serial: req.uint()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Invalid input provided,
        InvalidInput = 0,
    }
    #[derive(Debug)]
    pub enum Anchor {
        None = 0,
        Top = 1,
        Bottom = 2,
        Left = 3,
        Right = 4,
        TopLeft = 5,
        BottomLeft = 6,
        TopRight = 7,
        BottomRight = 8,
    }
    #[derive(Debug)]
    pub enum Gravity {
        None = 0,
        Top = 1,
        Bottom = 2,
        Left = 3,
        Right = 4,
        TopLeft = 5,
        BottomLeft = 6,
        TopRight = 7,
        BottomRight = 8,
    }
    /// Constraint adjustments
    ///
    /// The constraint adjustment value define ways the compositor will adjust
    /// the position of the surface, if the unadjusted position would result
    /// in the surface being partly constrained.
    ///
    /// Whether a surface is considered 'constrained' is left to the compositor
    /// to determine. For example, the surface may be partly outside the
    /// compositor's defined 'work area', thus necessitating the child surface's
    /// position be adjusted until it is entirely inside the work area.
    ///
    /// The adjustments can be combined, according to a defined precedence: 1)
    /// Flip, 2) Slide, 3) Resize.
    #[derive(Debug)]
    pub enum ConstraintAdjustment {
        /// Don't move the child surface when constrained
        ///
        /// Don't alter the surface position even if it is constrained on some
        /// axis, for example partially outside the edge of an output.
        None = 0,
        /// Move along the x axis until unconstrained
        ///
        /// Slide the surface along the x axis until it is no longer constrained.
        ///
        /// First try to slide towards the direction of the gravity on the x axis
        /// until either the edge in the opposite direction of the gravity is
        /// unconstrained or the edge in the direction of the gravity is
        /// constrained.
        ///
        /// Then try to slide towards the opposite direction of the gravity on the
        /// x axis until either the edge in the direction of the gravity is
        /// unconstrained or the edge in the opposite direction of the gravity is
        /// constrained.
        SlideX = 1,
        /// Move along the y axis until unconstrained
        ///
        /// Slide the surface along the y axis until it is no longer constrained.
        ///
        /// First try to slide towards the direction of the gravity on the y axis
        /// until either the edge in the opposite direction of the gravity is
        /// unconstrained or the edge in the direction of the gravity is
        /// constrained.
        ///
        /// Then try to slide towards the opposite direction of the gravity on the
        /// y axis until either the edge in the direction of the gravity is
        /// unconstrained or the edge in the opposite direction of the gravity is
        /// constrained.
        SlideY = 2,
        /// Invert the anchor and gravity on the x axis
        ///
        /// Invert the anchor and gravity on the x axis if the surface is
        /// constrained on the x axis. For example, if the left edge of the
        /// surface is constrained, the gravity is 'left' and the anchor is
        /// 'left', change the gravity to 'right' and the anchor to 'right'.
        ///
        /// If the adjusted position also ends up being constrained, the resulting
        /// position of the flip_x adjustment will be the one before the
        /// adjustment.
        FlipX = 4,
        /// Invert the anchor and gravity on the y axis
        ///
        /// Invert the anchor and gravity on the y axis if the surface is
        /// constrained on the y axis. For example, if the bottom edge of the
        /// surface is constrained, the gravity is 'bottom' and the anchor is
        /// 'bottom', change the gravity to 'top' and the anchor to 'top'.
        ///
        /// The adjusted position is calculated given the original anchor
        /// rectangle and offset, but with the new flipped anchor and gravity
        /// values.
        ///
        /// If the adjusted position also ends up being constrained, the resulting
        /// position of the flip_y adjustment will be the one before the
        /// adjustment.
        FlipY = 8,
        /// Horizontally resize the surface
        ///
        /// Resize the surface horizontally so that it is completely
        /// unconstrained.
        ResizeX = 16,
        /// Vertically resize the surface
        ///
        /// Resize the surface vertically so that it is completely unconstrained.
        ResizeY = 32,
    }
}

/// Desktop user interface surface base interface
///
/// An interface that may be implemented by a wl_surface, for
/// implementations that provide a desktop-style user interface.
///
/// It provides a base set of functionality required to construct user
/// interface elements requiring management by the compositor, such as
/// toplevel windows, menus, etc. The types of functionality are split into
/// xdg_surface roles.
///
/// Creating an xdg_surface does not set the role for a wl_surface. In order
/// to map an xdg_surface, the client must create a role-specific object
/// using, e.g., get_toplevel, get_popup. The wl_surface for any given
/// xdg_surface can have at most one role, and may not be assigned any role
/// not based on xdg_surface.
///
/// A role must be assigned before any other requests are made to the
/// xdg_surface object.
///
/// The client must call wl_surface.commit on the corresponding wl_surface
/// for the xdg_surface state to take effect.
///
/// Creating an xdg_surface from a wl_surface which has a buffer attached or
/// committed is a client error, and any attempts by a client to attach or
/// manipulate a buffer prior to the first xdg_surface.configure call must
/// also be treated as errors.
///
/// After creating a role-specific object and setting it up, the client must
/// perform an initial commit without any buffer attached. The compositor
/// will reply with initial wl_surface state such as
/// wl_surface.preferred_buffer_scale followed by an xdg_surface.configure
/// event. The client must acknowledge it and is then allowed to attach a
/// buffer to map the surface.
///
/// Mapping an xdg_surface-based role surface is defined as making it
/// possible for the surface to be shown by the compositor. Note that
/// a mapped surface is not guaranteed to be visible once it is mapped.
///
/// For an xdg_surface to be mapped by the compositor, the following
/// conditions must be met:
/// (1) the client has assigned an xdg_surface-based role to the surface
/// (2) the client has set and committed the xdg_surface state and the
/// role-dependent state to the surface
/// (3) the client has committed a buffer to the surface
///
/// A newly-unmapped surface is considered to have met condition (1) out
/// of the 3 required conditions for mapping a surface if its role surface
/// has not been destroyed, i.e. the client must perform the initial commit
/// again before attaching a buffer.
pub mod xdg_surface {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the xdg_surface
        ///
        /// Destroy the xdg_surface object. An xdg_surface must only be destroyed
        /// after its role object has been destroyed, otherwise
        /// a defunct_role_object error is raised.
        Destroy,
        /// Assign the xdg_toplevel surface role
        ///
        /// This creates an xdg_toplevel object for the given xdg_surface and gives
        /// the associated wl_surface the xdg_toplevel role.
        ///
        /// See the documentation of xdg_toplevel for more details about what an
        /// xdg_toplevel is and how it is used.
        GetToplevel { id: XdgToplevelId },
        /// Assign the xdg_popup surface role
        ///
        /// This creates an xdg_popup object for the given xdg_surface and gives
        /// the associated wl_surface the xdg_popup role.
        ///
        /// If null is passed as a parent, a parent surface must be specified using
        /// some other protocol, before committing the initial state.
        ///
        /// See the documentation of xdg_popup for more details about what an
        /// xdg_popup is and how it is used.
        GetPopup { id: XdgPopupId, parent: XdgSurfaceId, positioner: XdgPositionerId },
        /// Set the new window geometry
        ///
        /// The window geometry of a surface is its "visible bounds" from the
        /// user's perspective. Client-side decorations often have invisible
        /// portions like drop-shadows which should be ignored for the
        /// purposes of aligning, placing and constraining windows.
        ///
        /// The window geometry is double buffered, and will be applied at the
        /// time wl_surface.commit of the corresponding wl_surface is called.
        ///
        /// When maintaining a position, the compositor should treat the (x, y)
        /// coordinate of the window geometry as the top left corner of the window.
        /// A client changing the (x, y) window geometry coordinate should in
        /// general not alter the position of the window.
        ///
        /// Once the window geometry of the surface is set, it is not possible to
        /// unset it, and it will remain the same until set_window_geometry is
        /// called again, even if a new subsurface or buffer is attached.
        ///
        /// If never set, the value is the full bounds of the surface,
        /// including any subsurfaces. This updates dynamically on every
        /// commit. This unset is meant for extremely simple clients.
        ///
        /// The arguments are given in the surface-local coordinate space of
        /// the wl_surface associated with this xdg_surface, and may extend outside
        /// of the wl_surface itself to mark parts of the subsurface tree as part of
        /// the window geometry.
        ///
        /// When applied, the effective window geometry will be the set window
        /// geometry clamped to the bounding rectangle of the combined
        /// geometry of the surface of the xdg_surface and the associated
        /// subsurfaces.
        ///
        /// The effective geometry will not be recalculated unless a new call to
        /// set_window_geometry is done and the new pending surface state is
        /// subsequently applied.
        ///
        /// The width and height of the effective window geometry must be
        /// greater than zero. Setting an invalid size will raise an
        /// invalid_size error.
        SetWindowGeometry { x: i32, y: i32, width: i32, height: i32 },
        /// Ack a configure event
        ///
        /// When a configure event is received, if a client commits the
        /// surface in response to the configure event, then the client
        /// must make an ack_configure request sometime before the commit
        /// request, passing along the serial of the configure event.
        ///
        /// For instance, for toplevel surfaces the compositor might use this
        /// information to move a surface to the top left only when the client has
        /// drawn itself for the maximized or fullscreen state.
        ///
        /// If the client receives multiple configure events before it
        /// can respond to one, it only has to ack the last configure event.
        /// Acking a configure event that was never sent raises an invalid_serial
        /// error.
        ///
        /// A client is not required to commit immediately after sending
        /// an ack_configure request - it may even ack_configure several times
        /// before its next surface commit.
        ///
        /// A client may send multiple ack_configure requests before committing, but
        /// only the last request sent before a commit indicates which configure
        /// event the client really is responding to.
        ///
        /// Sending an ack_configure request consumes the serial number sent with
        /// the request, as well as serial numbers sent by all configure events
        /// sent on this xdg_surface prior to the configure event referenced by
        /// the committed serial.
        ///
        /// It is an error to issue multiple ack_configure requests referencing a
        /// serial from the same configure event, or to issue an ack_configure
        /// request referencing a serial from a configure event issued before the
        /// event identified by the last ack_configure request for the same
        /// xdg_surface. Doing so will raise an invalid_serial error.
        AckConfigure {
            /// The serial from the configure event,
            serial: u32,
        },
    }
    impl Parser for crate::wayland_objects::xdg_surface::XdgSurface {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetToplevel { id: req.id()? },
                2 => GetPopup { id: req.id()?, parent: req.id()?, positioner: req.id()? },
                3 => SetWindowGeometry {
                    x: req.int()?,
                    y: req.int()?,
                    width: req.int()?,
                    height: req.int()?,
                },
                4 => AckConfigure { serial: req.uint()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Suggest a surface change
        ///
        /// The configure event marks the end of a configure sequence. A configure
        /// sequence is a set of one or more events configuring the state of the
        /// xdg_surface, including the final xdg_surface.configure event.
        ///
        /// Where applicable, xdg_surface surface roles will during a configure
        /// sequence extend this event as a latched state sent as events before the
        /// xdg_surface.configure event. Such events should be considered to make up
        /// a set of atomically applied configuration states, where the
        /// xdg_surface.configure commits the accumulated state.
        ///
        /// Clients should arrange their surface for the new states, and then send
        /// an ack_configure request with the serial sent in this configure event at
        /// some point before committing the new surface.
        ///
        /// If the client receives multiple configure events before it can respond
        /// to one, it is free to discard all but the last event it received.
        Configure {
            /// Serial of the configure event,
            serial: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::xdg_surface::XdgSurface {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Configure { serial } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(serial);
                    raw
                }
            }
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Surface was not fully constructed,
        NotConstructed = 1,
        /// Surface was already constructed,
        AlreadyConstructed = 2,
        /// Attaching a buffer to an unconfigured surface,
        UnconfiguredBuffer = 3,
        /// Invalid serial number when acking a configure event,
        InvalidSerial = 4,
        /// Width or height was zero or negative,
        InvalidSize = 5,
        /// Surface was destroyed before its role object,
        DefunctRoleObject = 6,
    }
}

/// Toplevel surface
///
/// This interface defines an xdg_surface role which allows a surface to,
/// among other things, set window-like properties such as maximize,
/// fullscreen, and minimize, set application-specific metadata like title and
/// id, and well as trigger user interactive operations such as interactive
/// resize and move.
///
/// Unmapping an xdg_toplevel means that the surface cannot be shown
/// by the compositor until it is explicitly mapped again.
/// All active operations (e.g., move, resize) are canceled and all
/// attributes (e.g. title, state, stacking, ...) are discarded for
/// an xdg_toplevel surface when it is unmapped. The xdg_toplevel returns to
/// the state it had right after xdg_surface.get_toplevel. The client
/// can re-map the toplevel by perfoming a commit without any buffer
/// attached, waiting for a configure event and handling it as usual (see
/// xdg_surface description).
///
/// Attaching a null buffer to a toplevel unmaps the surface.
pub mod xdg_toplevel {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the xdg_toplevel
        ///
        /// This request destroys the role surface and unmaps the surface;
        /// see "Unmapping" behavior in interface section for details.
        Destroy,
        /// Set the parent of this surface
        ///
        /// Set the "parent" of this surface. This surface should be stacked
        /// above the parent surface and all other ancestor surfaces.
        ///
        /// Parent surfaces should be set on dialogs, toolboxes, or other
        /// "auxiliary" surfaces, so that the parent is raised when the dialog
        /// is raised.
        ///
        /// Setting a null parent for a child surface unsets its parent. Setting
        /// a null parent for a surface which currently has no parent is a no-op.
        ///
        /// Only mapped surfaces can have child surfaces. Setting a parent which
        /// is not mapped is equivalent to setting a null parent. If a surface
        /// becomes unmapped, its children's parent is set to the parent of
        /// the now-unmapped surface. If the now-unmapped surface has no parent,
        /// its children's parent is unset. If the now-unmapped surface becomes
        /// mapped again, its parent-child relationship is not restored.
        ///
        /// The parent toplevel must not be one of the child toplevel's
        /// descendants, and the parent must be different from the child toplevel,
        /// otherwise the invalid_parent protocol error is raised.
        SetParent { parent: XdgToplevelId },
        /// Set surface title
        ///
        /// Set a short title for the surface.
        ///
        /// This string may be used to identify the surface in a task bar,
        /// window list, or other user interface elements provided by the
        /// compositor.
        ///
        /// The string must be encoded in UTF-8.
        SetTitle { title: String },
        /// Set application ID
        ///
        /// Set an application identifier for the surface.
        ///
        /// The app ID identifies the general class of applications to which
        /// the surface belongs. The compositor can use this to group multiple
        /// surfaces together, or to determine how to launch a new application.
        ///
        /// For D-Bus activatable applications, the app ID is used as the D-Bus
        /// service name.
        ///
        /// The compositor shell will try to group application surfaces together
        /// by their app ID. As a best practice, it is suggested to select app
        /// ID's that match the basename of the application's .desktop file.
        /// For example, "org.freedesktop.FooViewer" where the .desktop file is
        /// "org.freedesktop.FooViewer.desktop".
        ///
        /// Like other properties, a set_app_id request can be sent after the
        /// xdg_toplevel has been mapped to update the property.
        ///
        /// See the desktop-entry specification [0] for more details on
        /// application identifiers and how they relate to well-known D-Bus
        /// names and .desktop files.
        ///
        /// [0] https://standards.freedesktop.org/desktop-entry-spec/
        SetAppId { app_id: String },
        /// Show the window menu
        ///
        /// Clients implementing client-side decorations might want to show
        /// a context menu when right-clicking on the decorations, giving the
        /// user a menu that they can use to maximize or minimize the window.
        ///
        /// This request asks the compositor to pop up such a window menu at
        /// the given position, relative to the local surface coordinates of
        /// the parent surface. There are no guarantees as to what menu items
        /// the window menu contains, or even if a window menu will be drawn
        /// at all.
        ///
        /// This request must be used in response to some sort of user action
        /// like a button press, key press, or touch down event.
        ShowWindowMenu {
            /// The wl_seat of the user event,
            seat: WlSeatId,
            /// The serial of the user event,
            serial: u32,
            /// The x position to pop up the window menu at,
            x: i32,
            /// The y position to pop up the window menu at,
            y: i32,
        },
        /// Start an interactive move
        ///
        /// Start an interactive, user-driven move of the surface.
        ///
        /// This request must be used in response to some sort of user action
        /// like a button press, key press, or touch down event. The passed
        /// serial is used to determine the type of interactive move (touch,
        /// pointer, etc).
        ///
        /// The server may ignore move requests depending on the state of
        /// the surface (e.g. fullscreen or maximized), or if the passed serial
        /// is no longer valid.
        ///
        /// If triggered, the surface will lose the focus of the device
        /// (wl_pointer, wl_touch, etc) used for the move. It is up to the
        /// compositor to visually indicate that the move is taking place, such as
        /// updating a pointer cursor, during the move. There is no guarantee
        /// that the device focus will return when the move is completed.
        Move {
            /// The wl_seat of the user event,
            seat: WlSeatId,
            /// The serial of the user event,
            serial: u32,
        },
        /// Start an interactive resize
        ///
        /// Start a user-driven, interactive resize of the surface.
        ///
        /// This request must be used in response to some sort of user action
        /// like a button press, key press, or touch down event. The passed
        /// serial is used to determine the type of interactive resize (touch,
        /// pointer, etc).
        ///
        /// The server may ignore resize requests depending on the state of
        /// the surface (e.g. fullscreen or maximized).
        ///
        /// If triggered, the client will receive configure events with the
        /// "resize" state enum value and the expected sizes. See the "resize"
        /// enum value for more details about what is required. The client
        /// must also acknowledge configure events using "ack_configure". After
        /// the resize is completed, the client will receive another "configure"
        /// event without the resize state.
        ///
        /// If triggered, the surface also will lose the focus of the device
        /// (wl_pointer, wl_touch, etc) used for the resize. It is up to the
        /// compositor to visually indicate that the resize is taking place,
        /// such as updating a pointer cursor, during the resize. There is no
        /// guarantee that the device focus will return when the resize is
        /// completed.
        ///
        /// The edges parameter specifies how the surface should be resized, and
        /// is one of the values of the resize_edge enum. Values not matching
        /// a variant of the enum will cause the invalid_resize_edge protocol error.
        /// The compositor may use this information to update the surface position
        /// for example when dragging the top left corner. The compositor may also
        /// use this information to adapt its behavior, e.g. choose an appropriate
        /// cursor image.
        Resize {
            /// The wl_seat of the user event,
            seat: WlSeatId,
            /// The serial of the user event,
            serial: u32,
            /// Which edge or corner is being dragged,
            edges: u32,
        },
        /// Set the maximum size
        ///
        /// Set a maximum size for the window.
        ///
        /// The client can specify a maximum size so that the compositor does
        /// not try to configure the window beyond this size.
        ///
        /// The width and height arguments are in window geometry coordinates.
        /// See xdg_surface.set_window_geometry.
        ///
        /// Values set in this way are double-buffered. They will get applied
        /// on the next commit.
        ///
        /// The compositor can use this information to allow or disallow
        /// different states like maximize or fullscreen and draw accurate
        /// animations.
        ///
        /// Similarly, a tiling window manager may use this information to
        /// place and resize client windows in a more effective way.
        ///
        /// The client should not rely on the compositor to obey the maximum
        /// size. The compositor may decide to ignore the values set by the
        /// client and request a larger size.
        ///
        /// If never set, or a value of zero in the request, means that the
        /// client has no expected maximum size in the given dimension.
        /// As a result, a client wishing to reset the maximum size
        /// to an unspecified state can use zero for width and height in the
        /// request.
        ///
        /// Requesting a maximum size to be smaller than the minimum size of
        /// a surface is illegal and will result in an invalid_size error.
        ///
        /// The width and height must be greater than or equal to zero. Using
        /// strictly negative values for width or height will result in a
        /// invalid_size error.
        SetMaxSize { width: i32, height: i32 },
        /// Set the minimum size
        ///
        /// Set a minimum size for the window.
        ///
        /// The client can specify a minimum size so that the compositor does
        /// not try to configure the window below this size.
        ///
        /// The width and height arguments are in window geometry coordinates.
        /// See xdg_surface.set_window_geometry.
        ///
        /// Values set in this way are double-buffered. They will get applied
        /// on the next commit.
        ///
        /// The compositor can use this information to allow or disallow
        /// different states like maximize or fullscreen and draw accurate
        /// animations.
        ///
        /// Similarly, a tiling window manager may use this information to
        /// place and resize client windows in a more effective way.
        ///
        /// The client should not rely on the compositor to obey the minimum
        /// size. The compositor may decide to ignore the values set by the
        /// client and request a smaller size.
        ///
        /// If never set, or a value of zero in the request, means that the
        /// client has no expected minimum size in the given dimension.
        /// As a result, a client wishing to reset the minimum size
        /// to an unspecified state can use zero for width and height in the
        /// request.
        ///
        /// Requesting a minimum size to be larger than the maximum size of
        /// a surface is illegal and will result in an invalid_size error.
        ///
        /// The width and height must be greater than or equal to zero. Using
        /// strictly negative values for width and height will result in a
        /// invalid_size error.
        SetMinSize { width: i32, height: i32 },
        /// Maximize the window
        ///
        /// Maximize the surface.
        ///
        /// After requesting that the surface should be maximized, the compositor
        /// will respond by emitting a configure event. Whether this configure
        /// actually sets the window maximized is subject to compositor policies.
        /// The client must then update its content, drawing in the configured
        /// state. The client must also acknowledge the configure when committing
        /// the new content (see ack_configure).
        ///
        /// It is up to the compositor to decide how and where to maximize the
        /// surface, for example which output and what region of the screen should
        /// be used.
        ///
        /// If the surface was already maximized, the compositor will still emit
        /// a configure event with the "maximized" state.
        ///
        /// If the surface is in a fullscreen state, this request has no direct
        /// effect. It may alter the state the surface is returned to when
        /// unmaximized unless overridden by the compositor.
        SetMaximized,
        /// Unmaximize the window
        ///
        /// Unmaximize the surface.
        ///
        /// After requesting that the surface should be unmaximized, the compositor
        /// will respond by emitting a configure event. Whether this actually
        /// un-maximizes the window is subject to compositor policies.
        /// If available and applicable, the compositor will include the window
        /// geometry dimensions the window had prior to being maximized in the
        /// configure event. The client must then update its content, drawing it in
        /// the configured state. The client must also acknowledge the configure
        /// when committing the new content (see ack_configure).
        ///
        /// It is up to the compositor to position the surface after it was
        /// unmaximized; usually the position the surface had before maximizing, if
        /// applicable.
        ///
        /// If the surface was already not maximized, the compositor will still
        /// emit a configure event without the "maximized" state.
        ///
        /// If the surface is in a fullscreen state, this request has no direct
        /// effect. It may alter the state the surface is returned to when
        /// unmaximized unless overridden by the compositor.
        UnsetMaximized,
        /// Set the window as fullscreen on an output
        ///
        /// Make the surface fullscreen.
        ///
        /// After requesting that the surface should be fullscreened, the
        /// compositor will respond by emitting a configure event. Whether the
        /// client is actually put into a fullscreen state is subject to compositor
        /// policies. The client must also acknowledge the configure when
        /// committing the new content (see ack_configure).
        ///
        /// The output passed by the request indicates the client's preference as
        /// to which display it should be set fullscreen on. If this value is NULL,
        /// it's up to the compositor to choose which display will be used to map
        /// this surface.
        ///
        /// If the surface doesn't cover the whole output, the compositor will
        /// position the surface in the center of the output and compensate with
        /// with border fill covering the rest of the output. The content of the
        /// border fill is undefined, but should be assumed to be in some way that
        /// attempts to blend into the surrounding area (e.g. solid black).
        ///
        /// If the fullscreened surface is not opaque, the compositor must make
        /// sure that other screen content not part of the same surface tree (made
        /// up of subsurfaces, popups or similarly coupled surfaces) are not
        /// visible below the fullscreened surface.
        SetFullscreen { output: WlOutputId },
        /// Unset the window as fullscreen
        ///
        /// Make the surface no longer fullscreen.
        ///
        /// After requesting that the surface should be unfullscreened, the
        /// compositor will respond by emitting a configure event.
        /// Whether this actually removes the fullscreen state of the client is
        /// subject to compositor policies.
        ///
        /// Making a surface unfullscreen sets states for the surface based on the following:
        /// * the state(s) it may have had before becoming fullscreen
        /// * any state(s) decided by the compositor
        /// * any state(s) requested by the client while the surface was fullscreen
        ///
        /// The compositor may include the previous window geometry dimensions in
        /// the configure event, if applicable.
        ///
        /// The client must also acknowledge the configure when committing the new
        /// content (see ack_configure).
        UnsetFullscreen,
        /// Set the window as minimized
        ///
        /// Request that the compositor minimize your surface. There is no
        /// way to know if the surface is currently minimized, nor is there
        /// any way to unset minimization on this surface.
        ///
        /// If you are looking to throttle redrawing when minimized, please
        /// instead use the wl_surface.frame event for this, as this will
        /// also work with live previews on windows in Alt-Tab, Expose or
        /// similar compositor features.
        SetMinimized,
    }
    impl Parser for crate::wayland_objects::xdg_toplevel::XdgToplevel {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => SetParent { parent: req.id()? },
                2 => SetTitle { title: req.string()? },
                3 => SetAppId { app_id: req.string()? },
                4 => ShowWindowMenu {
                    seat: req.id()?,
                    serial: req.uint()?,
                    x: req.int()?,
                    y: req.int()?,
                },
                5 => Move { seat: req.id()?, serial: req.uint()? },
                6 => Resize { seat: req.id()?, serial: req.uint()?, edges: req.uint()? },
                7 => SetMaxSize { width: req.int()?, height: req.int()? },
                8 => SetMinSize { width: req.int()?, height: req.int()? },
                9 => SetMaximized,
                10 => UnsetMaximized,
                11 => SetFullscreen { output: req.id()? },
                12 => UnsetFullscreen,
                13 => SetMinimized,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Suggest a surface change
        ///
        /// This configure event asks the client to resize its toplevel surface or
        /// to change its state. The configured state should not be applied
        /// immediately. See xdg_surface.configure for details.
        ///
        /// The width and height arguments specify a hint to the window
        /// about how its surface should be resized in window geometry
        /// coordinates. See set_window_geometry.
        ///
        /// If the width or height arguments are zero, it means the client
        /// should decide its own window dimension. This may happen when the
        /// compositor needs to configure the state of the surface but doesn't
        /// have any information about any previous or expected dimension.
        ///
        /// The states listed in the event specify how the width/height
        /// arguments should be interpreted, and possibly how it should be
        /// drawn.
        ///
        /// Clients must send an ack_configure in response to this event. See
        /// xdg_surface.configure and xdg_surface.ack_configure for details.
        Configure { width: i32, height: i32, states: Array },
        /// Surface wants to be closed
        ///
        /// The close event is sent by the compositor when the user
        /// wants the surface to be closed. This should be equivalent to
        /// the user clicking the close button in client-side decorations,
        /// if your application has any.
        ///
        /// This is only a request that the user intends to close the
        /// window. The client may choose to ignore this request, or show
        /// a dialog to ask the user to save their data, etc.
        Close,
        /// Recommended window geometry bounds
        ///
        /// The configure_bounds event may be sent prior to a xdg_toplevel.configure
        /// event to communicate the bounds a window geometry size is recommended
        /// to constrain to.
        ///
        /// The passed width and height are in surface coordinate space. If width
        /// and height are 0, it means bounds is unknown and equivalent to as if no
        /// configure_bounds event was ever sent for this surface.
        ///
        /// The bounds can for example correspond to the size of a monitor excluding
        /// any panels or other shell components, so that a surface isn't created in
        /// a way that it cannot fit.
        ///
        /// The bounds may change at any point, and in such a case, a new
        /// xdg_toplevel.configure_bounds will be sent, followed by
        /// xdg_toplevel.configure and xdg_surface.configure.
        ConfigureBounds { width: i32, height: i32 },
        /// Compositor capabilities
        ///
        /// This event advertises the capabilities supported by the compositor. If
        /// a capability isn't supported, clients should hide or disable the UI
        /// elements that expose this functionality. For instance, if the
        /// compositor doesn't advertise support for minimized toplevels, a button
        /// triggering the set_minimized request should not be displayed.
        ///
        /// The compositor will ignore requests it doesn't support. For instance,
        /// a compositor which doesn't advertise support for minimized will ignore
        /// set_minimized requests.
        ///
        /// Compositors must send this event once before the first
        /// xdg_surface.configure event. When the capabilities change, compositors
        /// must send this event again and then send an xdg_surface.configure
        /// event.
        ///
        /// The configured state should not be applied immediately. See
        /// xdg_surface.configure for details.
        ///
        /// The capabilities are sent as an array of 32-bit unsigned integers in
        /// native endianness.
        WmCapabilities {
            /// Array of 32-bit capabilities,
            capabilities: Array,
        },
    }
    impl EventWriter for crate::wayland_objects::xdg_toplevel::XdgToplevel {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Configure { width, height, states } => {
                    let mut raw = RawEvent::new(self.id(), 0, 3);
                    raw.int(width);
                    raw.int(height);
                    raw.array(states);
                    raw
                }
                Close {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
                ConfigureBounds { width, height } => {
                    let mut raw = RawEvent::new(self.id(), 2, 2);
                    raw.int(width);
                    raw.int(height);
                    raw
                }
                WmCapabilities { capabilities } => {
                    let mut raw = RawEvent::new(self.id(), 3, 1);
                    raw.array(capabilities);
                    raw
                }
            }
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Provided value is not a valid variant of the resize_edge enum,
        InvalidResizeEdge = 0,
        /// Invalid parent toplevel,
        InvalidParent = 1,
        /// Client provided an invalid min or max size,
        InvalidSize = 2,
    }
    /// Edge values for resizing
    ///
    /// These values are used to indicate which edge of a surface
    /// is being dragged in a resize operation.
    #[derive(Debug)]
    pub enum ResizeEdge {
        None = 0,
        Top = 1,
        Bottom = 2,
        Left = 4,
        TopLeft = 5,
        BottomLeft = 6,
        Right = 8,
        TopRight = 9,
        BottomRight = 10,
    }
    /// Types of state on the surface
    ///
    /// The different state values used on the surface. This is designed for
    /// state values like maximized, fullscreen. It is paired with the
    /// configure event to ensure that both the client and the compositor
    /// setting the state can be synchronized.
    ///
    /// States set in this way are double-buffered. They will get applied on
    /// the next commit.
    #[derive(Debug)]
    pub enum State {
        /// The surface is maximized,
        /// The surface is maximized
        ///
        /// The surface is maximized. The window geometry specified in the configure
        /// event must be obeyed by the client, or the xdg_wm_base.invalid_surface_state
        /// error is raised.
        ///
        /// The client should draw without shadow or other
        /// decoration outside of the window geometry.
        Maximized = 1,
        /// The surface is fullscreen,
        /// The surface is fullscreen
        ///
        /// The surface is fullscreen. The window geometry specified in the
        /// configure event is a maximum; the client cannot resize beyond it. For
        /// a surface to cover the whole fullscreened area, the geometry
        /// dimensions must be obeyed by the client. For more details, see
        /// xdg_toplevel.set_fullscreen.
        Fullscreen = 2,
        /// The surface is being resized,
        /// The surface is being resized
        ///
        /// The surface is being resized. The window geometry specified in the
        /// configure event is a maximum; the client cannot resize beyond it.
        /// Clients that have aspect ratio or cell sizing configuration can use
        /// a smaller size, however.
        Resizing = 3,
        /// The surface is now activated,
        /// The surface is now activated
        ///
        /// Client window decorations should be painted as if the window is
        /// active. Do not assume this means that the window actually has
        /// keyboard or pointer focus.
        Activated = 4,
        /// The surface’s left edge is tiled
        ///
        /// The window is currently in a tiled layout and the left edge is
        /// considered to be adjacent to another part of the tiling grid.
        TiledLeft = 5,
        /// The surface’s right edge is tiled
        ///
        /// The window is currently in a tiled layout and the right edge is
        /// considered to be adjacent to another part of the tiling grid.
        TiledRight = 6,
        /// The surface’s top edge is tiled
        ///
        /// The window is currently in a tiled layout and the top edge is
        /// considered to be adjacent to another part of the tiling grid.
        TiledTop = 7,
        /// The surface’s bottom edge is tiled
        ///
        /// The window is currently in a tiled layout and the bottom edge is
        /// considered to be adjacent to another part of the tiling grid.
        TiledBottom = 8,
        /// Surface repaint is suspended
        ///
        /// The surface is currently not ordinarily being repainted; for
        /// example because its content is occluded by another window, or its
        /// outputs are switched off due to screen locking.
        Suspended = 9,
    }
    #[derive(Debug)]
    pub enum WmCapabilities {
        /// Show_window_menu is available,
        WindowMenu = 1,
        /// Set_maximized and unset_maximized are available,
        Maximize = 2,
        /// Set_fullscreen and unset_fullscreen are available,
        Fullscreen = 3,
        /// Set_minimized is available,
        Minimize = 4,
    }
}

/// Short-lived, popup surfaces for menus
///
/// A popup surface is a short-lived, temporary surface. It can be used to
/// implement for example menus, popovers, tooltips and other similar user
/// interface concepts.
///
/// A popup can be made to take an explicit grab. See xdg_popup.grab for
/// details.
///
/// When the popup is dismissed, a popup_done event will be sent out, and at
/// the same time the surface will be unmapped. See the xdg_popup.popup_done
/// event for details.
///
/// Explicitly destroying the xdg_popup object will also dismiss the popup and
/// unmap the surface. Clients that want to dismiss the popup when another
/// surface of their own is clicked should dismiss the popup using the destroy
/// request.
///
/// A newly created xdg_popup will be stacked on top of all previously created
/// xdg_popup surfaces associated with the same xdg_toplevel.
///
/// The parent of an xdg_popup must be mapped (see the xdg_surface
/// description) before the xdg_popup itself.
///
/// The client must call wl_surface.commit on the corresponding wl_surface
/// for the xdg_popup state to take effect.
pub mod xdg_popup {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Remove xdg_popup interface
        ///
        /// This destroys the popup. Explicitly destroying the xdg_popup
        /// object will also dismiss the popup, and unmap the surface.
        ///
        /// If this xdg_popup is not the "topmost" popup, the
        /// xdg_wm_base.not_the_topmost_popup protocol error will be sent.
        Destroy,
        /// Make the popup take an explicit grab
        ///
        /// This request makes the created popup take an explicit grab. An explicit
        /// grab will be dismissed when the user dismisses the popup, or when the
        /// client destroys the xdg_popup. This can be done by the user clicking
        /// outside the surface, using the keyboard, or even locking the screen
        /// through closing the lid or a timeout.
        ///
        /// If the compositor denies the grab, the popup will be immediately
        /// dismissed.
        ///
        /// This request must be used in response to some sort of user action like a
        /// button press, key press, or touch down event. The serial number of the
        /// event should be passed as 'serial'.
        ///
        /// The parent of a grabbing popup must either be an xdg_toplevel surface or
        /// another xdg_popup with an explicit grab. If the parent is another
        /// xdg_popup it means that the popups are nested, with this popup now being
        /// the topmost popup.
        ///
        /// Nested popups must be destroyed in the reverse order they were created
        /// in, e.g. the only popup you are allowed to destroy at all times is the
        /// topmost one.
        ///
        /// When compositors choose to dismiss a popup, they may dismiss every
        /// nested grabbing popup as well. When a compositor dismisses popups, it
        /// will follow the same dismissing order as required from the client.
        ///
        /// If the topmost grabbing popup is destroyed, the grab will be returned to
        /// the parent of the popup, if that parent previously had an explicit grab.
        ///
        /// If the parent is a grabbing popup which has already been dismissed, this
        /// popup will be immediately dismissed. If the parent is a popup that did
        /// not take an explicit grab, an error will be raised.
        ///
        /// During a popup grab, the client owning the grab will receive pointer
        /// and touch events for all their surfaces as normal (similar to an
        /// "owner-events" grab in X11 parlance), while the top most grabbing popup
        /// will always have keyboard focus.
        Grab {
            /// The wl_seat of the user event,
            seat: WlSeatId,
            /// The serial of the user event,
            serial: u32,
        },
        /// Recalculate the popup's location
        ///
        /// Reposition an already-mapped popup. The popup will be placed given the
        /// details in the passed xdg_positioner object, and a
        /// xdg_popup.repositioned followed by xdg_popup.configure and
        /// xdg_surface.configure will be emitted in response. Any parameters set
        /// by the previous positioner will be discarded.
        ///
        /// The passed token will be sent in the corresponding
        /// xdg_popup.repositioned event. The new popup position will not take
        /// effect until the corresponding configure event is acknowledged by the
        /// client. See xdg_popup.repositioned for details. The token itself is
        /// opaque, and has no other special meaning.
        ///
        /// If multiple reposition requests are sent, the compositor may skip all
        /// but the last one.
        ///
        /// If the popup is repositioned in response to a configure event for its
        /// parent, the client should send an xdg_positioner.set_parent_configure
        /// and possibly an xdg_positioner.set_parent_size request to allow the
        /// compositor to properly constrain the popup.
        ///
        /// If the popup is repositioned together with a parent that is being
        /// resized, but not in response to a configure event, the client should
        /// send an xdg_positioner.set_parent_size request.
        Reposition {
            positioner: XdgPositionerId,
            /// Reposition request token,
            token: u32,
        },
    }
    impl Parser for crate::wayland_objects::xdg_popup::XdgPopup {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => Grab { seat: req.id()?, serial: req.uint()? },
                2 => Reposition { positioner: req.id()?, token: req.uint()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Configure the popup surface
        ///
        /// This event asks the popup surface to configure itself given the
        /// configuration. The configured state should not be applied immediately.
        /// See xdg_surface.configure for details.
        ///
        /// The x and y arguments represent the position the popup was placed at
        /// given the xdg_positioner rule, relative to the upper left corner of the
        /// window geometry of the parent surface.
        ///
        /// For version 2 or older, the configure event for an xdg_popup is only
        /// ever sent once for the initial configuration. Starting with version 3,
        /// it may be sent again if the popup is setup with an xdg_positioner with
        /// set_reactive requested, or in response to xdg_popup.reposition requests.
        Configure {
            /// X position relative to parent surface window geometry,
            x: i32,
            /// Y position relative to parent surface window geometry,
            y: i32,
            /// Window geometry width,
            width: i32,
            /// Window geometry height,
            height: i32,
        },
        /// Popup interaction is done
        ///
        /// The popup_done event is sent out when a popup is dismissed by the
        /// compositor. The client should destroy the xdg_popup object at this
        /// point.
        PopupDone,
        /// Signal the completion of a repositioned request
        ///
        /// The repositioned event is sent as part of a popup configuration
        /// sequence, together with xdg_popup.configure and lastly
        /// xdg_surface.configure to notify the completion of a reposition request.
        ///
        /// The repositioned event is to notify about the completion of a
        /// xdg_popup.reposition request. The token argument is the token passed
        /// in the xdg_popup.reposition request.
        ///
        /// Immediately after this event is emitted, xdg_popup.configure and
        /// xdg_surface.configure will be sent with the updated size and position,
        /// as well as a new configure serial.
        ///
        /// The client should optionally update the content of the popup, but must
        /// acknowledge the new popup configuration for the new position to take
        /// effect. See xdg_surface.ack_configure for details.
        Repositioned {
            /// Reposition request token,
            token: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::xdg_popup::XdgPopup {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Configure { x, y, width, height } => {
                    let mut raw = RawEvent::new(self.id(), 0, 4);
                    raw.int(x);
                    raw.int(y);
                    raw.int(width);
                    raw.int(height);
                    raw
                }
                PopupDone {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
                Repositioned { token } => {
                    let mut raw = RawEvent::new(self.id(), 2, 1);
                    raw.uint(token);
                    raw
                }
            }
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Tried to grab after being mapped,
        InvalidGrab = 0,
    }
}

/// Surface content type manager
///
/// This interface allows a client to describe the kind of content a surface
/// will display, to allow the compositor to optimize its behavior for it.
///
/// Warning! The protocol described in this file is currently in the testing
/// phase. Backward compatible changes may be added together with the
/// corresponding interface version bump. Backward incompatible changes can
/// only be done by creating a new major version of the extension.
pub mod wp_content_type_manager_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the content type manager object
        ///
        /// Destroy the content type manager. This doesn't destroy objects created
        /// with the manager.
        Destroy,
        /// Create a new toplevel decoration object
        ///
        /// Create a new content type object associated with the given surface.
        ///
        /// Creating a wp_content_type_v1 from a wl_surface which already has one
        /// attached is a client error: already_constructed.
        GetSurfaceContentType { id: WpContentTypeV1Id, surface: WlSurfaceId },
    }
    impl Parser for crate::wayland_objects::wp_content_type_manager_v1::WpContentTypeManagerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetSurfaceContentType { id: req.id()?, surface: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Wl_surface already has a content type object,
        AlreadyConstructed = 0,
    }
}

/// Content type object for a surface
///
/// The content type object allows the compositor to optimize for the kind
/// of content shown on the surface. A compositor may for example use it to
/// set relevant drm properties like "content type".
///
/// The client may request to switch to another content type at any time.
/// When the associated surface gets destroyed, this object becomes inert and
/// the client should destroy it.
pub mod wp_content_type_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the content type object
        ///
        /// Switch back to not specifying the content type of this surface. This is
        /// equivalent to setting the content type to none, including double
        /// buffering semantics. See set_content_type for details.
        Destroy,
        /// Specify the content type
        ///
        /// Set the surface content type. This informs the compositor that the
        /// client believes it is displaying buffers matching this content type.
        ///
        /// This is purely a hint for the compositor, which can be used to adjust
        /// its behavior or hardware settings to fit the presented content best.
        ///
        /// The content type is double-buffered state, see wl_surface.commit for
        /// details.
        SetContentType {
            /// The content type,
            content_type: u32,
        },
    }
    impl Parser for crate::wayland_objects::wp_content_type_v1::WpContentTypeV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => SetContentType { content_type: req.uint()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    /// Possible content types
    ///
    /// These values describe the available content types for a surface.
    #[derive(Debug)]
    pub enum Type {
        /// No content type applies
        ///
        /// The content type none means that either the application has no data
        /// about the content type, or that the content doesn't fit into one of
        /// the other categories.
        None = 0,
        /// Photo content type
        ///
        /// The content type photo describes content derived from digital still
        /// pictures and may be presented with minimal processing.
        Photo = 1,
        /// Video content type
        ///
        /// The content type video describes a video or animation and may be
        /// presented with more accurate timing to avoid stutter. Where scaling
        /// is needed, scaling methods more appropriate for video may be used.
        Video = 2,
        /// Game content type
        ///
        /// The content type game describes a running game. Its content may be
        /// presented with reduced latency.
        Game = 3,
    }
}

/// Lease device
///
/// This protocol is used by Wayland compositors which act as Direct
/// Renderering Manager (DRM) masters to lease DRM resources to Wayland
/// clients.
///
/// The compositor will advertise one wp_drm_lease_device_v1 global for each
/// DRM node. Some time after a client binds to the wp_drm_lease_device_v1
/// global, the compositor will send a drm_fd event followed by zero, one or
/// more connector events. After all currently available connectors have been
/// sent, the compositor will send a wp_drm_lease_device_v1.done event.
///
/// When the list of connectors available for lease changes the compositor
/// will send wp_drm_lease_device_v1.connector events for added connectors and
/// wp_drm_lease_connector_v1.withdrawn events for removed connectors,
/// followed by a wp_drm_lease_device_v1.done event.
///
/// The compositor will indicate when a device is gone by removing the global
/// via a wl_registry.global_remove event. Upon receiving this event, the
/// client should destroy any matching wp_drm_lease_device_v1 object.
///
/// To destroy a wp_drm_lease_device_v1 object, the client must first issue
/// a release request. Upon receiving this request, the compositor will
/// immediately send a released event and destroy the object. The client must
/// continue to process and discard drm_fd and connector events until it
/// receives the released event. Upon receiving the released event, the
/// client can safely cleanup any client-side resources.
///
/// Warning! The protocol described in this file is currently in the testing
/// phase. Backward compatible changes may be added together with the
/// corresponding interface version bump. Backward incompatible changes can
/// only be done by creating a new major version of the extension.
pub mod wp_drm_lease_device_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Create a lease request object
        ///
        /// Creates a lease request object.
        ///
        /// See the documentation for wp_drm_lease_request_v1 for details.
        CreateLeaseRequest { id: WpDrmLeaseRequestV1Id },
        /// Release this object
        ///
        /// Indicates the client no longer wishes to use this object. In response
        /// the compositor will immediately send the released event and destroy
        /// this object. It can however not guarantee that the client won't receive
        /// connector events before the released event. The client must not send any
        /// requests after this one, doing so will raise a wl_display error.
        /// Existing connectors, lease request and leases will not be affected.
        Release,
    }
    impl Parser for crate::wayland_objects::wp_drm_lease_device_v1::WpDrmLeaseDeviceV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => CreateLeaseRequest { id: req.id()? },
                1 => Release,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Open a non-master fd for this DRM node
        ///
        /// The compositor will send this event when the wp_drm_lease_device_v1
        /// global is bound, although there are no guarantees as to how long this
        /// takes - the compositor might need to wait until regaining DRM master.
        /// The included fd is a non-master DRM file descriptor opened for this
        /// device and the compositor must not authenticate it.
        /// The purpose of this event is to give the client the ability to
        /// query DRM and discover information which may help them pick the
        /// appropriate DRM device or select the appropriate connectors therein.
        DrmFd {
            /// DRM file descriptor,
            fd: RawFd,
        },
        /// Advertise connectors available for leases
        ///
        /// The compositor will use this event to advertise connectors available for
        /// lease by clients. This object may be passed into a lease request to
        /// indicate the client would like to lease that connector, see
        /// wp_drm_lease_request_v1.request_connector for details. While the
        /// compositor will make a best effort to not send disconnected connectors,
        /// no guarantees can be made.
        ///
        /// The compositor must send the drm_fd event before sending connectors.
        /// After the drm_fd event it will send all available connectors but may
        /// send additional connectors at any time.
        Connector { id: WpDrmLeaseConnectorV1Id },
        /// Signals grouping of connectors
        ///
        /// The compositor will send this event to indicate that it has sent all
        /// currently available connectors after the client binds to the global or
        /// when it updates the connector list, for example on hotplug, drm master
        /// change or when a leased connector becomes available again. It will
        /// similarly send this event to group wp_drm_lease_connector_v1.withdrawn
        /// events of connectors of this device.
        Done,
        /// The compositor has finished using the device
        ///
        /// This event is sent in response to the release request and indicates
        /// that the compositor is done sending connector events.
        /// The compositor will destroy this object immediately after sending the
        /// event and it will become invalid. The client should release any
        /// resources associated with this device after receiving this event.
        Released,
    }
    impl EventWriter for crate::wayland_objects::wp_drm_lease_device_v1::WpDrmLeaseDeviceV1 {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                DrmFd { fd } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.fd(fd);
                    raw
                }
                Connector { id } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.id(id);
                    raw
                }
                Done {} => {
                    let mut raw = RawEvent::new(self.id(), 2, 0);
                    raw
                }
                Released {} => {
                    let mut raw = RawEvent::new(self.id(), 3, 0);
                    raw
                }
            }
        }
    }
}

/// A leasable DRM connector
///
/// Represents a DRM connector which is available for lease. These objects are
/// created via wp_drm_lease_device_v1.connector events, and should be passed
/// to lease requests via wp_drm_lease_request_v1.request_connector.
/// Immediately after the wp_drm_lease_connector_v1 object is created the
/// compositor will send a name, a description, a connector_id and a done
/// event. When the description is updated the compositor will send a
/// description event followed by a done event.
pub mod wp_drm_lease_connector_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy connector
        ///
        /// The client may send this request to indicate that it will not use this
        /// connector. Clients are encouraged to send this after receiving the
        /// "withdrawn" event so that the server can release the resources
        /// associated with this connector offer. Neither existing lease requests
        /// nor leases will be affected.
        Destroy,
    }
    impl Parser for crate::wayland_objects::wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Name
        ///
        /// The compositor sends this event once the connector is created to
        /// indicate the name of this connector. This will not change for the
        /// duration of the Wayland session, but is not guaranteed to be consistent
        /// between sessions.
        ///
        /// If the compositor supports wl_output version 4 and this connector
        /// corresponds to a wl_output, the compositor should use the same name as
        /// for the wl_output.
        Name {
            /// Connector name,
            name: String,
        },
        /// Description
        ///
        /// The compositor sends this event once the connector is created to provide
        /// a human-readable description for this connector, which may be presented
        /// to the user. The compositor may send this event multiple times over the
        /// lifetime of this object to reflect changes in the description.
        Description {
            /// Connector description,
            description: String,
        },
        /// Connector_id
        ///
        /// The compositor sends this event once the connector is created to
        /// indicate the DRM object ID which represents the underlying connector
        /// that is being offered. Note that the final lease may include additional
        /// object IDs, such as CRTCs and planes.
        ConnectorId {
            /// DRM connector ID,
            connector_id: u32,
        },
        /// All properties have been sent
        ///
        /// This event is sent after all properties of a connector have been sent.
        /// This allows changes to the properties to be seen as atomic even if they
        /// happen via multiple events.
        Done,
        /// Lease offer withdrawn
        ///
        /// Sent to indicate that the compositor will no longer honor requests for
        /// DRM leases which include this connector. The client may still issue a
        /// lease request including this connector, but the compositor will send
        /// wp_drm_lease_v1.finished without issuing a lease fd. Compositors are
        /// encouraged to send this event when they lose access to connector, for
        /// example when the connector is hot-unplugged, when the connector gets
        /// leased to a client or when the compositor loses DRM master.
        Withdrawn,
    }
    impl EventWriter for crate::wayland_objects::wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1 {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Name { name } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.string(name);
                    raw
                }
                Description { description } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.string(description);
                    raw
                }
                ConnectorId { connector_id } => {
                    let mut raw = RawEvent::new(self.id(), 2, 1);
                    raw.uint(connector_id);
                    raw
                }
                Done {} => {
                    let mut raw = RawEvent::new(self.id(), 3, 0);
                    raw
                }
                Withdrawn {} => {
                    let mut raw = RawEvent::new(self.id(), 4, 0);
                    raw
                }
            }
        }
    }
}

/// DRM lease request
///
/// A client that wishes to lease DRM resources will attach the list of
/// connectors advertised with wp_drm_lease_device_v1.connector that they
/// wish to lease, then use wp_drm_lease_request_v1.submit to submit the
/// request.
pub mod wp_drm_lease_request_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Request a connector for this lease
        ///
        /// Indicates that the client would like to lease the given connector.
        /// This is only used as a suggestion, the compositor may choose to
        /// include any resources in the lease it issues, or change the set of
        /// leased resources at any time. Compositors are however encouraged to
        /// include the requested connector and other resources necessary
        /// to drive the connected output in the lease.
        ///
        /// Requesting a connector that was created from a different lease device
        /// than this lease request raises the wrong_device error. Requesting a
        /// connector twice will raise the duplicate_connector error.
        RequestConnector { connector: WpDrmLeaseConnectorV1Id },
        /// Submit the lease request
        ///
        /// Submits the lease request and creates a new wp_drm_lease_v1 object.
        /// After calling submit the compositor will immediately destroy this
        /// object, issuing any more requests will cause a wl_diplay error.
        /// The compositor doesn't make any guarantees about the events of the
        /// lease object, clients cannot expect an immediate response.
        /// Not requesting any connectors before submitting the lease request
        /// will raise the empty_lease error.
        Submit { id: WpDrmLeaseV1Id },
    }
    impl Parser for crate::wayland_objects::wp_drm_lease_request_v1::WpDrmLeaseRequestV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => RequestConnector { connector: req.id()? },
                1 => Submit { id: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Requested a connector from a different lease device,
        WrongDevice = 0,
        /// Requested a connector twice,
        DuplicateConnector = 1,
        /// Requested a lease without requesting a connector,
        EmptyLease = 2,
    }
}

/// A DRM lease
///
/// A DRM lease object is used to transfer the DRM file descriptor to the
/// client and manage the lifetime of the lease.
///
/// Some time after the wp_drm_lease_v1 object is created, the compositor
/// will reply with the lease request's result. If the lease request is
/// granted, the compositor will send a lease_fd event. If the lease request
/// is denied, the compositor will send a finished event without a lease_fd
/// event.
pub mod wp_drm_lease_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroys the lease object
        ///
        /// The client should send this to indicate that it no longer wishes to use
        /// this lease. The compositor should use drmModeRevokeLease on the
        /// appropriate file descriptor, if necessary.
        Destroy,
    }
    impl Parser for crate::wayland_objects::wp_drm_lease_v1::WpDrmLeaseV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Shares the DRM file descriptor
        ///
        /// This event returns a file descriptor suitable for use with DRM-related
        /// ioctls. The client should use drmModeGetLease to enumerate the DRM
        /// objects which have been leased to them. The compositor guarantees it
        /// will not use the leased DRM objects itself until it sends the finished
        /// event. If the compositor cannot or will not grant a lease for the
        /// requested connectors, it will not send this event, instead sending the
        /// finished event.
        ///
        /// The compositor will send this event at most once during this objects
        /// lifetime.
        LeaseFd {
            /// Leased DRM file descriptor,
            leased_fd: RawFd,
        },
        /// Sent when the lease has been revoked
        ///
        /// The compositor uses this event to either reject a lease request, or if
        /// it previously sent a lease_fd, to notify the client that the lease has
        /// been revoked. If the client requires a new lease, they should destroy
        /// this object and submit a new lease request. The compositor will send
        /// no further events for this object after sending the finish event.
        /// Compositors should revoke the lease when any of the leased resources
        /// become unavailable, namely when a hot-unplug occurs or when the
        /// compositor loses DRM master.
        Finished,
    }
    impl EventWriter for crate::wayland_objects::wp_drm_lease_v1::WpDrmLeaseV1 {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                LeaseFd { leased_fd } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.fd(leased_fd);
                    raw
                }
                Finished {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
            }
        }
    }
}

/// Idle notification manager
///
/// This interface allows clients to monitor user idle status.
///
/// After binding to this global, clients can create ext_idle_notification_v1
/// objects to get notified when the user is idle for a given amount of time.
pub mod ext_idle_notifier_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the manager
        ///
        /// Destroy the manager object. All objects created via this interface
        /// remain valid.
        Destroy,
        /// Create a notification object
        ///
        /// Create a new idle notification object.
        ///
        /// The notification object has a minimum timeout duration and is tied to a
        /// seat. The client will be notified if the seat is inactive for at least
        /// the provided timeout. See ext_idle_notification_v1 for more details.
        ///
        /// A zero timeout is valid and means the client wants to be notified as
        /// soon as possible when the seat is inactive.
        GetIdleNotification {
            id: ExtIdleNotificationV1Id,
            /// Minimum idle timeout in msec,
            timeout: u32,
            seat: WlSeatId,
        },
    }
    impl Parser for crate::wayland_objects::ext_idle_notifier_v1::ExtIdleNotifierV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetIdleNotification { id: req.id()?, timeout: req.uint()?, seat: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// Idle notification
///
/// This interface is used by the compositor to send idle notification events
/// to clients.
///
/// Initially the notification object is not idle. The notification object
/// becomes idle when no user activity has happened for at least the timeout
/// duration, starting from the creation of the notification object. User
/// activity may include input events or a presence sensor, but is
/// compositor-specific. If an idle inhibitor is active (e.g. another client
/// has created a zwp_idle_inhibitor_v1 on a visible surface), the compositor
/// must not make the notification object idle.
///
/// When the notification object becomes idle, an idled event is sent. When
/// user activity starts again, the notification object stops being idle,
/// a resumed event is sent and the timeout is restarted.
pub mod ext_idle_notification_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the notification object
        ///
        /// Destroy the notification object.
        Destroy,
    }
    impl Parser for crate::wayland_objects::ext_idle_notification_v1::ExtIdleNotificationV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Notification object is idle
        ///
        /// This event is sent when the notification object becomes idle.
        ///
        /// It's a compositor protocol error to send this event twice without a
        /// resumed event in-between.
        Idled,
        /// Notification object is no longer idle
        ///
        /// This event is sent when the notification object stops being idle.
        ///
        /// It's a compositor protocol error to send this event twice without an
        /// idled event in-between. It's a compositor protocol error to send this
        /// event prior to any idled event.
        Resumed,
    }
    impl EventWriter for crate::wayland_objects::ext_idle_notification_v1::ExtIdleNotificationV1 {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Idled {} => {
                    let mut raw = RawEvent::new(self.id(), 0, 0);
                    raw
                }
                Resumed {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
            }
        }
    }
}

/// Used to lock the session
///
/// This interface is used to request that the session be locked.
pub mod ext_session_lock_manager_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the session lock manager object
        ///
        /// This informs the compositor that the session lock manager object will
        /// no longer be used. Existing objects created through this interface
        /// remain valid.
        Destroy,
        /// Attempt to lock the session
        ///
        /// This request creates a session lock and asks the compositor to lock the
        /// session. The compositor will send either the ext_session_lock_v1.locked
        /// or ext_session_lock_v1.finished event on the created object in
        /// response to this request.
        Lock { id: ExtSessionLockV1Id },
    }
    impl Parser for crate::wayland_objects::ext_session_lock_manager_v1::ExtSessionLockManagerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => Lock { id: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// Manage lock state and create lock surfaces
///
/// In response to the creation of this object the compositor must send
/// either the locked or finished event.
///
/// The locked event indicates that the session is locked. This means
/// that the compositor must stop rendering and providing input to normal
/// clients. Instead the compositor must blank all outputs with an opaque
/// color such that their normal content is fully hidden.
///
/// The only surfaces that should be rendered while the session is locked
/// are the lock surfaces created through this interface and optionally,
/// at the compositor's discretion, special privileged surfaces such as
/// input methods or portions of desktop shell UIs.
///
/// The locked event must not be sent until a new "locked" frame (either
/// from a session lock surface or the compositor blanking the output) has
/// been presented on all outputs and no security sensitive normal/unlocked
/// content is possibly visible.
///
/// The finished event should be sent immediately on creation of this
/// object if the compositor decides that the locked event will not be sent.
///
/// The compositor may wait for the client to create and render session lock
/// surfaces before sending the locked event to avoid displaying intermediate
/// blank frames. However, it must impose a reasonable time limit if
/// waiting and send the locked event as soon as the hard requirements
/// described above can be met if the time limit expires. Clients should
/// immediately create lock surfaces for all outputs on creation of this
/// object to make this possible.
///
/// This behavior of the locked event is required in order to prevent
/// possible race conditions with clients that wish to suspend the system
/// or similar after locking the session. Without these semantics, clients
/// triggering a suspend after receiving the locked event would race with
/// the first "locked" frame being presented and normal/unlocked frames
/// might be briefly visible as the system is resumed if the suspend
/// operation wins the race.
///
/// If the client dies while the session is locked, the compositor must not
/// unlock the session in response. It is acceptable for the session to be
/// permanently locked if this happens. The compositor may choose to continue
/// to display the lock surfaces the client had mapped before it died or
/// alternatively fall back to a solid color, this is compositor policy.
///
/// Compositors may also allow a secure way to recover the session, the
/// details of this are compositor policy. Compositors may allow a new
/// client to create a ext_session_lock_v1 object and take responsibility
/// for unlocking the session, they may even start a new lock client
/// instance automatically.
pub mod ext_session_lock_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the session lock
        ///
        /// This informs the compositor that the lock object will no longer be
        /// used. Existing objects created through this interface remain valid.
        ///
        /// After this request is made, lock surfaces created through this object
        /// should be destroyed by the client as they will no longer be used by
        /// the compositor.
        ///
        /// It is a protocol error to make this request if the locked event was
        /// sent, the unlock_and_destroy request must be used instead.
        Destroy,
        /// Create a lock surface for a given output
        ///
        /// The client is expected to create lock surfaces for all outputs
        /// currently present and any new outputs as they are advertised. These
        /// won't be displayed by the compositor unless the lock is successful
        /// and the locked event is sent.
        ///
        /// Providing a wl_surface which already has a role or already has a buffer
        /// attached or committed is a protocol error, as is attaching/committing
        /// a buffer before the first ext_session_lock_surface_v1.configure event.
        ///
        /// Attempting to create more than one lock surface for a given output
        /// is a duplicate_output protocol error.
        GetLockSurface { id: ExtSessionLockSurfaceV1Id, surface: WlSurfaceId, output: WlOutputId },
        /// Unlock the session, destroying the object
        ///
        /// This request indicates that the session should be unlocked, for
        /// example because the user has entered their password and it has been
        /// verified by the client.
        ///
        /// This request also informs the compositor that the lock object will
        /// no longer be used and should be destroyed. Existing objects created
        /// through this interface remain valid.
        ///
        /// After this request is made, lock surfaces created through this object
        /// should be destroyed by the client as they will no longer be used by
        /// the compositor.
        ///
        /// It is a protocol error to make this request if the locked event has
        /// not been sent. In that case, the lock object must be destroyed using
        /// the destroy request.
        ///
        /// Note that a correct client that wishes to exit directly after unlocking
        /// the session must use the wl_display.sync request to ensure the server
        /// receives and processes the unlock_and_destroy request. Otherwise
        /// there is no guarantee that the server has unlocked the session due
        /// to the asynchronous nature of the Wayland protocol. For example,
        /// the server might terminate the client with a protocol error before
        /// it processes the unlock_and_destroy request.
        UnlockAndDestroy,
    }
    impl Parser for crate::wayland_objects::ext_session_lock_v1::ExtSessionLockV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetLockSurface { id: req.id()?, surface: req.id()?, output: req.id()? },
                2 => UnlockAndDestroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Session successfully locked
        ///
        /// This client is now responsible for displaying graphics while the
        /// session is locked and deciding when to unlock the session.
        ///
        /// The locked event must not be sent until a new "locked" frame has been
        /// presented on all outputs and no security sensitive normal/unlocked
        /// content is possibly visible.
        ///
        /// If this event is sent, making the destroy request is a protocol error,
        /// the lock object must be destroyed using the unlock_and_destroy request.
        Locked,
        /// The session lock object should be destroyed
        ///
        /// The compositor has decided that the session lock should be destroyed
        /// as it will no longer be used by the compositor. Exactly when this
        /// event is sent is compositor policy, but it must never be sent more
        /// than once for a given session lock object.
        ///
        /// This might be sent because there is already another ext_session_lock_v1
        /// object held by a client, or the compositor has decided to deny the
        /// request to lock the session for some other reason. This might also
        /// be sent because the compositor implements some alternative, secure
        /// way to authenticate and unlock the session.
        ///
        /// The finished event should be sent immediately on creation of this
        /// object if the compositor decides that the locked event will not
        /// be sent.
        ///
        /// If the locked event is sent on creation of this object the finished
        /// event may still be sent at some later time in this object's
        /// lifetime. This is compositor policy.
        ///
        /// Upon receiving this event, the client should make either the destroy
        /// request or the unlock_and_destroy request, depending on whether or
        /// not the locked event was received on this object.
        Finished,
    }
    impl EventWriter for crate::wayland_objects::ext_session_lock_v1::ExtSessionLockV1 {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Locked {} => {
                    let mut raw = RawEvent::new(self.id(), 0, 0);
                    raw
                }
                Finished {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
            }
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Attempted to destroy session lock while locked,
        InvalidDestroy = 0,
        /// Unlock requested but locked event was never sent,
        InvalidUnlock = 1,
        /// Given wl_surface already has a role,
        Role = 2,
        /// Given output already has a lock surface,
        DuplicateOutput = 3,
        /// Given wl_surface has a buffer attached or committed,
        AlreadyConstructed = 4,
    }
}

/// A surface displayed while the session is locked
///
/// The client may use lock surfaces to display a screensaver, render a
/// dialog to enter a password and unlock the session, or however else it
/// sees fit.
///
/// On binding this interface the compositor will immediately send the
/// first configure event. After making the ack_configure request in
/// response to this event the client should attach and commit the first
/// buffer. Committing the surface before acking the first configure is a
/// protocol error. Committing the surface with a null buffer at any time
/// is a protocol error.
///
/// The compositor is free to handle keyboard/pointer focus for lock
/// surfaces however it chooses. A reasonable way to do this would be to
/// give the first lock surface created keyboard focus and change keyboard
/// focus if the user clicks on other surfaces.
pub mod ext_session_lock_surface_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the lock surface object
        ///
        /// This informs the compositor that the lock surface object will no
        /// longer be used.
        ///
        /// It is recommended for a lock client to destroy lock surfaces if
        /// their corresponding wl_output global is removed.
        ///
        /// If a lock surface on an active output is destroyed before the
        /// ext_session_lock_v1.unlock_and_destroy event is sent, the compositor
        /// must fall back to rendering a solid color.
        Destroy,
        /// Ack a configure event
        ///
        /// When a configure event is received, if a client commits the surface
        /// in response to the configure event, then the client must make an
        /// ack_configure request sometime before the commit request, passing
        /// along the serial of the configure event.
        ///
        /// If the client receives multiple configure events before it can
        /// respond to one, it only has to ack the last configure event.
        ///
        /// A client is not required to commit immediately after sending an
        /// ack_configure request - it may even ack_configure several times
        /// before its next surface commit.
        ///
        /// A client may send multiple ack_configure requests before committing,
        /// but only the last request sent before a commit indicates which
        /// configure event the client really is responding to.
        ///
        /// Sending an ack_configure request consumes the configure event
        /// referenced by the given serial, as well as all older configure events
        /// sent on this object.
        ///
        /// It is a protocol error to issue multiple ack_configure requests
        /// referencing the same configure event or to issue an ack_configure
        /// request referencing a configure event older than the last configure
        /// event acked for a given lock surface.
        AckConfigure {
            /// Serial from the configure event,
            serial: u32,
        },
    }
    impl Parser for crate::wayland_objects::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => AckConfigure { serial: req.uint()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// The client should resize its surface
        ///
        /// This event is sent once on binding the interface and may be sent again
        /// at the compositor's discretion, for example if output geometry changes.
        ///
        /// The width and height are in surface-local coordinates and are exact
        /// requirements. Failing to match these surface dimensions in the next
        /// commit after acking a configure is a protocol error.
        Configure {
            /// Serial for use in ack_configure,
            serial: u32,
            width: u32,
            height: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1 {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Configure { serial, width, height } => {
                    let mut raw = RawEvent::new(self.id(), 0, 3);
                    raw.uint(serial);
                    raw.uint(width);
                    raw.uint(height);
                    raw
                }
            }
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Surface committed before first ack_configure request,
        CommitBeforeFirstAck = 0,
        /// Surface committed with a null buffer,
        NullBuffer = 1,
        /// Failed to match ack'd width/height,
        DimensionsMismatch = 2,
        /// Serial provided in ack_configure is invalid,
        InvalidSerial = 3,
    }
}

/// Fractional surface scale information
///
/// A global interface for requesting surfaces to use fractional scales.
pub mod wp_fractional_scale_manager_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Unbind the fractional surface scale interface
        ///
        /// Informs the server that the client will not be using this protocol
        /// object anymore. This does not affect any other objects,
        /// wp_fractional_scale_v1 objects included.
        Destroy,
        /// Extend surface interface for scale information
        ///
        /// Create an add-on object for the the wl_surface to let the compositor
        /// request fractional scales. If the given wl_surface already has a
        /// wp_fractional_scale_v1 object associated, the fractional_scale_exists
        /// protocol error is raised.
        GetFractionalScale {
            /// The new surface scale info interface id,
            id: WpFractionalScaleV1Id,
            /// The surface,
            surface: WlSurfaceId,
        },
    }
    impl Parser for crate::wayland_objects::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetFractionalScale { id: req.id()?, surface: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// The surface already has a fractional_scale object associated,
        FractionalScaleExists = 0,
    }
}

/// Fractional scale interface to a wl_surface
///
/// An additional interface to a wl_surface object which allows the compositor
/// to inform the client of the preferred scale.
pub mod wp_fractional_scale_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Remove surface scale information for surface
        ///
        /// Destroy the fractional scale object. When this object is destroyed,
        /// preferred_scale events will no longer be sent.
        Destroy,
    }
    impl Parser for crate::wayland_objects::wp_fractional_scale_v1::WpFractionalScaleV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Notify of new preferred scale
        ///
        /// Notification of a new preferred scale for this surface that the
        /// compositor suggests that the client should use.
        ///
        /// The sent scale is the numerator of a fraction with a denominator of 120.
        PreferredScale {
            /// The new preferred scale,
            scale: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::wp_fractional_scale_v1::WpFractionalScaleV1 {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                PreferredScale { scale } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(scale);
                    raw
                }
            }
        }
    }
}

/// Global factory for single-pixel buffers
///
/// The wp_single_pixel_buffer_manager_v1 interface is a factory for
/// single-pixel buffers.
pub mod wp_single_pixel_buffer_manager_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the manager
        ///
        /// Destroy the wp_single_pixel_buffer_manager_v1 object.
        ///
        /// The child objects created via this interface are unaffected.
        Destroy,
        /// Create a 1×1 buffer from 32-bit RGBA values
        ///
        /// Create a single-pixel buffer from four 32-bit RGBA values.
        ///
        /// Unless specified in another protocol extension, the RGBA values use
        /// pre-multiplied alpha.
        ///
        /// The width and height of the buffer are 1.
        CreateU32RgbaBuffer {
            id: WlBufferId,
            /// Value of the buffer's red channel,
            r: u32,
            /// Value of the buffer's green channel,
            g: u32,
            /// Value of the buffer's blue channel,
            b: u32,
            /// Value of the buffer's alpha channel,
            a: u32,
        },
    }
    impl Parser
        for crate::wayland_objects::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1
    {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => CreateU32RgbaBuffer {
                    id: req.id()?,
                    r: req.uint()?,
                    g: req.uint()?,
                    b: req.uint()?,
                    a: req.uint()?,
                },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// Protocol for tearing control
///
/// For some use cases like games or drawing tablets it can make sense to
/// reduce latency by accepting tearing with the use of asynchronous page
/// flips. This global is a factory interface, allowing clients to inform
/// which type of presentation the content of their surfaces is suitable for.
///
/// Graphics APIs like EGL or Vulkan, that manage the buffer queue and commits
/// of a wl_surface themselves, are likely to be using this extension
/// internally. If a client is using such an API for a wl_surface, it should
/// not directly use this extension on that surface, to avoid raising a
/// tearing_control_exists protocol error.
///
/// Warning! The protocol described in this file is currently in the testing
/// phase. Backward compatible changes may be added together with the
/// corresponding interface version bump. Backward incompatible changes can
/// only be done by creating a new major version of the extension.
pub mod wp_tearing_control_manager_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy tearing control factory object
        ///
        /// Destroy this tearing control factory object. Other objects, including
        /// wp_tearing_control_v1 objects created by this factory, are not affected
        /// by this request.
        Destroy,
        /// Extend surface interface for tearing control
        ///
        /// Instantiate an interface extension for the given wl_surface to request
        /// asynchronous page flips for presentation.
        ///
        /// If the given wl_surface already has a wp_tearing_control_v1 object
        /// associated, the tearing_control_exists protocol error is raised.
        GetTearingControl { id: WpTearingControlV1Id, surface: WlSurfaceId },
    }
    impl Parser for crate::wayland_objects::wp_tearing_control_manager_v1::WpTearingControlManagerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetTearingControl { id: req.id()?, surface: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// The surface already has a tearing object associated,
        TearingControlExists = 0,
    }
}

/// Per-surface tearing control interface
///
/// An additional interface to a wl_surface object, which allows the client
/// to hint to the compositor if the content on the surface is suitable for
/// presentation with tearing.
/// The default presentation hint is vsync. See presentation_hint for more
/// details.
pub mod wp_tearing_control_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Set presentation hint
        ///
        /// Set the presentation hint for the associated wl_surface. This state is
        /// double-buffered and is applied on the next wl_surface.commit.
        ///
        /// The compositor is free to dynamically respect or ignore this hint based
        /// on various conditions like hardware capabilities, surface state and
        /// user preferences.
        SetPresentationHint { hint: u32 },
        /// Destroy tearing control object
        ///
        /// Destroy this surface tearing object and revert the presentation hint to
        /// vsync. The change will be applied on the next wl_surface.commit.
        Destroy,
    }
    impl Parser for crate::wayland_objects::wp_tearing_control_v1::WpTearingControlV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetPresentationHint { hint: req.uint()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    /// Presentation hint values
    ///
    /// This enum provides information for if submitted frames from the client
    /// may be presented with tearing.
    #[derive(Debug)]
    pub enum PresentationHint {
        /// Tearing-free presentation
        ///
        /// The content of this surface is meant to be synchronized to the
        /// vertical blanking period. This should not result in visible tearing
        /// and may result in a delay before a surface commit is presented.
        Vsync = 0,
        /// Asynchronous presentation
        ///
        /// The content of this surface is meant to be presented with minimal
        /// latency and tearing is acceptable.
        Async = 1,
    }
}

/// Interface for activating surfaces
///
/// A global interface used for informing the compositor about applications
/// being activated or started, or for applications to request to be
/// activated.
pub mod xdg_activation_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the xdg_activation object
        ///
        /// Notify the compositor that the xdg_activation object will no longer be
        /// used.
        ///
        /// The child objects created via this interface are unaffected and should
        /// be destroyed separately.
        Destroy,
        /// Requests a token
        ///
        /// Creates an xdg_activation_token_v1 object that will provide
        /// the initiating client with a unique token for this activation. This
        /// token should be offered to the clients to be activated.
        GetActivationToken { id: XdgActivationTokenV1Id },
        /// Notify new interaction being available
        ///
        /// Requests surface activation. It's up to the compositor to display
        /// this information as desired, for example by placing the surface above
        /// the rest.
        ///
        /// The compositor may know who requested this by checking the activation
        /// token and might decide not to follow through with the activation if it's
        /// considered unwanted.
        ///
        /// Compositors can ignore unknown activation tokens when an invalid
        /// token is passed.
        Activate {
            /// The activation token of the initiating client,
            token: String,
            /// The wl_surface to activate,
            surface: WlSurfaceId,
        },
    }
    impl Parser for crate::wayland_objects::xdg_activation_v1::XdgActivationV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetActivationToken { id: req.id()? },
                2 => Activate { token: req.string()?, surface: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// An exported activation handle
///
/// An object for setting up a token and receiving a token handle that can
/// be passed as an activation token to another client.
///
/// The object is created using the xdg_activation_v1.get_activation_token
/// request. This object should then be populated with the app_id, surface
/// and serial information and committed. The compositor shall then issue a
/// done event with the token. In case the request's parameters are invalid,
/// the compositor will provide an invalid token.
pub mod xdg_activation_token_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Specifies the seat and serial of the activating event
        ///
        /// Provides information about the seat and serial event that requested the
        /// token.
        ///
        /// The serial can come from an input or focus event. For instance, if a
        /// click triggers the launch of a third-party client, the launcher client
        /// should send a set_serial request with the serial and seat from the
        /// wl_pointer.button event.
        ///
        /// Some compositors might refuse to activate toplevels when the token
        /// doesn't have a valid and recent enough event serial.
        ///
        /// Must be sent before commit. This information is optional.
        SetSerial {
            /// The serial of the event that triggered the activation,
            serial: u32,
            /// The wl_seat of the event,
            seat: WlSeatId,
        },
        /// Specifies the application being activated
        ///
        /// The requesting client can specify an app_id to associate the token
        /// being created with it.
        ///
        /// Must be sent before commit. This information is optional.
        SetAppId {
            /// The application id of the client being activated.,
            app_id: String,
        },
        /// Specifies the surface requesting activation
        ///
        /// This request sets the surface requesting the activation. Note, this is
        /// different from the surface that will be activated.
        ///
        /// Some compositors might refuse to activate toplevels when the token
        /// doesn't have a requesting surface.
        ///
        /// Must be sent before commit. This information is optional.
        SetSurface {
            /// The requesting surface,
            surface: WlSurfaceId,
        },
        /// Issues the token request
        ///
        /// Requests an activation token based on the different parameters that
        /// have been offered through set_serial, set_surface and set_app_id.
        Commit,
        /// Destroy the xdg_activation_token_v1 object
        ///
        /// Notify the compositor that the xdg_activation_token_v1 object will no
        /// longer be used. The received token stays valid.
        Destroy,
    }
    impl Parser for crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetSerial { serial: req.uint()?, seat: req.id()? },
                1 => SetAppId { app_id: req.string()? },
                2 => SetSurface { surface: req.id()? },
                3 => Commit,
                4 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// The exported activation token
        ///
        /// The 'done' event contains the unique token of this activation request
        /// and notifies that the provider is done.
        Done {
            /// The exported activation token,
            token: String,
        },
    }
    impl EventWriter for crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1 {
        type Event = Event;
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Done { token } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.string(token);
                    raw
                }
            }
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// The token has already been used previously,
        AlreadyUsed = 0,
    }
}

/// Context object for Xwayland shell
///
/// xwayland_shell_v1 is a singleton global object that
/// provides the ability to create a xwayland_surface_v1 object
/// for a given wl_surface.
///
/// This interface is intended to be bound by the Xwayland server.
///
/// A compositor must not allow clients other than Xwayland to
/// bind to this interface. A compositor should hide this global
/// from other clients' wl_registry.
/// A client the compositor does not consider to be an Xwayland
/// server attempting to bind this interface will result in
/// an implementation-defined error.
///
/// An Xwayland server that has bound this interface must not
/// set the `WL_SURFACE_ID` atom on a window.
pub mod xwayland_shell_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the Xwayland shell object
        ///
        /// Destroy the xwayland_shell_v1 object.
        ///
        /// The child objects created via this interface are unaffected.
        Destroy,
        /// Assign the xwayland_surface surface role
        ///
        /// Create an xwayland_surface_v1 interface for a given wl_surface
        /// object and gives it the xwayland_surface role.
        ///
        /// It is illegal to create an xwayland_surface_v1 for a wl_surface
        /// which already has an assigned role and this will result in the
        /// `role` protocol error.
        ///
        /// See the documentation of xwayland_surface_v1 for more details
        /// about what an xwayland_surface_v1 is and how it is used.
        GetXwaylandSurface { id: XwaylandSurfaceV1Id, surface: WlSurfaceId },
    }
    impl Parser for crate::wayland_objects::xwayland_shell_v1::XwaylandShellV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetXwaylandSurface { id: req.id()?, surface: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Given wl_surface has another role,
        Role = 0,
    }
}

/// Interface for associating Xwayland windows to wl_surfaces
///
/// An Xwayland surface is a surface managed by an Xwayland server.
/// It is used for associating surfaces to Xwayland windows.
///
/// The Xwayland server associated with actions in this interface is
/// determined by the Wayland client making the request.
///
/// The client must call wl_surface.commit on the corresponding wl_surface
/// for the xwayland_surface_v1 state to take effect.
pub mod xwayland_surface_v1 {
    use super::*;
    #[derive(Debug)]
    pub enum Request {
        /// Associates a Xwayland window to a wl_surface
        ///
        /// Associates an Xwayland window to a wl_surface.
        /// The association state is double-buffered and will be applied at
        /// the time wl_surface.commit of the corresponding wl_surface is called.
        ///
        /// The `serial_lo` and `serial_hi` parameters specify a non-zero
        /// monotonic serial number which is entirely unique and provided by the
        /// Xwayland server equal to the serial value provided by a client message
        /// with a message type of the `WL_SURFACE_SERIAL` atom on the X11 window
        /// for this surface to be associated to.
        ///
        /// The serial value in the `WL_SURFACE_SERIAL` client message is specified
        /// as having the lo-bits specified in `l[0]` and the hi-bits specified
        /// in `l[1]`.
        ///
        /// If the serial value provided by `serial_lo` and `serial_hi` is not
        /// valid, the `invalid_serial` protocol error will be raised.
        ///
        /// An X11 window may be associated with multiple surfaces throughout its
        /// lifespan. (eg. unmapping and remapping a window).
        ///
        /// For each wl_surface, this state must not be committed more than once,
        /// otherwise the `already_associated` protocol error will be raised.
        SetSerial {
            /// The lower 32-bits of the serial number associated with the X11 window,
            serial_lo: u32,
            /// The upper 32-bits of the serial number associated with the X11 window,
            serial_hi: u32,
        },
        /// Destroy the Xwayland surface object
        ///
        /// Destroy the xwayland_surface_v1 object.
        ///
        /// Any already existing associations are unaffected by this action.
        Destroy,
    }
    impl Parser for crate::wayland_objects::xwayland_surface_v1::XwaylandSurfaceV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetSerial { serial_lo: req.uint()?, serial_hi: req.uint()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Error {
        /// Given wl_surface is already associated with an X11 window,
        AlreadyAssociated = 0,
        /// Serial was not valid,
        InvalidSerial = 1,
    }
}
