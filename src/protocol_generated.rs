#![allow(dead_code, unused_imports, unused_mut, clippy::let_and_return)]
#![allow(clippy::enum_variant_names)]

use {
    crate::{
        clients::{EventWriter, RawEvent, RawRequest},
        object::{Parser, ParserError},
    },
    std::os::unix::io::RawFd,
};

#[derive(
    Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, derive_more::From, derive_more::Into,
)]
pub struct WaylandId(u32);
#[derive(
    Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, derive_more::From, derive_more::Into,
)]
pub struct NewId(u32);
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    derive_more::From,
    derive_more::Into,
    derive_more::Deref,
)]
pub struct Array(pub Vec<u32>);

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
  WlFixesId,
  ZwpLinuxDmabufV1Id,
  ZwpLinuxBufferParamsV1Id,
  ZwpLinuxDmabufFeedbackV1Id,
  ZwpTabletManagerV2Id,
  ZwpTabletSeatV2Id,
  ZwpTabletToolV2Id,
  ZwpTabletV2Id,
  ZwpTabletPadRingV2Id,
  ZwpTabletPadStripV2Id,
  ZwpTabletPadGroupV2Id,
  ZwpTabletPadV2Id,
  ZwpTabletPadDialV2Id,
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
  ExtDataControlManagerV1Id,
  ExtDataControlDeviceV1Id,
  ExtDataControlSourceV1Id,
  ExtDataControlOfferV1Id,
  ExtForeignToplevelListV1Id,
  ExtForeignToplevelHandleV1Id,
  ExtIdleNotifierV1Id,
  ExtIdleNotificationV1Id,
  ExtImageCaptureSourceV1Id,
  ExtOutputImageCaptureSourceManagerV1Id,
  ExtForeignToplevelImageCaptureSourceManagerV1Id,
  ExtImageCopyCaptureManagerV1Id,
  ExtImageCopyCaptureSessionV1Id,
  ExtImageCopyCaptureFrameV1Id,
  ExtImageCopyCaptureCursorSessionV1Id,
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
  WpCursorShapeManagerV1Id,
  WpCursorShapeDeviceV1Id,
  WpLinuxDrmSyncobjManagerV1Id,
  WpLinuxDrmSyncobjTimelineV1Id,
  WpLinuxDrmSyncobjSurfaceV1Id,
  ZwpIdleInhibitManagerV1Id,
  ZwpIdleInhibitorV1Id,
  ZwpLinuxExplicitSynchronizationV1Id,
  ZwpLinuxSurfaceSynchronizationV1Id,
  ZwpLinuxBufferReleaseV1Id,
  ZwpPointerConstraintsV1Id,
  ZwpLockedPointerV1Id,
  ZwpConfinedPointerV1Id,
  ZwpPrimarySelectionDeviceManagerV1Id,
  ZwpPrimarySelectionDeviceV1Id,
  ZwpPrimarySelectionOfferV1Id,
  ZwpPrimarySelectionSourceV1Id,
  ZwpRelativePointerManagerV1Id,
  ZwpRelativePointerV1Id,
  ZxdgDecorationManagerV1Id,
  ZxdgToplevelDecorationV1Id,
  ZxdgOutputManagerV1Id,
  ZxdgOutputV1Id,
  ExtWorkspaceManagerV1Id,
  ExtWorkspaceGroupHandleV1Id,
  ExtWorkspaceHandleV1Id,
  ZwlrDataControlManagerV1Id,
  ZwlrDataControlDeviceV1Id,
  ZwlrDataControlSourceV1Id,
  ZwlrDataControlOfferV1Id,
  ZwlrForeignToplevelManagerV1Id,
  ZwlrForeignToplevelHandleV1Id,
  ZwlrScreencopyManagerV1Id,
  ZwlrScreencopyFrameV1Id,
  ZwlrGammaControlManagerV1Id,
  ZwlrGammaControlV1Id,
  ZwlrLayerShellV1Id,
  ZwlrLayerSurfaceV1Id,
  HyprlandCtmControlManagerV1Id,
}

pub trait Constants {
    const INTERFACE_NAME: &'static str;
    const INTERFACE_VERSION: u32;
}

/// Core global object
///
/// The core global object.  This is a special singleton object.  It
/// is used for internal Wayland protocol features.
pub mod wl_display {
    use super::*;
    impl Constants for crate::wayland_objects::wl_display::WlDisplay {
        const INTERFACE_NAME: &'static str = "wl_display";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WlDisplayId {
        type Object = crate::wayland_objects::wl_display::WlDisplay;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_display::WlDisplay {
        type Id = WlDisplayId;
    }
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
        /// The callback_data passed in the callback is undefined and should be ignored.
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
        /// This event is used internally by the object ID management logic.
        ///
        /// When the server stops using an object created by the client, the server
        /// sends this event. In particular, after sending this event, the server
        /// will no longer send any events that contain the object as the receiver
        /// or as an argument.
        ///
        /// When the client receives this event, it knows that it can reuse the
        /// object ID.
        DeleteId {
            /// Deleted object ID,
            id: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_display::WlDisplay {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::wl_registry::WlRegistry {
        const INTERFACE_NAME: &'static str = "wl_registry";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WlRegistryId {
        type Object = crate::wayland_objects::wl_registry::WlRegistry;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_registry::WlRegistry {
        type Id = WlRegistryId;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Bind an object to the display
        ///
        /// Binds a new, client-created object to the server using the
        /// specified name as the identifier.
        Bind {
            /// Unique numeric name of the object,
            name: u32,
            /// Bound object,
            id: NewId,
            /// Client-requested interface version (from `new_id` marshalling),
            requested_version: u32,
        },
    }
    impl Parser for crate::wayland_objects::wl_registry::WlRegistry {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => {
                    let name = req.uint()?;
                    let (id, requested_version) = req.new_id_with_interface()?;
                    Bind { name, id, requested_version }
                }
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    impl Constants for crate::wayland_objects::wl_callback::WlCallback {
        const INTERFACE_NAME: &'static str = "wl_callback";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WlCallbackId {
        type Object = crate::wayland_objects::wl_callback::WlCallback;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_callback::WlCallback {
        type Id = WlCallbackId;
    }
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    impl Constants for crate::wayland_objects::wl_compositor::WlCompositor {
        const INTERFACE_NAME: &'static str = "wl_compositor";
        const INTERFACE_VERSION: u32 = 7;
    }
    impl crate::object::IdToObject for WlCompositorId {
        type Object = crate::wayland_objects::wl_compositor::WlCompositor;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_compositor::WlCompositor {
        type Id = WlCompositorId;
    }
    /// Minimum interface version for the `release` message (XML `since` / `version`).
    pub const RELEASE_SINCE: u32 = 7;
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
        /// Destroy wl_compositor
        ///
        /// This request destroys the wl_compositor. This has no effect on any other objects.
        Release,
    }
    impl Parser for crate::wayland_objects::wl_compositor::WlCompositor {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => CreateSurface { id: req.id()? },
                1 => CreateRegion { id: req.id()? },
                2 => Release,
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
    impl Constants for crate::wayland_objects::wl_shm_pool::WlShmPool {
        const INTERFACE_NAME: &'static str = "wl_shm_pool";
        const INTERFACE_VERSION: u32 = 3;
    }
    impl crate::object::IdToObject for WlShmPoolId {
        type Object = crate::wayland_objects::wl_shm_pool::WlShmPool;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_shm_pool::WlShmPool {
        type Id = WlShmPoolId;
    }
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
    /// Wl_shm_pool error values
    ///
    /// These errors can be emitted in response to wl_shm_pool requests.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Buffer format is not known,
        InvalidFormat = 0,
        /// Invalid size or stride during buffer creation,
        InvalidStride = 1,
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
    impl Constants for crate::wayland_objects::wl_shm::WlShm {
        const INTERFACE_NAME: &'static str = "wl_shm";
        const INTERFACE_VERSION: u32 = 3;
    }
    impl crate::object::IdToObject for WlShmId {
        type Object = crate::wayland_objects::wl_shm::WlShm;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_shm::WlShm {
        type Id = WlShmId;
    }
    /// Minimum interface version for the `release` message (XML `since` / `version`).
    pub const RELEASE_SINCE: u32 = 2;
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
        /// Release the shm object
        ///
        /// Using this request a client can tell the server that it is not going to
        /// use the shm object anymore.
        ///
        /// Objects created via this interface remain unaffected.
        Release,
    }
    impl Parser for crate::wayland_objects::wl_shm::WlShm {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => CreatePool { id: req.id()?, fd: req.fd()?, size: req.int()? },
                1 => Release,
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
        ///
        /// Extensions to drm_fourcc.h (or the format enum) do not require
        /// increasing the wl_shm version; as a result, clients may receive format
        /// codes which were not in the list at the time the client was made.
        Format {
            /// Buffer pixel format,
            format: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_shm::WlShm {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Buffer format is not known,
        InvalidFormat = 0,
        /// Invalid size or stride during pool creation,
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
    /// will be reported by the format event. See drm_fourcc.h for more detailed
    /// format descriptions.
    ///
    /// For all wl_shm formats and unless specified in another protocol
    /// extension, pre-multiplied alpha is used for pixel values.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
        /// 16-bit RGBA format, [15:0] R:G:B:A 4:4:4:4 little endian,
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
        /// [7:0] C0:C1:C2:C3:C4:C5:C6:C7 1:1:1:1:1:1:1:1 eight pixels/byte,
        C1 = 538980675,
        /// [7:0] C0:C1:C2:C3 2:2:2:2 four pixels/byte,
        C2 = 538980931,
        /// [7:0] C0:C1 4:4 two pixels/byte,
        C4 = 538981443,
        /// [7:0] D0:D1:D2:D3:D4:D5:D6:D7 1:1:1:1:1:1:1:1 eight pixels/byte,
        D1 = 538980676,
        /// [7:0] D0:D1:D2:D3 2:2:2:2 four pixels/byte,
        D2 = 538980932,
        /// [7:0] D0:D1 4:4 two pixels/byte,
        D4 = 538981444,
        /// [7:0] D,
        D8 = 538982468,
        /// [7:0] R0:R1:R2:R3:R4:R5:R6:R7 1:1:1:1:1:1:1:1 eight pixels/byte,
        R1 = 538980690,
        /// [7:0] R0:R1:R2:R3 2:2:2:2 four pixels/byte,
        R2 = 538980946,
        /// [7:0] R0:R1 4:4 two pixels/byte,
        R4 = 538981458,
        /// [15:0] x:R 6:10 little endian,
        R10 = 540029266,
        /// [15:0] x:R 4:12 little endian,
        R12 = 540160338,
        /// [31:0] A:Cr:Cb:Y 8:8:8:8 little endian,
        Avuy8888 = 1498764865,
        /// [31:0] X:Cr:Cb:Y 8:8:8:8 little endian,
        Xvuy8888 = 1498764888,
        /// 2x2 subsampled Cr:Cb plane 10 bits per channel packed,
        P030 = 808661072,
        /// [47:0] R:G:B 16:16:16 little endian,
        Rgb161616 = 942950226,
        /// [47:0] B:G:R 16:16:16 little endian,
        Bgr161616 = 942950210,
        /// [15:0] R 16 little endian,
        R16f = 1210064978,
        /// [31:0] G:R 16:16 little endian,
        Gr1616f = 1210077767,
        /// [47:0] B:G:R 16:16:16 little endian,
        Bgr161616f = 1213351746,
        /// [31:0] R 32 little endian,
        R32f = 1176510546,
        /// [63:0] G:R 32:32 little endian,
        Gr3232f = 1176523335,
        /// [95:0] B:G:R 32:32:32 little endian,
        Bgr323232f = 1179797314,
        /// [127:0] A:B:G:R 32:32:32:32 little endian,
        Abgr32323232f = 1178092097,
        /// 2x1 subsampled Cr:Cb plane,
        Nv20 = 808605262,
        /// Non-subsampled Cr:Cb plane,
        Nv30 = 808670798,
        /// 2x2 subsampled Cb (1) and Cr (2) planes 10 bits per channel,
        S010 = 808530003,
        /// 2x1 subsampled Cb (1) and Cr (2) planes 10 bits per channel,
        S210 = 808530515,
        /// Non-subsampled Cb (1) and Cr (2) planes 10 bits per channel,
        S410 = 808531027,
        /// 2x2 subsampled Cb (1) and Cr (2) planes 12 bits per channel,
        S012 = 842084435,
        /// 2x1 subsampled Cb (1) and Cr (2) planes 12 bits per channel,
        S212 = 842084947,
        /// Non-subsampled Cb (1) and Cr (2) planes 12 bits per channel,
        S412 = 842085459,
        /// 2x2 subsampled Cb (1) and Cr (2) planes 16 bits per channel,
        S016 = 909193299,
        /// 2x1 subsampled Cb (1) and Cr (2) planes 16 bits per channel,
        S216 = 909193811,
        /// Non-subsampled Cb (1) and Cr (2) planes 16 bits per channel,
        S416 = 909194323,
        /// [31:0] x:Cr:Cb:Y 2:10:10:10 little endian,
        Xvuy2101010 = 808671576,
        /// 2x1 subsampled Cr:Cb plane 10 bits per channel packed,
        P230 = 808661584,
        T430 = 808662100,
        /// 8-bit Y-only,
        Y8 = 1497715271,
        /// [31:0] x:Y2:Y1:Y0 2:10:10:10 little endian,
        Xyyy2101010 = 876695641,
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
/// Color channels are assumed to be electrical rather than optical (in other
/// words, encoded with a transfer function) unless otherwise specified. If
/// the buffer uses a format that has an alpha channel, the alpha channel is
/// assumed to be premultiplied into the electrical color channel values
/// (after transfer function encoding) unless otherwise specified.
///
/// Note, because wl_buffer objects are created from multiple independent
/// factory interfaces, the wl_buffer interface is frozen at version 1.
pub mod wl_buffer {
    use super::*;
    impl Constants for crate::wayland_objects::wl_buffer::WlBuffer {
        const INTERFACE_NAME: &'static str = "wl_buffer";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WlBufferId {
        type Object = crate::wayland_objects::wl_buffer::WlBuffer;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_buffer::WlBuffer {
        type Id = WlBufferId;
    }
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
        ///
        /// For more information on when release events may or may not be sent,
        /// and what consequences it has, please see the description of
        /// wl_surface.attach.
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    impl Constants for crate::wayland_objects::wl_data_offer::WlDataOffer {
        const INTERFACE_NAME: &'static str = "wl_data_offer";
        const INTERFACE_VERSION: u32 = 4;
    }
    impl crate::object::IdToObject for WlDataOfferId {
        type Object = crate::wayland_objects::wl_data_offer::WlDataOffer;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_data_offer::WlDataOffer {
        type Id = WlDataOfferId;
    }
    /// Minimum interface version for the `finish` message (XML `since` / `version`).
    pub const FINISH_SINCE: u32 = 3;
    /// Minimum interface version for the `set_actions` message (XML `since` / `version`).
    pub const SET_ACTIONS_SINCE: u32 = 3;
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
    /// Minimum interface version for the `source_actions` message (XML `since` / `version`).
    pub const SOURCE_ACTIONS_SINCE: u32 = 3;
    /// Minimum interface version for the `action` message (XML `since` / `version`).
    pub const ACTION_SINCE: u32 = 3;
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::wl_data_source::WlDataSource {
        const INTERFACE_NAME: &'static str = "wl_data_source";
        const INTERFACE_VERSION: u32 = 4;
    }
    impl crate::object::IdToObject for WlDataSourceId {
        type Object = crate::wayland_objects::wl_data_source::WlDataSource;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_data_source::WlDataSource {
        type Id = WlDataSourceId;
    }
    /// Minimum interface version for the `set_actions` message (XML `since` / `version`).
    pub const SET_ACTIONS_SINCE: u32 = 3;
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
    /// Minimum interface version for the `dnd_drop_performed` message (XML `since` / `version`).
    pub const DND_DROP_PERFORMED_SINCE: u32 = 3;
    /// Minimum interface version for the `dnd_finished` message (XML `since` / `version`).
    pub const DND_FINISHED_SINCE: u32 = 3;
    /// Minimum interface version for the `action` message (XML `since` / `version`).
    pub const ACTION_SINCE: u32 = 3;
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
        /// However, this event might not be received if the compositor cancelled
        /// the drag-and-drop operation before this event could happen.
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
        /// always be applied in wl_data_source.dnd_finished.
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::wl_data_device::WlDataDevice {
        const INTERFACE_NAME: &'static str = "wl_data_device";
        const INTERFACE_VERSION: u32 = 4;
    }
    impl crate::object::IdToObject for WlDataDeviceId {
        type Object = crate::wayland_objects::wl_data_device::WlDataDevice;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_data_device::WlDataDevice {
        type Id = WlDataDeviceId;
    }
    /// Minimum interface version for the `release` message (XML `since` / `version`).
    pub const RELEASE_SINCE: u32 = 2;
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
        /// hotspot, but subsequent wl_surface.offset requests can move the
        /// relative position. Attach requests must be confirmed with
        /// wl_surface.commit as usual. The icon surface is given the role of
        /// a drag-and-drop icon. If the icon surface already has another role,
        /// it raises a protocol error.
        ///
        /// The input region is ignored for wl_surfaces with the role of a
        /// drag-and-drop icon.
        ///
        /// The given source may not be used in any further set_selection or
        /// start_drag requests. Attempting to reuse a previously-used source
        /// may send a used_source error.
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
        ///
        /// The given source may not be used in any further set_selection or
        /// start_drag requests. Attempting to reuse a previously-used source
        /// may send a used_source error.
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Given wl_surface has another role,
        Role = 0,
        /// Source has already been used,
        UsedSource = 1,
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
    impl Constants for crate::wayland_objects::wl_data_device_manager::WlDataDeviceManager {
        const INTERFACE_NAME: &'static str = "wl_data_device_manager";
        const INTERFACE_VERSION: u32 = 4;
    }
    impl crate::object::IdToObject for WlDataDeviceManagerId {
        type Object = crate::wayland_objects::wl_data_device_manager::WlDataDeviceManager;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wl_data_device_manager::WlDataDeviceManager
    {
        type Id = WlDataDeviceManagerId;
    }
    /// Minimum interface version for the `release` message (XML `since` / `version`).
    pub const RELEASE_SINCE: u32 = 4;
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
        /// Destroy wl_data_device_manager
        ///
        /// This request destroys the wl_data_device_manager. This has no effect on any other
        /// objects.
        Release,
    }
    impl Parser for crate::wayland_objects::wl_data_device_manager::WlDataDeviceManager {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => CreateDataSource { id: req.id()? },
                1 => GetDataDevice { id: req.id()?, seat: req.id()? },
                2 => Release,
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::wl_shell::WlShell {
        const INTERFACE_NAME: &'static str = "wl_shell";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WlShellId {
        type Object = crate::wayland_objects::wl_shell::WlShell;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_shell::WlShell {
        type Id = WlShellId;
    }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::wl_shell_surface::WlShellSurface {
        const INTERFACE_NAME: &'static str = "wl_shell_surface";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WlShellSurfaceId {
        type Object = crate::wayland_objects::wl_shell_surface::WlShellSurface;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_shell_surface::WlShellSurface {
        type Id = WlShellSurfaceId;
    }
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Transient {
        /// Do not set keyboard focus,
        Inactive = 1,
    }
    /// Different method to set the surface fullscreen
    ///
    /// Hints to indicate to the compositor how to deal with a conflict
    /// between the dimensions of the surface and the dimensions of the
    /// output. The compositor is free to ignore this parameter.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::wl_surface::WlSurface {
        const INTERFACE_NAME: &'static str = "wl_surface";
        const INTERFACE_VERSION: u32 = 7;
    }
    impl crate::object::IdToObject for WlSurfaceId {
        type Object = crate::wayland_objects::wl_surface::WlSurface;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_surface::WlSurface {
        type Id = WlSurfaceId;
    }
    /// Minimum interface version for the `set_buffer_transform` message (XML `since` / `version`).
    pub const SET_BUFFER_TRANSFORM_SINCE: u32 = 2;
    /// Minimum interface version for the `set_buffer_scale` message (XML `since` / `version`).
    pub const SET_BUFFER_SCALE_SINCE: u32 = 3;
    /// Minimum interface version for the `damage_buffer` message (XML `since` / `version`).
    pub const DAMAGE_BUFFER_SINCE: u32 = 4;
    /// Minimum interface version for the `offset` message (XML `since` / `version`).
    pub const OFFSET_SINCE: u32 = 5;
    /// Minimum interface version for the `get_release` message (XML `since` / `version`).
    pub const GET_RELEASE_SINCE: u32 = 7;
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
        /// case. Instead, clients hitting this case should use
        /// wl_surface.get_release or use a protocol extension providing per-commit
        /// release notifications (if none of these options are available, a
        /// fallback can be implemented by creating multiple wl_buffer objects from
        /// the same backing storage).
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
        ///
        /// If a pending wl_buffer has been destroyed, the result is not specified.
        /// Many compositors are known to remove the surface content on the following
        /// wl_surface.commit, but this behaviour is not universal. Clients seeking to
        /// maximise compatibility should not destroy pending buffers and should
        /// ensure that they explicitly remove content from surfaces, even after
        /// destroying buffers.
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
        /// as opposed to the active state in use by the compositor.
        ///
        /// All requests that need a commit to become effective are documented
        /// to affect double-buffered state.
        ///
        /// Other interfaces may add further double-buffered surface state.
        ///
        /// A commit request atomically creates a Content Update (CU) from the
        /// pending state, even if the pending state has not been touched. The
        /// content update is placed at the end of a per-surface queue until it
        /// becomes active. After commit, the new pending state is as documented for
        /// each related request.
        ///
        /// A CU is either a Desync Content Update (DCU) or a Sync Content Update
        /// (SCU). If the surface is effectively synchronized at the commit request,
        /// it is a SCU, otherwise a DCU.
        ///
        /// When a surface transitions from effectively synchronized to effectively
        /// desynchronized, all SCUs in its queue which are not reachable by any
        /// DCU become DCUs and dependency edges from outside the queue to these CUs
        /// are removed.
        ///
        /// See wl_subsurface for the definition of 'effectively synchronized' and
        /// 'effectively desynchronized'.
        ///
        /// When a CU is placed in the queue, the CU has a dependency on the CU in
        /// front of it and to the SCU at end of the queue of every direct child
        /// surface if that SCU exists and does not have another dependent. This can
        /// form a directed acyclic graph of CUs with dependencies as edges.
        ///
        /// In addition to surface state, the CU can have constraints that must be
        /// satisfied before it can be applied. Other interfaces may add CU
        /// constraints.
        ///
        /// All DCUs which do not have a SCU in front of themselves in their queue,
        /// are candidates. If the graph that's reachable by a candidate does not
        /// have any unsatisfied constraints, the entire graph must be applied
        /// atomically.
        ///
        /// When a CU is applied, the wl_buffer is applied before all other state.
        /// This means that all coordinates in double-buffered state are relative to
        /// the newly attached wl_buffers, except for wl_surface.attach itself. If
        /// there is no newly attached wl_buffer, the coordinates are relative to
        /// the previous content update.
        Commit,
        /// Sets the buffer transformation
        ///
        /// This request sets the transformation that the client has already applied
        /// to the content of the buffer. The accepted values for the transform
        /// parameter are the values for wl_output.transform.
        ///
        /// The compositor applies the inverse of this transformation whenever it
        /// uses the buffer contents.
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
        /// If scale is not greater than 0 the invalid_scale protocol error is
        /// raised.
        SetBufferScale {
            /// Scale for interpreting buffer contents,
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
        /// The exact semantics of wl_surface.offset are role-specific. Refer to
        /// the documentation of specific roles for more information.
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
        /// Get a release callback
        ///
        /// Create a callback for the release of the buffer attached by the client
        /// with wl_surface.attach.
        ///
        /// The compositor will release the buffer when it has finished its usage of
        /// the underlying storage for the relevant commit. Once the client receives
        /// this event, and assuming the associated buffer is not pending release
        /// from other wl_surface.commit requests, the client can safely re-use the
        /// buffer.
        ///
        /// Release callbacks are double-buffered state, and will be associated
        /// with the pending buffer at wl_surface.commit time.
        ///
        /// The callback_data passed in the wl_callback.done event is unused and
        /// is always zero.
        ///
        /// Sending this request without attaching a non-null buffer in the same
        /// content update is a protocol error. The compositor will send the
        /// no_buffer error in this case.
        GetRelease {
            /// Callback object for the release,
            callback: WlCallbackId,
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
                11 => GetRelease { callback: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    /// Minimum interface version for the `preferred_buffer_scale` message (XML `since` / `version`).
    pub const PREFERRED_BUFFER_SCALE_SINCE: u32 = 6;
    /// Minimum interface version for the `preferred_buffer_transform` message (XML `since` / `version`).
    pub const PREFERRED_BUFFER_TRANSFORM_SINCE: u32 = 6;
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
        /// Before receiving this event the preferred buffer scale for this surface
        /// is 1.
        ///
        /// It is intended that scaling aware clients use this event to scale their
        /// content and use wl_surface.set_buffer_scale to indicate the scale they
        /// have rendered with. This allows clients to supply a higher detail
        /// buffer.
        ///
        /// The compositor shall emit a scale value greater than 0.
        PreferredBufferScale {
            /// Preferred scaling factor,
            factor: i32,
        },
        /// Preferred buffer transform for the surface
        ///
        /// This event indicates the preferred buffer transform for this surface.
        /// It is sent whenever the compositor's preference changes.
        ///
        /// Before receiving this event the preferred buffer transform for this
        /// surface is normal.
        ///
        /// Applying this transformation to the surface buffer contents and using
        /// wl_surface.set_buffer_transform might allow the compositor to use the
        /// surface buffer more efficiently.
        PreferredBufferTransform {
            /// Preferred transform,
            transform: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_surface::WlSurface {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
        /// No buffer was attached,
        NoBuffer = 5,
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
    impl Constants for crate::wayland_objects::wl_seat::WlSeat {
        const INTERFACE_NAME: &'static str = "wl_seat";
        const INTERFACE_VERSION: u32 = 11;
    }
    impl crate::object::IdToObject for WlSeatId {
        type Object = crate::wayland_objects::wl_seat::WlSeat;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_seat::WlSeat {
        type Id = WlSeatId;
    }
    /// Minimum interface version for the `release` message (XML `since` / `version`).
    pub const RELEASE_SINCE: u32 = 5;
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
    /// Minimum interface version for the `name` message (XML `since` / `version`).
    pub const NAME_SINCE: u32 = 2;
    #[derive(Debug)]
    pub enum Event {
        /// Seat capabilities changed
        ///
        /// This is sent on binding to the seat global or whenever a seat gains
        /// or loses the pointer, keyboard or touch capabilities.
        /// The argument is a capability enum containing the complete set of
        /// capabilities this seat has.
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
        /// The name event is sent after binding to the seat global, and should be sent
        /// before announcing capabilities. This event is only sent once per seat object,
        /// and the name does not change over the lifetime of the wl_seat global.
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::wl_pointer::WlPointer {
        const INTERFACE_NAME: &'static str = "wl_pointer";
        const INTERFACE_VERSION: u32 = 11;
    }
    impl crate::object::IdToObject for WlPointerId {
        type Object = crate::wayland_objects::wl_pointer::WlPointer;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_pointer::WlPointer {
        type Id = WlPointerId;
    }
    /// Minimum interface version for the `release` message (XML `since` / `version`).
    pub const RELEASE_SINCE: u32 = 3;
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
        /// On wl_surface.offset requests to the pointer surface, hotspot_x
        /// and hotspot_y are decremented by the x and y parameters
        /// passed to the request. The offset must be applied by
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
    /// Minimum interface version for the `frame` message (XML `since` / `version`).
    pub const FRAME_SINCE: u32 = 5;
    /// Minimum interface version for the `axis_source` message (XML `since` / `version`).
    pub const AXIS_SOURCE_SINCE: u32 = 5;
    /// Minimum interface version for the `axis_stop` message (XML `since` / `version`).
    pub const AXIS_STOP_SINCE: u32 = 5;
    /// Minimum interface version for the `axis_discrete` message (XML `since` / `version`).
    pub const AXIS_DISCRETE_SINCE: u32 = 5;
    /// Minimum interface version for the `axis_value120` message (XML `since` / `version`).
    pub const AXIS_VALUE120_SINCE: u32 = 8;
    /// Minimum interface version for the `axis_relative_direction` message (XML `since` / `version`).
    pub const AXIS_RELATIVE_DIRECTION_SINCE: u32 = 9;
    /// Minimum interface version for the `warp` message (XML `since` / `version`).
    pub const WARP_SINCE: u32 = 11;
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
        /// The location of the click is given by the last motion, warp or
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
        /// Pointer warp event
        ///
        /// Notification of pointer location change within a surface.
        ///
        /// This location change is not due to events on the input device,
        /// but because either the surface under the pointer was moved and
        /// thus the relative position of the pointer changed, or because
        /// the compositor changed the pointer position in response to an
        /// event like pointer confinement being exited.
        ///
        /// The arguments surface_x and surface_y are the location relative to
        /// the focused surface.
        ///
        /// This event must not occur in the same wl_pointer.frame as a
        /// wl_pointer.enter or wl_pointer.motion event.
        Warp {
            /// Surface-local x coordinate,
            surface_x: f32,
            /// Surface-local y coordinate,
            surface_y: f32,
        },
    }
    impl EventWriter for crate::wayland_objects::wl_pointer::WlPointer {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
                Warp { surface_x, surface_y } => {
                    let mut raw = RawEvent::new(self.id(), 11, 2);
                    raw.fixed(surface_x);
                    raw.fixed(surface_y);
                    raw
                }
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Given wl_surface has another role,
        Role = 0,
    }
    /// Physical button state
    ///
    /// Describes the physical state of a button that produced the button
    /// event.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum ButtonState {
        /// The button is not pressed,
        Released = 0,
        /// The button is pressed,
        Pressed = 1,
    }
    /// Axis types
    ///
    /// Describes the axis types of scroll events.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
///
/// Each wl_keyboard has the following logical state:
///
/// - an active surface (possibly null),
/// - the keys currently logically down,
/// - the active modifiers,
/// - the active group.
///
/// By default, the active surface is null, the keys currently logically down
/// are empty, the active modifiers and the active group are 0.
pub mod wl_keyboard {
    use super::*;
    impl Constants for crate::wayland_objects::wl_keyboard::WlKeyboard {
        const INTERFACE_NAME: &'static str = "wl_keyboard";
        const INTERFACE_VERSION: u32 = 11;
    }
    impl crate::object::IdToObject for WlKeyboardId {
        type Object = crate::wayland_objects::wl_keyboard::WlKeyboard;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_keyboard::WlKeyboard {
        type Id = WlKeyboardId;
    }
    /// Minimum interface version for the `release` message (XML `since` / `version`).
    pub const RELEASE_SINCE: u32 = 3;
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
    /// Minimum interface version for the `repeat_info` message (XML `since` / `version`).
    pub const REPEAT_INFO_SINCE: u32 = 4;
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
        ///
        /// In the wl_keyboard logical state, this event sets the active surface to
        /// the surface argument and the keys currently logically down to the keys
        /// in the keys argument. The compositor must not send this event if the
        /// wl_keyboard already had an active surface immediately before this event.
        ///
        /// Clients should not use the list of pressed keys to emulate key-press
        /// events. The order of keys in the list is unspecified.
        Enter {
            /// Serial number of the enter event,
            serial: u32,
            /// Surface gaining keyboard focus,
            surface: WlSurfaceId,
            /// The keys currently logically down,
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
        /// In the wl_keyboard logical state, this event resets all values to their
        /// defaults. The compositor must not send this event if the active surface
        /// of the wl_keyboard was not equal to the surface argument immediately
        /// before this event.
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
        ///
        /// In the wl_keyboard logical state, this event adds the key to the keys
        /// currently logically down (if the state argument is pressed) or removes
        /// the key from the keys currently logically down (if the state argument is
        /// released). The compositor must not send this event if the wl_keyboard
        /// did not have an active surface immediately before this event. The
        /// compositor must not send this event if state is pressed (resp. released)
        /// and the key was already logically down (resp. was not logically down)
        /// immediately before this event.
        ///
        /// Since version 10, compositors may send key events with the "repeated"
        /// key state when a wl_keyboard.repeat_info event with a rate argument of
        /// 0 has been received. This allows the compositor to take over the
        /// responsibility of key repetition.
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
        ///
        /// The compositor may send this event without a surface of the client
        /// having keyboard focus, for example to tie modifier information to
        /// pointer focus instead. If a modifier event with pressed modifiers is sent
        /// without a prior enter event, the client can assume the modifier state is
        /// valid until it receives the next wl_keyboard.modifiers event. In order to
        /// reset the modifier state again, the compositor can send a
        /// wl_keyboard.modifiers event with no pressed modifiers.
        ///
        /// In the wl_keyboard logical state, this event updates the modifiers and
        /// group.
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum KeymapFormat {
        /// No keymap; client must understand how to interpret the raw keycode,
        NoKeymap = 0,
        /// Libxkbcommon compatible, null-terminated string; to determine the xkb keycode, clients must add 8 to the key event keycode,
        XkbV1 = 1,
    }
    /// Physical key state
    ///
    /// Describes the physical state of a key that produced the key event.
    ///
    /// Since version 10, the key can be in a "repeated" pseudo-state which
    /// means the same as "pressed", but is used to signal repetition in the
    /// key event.
    ///
    /// The key may only enter the repeated state after entering the pressed
    /// state and before entering the released state. This event may be
    /// generated multiple times while the key is down.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum KeyState {
        /// Key is not pressed,
        Released = 0,
        /// Key is pressed,
        Pressed = 1,
        /// Key was repeated,
        Repeated = 2,
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
    impl Constants for crate::wayland_objects::wl_touch::WlTouch {
        const INTERFACE_NAME: &'static str = "wl_touch";
        const INTERFACE_VERSION: u32 = 11;
    }
    impl crate::object::IdToObject for WlTouchId {
        type Object = crate::wayland_objects::wl_touch::WlTouch;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_touch::WlTouch {
        type Id = WlTouchId;
    }
    /// Minimum interface version for the `release` message (XML `since` / `version`).
    pub const RELEASE_SINCE: u32 = 3;
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
    /// Minimum interface version for the `shape` message (XML `since` / `version`).
    pub const SHAPE_SINCE: u32 = 6;
    /// Minimum interface version for the `orientation` message (XML `since` / `version`).
    pub const ORIENTATION_SINCE: u32 = 6;
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
        ///
        /// No frame event is required after the cancel event.
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
        /// touchpoint location as reported by wl_touch.down or wl_touch.motion.
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    impl Constants for crate::wayland_objects::wl_output::WlOutput {
        const INTERFACE_NAME: &'static str = "wl_output";
        const INTERFACE_VERSION: u32 = 4;
    }
    impl crate::object::IdToObject for WlOutputId {
        type Object = crate::wayland_objects::wl_output::WlOutput;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_output::WlOutput {
        type Id = WlOutputId;
    }
    /// Minimum interface version for the `release` message (XML `since` / `version`).
    pub const RELEASE_SINCE: u32 = 3;
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
    /// Minimum interface version for the `done` message (XML `since` / `version`).
    pub const DONE_SINCE: u32 = 2;
    /// Minimum interface version for the `scale` message (XML `since` / `version`).
    pub const SCALE_SINCE: u32 = 2;
    /// Minimum interface version for the `name` message (XML `since` / `version`).
    pub const NAME_SINCE: u32 = 4;
    /// Minimum interface version for the `description` message (XML `since` / `version`).
    pub const DESCRIPTION_SINCE: u32 = 4;
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
        /// Clients should use wl_surface.preferred_buffer_transform instead of the
        /// transform advertised by this event to find the preferred buffer
        /// transform to use for a surface.
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
            /// Additional transformation applied to buffer contents during presentation,
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
        /// later. The compositor will emit a non-zero, positive
        /// value for scale. If it is not sent, the client should
        /// assume a scale of 1.
        ///
        /// A scale larger than 1 means that the compositor will
        /// automatically scale surface buffers by this amount
        /// when rendering. This is used for very high resolution
        /// displays where applications rendering at the native
        /// resolution would be too small to be legible.
        ///
        /// Clients should use wl_surface.preferred_buffer_scale
        /// instead of this event to find the preferred buffer
        /// scale to use for a surface.
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    /// Transformation applied to buffer contents
    ///
    /// This describes transformations that clients and compositors apply to
    /// buffer contents.
    ///
    /// The flipped values correspond to an initial flip around a
    /// vertical axis followed by rotation.
    ///
    /// The purpose is mainly to allow clients to render accordingly and
    /// tell the compositor, so that for fullscreen surfaces, the
    /// compositor will still be able to scan out directly from client
    /// surfaces.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::wl_region::WlRegion {
        const INTERFACE_NAME: &'static str = "wl_region";
        const INTERFACE_VERSION: u32 = 7;
    }
    impl crate::object::IdToObject for WlRegionId {
        type Object = crate::wayland_objects::wl_region::WlRegion;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_region::WlRegion {
        type Id = WlRegionId;
    }
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
    impl Constants for crate::wayland_objects::wl_subcompositor::WlSubcompositor {
        const INTERFACE_NAME: &'static str = "wl_subcompositor";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WlSubcompositorId {
        type Object = crate::wayland_objects::wl_subcompositor::WlSubcompositor;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_subcompositor::WlSubcompositor {
        type Id = WlSubcompositorId;
    }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
/// A sub-surface can be in one of two modes. The possible modes are
/// synchronized and desynchronized, see methods wl_subsurface.set_sync and
/// wl_subsurface.set_desync.
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
/// If a surface behaves as in synchronized mode, it is effectively
/// synchronized, otherwise it is effectively desynchronized.
///
/// A sub-surface is initially in the synchronized mode.
///
/// The wl_subsurface interface has requests which modify double-buffered
/// state of the parent surface (wl_subsurface.set_position, .place_above and
/// .place_below).
///
/// Destroying a sub-surface takes effect immediately. If you need to
/// synchronize the removal of a sub-surface to the parent surface update,
/// unmap the sub-surface first by attaching a NULL wl_buffer, update parent,
/// and then destroy the sub-surface.
///
/// If the parent wl_surface object is destroyed, the sub-surface is
/// unmapped.
///
/// A sub-surface never has the keyboard focus of any seat.
///
/// The wl_surface.offset request is ignored: clients must use set_position
/// instead to move the sub-surface.
pub mod wl_subsurface {
    use super::*;
    impl Constants for crate::wayland_objects::wl_subsurface::WlSubsurface {
        const INTERFACE_NAME: &'static str = "wl_subsurface";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WlSubsurfaceId {
        type Object = crate::wayland_objects::wl_subsurface::WlSubsurface;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_subsurface::WlSubsurface {
        type Id = WlSubsurfaceId;
    }
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
        /// This sets the position of the sub-surface, relative to the parent
        /// surface.
        ///
        /// The sub-surface will be moved so that its origin (top left
        /// corner pixel) will be at the location x, y of the parent surface
        /// coordinate system. The coordinates are not restricted to the parent
        /// surface area. Negative values are allowed.
        ///
        /// The initial position is 0, 0.
        ///
        /// Position is double-buffered state on the parent surface, see
        /// wl_subsurface and wl_surface.commit for more information.
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
        /// A new sub-surface is initially added as the top-most in the stack
        /// of its siblings and parent.
        ///
        /// Z-order is double-buffered state on the parent surface, see
        /// wl_subsurface and wl_surface.commit for more information.
        PlaceAbove {
            /// The reference surface,
            sibling: WlSurfaceId,
        },
        /// Restack the sub-surface
        ///
        /// The sub-surface is placed just below the reference surface.
        ///
        /// See wl_subsurface.place_above.
        PlaceBelow {
            /// The reference surface,
            sibling: WlSurfaceId,
        },
        /// Set sub-surface to synchronized mode
        ///
        /// Change the commit behaviour of the sub-surface to synchronized
        /// mode.
        ///
        /// See wl_subsurface and wl_surface.commit for more information.
        SetSync,
        /// Set sub-surface to desynchronized mode
        ///
        /// Change the commit behaviour of the sub-surface to desynchronized
        /// mode.
        ///
        /// See wl_subsurface and wl_surface.commit for more information.
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Wl_surface is not a sibling or the parent,
        BadSurface = 0,
    }
}

/// Wayland protocol fixes
///
/// This global fixes problems with other core-protocol interfaces that
/// cannot be fixed in these interfaces themselves.
pub mod wl_fixes {
    use super::*;
    impl Constants for crate::wayland_objects::wl_fixes::WlFixes {
        const INTERFACE_NAME: &'static str = "wl_fixes";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for WlFixesId {
        type Object = crate::wayland_objects::wl_fixes::WlFixes;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wl_fixes::WlFixes {
        type Id = WlFixesId;
    }
    /// Minimum interface version for the `ack_global_remove` message (XML `since` / `version`).
    pub const ACK_GLOBAL_REMOVE_SINCE: u32 = 2;
    #[derive(Debug)]
    pub enum Request {
        /// Destroys this object
        ///
        ///
        Destroy,
        /// Destroy a wl_registry
        ///
        /// This request destroys a wl_registry object.
        ///
        /// The client should no longer use the wl_registry after making this
        /// request.
        ///
        /// The compositor will emit a wl_display.delete_id event with the object ID
        /// of the registry and will no longer emit any events on the registry. The
        /// client should re-use the object ID once it receives the
        /// wl_display.delete_id event.
        DestroyRegistry {
            /// The registry to destroy,
            registry: WlRegistryId,
        },
        /// Acknowledge global removal
        ///
        /// Acknowledge the removal of the specified global.
        ///
        /// If no global with the specified name exists or the global is not removed,
        /// the wl_fixes.invalid_ack_remove protocol error will be posted.
        ///
        /// Due to the Wayland protocol being asynchronous, the wl_global objects
        /// cannot be destroyed immediately. For example, if a wl_global is removed
        /// and a client attempts to bind that global around same time, it can
        /// result in a protocol error due to an unknown global name in the bind
        /// request.
        ///
        /// In order to avoid crashing clients, the compositor should remove the
        /// wl_global once it is guaranteed that no more bind requests will come.
        ///
        /// The wl_fixes.ack_global_remove() request is used to signal to the
        /// compositor that the client will not bind the given global anymore. After
        /// all clients acknowledge the removal of the global, the compositor can
        /// safely destroy it.
        ///
        /// The client must call the wl_fixes.ack_global_remove() request in
        /// response to a wl_registry.global_remove() event even if it did not bind
        /// the corresponding global.
        AckGlobalRemove {
            /// The registry object,
            registry: WlRegistryId,
            /// Unique name of the global,
            name: u32,
        },
    }
    impl Parser for crate::wayland_objects::wl_fixes::WlFixes {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => DestroyRegistry { registry: req.id()? },
                2 => AckGlobalRemove { registry: req.id()?, name: req.uint()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    /// Wl_fixes error values
    ///
    /// These errors can be emitted in response to wl_fixes requests.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Unknown global or the global is not removed,
        InvalidAckRemove = 0,
    }
}

/// Factory for creating dmabuf-based wl_buffers
///
/// This interface offers ways to create generic dmabuf-based wl_buffers.
///
/// For more information about dmabuf, see:
/// https://www.kernel.org/doc/html/next/userspace-api/dma-buf-alloc-exchange.html
///
/// Clients can use the get_surface_feedback request to get dmabuf feedback
/// for a particular surface. If the client wants to retrieve feedback not
/// tied to a surface, they can use the get_default_feedback request.
///
/// The following are required from clients:
///
/// - Clients must ensure that either all data in the dma-buf is
/// coherent for all subsequent read access or that coherency is
/// correctly handled by the underlying kernel-side dma-buf
/// implementation.
///
/// - Don't make any more attachments after sending the buffer to the
/// compositor. Making more attachments later increases the risk of
/// the compositor not being able to use (re-import) an existing
/// dmabuf-based wl_buffer.
///
/// The underlying graphics stack must ensure the following:
///
/// - The dmabuf file descriptors relayed to the server will stay valid
/// for the whole lifetime of the wl_buffer. This means the server may
/// at any time use those fds to import the dmabuf into any kernel
/// sub-system that might accept it.
///
/// However, when the underlying graphics stack fails to deliver the
/// promise, because of e.g. a device hot-unplug which raises internal
/// errors, after the wl_buffer has been successfully created the
/// compositor must not raise protocol errors to the client when dmabuf
/// import later fails.
///
/// To create a wl_buffer from one or more dmabufs, a client creates a
/// zwp_linux_buffer_params_v1 object with a zwp_linux_dmabuf_v1.create_params
/// request. All planes required by the intended format are added with
/// the 'add' request. Finally, a 'create' or 'create_immed' request is
/// issued, which has the following outcome depending on the import success.
///
/// The 'create' request,
/// - on success, triggers a 'created' event which provides the final
/// wl_buffer to the client.
/// - on failure, triggers a 'failed' event to convey that the server
/// cannot use the dmabufs received from the client.
///
/// For the 'create_immed' request,
/// - on success, the server immediately imports the added dmabufs to
/// create a wl_buffer. No event is sent from the server in this case.
/// - on failure, the server can choose to either:
/// - terminate the client by raising a fatal error.
/// - mark the wl_buffer as failed, and send a 'failed' event to the
/// client. If the client uses a failed wl_buffer as an argument to any
/// request, the behaviour is compositor implementation-defined.
///
/// For all DRM formats and unless specified in another protocol extension,
/// pre-multiplied alpha is used for pixel values.
///
/// Unless specified otherwise in another protocol extension, implicit
/// synchronization is used. In other words, compositors and clients must
/// wait and signal fences implicitly passed via the DMA-BUF's reservation
/// mechanism.
pub mod zwp_linux_dmabuf_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 {
        const INTERFACE_NAME: &'static str = "zwp_linux_dmabuf_v1";
        const INTERFACE_VERSION: u32 = 6;
    }
    impl crate::object::IdToObject for ZwpLinuxDmabufV1Id {
        type Object = crate::wayland_objects::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 {
        type Id = ZwpLinuxDmabufV1Id;
    }
    /// Minimum interface version for the `get_default_feedback` message (XML `since` / `version`).
    pub const GET_DEFAULT_FEEDBACK_SINCE: u32 = 4;
    /// Minimum interface version for the `get_surface_feedback` message (XML `since` / `version`).
    pub const GET_SURFACE_FEEDBACK_SINCE: u32 = 4;
    #[derive(Debug)]
    pub enum Request {
        /// Unbind the factory
        ///
        /// Objects created through this interface, especially wl_buffers, will
        /// remain valid.
        Destroy,
        /// Create a temporary object for buffer parameters
        ///
        /// This temporary object is used to collect multiple dmabuf handles into
        /// a single batch to create a wl_buffer. It can only be used once and
        /// should be destroyed after a 'created' or 'failed' event has been
        /// received.
        CreateParams {
            /// Id for the newly created zwp_linux_buffer_params_v1,
            params_id: ZwpLinuxBufferParamsV1Id,
        },
        /// Get default feedback
        ///
        /// This request creates a new zwp_linux_dmabuf_feedback_v1 object not bound
        /// to a particular surface. This object will deliver feedback about dmabuf
        /// parameters to use if the client doesn't support per-surface feedback
        /// (see get_surface_feedback).
        GetDefaultFeedback { id: ZwpLinuxDmabufFeedbackV1Id },
        /// Get feedback for a surface
        ///
        /// This request creates a new zwp_linux_dmabuf_feedback_v1 object for the
        /// specified wl_surface. This object will deliver feedback about dmabuf
        /// parameters to use for buffers attached to this surface.
        ///
        /// If the surface is destroyed before the zwp_linux_dmabuf_feedback_v1 object,
        /// the feedback object becomes inert.
        GetSurfaceFeedback { id: ZwpLinuxDmabufFeedbackV1Id, surface: WlSurfaceId },
    }
    impl Parser for crate::wayland_objects::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => CreateParams { params_id: req.id()? },
                2 => GetDefaultFeedback { id: req.id()? },
                3 => GetSurfaceFeedback { id: req.id()?, surface: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    /// Minimum interface version for the `modifier` message (XML `since` / `version`).
    pub const MODIFIER_SINCE: u32 = 3;
    #[derive(Debug)]
    pub enum Event {
        /// Supported buffer format
        ///
        /// This event advertises one buffer format that the server supports.
        /// All the supported formats are advertised once when the client
        /// binds to this interface. A roundtrip after binding guarantees
        /// that the client has received all supported formats.
        ///
        /// For the definition of the format codes, see the
        /// zwp_linux_buffer_params_v1::create request.
        ///
        /// Starting version 4, the format event is deprecated and must not be
        /// sent by compositors. Instead, use get_default_feedback or
        /// get_surface_feedback.
        Format {
            /// DRM_FORMAT code,
            format: u32,
        },
        /// Supported buffer format modifier
        ///
        /// This event advertises the formats that the server supports, along with
        /// the modifiers supported for each format. All the supported modifiers
        /// for all the supported formats are advertised once when the client
        /// binds to this interface. A roundtrip after binding guarantees that
        /// the client has received all supported format-modifier pairs.
        ///
        /// For legacy support, DRM_FORMAT_MOD_INVALID (that is, modifier_hi ==
        /// 0x00ffffff and modifier_lo == 0xffffffff) is allowed in this event.
        /// It indicates that the server can support the format with an implicit
        /// modifier. When a plane has DRM_FORMAT_MOD_INVALID as its modifier, it
        /// is as if no explicit modifier is specified. The effective modifier
        /// will be derived from the dmabuf.
        ///
        /// A compositor that sends valid modifiers and DRM_FORMAT_MOD_INVALID for
        /// a given format supports both explicit modifiers and implicit modifiers.
        ///
        /// For the definition of the format and modifier codes, see the
        /// zwp_linux_buffer_params_v1::create and zwp_linux_buffer_params_v1::add
        /// requests.
        ///
        /// Starting version 4, the modifier event is deprecated and must not be
        /// sent by compositors. Instead, use get_default_feedback or
        /// get_surface_feedback.
        Modifier {
            /// DRM_FORMAT code,
            format: u32,
            /// High 32 bits of layout modifier,
            modifier_hi: u32,
            /// Low 32 bits of layout modifier,
            modifier_lo: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Format { format } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(format);
                    raw
                }
                Modifier { format, modifier_hi, modifier_lo } => {
                    let mut raw = RawEvent::new(self.id(), 1, 3);
                    raw.uint(format);
                    raw.uint(modifier_hi);
                    raw.uint(modifier_lo);
                    raw
                }
            }
        }
    }
}

/// Parameters for creating a dmabuf-based wl_buffer
///
/// This temporary object is a collection of dmabufs and other
/// parameters that together form a single logical buffer. The temporary
/// object may eventually create one wl_buffer unless cancelled by
/// destroying it before requesting 'create'.
///
/// Single-planar formats only require one dmabuf, however
/// multi-planar formats may require more than one dmabuf. For all
/// formats, an 'add' request must be called once per plane (even if the
/// underlying dmabuf fd is identical).
///
/// You must use consecutive plane indices ('plane_idx' argument for 'add')
/// from zero to the number of planes used by the drm_fourcc format code.
/// All planes required by the format must be given exactly once, but can
/// be given in any order. Each plane index can only be set once; subsequent
/// calls with a plane index which has already been set will result in a
/// plane_set error being generated.
pub mod zwp_linux_buffer_params_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1 {
        const INTERFACE_NAME: &'static str = "zwp_linux_buffer_params_v1";
        const INTERFACE_VERSION: u32 = 6;
    }
    impl crate::object::IdToObject for ZwpLinuxBufferParamsV1Id {
        type Object = crate::wayland_objects::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1
    {
        type Id = ZwpLinuxBufferParamsV1Id;
    }
    /// Minimum interface version for the `create_immed` message (XML `since` / `version`).
    pub const CREATE_IMMED_SINCE: u32 = 2;
    /// Minimum interface version for the `set_sampling_device` message (XML `since` / `version`).
    pub const SET_SAMPLING_DEVICE_SINCE: u32 = 6;
    #[derive(Debug)]
    pub enum Request {
        /// Delete this object, used or not
        ///
        /// Cleans up the temporary data sent to the server for dmabuf-based
        /// wl_buffer creation.
        Destroy,
        /// Add a dmabuf to the temporary set
        ///
        /// This request adds one dmabuf to the set in this
        /// zwp_linux_buffer_params_v1.
        ///
        /// The 64-bit unsigned value combined from modifier_hi and modifier_lo
        /// is the dmabuf layout modifier. DRM AddFB2 ioctl calls this the
        /// fb modifier, which is defined in drm_mode.h of Linux UAPI.
        /// This is an opaque token. Drivers use this token to express tiling,
        /// compression, etc. driver-specific modifications to the base format
        /// defined by the DRM fourcc code.
        ///
        /// Starting from version 4, the invalid_format protocol error is sent if
        /// the format + modifier pair was not advertised as supported.
        ///
        /// Starting from version 5, the invalid_format protocol error is sent if
        /// all planes don't use the same modifier.
        ///
        /// This request raises the PLANE_IDX error if plane_idx is too large.
        /// The error PLANE_SET is raised if attempting to set a plane that
        /// was already set.
        Add {
            /// Dmabuf fd,
            fd: RawFd,
            /// Plane index,
            plane_idx: u32,
            /// Offset in bytes,
            offset: u32,
            /// Stride in bytes,
            stride: u32,
            /// High 32 bits of layout modifier,
            modifier_hi: u32,
            /// Low 32 bits of layout modifier,
            modifier_lo: u32,
        },
        /// Create a wl_buffer from the given dmabufs
        ///
        /// This asks for creation of a wl_buffer from the added dmabuf
        /// buffers. The wl_buffer is not created immediately but returned via
        /// the 'created' event if the dmabuf sharing succeeds. The sharing
        /// may fail at runtime for reasons a client cannot predict, in
        /// which case the 'failed' event is triggered.
        ///
        /// The 'format' argument is a DRM_FORMAT code, as defined by the
        /// libdrm's drm_fourcc.h. The Linux kernel's DRM sub-system is the
        /// authoritative source on how the format codes should work.
        ///
        /// The 'flags' is a bitfield of the flags defined in enum "flags".
        /// 'y_invert' means that the image needs to be y-flipped.
        ///
        /// Flag 'interlaced' means that the frame in the buffer is not
        /// progressive as usual, but interlaced. An interlaced buffer as
        /// supported here must always contain both top and bottom fields.
        /// The top field always begins on the first pixel row. The temporal
        /// ordering between the two fields is top field first, unless
        /// 'bottom_first' is specified. It is undefined whether 'bottom_first'
        /// is ignored if 'interlaced' is not set.
        ///
        /// This protocol does not convey any information about field rate,
        /// duration, or timing, other than the relative ordering between the
        /// two fields in one buffer. A compositor may have to estimate the
        /// intended field rate from the incoming buffer rate. It is undefined
        /// whether the time of receiving wl_surface.commit with a new buffer
        /// attached, applying the wl_surface state, wl_surface.frame callback
        /// trigger, presentation, or any other point in the compositor cycle
        /// is used to measure the frame or field times. There is no support
        /// for detecting missed or late frames/fields/buffers either, and
        /// there is no support whatsoever for cooperating with interlaced
        /// compositor output.
        ///
        /// The composited image quality resulting from the use of interlaced
        /// buffers is explicitly undefined. A compositor may use elaborate
        /// hardware features or software to deinterlace and create progressive
        /// output frames from a sequence of interlaced input buffers, or it
        /// may produce substandard image quality. However, compositors that
        /// cannot guarantee reasonable image quality in all cases are recommended
        /// to just reject all interlaced buffers.
        ///
        /// Any argument errors, including non-positive width or height,
        /// mismatch between the number of planes and the format, bad
        /// format, bad offset or stride, may be indicated by fatal protocol
        /// errors: INCOMPLETE, INVALID_FORMAT, INVALID_DIMENSIONS,
        /// OUT_OF_BOUNDS.
        ///
        /// Dmabuf import errors in the server that are not obvious client
        /// bugs are returned via the 'failed' event as non-fatal. This
        /// allows attempting dmabuf sharing and falling back in the client
        /// if it fails.
        ///
        /// This request can be sent only once in the object's lifetime, after
        /// which the only legal request is destroy. This object should be
        /// destroyed after issuing a 'create' request. Attempting to use this
        /// object after issuing 'create' raises the ALREADY_USED protocol error.
        ///
        /// It is not mandatory to issue 'create'. If a client wants to
        /// cancel the buffer creation, it can just destroy this object.
        Create {
            /// Base plane width in pixels,
            width: i32,
            /// Base plane height in pixels,
            height: i32,
            /// DRM_FORMAT code,
            format: u32,
            /// See enum flags,
            flags: u32,
        },
        /// Immediately create a wl_buffer from the given dmabufs
        ///
        /// This asks for immediate creation of a wl_buffer by importing the
        /// added dmabufs.
        ///
        /// In case of import success, no event is sent from the server, and the
        /// wl_buffer is ready to be used by the client.
        ///
        /// Upon import failure, either of the following may happen, as seen fit
        /// by the implementation:
        /// - the client is terminated with one of the following fatal protocol
        /// errors:
        /// - INCOMPLETE, INVALID_FORMAT, INVALID_DIMENSIONS, OUT_OF_BOUNDS,
        /// in case of argument errors such as mismatch between the number
        /// of planes and the format, bad format, non-positive width or
        /// height, or bad offset or stride.
        /// - INVALID_WL_BUFFER, in case the cause for failure is unknown or
        /// platform specific.
        /// - the server creates an invalid wl_buffer, marks it as failed and
        /// sends a 'failed' event to the client. The result of using this
        /// invalid wl_buffer as an argument in any request by the client is
        /// defined by the compositor implementation.
        ///
        /// This takes the same arguments as a 'create' request, and obeys the
        /// same restrictions.
        CreateImmed {
            /// Id for the newly created wl_buffer,
            buffer_id: WlBufferId,
            /// Base plane width in pixels,
            width: i32,
            /// Base plane height in pixels,
            height: i32,
            /// DRM_FORMAT code,
            format: u32,
            /// See enum flags,
            flags: u32,
        },
        /// Set the target device of the wl_buffer
        ///
        /// Set the device the compositor should import the dmabufs to for sampling
        /// in the next create or create_immed request.
        ///
        /// To avoid race conditions when the compositor removes a device from the
        /// tranches, it is not a protocol error if the device hasn't been advertised
        /// by the compositor in a tranche with the sampling flag, but the import is
        /// likely to fail in that case.
        ///
        /// If the client doesn't know a suitable target device, it shouldn't set one,
        /// and the compositor should attempt import on all devices it supports.
        ///
        /// If the array is too small to contain a dev_t or larger than required, the
        /// invalid_dev_t_size error will be emitted.
        SetSamplingDevice {
            /// Device dev_t value,
            device: Array,
        },
    }
    impl Parser for crate::wayland_objects::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => Add {
                    fd: req.fd()?,
                    plane_idx: req.uint()?,
                    offset: req.uint()?,
                    stride: req.uint()?,
                    modifier_hi: req.uint()?,
                    modifier_lo: req.uint()?,
                },
                2 => Create {
                    width: req.int()?,
                    height: req.int()?,
                    format: req.uint()?,
                    flags: req.uint()?,
                },
                3 => CreateImmed {
                    buffer_id: req.id()?,
                    width: req.int()?,
                    height: req.int()?,
                    format: req.uint()?,
                    flags: req.uint()?,
                },
                4 => SetSamplingDevice { device: req.array()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Buffer creation succeeded
        ///
        /// This event indicates that the attempted buffer creation was
        /// successful. It provides the new wl_buffer referencing the dmabuf(s).
        ///
        /// Upon receiving this event, the client should destroy the
        /// zwp_linux_buffer_params_v1 object.
        Created {
            /// Id for the the newly created wl_buffer,
            buffer: WlBufferId,
        },
        /// Buffer creation failed
        ///
        /// This event indicates that the attempted buffer creation has
        /// failed. It usually means that one of the dmabuf constraints
        /// has not been fulfilled.
        ///
        /// Upon receiving this event, the client should destroy the
        /// zwp_linux_buffer_params_v1 object.
        Failed,
    }
    impl EventWriter for crate::wayland_objects::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Created { buffer } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.id(buffer);
                    raw
                }
                Failed {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// The zwp_linux_buffer_params_v1 object has already been used to create a wl_buffer,
        AlreadyUsed = 0,
        /// Plane index out of bounds,
        PlaneIdx = 1,
        /// The plane index was already set,
        PlaneSet = 2,
        /// Missing or too many planes to create a buffer,
        Incomplete = 3,
        /// Format not supported,
        InvalidFormat = 4,
        /// Invalid width or height,
        InvalidDimensions = 5,
        /// Offset + stride * height goes out of dmabuf bounds,
        OutOfBounds = 6,
        /// Invalid wl_buffer resulted from importing dmabufs via the create_immed request on given buffer_params,
        InvalidWlBuffer = 7,
        /// An array with mismatching size for a dev_t was used,
        InvalidDevTSize = 8,
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Flags {
        /// Contents are y-inverted,
        YInvert = 1,
        /// Content is interlaced,
        Interlaced = 2,
        /// Bottom field first,
        BottomFirst = 4,
    }
}

/// Dmabuf feedback
///
/// This object advertises dmabuf parameters feedback. This includes the
/// preferred devices and the supported formats/modifiers.
///
/// The parameters are sent once when this object is created and whenever they
/// change. The done event is always sent once after all parameters have been
/// sent. When a single parameter changes, all parameters are re-sent by the
/// compositor.
///
/// Compositors can re-send the parameters when the current client buffer
/// allocations are sub-optimal. Compositors should not re-send the
/// parameters if re-allocating the buffers would not result in a more optimal
/// configuration. In particular, compositors should avoid sending the exact
/// same parameters multiple times in a row.
///
/// The tranche_target_device and tranche_formats events are grouped by
/// tranches of preference. For each tranche, a tranche_target_device, one
/// tranche_flags and one or more tranche_formats events are sent, followed
/// by a tranche_done event finishing the list. The tranches are sent in
/// descending order of preference. All formats and modifiers in the same
/// tranche have the same preference.
///
/// To send parameters, the compositor sends one main_device event (unless
/// the client bound version 6 or above), tranches (each consisting of one
/// tranche_target_device event, one tranche_flags event, tranche_formats
/// events and then a tranche_done event), then one done event.
///
/// With version 6 and above, the compositor must always advertise at least
/// one tranche with the sampling flag set.
pub mod zwp_linux_dmabuf_feedback_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1 {
        const INTERFACE_NAME: &'static str = "zwp_linux_dmabuf_feedback_v1";
        const INTERFACE_VERSION: u32 = 6;
    }
    impl crate::object::IdToObject for ZwpLinuxDmabufFeedbackV1Id {
        type Object =
            crate::wayland_objects::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1
    {
        type Id = ZwpLinuxDmabufFeedbackV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the feedback object
        ///
        /// Using this request a client can tell the server that it is not going to
        /// use the zwp_linux_dmabuf_feedback_v1 object anymore.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1 {
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
        /// All feedback has been sent
        ///
        /// This event is sent after all parameters of a zwp_linux_dmabuf_feedback_v1
        /// object have been sent.
        ///
        /// This allows changes to the zwp_linux_dmabuf_feedback_v1 parameters to be
        /// seen as atomic, even if they happen via multiple events.
        Done,
        /// Format and modifier table
        ///
        /// This event provides a file descriptor which can be memory-mapped to
        /// access the format and modifier table.
        ///
        /// The table contains a tightly packed array of consecutive format +
        /// modifier pairs. Each pair is 16 bytes wide. It contains a format as a
        /// 32-bit unsigned integer, followed by 4 bytes of unused padding, and a
        /// modifier as a 64-bit unsigned integer. The native endianness is used.
        ///
        /// The client must map the file descriptor in read-only private mode.
        ///
        /// Compositors are not allowed to mutate the table file contents once this
        /// event has been sent. Instead, compositors must create a new, separate
        /// table file and re-send feedback parameters. Compositors are allowed to
        /// store duplicate format + modifier pairs in the table.
        FormatTable {
            /// Table file descriptor,
            fd: RawFd,
            /// Table size, in bytes,
            size: u32,
        },
        /// Preferred main device
        ///
        /// This event advertises the main device that the server prefers to use
        /// when direct scan-out to the target device isn't possible. The
        /// advertised main device may be different for each
        /// zwp_linux_dmabuf_feedback_v1 object, and may change over time.
        ///
        /// There is exactly one main device. The compositor must send at least
        /// one preference tranche with tranche_target_device equal to main_device.
        ///
        /// Clients need to create buffers that the main device can import and
        /// read from, otherwise creating the dmabuf wl_buffer will fail (see the
        /// zwp_linux_buffer_params_v1.create and create_immed requests for details).
        /// The main device will also likely be kept active by the compositor,
        /// so clients can use it instead of waking up another device for power
        /// savings.
        ///
        /// In general the device is a DRM node. The DRM node type (primary vs.
        /// render) is unspecified. Clients must not rely on the compositor sending
        /// a particular node type. Clients cannot check two devices for equality
        /// by comparing the dev_t value.
        ///
        /// If explicit modifiers are not supported and the client performs buffer
        /// allocations on a different device than the main device, then the client
        /// must force the buffer to have a linear layout.
        ///
        /// With version 6 and above, this event is no longer sent. Clients should
        /// use a device with the sampling flag in the tranches instead.
        MainDevice {
            /// Device dev_t value,
            device: Array,
        },
        /// A preference tranche has been sent
        ///
        /// This event splits tranche_target_device and tranche_formats events into
        /// preference tranches. It is sent after a set of tranche_target_device
        /// and tranche_formats events; it represents the end of a tranche. The
        /// next tranche will have a lower preference.
        TrancheDone,
        /// Target device
        ///
        /// This event advertises the target device that the server prefers to use
        /// for a buffer created given this tranche. The advertised target device
        /// may be different for each preference tranche, and may change over time.
        ///
        /// There is exactly one target device per tranche.
        ///
        /// The target device may be a scan-out device, for example if the
        /// compositor prefers to directly scan-out a buffer created given this
        /// tranche. The target device may be a rendering device, for example if
        /// the compositor prefers to texture from said buffer.
        ///
        /// The client can use this hint to allocate the buffer in a way that makes
        /// it accessible from the target device, ideally directly. The buffer must
        /// still be accessible from a device with the sampling flag, either through
        /// direct import or a potentially more expensive fallback path. If the
        /// buffer can't be directly imported for sampling, then clients must be
        /// prepared for the compositor changing the tranche priority or making
        /// wl_buffer creation fail (see the zwp_linux_buffer_params_v1.create and
        /// create_immed requests for details).
        ///
        /// If the device is a DRM node, the DRM node type (primary vs. render) is
        /// unspecified. Clients must not rely on the compositor sending a
        /// particular node type. Clients cannot check two devices for equality by
        /// comparing the dev_t value.
        ///
        /// This event is tied to a preference tranche, see the tranche_done event.
        TrancheTargetDevice {
            /// Device dev_t value,
            device: Array,
        },
        /// Supported buffer format modifiers
        ///
        /// This event advertises the format + modifier combinations that the
        /// compositor supports.
        ///
        /// It carries an array of indices, each referring to a format + modifier
        /// pair in the last received format table (see the format_table event).
        /// Each index is a 16-bit unsigned integer in native endianness.
        ///
        /// For legacy support, DRM_FORMAT_MOD_INVALID is an allowed modifier.
        /// It indicates that the server can support the format with an implicit
        /// modifier. When a buffer has DRM_FORMAT_MOD_INVALID as its modifier, it
        /// is as if no explicit modifier is specified. The effective modifier
        /// will be derived from the dmabuf.
        ///
        /// A compositor that sends valid modifiers and DRM_FORMAT_MOD_INVALID for
        /// a given format supports both explicit modifiers and implicit modifiers.
        ///
        /// Compositors must not send duplicate format + modifier pairs within the
        /// same tranche or across two different tranches with the same target
        /// device and flags.
        ///
        /// This event is tied to a preference tranche, see the tranche_done event.
        ///
        /// For the definition of the format and modifier codes, see the
        /// zwp_linux_buffer_params_v1.create request.
        TrancheFormats {
            /// Array of 16-bit indexes,
            indices: Array,
        },
        /// Tranche flags
        ///
        /// This event sets tranche-specific flags. This event is tied to a
        /// preference tranche, see the tranche_done event.
        /// With version 6 and above, the compositor must set at least one flag
        /// in each tranche.
        TrancheFlags {
            /// Tranche flags,
            flags: u32,
        },
    }
    impl EventWriter
        for crate::wayland_objects::zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1
    {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Done {} => {
                    let mut raw = RawEvent::new(self.id(), 0, 0);
                    raw
                }
                FormatTable { fd, size } => {
                    let mut raw = RawEvent::new(self.id(), 1, 2);
                    raw.fd(fd);
                    raw.uint(size);
                    raw
                }
                MainDevice { device } => {
                    let mut raw = RawEvent::new(self.id(), 2, 1);
                    raw.array(device);
                    raw
                }
                TrancheDone {} => {
                    let mut raw = RawEvent::new(self.id(), 3, 0);
                    raw
                }
                TrancheTargetDevice { device } => {
                    let mut raw = RawEvent::new(self.id(), 4, 1);
                    raw.array(device);
                    raw
                }
                TrancheFormats { indices } => {
                    let mut raw = RawEvent::new(self.id(), 5, 1);
                    raw.array(indices);
                    raw
                }
                TrancheFlags { flags } => {
                    let mut raw = RawEvent::new(self.id(), 6, 1);
                    raw.uint(flags);
                    raw
                }
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum TrancheFlags {
        /// Direct scan-out tranche
        ///
        /// The scanout flag is a hint that direct scan-out may be attempted by
        /// the compositor on the target device if the client appropriately
        /// allocates a buffer. How to allocate a buffer that can be scanned out
        /// on the target device is implementation-defined.
        Scanout = 1,
        /// Sampling tranche
        ///
        /// The sampling flag describes that the compositor is able to efficiently
        /// sample from buffers imported to the target device if the client
        /// appropriately allocates a buffer. How to allocate a buffer that can be
        /// efficiently sampled on the target device is implementation defined.
        Sampling = 2,
    }
}

/// Controller object for graphic tablet devices
///
/// An object that provides access to the graphics tablets available on this
/// system. All tablets are associated with a seat, to get access to the
/// actual tablets, use zwp_tablet_manager_v2.get_tablet_seat.
pub mod zwp_tablet_manager_v2 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_tablet_manager_v2::ZwpTabletManagerV2 {
        const INTERFACE_NAME: &'static str = "zwp_tablet_manager_v2";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ZwpTabletManagerV2Id {
        type Object = crate::wayland_objects::zwp_tablet_manager_v2::ZwpTabletManagerV2;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_tablet_manager_v2::ZwpTabletManagerV2
    {
        type Id = ZwpTabletManagerV2Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Get the tablet seat
        ///
        /// Get the zwp_tablet_seat_v2 object for the given seat. This object
        /// provides access to all graphics tablets in this seat.
        GetTabletSeat {
            tablet_seat: ZwpTabletSeatV2Id,
            /// The wl_seat object to retrieve the tablets for,
            seat: WlSeatId,
        },
        /// Release the memory for the tablet manager object
        ///
        /// Destroy the zwp_tablet_manager_v2 object. Objects created from this
        /// object are unaffected and should be destroyed separately.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwp_tablet_manager_v2::ZwpTabletManagerV2 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => GetTabletSeat { tablet_seat: req.id()?, seat: req.id()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// Controller object for graphic tablet devices of a seat
///
/// An object that provides access to the graphics tablets available on this
/// seat. After binding to this interface, the compositor sends a set of
/// zwp_tablet_seat_v2.tablet_added and zwp_tablet_seat_v2.tool_added events.
pub mod zwp_tablet_seat_v2 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_tablet_seat_v2::ZwpTabletSeatV2 {
        const INTERFACE_NAME: &'static str = "zwp_tablet_seat_v2";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ZwpTabletSeatV2Id {
        type Object = crate::wayland_objects::zwp_tablet_seat_v2::ZwpTabletSeatV2;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::zwp_tablet_seat_v2::ZwpTabletSeatV2 {
        type Id = ZwpTabletSeatV2Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Release the memory for the tablet seat object
        ///
        /// Destroy the zwp_tablet_seat_v2 object. Objects created from this
        /// object are unaffected and should be destroyed separately.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwp_tablet_seat_v2::ZwpTabletSeatV2 {
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
        /// New device notification
        ///
        /// This event is sent whenever a new tablet becomes available on this
        /// seat. This event only provides the object id of the tablet, any
        /// static information about the tablet (device name, vid/pid, etc.) is
        /// sent through the zwp_tablet_v2 interface.
        TabletAdded {
            /// The newly added graphics tablet,
            id: ZwpTabletV2Id,
        },
        /// A new tool has been used with a tablet
        ///
        /// This event is sent whenever a tool that has not previously been used
        /// with a tablet comes into use. This event only provides the object id
        /// of the tool; any static information about the tool (capabilities,
        /// type, etc.) is sent through the zwp_tablet_tool_v2 interface.
        ToolAdded {
            /// The newly added tablet tool,
            id: ZwpTabletToolV2Id,
        },
        /// New pad notification
        ///
        /// This event is sent whenever a new pad is known to the system. Typically,
        /// pads are physically attached to tablets and a pad_added event is
        /// sent immediately after the zwp_tablet_seat_v2.tablet_added.
        /// However, some standalone pad devices logically attach to tablets at
        /// runtime, and the client must wait for zwp_tablet_pad_v2.enter to know
        /// the tablet a pad is attached to.
        ///
        /// This event only provides the object id of the pad. All further
        /// features (buttons, strips, rings) are sent through the zwp_tablet_pad_v2
        /// interface.
        PadAdded {
            /// The newly added pad,
            id: ZwpTabletPadV2Id,
        },
    }
    impl EventWriter for crate::wayland_objects::zwp_tablet_seat_v2::ZwpTabletSeatV2 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                TabletAdded { id } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.id(id);
                    raw
                }
                ToolAdded { id } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.id(id);
                    raw
                }
                PadAdded { id } => {
                    let mut raw = RawEvent::new(self.id(), 2, 1);
                    raw.id(id);
                    raw
                }
            }
        }
    }
}

/// A physical tablet tool
///
/// An object that represents a physical tool that has been, or is
/// currently in use with a tablet in this seat. Each zwp_tablet_tool_v2
/// object stays valid until the client destroys it; the compositor
/// reuses the zwp_tablet_tool_v2 object to indicate that the object's
/// respective physical tool has come into proximity of a tablet again.
///
/// A zwp_tablet_tool_v2 object's relation to a physical tool depends on the
/// tablet's ability to report serial numbers. If the tablet supports
/// this capability, then the object represents a specific physical tool
/// and can be identified even when used on multiple tablets.
///
/// A tablet tool has a number of static characteristics, e.g. tool type,
/// hardware_serial and capabilities. These capabilities are sent in an
/// event sequence after the zwp_tablet_seat_v2.tool_added event before any
/// actual events from this tool. This initial event sequence is
/// terminated by a zwp_tablet_tool_v2.done event.
///
/// Tablet tool events are grouped by zwp_tablet_tool_v2.frame events.
/// Any events received before a zwp_tablet_tool_v2.frame event should be
/// considered part of the same hardware state change.
pub mod zwp_tablet_tool_v2 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_tablet_tool_v2::ZwpTabletToolV2 {
        const INTERFACE_NAME: &'static str = "zwp_tablet_tool_v2";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ZwpTabletToolV2Id {
        type Object = crate::wayland_objects::zwp_tablet_tool_v2::ZwpTabletToolV2;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::zwp_tablet_tool_v2::ZwpTabletToolV2 {
        type Id = ZwpTabletToolV2Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Set the tablet tool's surface
        ///
        /// Sets the surface of the cursor used for this tool on the given
        /// tablet. This request only takes effect if the tool is in proximity
        /// of one of the requesting client's surfaces or the surface parameter
        /// is the current pointer surface. If there was a previous surface set
        /// with this request it is replaced. If surface is NULL, the cursor
        /// image is hidden.
        ///
        /// The parameters hotspot_x and hotspot_y define the position of the
        /// pointer surface relative to the pointer location. Its top-left corner
        /// is always at (x, y) - (hotspot_x, hotspot_y), where (x, y) are the
        /// coordinates of the pointer location, in surface-local coordinates.
        ///
        /// On surface.attach requests to the pointer surface, hotspot_x and
        /// hotspot_y are decremented by the x and y parameters passed to the
        /// request. Attach must be confirmed by wl_surface.commit as usual.
        ///
        /// The hotspot can also be updated by passing the currently set pointer
        /// surface to this request with new values for hotspot_x and hotspot_y.
        ///
        /// The current and pending input regions of the wl_surface are cleared,
        /// and wl_surface.set_input_region is ignored until the wl_surface is no
        /// longer used as the cursor. When the use as a cursor ends, the current
        /// and pending input regions become undefined, and the wl_surface is
        /// unmapped.
        ///
        /// This request gives the surface the role of a zwp_tablet_tool_v2 cursor. A
        /// surface may only ever be used as the cursor surface for one
        /// zwp_tablet_tool_v2. If the surface already has another role or has
        /// previously been used as cursor surface for a different tool, a
        /// protocol error is raised.
        SetCursor {
            /// Serial of the proximity_in event,
            serial: u32,
            surface: WlSurfaceId,
            /// Surface-local x coordinate,
            hotspot_x: i32,
            /// Surface-local y coordinate,
            hotspot_y: i32,
        },
        /// Destroy the tool object
        ///
        /// This destroys the client's resource for this tool object.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwp_tablet_tool_v2::ZwpTabletToolV2 {
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
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Tool type
        ///
        /// The tool type is the high-level type of the tool and usually decides
        /// the interaction expected from this tool.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_tool_v2.done event.
        Type {
            /// The physical tool type,
            tool_type: u32,
        },
        /// Unique hardware serial number of the tool
        ///
        /// If the physical tool can be identified by a unique 64-bit serial
        /// number, this event notifies the client of this serial number.
        ///
        /// If multiple tablets are available in the same seat and the tool is
        /// uniquely identifiable by the serial number, that tool may move
        /// between tablets.
        ///
        /// Otherwise, if the tool has no serial number and this event is
        /// missing, the tool is tied to the tablet it first comes into
        /// proximity with. Even if the physical tool is used on multiple
        /// tablets, separate zwp_tablet_tool_v2 objects will be created, one per
        /// tablet.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_tool_v2.done event.
        HardwareSerial {
            /// The unique serial number of the tool, most significant bits,
            hardware_serial_hi: u32,
            /// The unique serial number of the tool, least significant bits,
            hardware_serial_lo: u32,
        },
        /// Hardware id notification in Wacom's format
        ///
        /// This event notifies the client of a hardware id available on this tool.
        ///
        /// The hardware id is a device-specific 64-bit id that provides extra
        /// information about the tool in use, beyond the wl_tool.type
        /// enumeration. The format of the id is specific to tablets made by
        /// Wacom Inc. For example, the hardware id of a Wacom Grip
        /// Pen (a stylus) is 0x802.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_tool_v2.done event.
        HardwareIdWacom {
            /// The hardware id, most significant bits,
            hardware_id_hi: u32,
            /// The hardware id, least significant bits,
            hardware_id_lo: u32,
        },
        /// Tool capability notification
        ///
        /// This event notifies the client of any capabilities of this tool,
        /// beyond the main set of x/y axes and tip up/down detection.
        ///
        /// One event is sent for each extra capability available on this tool.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_tool_v2.done event.
        Capability {
            /// The capability,
            capability: u32,
        },
        /// Tool description events sequence complete
        ///
        /// This event signals the end of the initial burst of descriptive
        /// events. A client may consider the static description of the tool to
        /// be complete and finalize initialization of the tool.
        Done,
        /// Tool removed
        ///
        /// This event is sent when the tool is removed from the system and will
        /// send no further events. Should the physical tool come back into
        /// proximity later, a new zwp_tablet_tool_v2 object will be created.
        ///
        /// It is compositor-dependent when a tool is removed. A compositor may
        /// remove a tool on proximity out, tablet removal or any other reason.
        /// A compositor may also keep a tool alive until shutdown.
        ///
        /// If the tool is currently in proximity, a proximity_out event will be
        /// sent before the removed event. See zwp_tablet_tool_v2.proximity_out for
        /// the handling of any buttons logically down.
        ///
        /// When this event is received, the client must zwp_tablet_tool_v2.destroy
        /// the object.
        Removed,
        /// Proximity in event
        ///
        /// Notification that this tool is focused on a certain surface.
        ///
        /// This event can be received when the tool has moved from one surface to
        /// another, or when the tool has come back into proximity above the
        /// surface.
        ///
        /// If any button is logically down when the tool comes into proximity,
        /// the respective button event is sent after the proximity_in event but
        /// within the same frame as the proximity_in event.
        ProximityIn {
            serial: u32,
            /// The tablet the tool is in proximity of,
            tablet: ZwpTabletV2Id,
            /// The current surface the tablet tool is over,
            surface: WlSurfaceId,
        },
        /// Proximity out event
        ///
        /// Notification that this tool has either left proximity, or is no
        /// longer focused on a certain surface.
        ///
        /// When the tablet tool leaves proximity of the tablet, button release
        /// events are sent for each button that was held down at the time of
        /// leaving proximity. These events are sent before the proximity_out
        /// event but within the same zwp_tablet_v2.frame.
        ///
        /// If the tool stays within proximity of the tablet, but the focus
        /// changes from one surface to another, a button release event may not
        /// be sent until the button is actually released or the tool leaves the
        /// proximity of the tablet.
        ProximityOut,
        /// Tablet tool is making contact
        ///
        /// Sent whenever the tablet tool comes in contact with the surface of the
        /// tablet.
        ///
        /// If the tool is already in contact with the tablet when entering the
        /// input region, the client owning said region will receive a
        /// zwp_tablet_v2.proximity_in event, followed by a zwp_tablet_v2.down
        /// event and a zwp_tablet_v2.frame event.
        ///
        /// Note that this event describes logical contact, not physical
        /// contact. On some devices, a compositor may not consider a tool in
        /// logical contact until a minimum physical pressure threshold is
        /// exceeded.
        Down { serial: u32 },
        /// Tablet tool is no longer making contact
        ///
        /// Sent whenever the tablet tool stops making contact with the surface of
        /// the tablet, or when the tablet tool moves out of the input region
        /// and the compositor grab (if any) is dismissed.
        ///
        /// If the tablet tool moves out of the input region while in contact
        /// with the surface of the tablet and the compositor does not have an
        /// ongoing grab on the surface, the client owning said region will
        /// receive a zwp_tablet_v2.up event, followed by a zwp_tablet_v2.proximity_out
        /// event and a zwp_tablet_v2.frame event. If the compositor has an ongoing
        /// grab on this device, this event sequence is sent whenever the grab
        /// is dismissed in the future.
        ///
        /// Note that this event describes logical contact, not physical
        /// contact. On some devices, a compositor may not consider a tool out
        /// of logical contact until physical pressure falls below a specific
        /// threshold.
        Up,
        /// Motion event
        ///
        /// Sent whenever a tablet tool moves.
        Motion {
            /// Surface-local x coordinate,
            x: f32,
            /// Surface-local y coordinate,
            y: f32,
        },
        /// Pressure change event
        ///
        /// Sent whenever the pressure axis on a tool changes. The value of this
        /// event is normalized to a value between 0 and 65535.
        ///
        /// Note that pressure may be nonzero even when a tool is not in logical
        /// contact. See the down and up events for more details.
        Pressure {
            /// The current pressure value,
            pressure: u32,
        },
        /// Distance change event
        ///
        /// Sent whenever the distance axis on a tool changes. The value of this
        /// event is normalized to a value between 0 and 65535.
        ///
        /// Note that distance may be nonzero even when a tool is not in logical
        /// contact. See the down and up events for more details.
        Distance {
            /// The current distance value,
            distance: u32,
        },
        /// Tilt change event
        ///
        /// Sent whenever one or both of the tilt axes on a tool change. Each tilt
        /// value is in degrees, relative to the z-axis of the tablet.
        /// The angle is positive when the top of a tool tilts along the
        /// positive x or y axis.
        Tilt {
            /// The current value of the X tilt axis,
            tilt_x: f32,
            /// The current value of the Y tilt axis,
            tilt_y: f32,
        },
        /// Z-rotation change event
        ///
        /// Sent whenever the z-rotation axis on the tool changes. The
        /// rotation value is in degrees clockwise from the tool's
        /// logical neutral position.
        Rotation {
            /// The current rotation of the Z axis,
            degrees: f32,
        },
        /// Slider position change event
        ///
        /// Sent whenever the slider position on the tool changes. The
        /// value is normalized between -65535 and 65535, with 0 as the logical
        /// neutral position of the slider.
        ///
        /// The slider is available on e.g. the Wacom Airbrush tool.
        Slider {
            /// The current position of slider,
            position: i32,
        },
        /// Wheel delta event
        ///
        /// Sent whenever the wheel on the tool emits an event. This event
        /// contains two values for the same axis change. The degrees value is
        /// in the same orientation as the wl_pointer.vertical_scroll axis. The
        /// clicks value is in discrete logical clicks of the mouse wheel. This
        /// value may be zero if the movement of the wheel was less
        /// than one logical click.
        ///
        /// Clients should choose either value and avoid mixing degrees and
        /// clicks. The compositor may accumulate values smaller than a logical
        /// click and emulate click events when a certain threshold is met.
        /// Thus, zwp_tablet_tool_v2.wheel events with non-zero clicks values may
        /// have different degrees values.
        Wheel {
            /// The wheel delta in degrees,
            degrees: f32,
            /// The wheel delta in discrete clicks,
            clicks: i32,
        },
        /// Button event
        ///
        /// Sent whenever a button on the tool is pressed or released.
        ///
        /// If a button is held down when the tool moves in or out of proximity,
        /// button events are generated by the compositor. See
        /// zwp_tablet_tool_v2.proximity_in and zwp_tablet_tool_v2.proximity_out for
        /// details.
        Button {
            serial: u32,
            /// The button whose state has changed,
            button: u32,
            /// Whether the button was pressed or released,
            state: u32,
        },
        /// Frame event
        ///
        /// Marks the end of a series of axis and/or button updates from the
        /// tablet. The Wayland protocol requires axis updates to be sent
        /// sequentially, however all events within a frame should be considered
        /// one hardware event.
        Frame {
            /// The time of the event with millisecond granularity,
            time: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::zwp_tablet_tool_v2::ZwpTabletToolV2 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Type { tool_type } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(tool_type);
                    raw
                }
                HardwareSerial { hardware_serial_hi, hardware_serial_lo } => {
                    let mut raw = RawEvent::new(self.id(), 1, 2);
                    raw.uint(hardware_serial_hi);
                    raw.uint(hardware_serial_lo);
                    raw
                }
                HardwareIdWacom { hardware_id_hi, hardware_id_lo } => {
                    let mut raw = RawEvent::new(self.id(), 2, 2);
                    raw.uint(hardware_id_hi);
                    raw.uint(hardware_id_lo);
                    raw
                }
                Capability { capability } => {
                    let mut raw = RawEvent::new(self.id(), 3, 1);
                    raw.uint(capability);
                    raw
                }
                Done {} => {
                    let mut raw = RawEvent::new(self.id(), 4, 0);
                    raw
                }
                Removed {} => {
                    let mut raw = RawEvent::new(self.id(), 5, 0);
                    raw
                }
                ProximityIn { serial, tablet, surface } => {
                    let mut raw = RawEvent::new(self.id(), 6, 3);
                    raw.uint(serial);
                    raw.id(tablet);
                    raw.id(surface);
                    raw
                }
                ProximityOut {} => {
                    let mut raw = RawEvent::new(self.id(), 7, 0);
                    raw
                }
                Down { serial } => {
                    let mut raw = RawEvent::new(self.id(), 8, 1);
                    raw.uint(serial);
                    raw
                }
                Up {} => {
                    let mut raw = RawEvent::new(self.id(), 9, 0);
                    raw
                }
                Motion { x, y } => {
                    let mut raw = RawEvent::new(self.id(), 10, 2);
                    raw.fixed(x);
                    raw.fixed(y);
                    raw
                }
                Pressure { pressure } => {
                    let mut raw = RawEvent::new(self.id(), 11, 1);
                    raw.uint(pressure);
                    raw
                }
                Distance { distance } => {
                    let mut raw = RawEvent::new(self.id(), 12, 1);
                    raw.uint(distance);
                    raw
                }
                Tilt { tilt_x, tilt_y } => {
                    let mut raw = RawEvent::new(self.id(), 13, 2);
                    raw.fixed(tilt_x);
                    raw.fixed(tilt_y);
                    raw
                }
                Rotation { degrees } => {
                    let mut raw = RawEvent::new(self.id(), 14, 1);
                    raw.fixed(degrees);
                    raw
                }
                Slider { position } => {
                    let mut raw = RawEvent::new(self.id(), 15, 1);
                    raw.int(position);
                    raw
                }
                Wheel { degrees, clicks } => {
                    let mut raw = RawEvent::new(self.id(), 16, 2);
                    raw.fixed(degrees);
                    raw.int(clicks);
                    raw
                }
                Button { serial, button, state } => {
                    let mut raw = RawEvent::new(self.id(), 17, 3);
                    raw.uint(serial);
                    raw.uint(button);
                    raw.uint(state);
                    raw
                }
                Frame { time } => {
                    let mut raw = RawEvent::new(self.id(), 18, 1);
                    raw.uint(time);
                    raw
                }
            }
        }
    }
    /// A physical tool type
    ///
    /// Describes the physical type of a tool. The physical type of a tool
    /// generally defines its base usage.
    ///
    /// The mouse tool represents a mouse-shaped tool that is not a relative
    /// device but bound to the tablet's surface, providing absolute
    /// coordinates.
    ///
    /// The lens tool is a mouse-shaped tool with an attached lens to
    /// provide precision focus.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Type {
        /// Pen,
        Pen = 320,
        /// Eraser,
        Eraser = 321,
        /// Brush,
        Brush = 322,
        /// Pencil,
        Pencil = 323,
        /// Airbrush,
        Airbrush = 324,
        /// Finger,
        Finger = 325,
        /// Mouse,
        Mouse = 326,
        /// Lens,
        Lens = 327,
    }
    /// Capability flags for a tool
    ///
    /// Describes extra capabilities on a tablet.
    ///
    /// Any tool must provide x and y values, extra axes are
    /// device-specific.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Capability {
        /// Tilt axes,
        Tilt = 1,
        /// Pressure axis,
        Pressure = 2,
        /// Distance axis,
        Distance = 3,
        /// Z-rotation axis,
        Rotation = 4,
        /// Slider axis,
        Slider = 5,
        /// Wheel axis,
        Wheel = 6,
    }
    /// Physical button state
    ///
    /// Describes the physical state of a button that produced the button event.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum ButtonState {
        /// Button is not pressed,
        Released = 0,
        /// Button is pressed,
        Pressed = 1,
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Given wl_surface has another role,
        Role = 0,
    }
}

/// Graphics tablet device
///
/// The zwp_tablet_v2 interface represents one graphics tablet device. The
/// tablet interface itself does not generate events; all events are
/// generated by zwp_tablet_tool_v2 objects when in proximity above a tablet.
///
/// A tablet has a number of static characteristics, e.g. device name and
/// pid/vid. These capabilities are sent in an event sequence after the
/// zwp_tablet_seat_v2.tablet_added event. This initial event sequence is
/// terminated by a zwp_tablet_v2.done event.
pub mod zwp_tablet_v2 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_tablet_v2::ZwpTabletV2 {
        const INTERFACE_NAME: &'static str = "zwp_tablet_v2";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ZwpTabletV2Id {
        type Object = crate::wayland_objects::zwp_tablet_v2::ZwpTabletV2;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::zwp_tablet_v2::ZwpTabletV2 {
        type Id = ZwpTabletV2Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the tablet object
        ///
        /// This destroys the client's resource for this tablet object.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwp_tablet_v2::ZwpTabletV2 {
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
    /// Minimum interface version for the `bustype` message (XML `since` / `version`).
    pub const BUSTYPE_SINCE: u32 = 2;
    #[derive(Debug)]
    pub enum Event {
        /// Tablet device name
        ///
        /// A descriptive name for the tablet device.
        ///
        /// If the device has no descriptive name, this event is not sent.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_v2.done event.
        Name {
            /// The device name,
            name: String,
        },
        /// Tablet device vendor/product id
        ///
        /// The vendor and product IDs for the tablet device.
        ///
        /// The interpretation of the id depends on the zwp_tablet_v2.bustype.
        /// Prior to version v2 of this protocol, the id was implied to be a USB
        /// vendor and product ID. If no zwp_tablet_v2.bustype is sent, the ID
        /// is to be interpreted as USB vendor and product ID.
        ///
        /// If the device has no vendor/product ID, this event is not sent.
        /// This can happen for virtual devices or non-USB devices, for instance.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_v2.done event.
        Id {
            /// Vendor id,
            vid: u32,
            /// Product id,
            pid: u32,
        },
        /// Path to the device
        ///
        /// A system-specific device path that indicates which device is behind
        /// this zwp_tablet_v2. This information may be used to gather additional
        /// information about the device, e.g. through libwacom.
        ///
        /// A device may have more than one device path. If so, multiple
        /// zwp_tablet_v2.path events are sent. A device may be emulated and not
        /// have a device path, and in that case this event will not be sent.
        ///
        /// The format of the path is unspecified, it may be a device node, a
        /// sysfs path, or some other identifier. It is up to the client to
        /// identify the string provided.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_v2.done event.
        Path {
            /// Path to local device,
            path: String,
        },
        /// Tablet description events sequence complete
        ///
        /// This event is sent immediately to signal the end of the initial
        /// burst of descriptive events. A client may consider the static
        /// description of the tablet to be complete and finalize initialization
        /// of the tablet.
        Done,
        /// Tablet removed event
        ///
        /// Sent when the tablet has been removed from the system. When a tablet
        /// is removed, some tools may be removed.
        ///
        /// When this event is received, the client must zwp_tablet_v2.destroy
        /// the object.
        Removed,
        /// Tablet device bus type
        ///
        /// The bustype argument is one of the BUS_ defines in the Linux kernel's
        /// linux/input.h
        ///
        /// If the device has no known bustype or the bustype cannot be
        /// queried, this event is not sent.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_v2.done event.
        Bustype {
            /// Bus type,
            bustype: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::zwp_tablet_v2::ZwpTabletV2 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Name { name } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.string(name);
                    raw
                }
                Id { vid, pid } => {
                    let mut raw = RawEvent::new(self.id(), 1, 2);
                    raw.uint(vid);
                    raw.uint(pid);
                    raw
                }
                Path { path } => {
                    let mut raw = RawEvent::new(self.id(), 2, 1);
                    raw.string(path);
                    raw
                }
                Done {} => {
                    let mut raw = RawEvent::new(self.id(), 3, 0);
                    raw
                }
                Removed {} => {
                    let mut raw = RawEvent::new(self.id(), 4, 0);
                    raw
                }
                Bustype { bustype } => {
                    let mut raw = RawEvent::new(self.id(), 5, 1);
                    raw.uint(bustype);
                    raw
                }
            }
        }
    }
    /// Bus type
    ///
    /// Describes the bus types this tablet is connected to.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Bustype {
        /// USB,
        Usb = 3,
        /// Bluetooth,
        Bluetooth = 5,
        /// Virtual,
        Virtual = 6,
        /// Serial,
        Serial = 17,
        /// I2C,
        I2c = 24,
    }
}

/// Pad ring
///
/// A circular interaction area, such as the touch ring on the Wacom Intuos
/// Pro series tablets.
///
/// Events on a ring are logically grouped by the zwp_tablet_pad_ring_v2.frame
/// event.
pub mod zwp_tablet_pad_ring_v2 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2 {
        const INTERFACE_NAME: &'static str = "zwp_tablet_pad_ring_v2";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ZwpTabletPadRingV2Id {
        type Object = crate::wayland_objects::zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2
    {
        type Id = ZwpTabletPadRingV2Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Set compositor feedback
        ///
        /// Request that the compositor use the provided feedback string
        /// associated with this ring. This request should be issued immediately
        /// after a zwp_tablet_pad_group_v2.mode_switch event from the corresponding
        /// group is received, or whenever the ring is mapped to a different
        /// action. See zwp_tablet_pad_group_v2.mode_switch for more details.
        ///
        /// Clients are encouraged to provide context-aware descriptions for
        /// the actions associated with the ring; compositors may use this
        /// information to offer visual feedback about the button layout
        /// (eg. on-screen displays).
        ///
        /// The provided string 'description' is a UTF-8 encoded string to be
        /// associated with this ring, and is considered user-visible; general
        /// internationalization rules apply.
        ///
        /// The serial argument will be that of the last
        /// zwp_tablet_pad_group_v2.mode_switch event received for the group of this
        /// ring. Requests providing other serials than the most recent one will be
        /// ignored.
        SetFeedback {
            /// Ring description,
            description: String,
            /// Serial of the mode switch event,
            serial: u32,
        },
        /// Destroy the ring object
        ///
        /// This destroys the client's resource for this ring object.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetFeedback { description: req.string()?, serial: req.uint()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Ring event source
        ///
        /// Source information for ring events.
        ///
        /// This event does not occur on its own. It is sent before a
        /// zwp_tablet_pad_ring_v2.frame event and carries the source information
        /// for all events within that frame.
        ///
        /// The source specifies how this event was generated. If the source is
        /// zwp_tablet_pad_ring_v2.source.finger, a zwp_tablet_pad_ring_v2.stop event
        /// will be sent when the user lifts the finger off the device.
        ///
        /// This event is optional. If the source is unknown for an interaction,
        /// no event is sent.
        Source {
            /// The event source,
            source: u32,
        },
        /// Angle changed
        ///
        /// Sent whenever the angle on a ring changes.
        ///
        /// The angle is provided in degrees clockwise from the logical
        /// north of the ring in the pad's current rotation.
        Angle {
            /// The current angle in degrees,
            degrees: f32,
        },
        /// Interaction stopped
        ///
        /// Stop notification for ring events.
        ///
        /// For some zwp_tablet_pad_ring_v2.source types, a zwp_tablet_pad_ring_v2.stop
        /// event is sent to notify a client that the interaction with the ring
        /// has terminated. This enables the client to implement kinetic scrolling.
        /// See the zwp_tablet_pad_ring_v2.source documentation for information on
        /// when this event may be generated.
        ///
        /// Any zwp_tablet_pad_ring_v2.angle events with the same source after this
        /// event should be considered as the start of a new interaction.
        Stop,
        /// End of a ring event sequence
        ///
        /// Indicates the end of a set of ring events that logically belong
        /// together. A client is expected to accumulate the data in all events
        /// within the frame before proceeding.
        ///
        /// All zwp_tablet_pad_ring_v2 events before a zwp_tablet_pad_ring_v2.frame event belong
        /// logically together. For example, on termination of a finger interaction
        /// on a ring the compositor will send a zwp_tablet_pad_ring_v2.source event,
        /// a zwp_tablet_pad_ring_v2.stop event and a zwp_tablet_pad_ring_v2.frame event.
        ///
        /// A zwp_tablet_pad_ring_v2.frame event is sent for every logical event
        /// group, even if the group only contains a single zwp_tablet_pad_ring_v2
        /// event. Specifically, a client may get a sequence: angle, frame,
        /// angle, frame, etc.
        Frame {
            /// Timestamp with millisecond granularity,
            time: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Source { source } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(source);
                    raw
                }
                Angle { degrees } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.fixed(degrees);
                    raw
                }
                Stop {} => {
                    let mut raw = RawEvent::new(self.id(), 2, 0);
                    raw
                }
                Frame { time } => {
                    let mut raw = RawEvent::new(self.id(), 3, 1);
                    raw.uint(time);
                    raw
                }
            }
        }
    }
    /// Ring axis source
    ///
    /// Describes the source types for ring events. This indicates to the
    /// client how a ring event was physically generated; a client may
    /// adjust the user interface accordingly. For example, events
    /// from a "finger" source may trigger kinetic scrolling.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Source {
        /// Finger,
        Finger = 1,
    }
}

/// Pad strip
///
/// A linear interaction area, such as the strips found in Wacom Cintiq
/// models.
///
/// Events on a strip are logically grouped by the zwp_tablet_pad_strip_v2.frame
/// event.
pub mod zwp_tablet_pad_strip_v2 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2 {
        const INTERFACE_NAME: &'static str = "zwp_tablet_pad_strip_v2";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ZwpTabletPadStripV2Id {
        type Object = crate::wayland_objects::zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2
    {
        type Id = ZwpTabletPadStripV2Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Set compositor feedback
        ///
        /// Requests the compositor to use the provided feedback string
        /// associated with this strip. This request should be issued immediately
        /// after a zwp_tablet_pad_group_v2.mode_switch event from the corresponding
        /// group is received, or whenever the strip is mapped to a different
        /// action. See zwp_tablet_pad_group_v2.mode_switch for more details.
        ///
        /// Clients are encouraged to provide context-aware descriptions for
        /// the actions associated with the strip, and compositors may use this
        /// information to offer visual feedback about the button layout
        /// (eg. on-screen displays).
        ///
        /// The provided string 'description' is a UTF-8 encoded string to be
        /// associated with this ring, and is considered user-visible; general
        /// internationalization rules apply.
        ///
        /// The serial argument will be that of the last
        /// zwp_tablet_pad_group_v2.mode_switch event received for the group of this
        /// strip. Requests providing other serials than the most recent one will be
        /// ignored.
        SetFeedback {
            /// Strip description,
            description: String,
            /// Serial of the mode switch event,
            serial: u32,
        },
        /// Destroy the strip object
        ///
        /// This destroys the client's resource for this strip object.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetFeedback { description: req.string()?, serial: req.uint()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Strip event source
        ///
        /// Source information for strip events.
        ///
        /// This event does not occur on its own. It is sent before a
        /// zwp_tablet_pad_strip_v2.frame event and carries the source information
        /// for all events within that frame.
        ///
        /// The source specifies how this event was generated. If the source is
        /// zwp_tablet_pad_strip_v2.source.finger, a zwp_tablet_pad_strip_v2.stop event
        /// will be sent when the user lifts their finger off the device.
        ///
        /// This event is optional. If the source is unknown for an interaction,
        /// no event is sent.
        Source {
            /// The event source,
            source: u32,
        },
        /// Position changed
        ///
        /// Sent whenever the position on a strip changes.
        ///
        /// The position is normalized to a range of [0, 65535], the 0-value
        /// represents the top-most and/or left-most position of the strip in
        /// the pad's current rotation.
        Position {
            /// The current position,
            position: u32,
        },
        /// Interaction stopped
        ///
        /// Stop notification for strip events.
        ///
        /// For some zwp_tablet_pad_strip_v2.source types, a zwp_tablet_pad_strip_v2.stop
        /// event is sent to notify a client that the interaction with the strip
        /// has terminated. This enables the client to implement kinetic
        /// scrolling. See the zwp_tablet_pad_strip_v2.source documentation for
        /// information on when this event may be generated.
        ///
        /// Any zwp_tablet_pad_strip_v2.position events with the same source after this
        /// event should be considered as the start of a new interaction.
        Stop,
        /// End of a strip event sequence
        ///
        /// Indicates the end of a set of events that represent one logical
        /// hardware strip event. A client is expected to accumulate the data
        /// in all events within the frame before proceeding.
        ///
        /// All zwp_tablet_pad_strip_v2 events before a zwp_tablet_pad_strip_v2.frame event belong
        /// logically together. For example, on termination of a finger interaction
        /// on a strip the compositor will send a zwp_tablet_pad_strip_v2.source event,
        /// a zwp_tablet_pad_strip_v2.stop event and a zwp_tablet_pad_strip_v2.frame
        /// event.
        ///
        /// A zwp_tablet_pad_strip_v2.frame event is sent for every logical event
        /// group, even if the group only contains a single zwp_tablet_pad_strip_v2
        /// event. Specifically, a client may get a sequence: position, frame,
        /// position, frame, etc.
        Frame {
            /// Timestamp with millisecond granularity,
            time: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Source { source } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(source);
                    raw
                }
                Position { position } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.uint(position);
                    raw
                }
                Stop {} => {
                    let mut raw = RawEvent::new(self.id(), 2, 0);
                    raw
                }
                Frame { time } => {
                    let mut raw = RawEvent::new(self.id(), 3, 1);
                    raw.uint(time);
                    raw
                }
            }
        }
    }
    /// Strip axis source
    ///
    /// Describes the source types for strip events. This indicates to the
    /// client how a strip event was physically generated; a client may
    /// adjust the user interface accordingly. For example, events
    /// from a "finger" source may trigger kinetic scrolling.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Source {
        /// Finger,
        Finger = 1,
    }
}

/// A set of buttons, rings and strips
///
/// A pad group describes a distinct (sub)set of buttons, rings and strips
/// present in the tablet. The criteria of this grouping is usually positional,
/// eg. if a tablet has buttons on the left and right side, 2 groups will be
/// presented. The physical arrangement of groups is undisclosed and may
/// change on the fly.
///
/// Pad groups will announce their features during pad initialization. Between
/// the corresponding zwp_tablet_pad_v2.group event and zwp_tablet_pad_group_v2.done, the
/// pad group will announce the buttons, rings and strips contained in it,
/// plus the number of supported modes.
///
/// Modes are a mechanism to allow multiple groups of actions for every element
/// in the pad group. The number of groups and available modes in each is
/// persistent across device plugs. The current mode is user-switchable, it
/// will be announced through the zwp_tablet_pad_group_v2.mode_switch event both
/// whenever it is switched, and after zwp_tablet_pad_v2.enter.
///
/// The current mode logically applies to all elements in the pad group,
/// although it is at clients' discretion whether to actually perform different
/// actions, and/or issue the respective .set_feedback requests to notify the
/// compositor. See the zwp_tablet_pad_group_v2.mode_switch event for more details.
pub mod zwp_tablet_pad_group_v2 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_tablet_pad_group_v2::ZwpTabletPadGroupV2 {
        const INTERFACE_NAME: &'static str = "zwp_tablet_pad_group_v2";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ZwpTabletPadGroupV2Id {
        type Object = crate::wayland_objects::zwp_tablet_pad_group_v2::ZwpTabletPadGroupV2;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_tablet_pad_group_v2::ZwpTabletPadGroupV2
    {
        type Id = ZwpTabletPadGroupV2Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the pad object
        ///
        /// Destroy the zwp_tablet_pad_group_v2 object. Objects created from this object
        /// are unaffected and should be destroyed separately.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwp_tablet_pad_group_v2::ZwpTabletPadGroupV2 {
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
    /// Minimum interface version for the `dial` message (XML `since` / `version`).
    pub const DIAL_SINCE: u32 = 2;
    #[derive(Debug)]
    pub enum Event {
        /// Buttons announced
        ///
        /// Sent on zwp_tablet_pad_group_v2 initialization to announce the available
        /// buttons in the group. Button indices start at 0, a button may only be
        /// in one group at a time.
        ///
        /// This event is first sent in the initial burst of events before the
        /// zwp_tablet_pad_group_v2.done event.
        ///
        /// Some buttons are reserved by the compositor. These buttons may not be
        /// assigned to any zwp_tablet_pad_group_v2. Compositors may broadcast this
        /// event in the case of changes to the mapping of these reserved buttons.
        /// If the compositor happens to reserve all buttons in a group, this event
        /// will be sent with an empty array.
        Buttons {
            /// Buttons in this group,
            buttons: Array,
        },
        /// Ring announced
        ///
        /// Sent on zwp_tablet_pad_group_v2 initialization to announce available rings.
        /// One event is sent for each ring available on this pad group.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_pad_group_v2.done event.
        Ring { ring: ZwpTabletPadRingV2Id },
        /// Strip announced
        ///
        /// Sent on zwp_tablet_pad_v2 initialization to announce available strips.
        /// One event is sent for each strip available on this pad group.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_pad_group_v2.done event.
        Strip { strip: ZwpTabletPadStripV2Id },
        /// Mode-switch ability announced
        ///
        /// Sent on zwp_tablet_pad_group_v2 initialization to announce that the pad
        /// group may switch between modes. A client may use a mode to store a
        /// specific configuration for buttons, rings and strips and use the
        /// zwp_tablet_pad_group_v2.mode_switch event to toggle between these
        /// configurations. Mode indices start at 0.
        ///
        /// Switching modes is compositor-dependent. See the
        /// zwp_tablet_pad_group_v2.mode_switch event for more details.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_pad_group_v2.done event. This event is only sent when
        /// more than one mode is available.
        Modes {
            /// The number of modes,
            modes: u32,
        },
        /// Tablet group description events sequence complete
        ///
        /// This event is sent immediately to signal the end of the initial
        /// burst of descriptive events. A client may consider the static
        /// description of the tablet to be complete and finalize initialization
        /// of the tablet group.
        Done,
        /// Mode switch event
        ///
        /// Notification that the mode was switched.
        ///
        /// A mode applies to all buttons, rings, strips and dials in a group
        /// simultaneously, but a client is not required to assign different actions
        /// for each mode. For example, a client may have mode-specific button
        /// mappings but map the ring to vertical scrolling in all modes. Mode
        /// indices start at 0.
        ///
        /// Switching modes is compositor-dependent. The compositor may provide
        /// visual cues to the user about the mode, e.g. by toggling LEDs on
        /// the tablet device. Mode-switching may be software-controlled or
        /// controlled by one or more physical buttons. For example, on a Wacom
        /// Intuos Pro, the button inside the ring may be assigned to switch
        /// between modes.
        ///
        /// The compositor will also send this event after zwp_tablet_pad_v2.enter on
        /// each group in order to notify of the current mode. Groups that only
        /// feature one mode will use mode=0 when emitting this event.
        ///
        /// If a button action in the new mode differs from the action in the
        /// previous mode, the client should immediately issue a
        /// zwp_tablet_pad_v2.set_feedback request for each changed button.
        ///
        /// If a ring, strip or dial action in the new mode differs from the action
        /// in the previous mode, the client should immediately issue a
        /// zwp_tablet_ring_v2.set_feedback, zwp_tablet_strip_v2.set_feedback or
        /// zwp_tablet_dial_v2.set_feedback request for each changed ring, strip or dial.
        ModeSwitch {
            /// The time of the event with millisecond granularity,
            time: u32,
            serial: u32,
            /// The new mode of the pad,
            mode: u32,
        },
        /// Dial announced
        ///
        /// Sent on zwp_tablet_pad_v2 initialization to announce available dials.
        /// One event is sent for each dial available on this pad group.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_pad_group_v2.done event.
        Dial { dial: ZwpTabletPadDialV2Id },
    }
    impl EventWriter for crate::wayland_objects::zwp_tablet_pad_group_v2::ZwpTabletPadGroupV2 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Buttons { buttons } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.array(buttons);
                    raw
                }
                Ring { ring } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.id(ring);
                    raw
                }
                Strip { strip } => {
                    let mut raw = RawEvent::new(self.id(), 2, 1);
                    raw.id(strip);
                    raw
                }
                Modes { modes } => {
                    let mut raw = RawEvent::new(self.id(), 3, 1);
                    raw.uint(modes);
                    raw
                }
                Done {} => {
                    let mut raw = RawEvent::new(self.id(), 4, 0);
                    raw
                }
                ModeSwitch { time, serial, mode } => {
                    let mut raw = RawEvent::new(self.id(), 5, 3);
                    raw.uint(time);
                    raw.uint(serial);
                    raw.uint(mode);
                    raw
                }
                Dial { dial } => {
                    let mut raw = RawEvent::new(self.id(), 6, 1);
                    raw.id(dial);
                    raw
                }
            }
        }
    }
}

/// A set of buttons, rings, strips and dials
///
/// A pad device is a set of buttons, rings, strips and dials
/// usually physically present on the tablet device itself. Some
/// exceptions exist where the pad device is physically detached, e.g. the
/// Wacom ExpressKey Remote.
///
/// Pad devices have no axes that control the cursor and are generally
/// auxiliary devices to the tool devices used on the tablet surface.
///
/// A pad device has a number of static characteristics, e.g. the number
/// of rings. These capabilities are sent in an event sequence after the
/// zwp_tablet_seat_v2.pad_added event before any actual events from this pad.
/// This initial event sequence is terminated by a zwp_tablet_pad_v2.done
/// event.
///
/// All pad features (buttons, rings, strips and dials) are logically divided into
/// groups and all pads have at least one group. The available groups are
/// notified through the zwp_tablet_pad_v2.group event; the compositor will
/// emit one event per group before emitting zwp_tablet_pad_v2.done.
///
/// Groups may have multiple modes. Modes allow clients to map multiple
/// actions to a single pad feature. Only one mode can be active per group,
/// although different groups may have different active modes.
pub mod zwp_tablet_pad_v2 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_tablet_pad_v2::ZwpTabletPadV2 {
        const INTERFACE_NAME: &'static str = "zwp_tablet_pad_v2";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ZwpTabletPadV2Id {
        type Object = crate::wayland_objects::zwp_tablet_pad_v2::ZwpTabletPadV2;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::zwp_tablet_pad_v2::ZwpTabletPadV2 {
        type Id = ZwpTabletPadV2Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Set compositor feedback
        ///
        /// Requests the compositor to use the provided feedback string
        /// associated with this button. This request should be issued immediately
        /// after a zwp_tablet_pad_group_v2.mode_switch event from the corresponding
        /// group is received, or whenever a button is mapped to a different
        /// action. See zwp_tablet_pad_group_v2.mode_switch for more details.
        ///
        /// Clients are encouraged to provide context-aware descriptions for
        /// the actions associated with each button, and compositors may use
        /// this information to offer visual feedback on the button layout
        /// (e.g. on-screen displays).
        ///
        /// Button indices start at 0. Setting the feedback string on a button
        /// that is reserved by the compositor (i.e. not belonging to any
        /// zwp_tablet_pad_group_v2) does not generate an error but the compositor
        /// is free to ignore the request.
        ///
        /// The provided string 'description' is a UTF-8 encoded string to be
        /// associated with this ring, and is considered user-visible; general
        /// internationalization rules apply.
        ///
        /// The serial argument will be that of the last
        /// zwp_tablet_pad_group_v2.mode_switch event received for the group of this
        /// button. Requests providing other serials than the most recent one will
        /// be ignored.
        SetFeedback {
            /// Button index,
            button: u32,
            /// Button description,
            description: String,
            /// Serial of the mode switch event,
            serial: u32,
        },
        /// Destroy the pad object
        ///
        /// Destroy the zwp_tablet_pad_v2 object. Objects created from this object
        /// are unaffected and should be destroyed separately.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwp_tablet_pad_v2::ZwpTabletPadV2 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetFeedback {
                    button: req.uint()?,
                    description: req.string()?,
                    serial: req.uint()?,
                },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Group announced
        ///
        /// Sent on zwp_tablet_pad_v2 initialization to announce available groups.
        /// One event is sent for each pad group available.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_pad_v2.done event. At least one group will be announced.
        Group { pad_group: ZwpTabletPadGroupV2Id },
        /// Path to the device
        ///
        /// A system-specific device path that indicates which device is behind
        /// this zwp_tablet_pad_v2. This information may be used to gather additional
        /// information about the device, e.g. through libwacom.
        ///
        /// The format of the path is unspecified, it may be a device node, a
        /// sysfs path, or some other identifier. It is up to the client to
        /// identify the string provided.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_pad_v2.done event.
        Path {
            /// Path to local device,
            path: String,
        },
        /// Buttons announced
        ///
        /// Sent on zwp_tablet_pad_v2 initialization to announce the available
        /// buttons.
        ///
        /// This event is sent in the initial burst of events before the
        /// zwp_tablet_pad_v2.done event. This event is only sent when at least one
        /// button is available.
        Buttons {
            /// The number of buttons,
            buttons: u32,
        },
        /// Pad description event sequence complete
        ///
        /// This event signals the end of the initial burst of descriptive
        /// events. A client may consider the static description of the pad to
        /// be complete and finalize initialization of the pad.
        Done,
        /// Physical button state
        ///
        /// Sent whenever the physical state of a button changes.
        Button {
            /// The time of the event with millisecond granularity,
            time: u32,
            /// The index of the button that changed state,
            button: u32,
            state: u32,
        },
        /// Enter event
        ///
        /// Notification that this pad is focused on the specified surface.
        Enter {
            /// Serial number of the enter event,
            serial: u32,
            /// The tablet the pad is attached to,
            tablet: ZwpTabletV2Id,
            /// Surface the pad is focused on,
            surface: WlSurfaceId,
        },
        /// Leave event
        ///
        /// Notification that this pad is no longer focused on the specified
        /// surface.
        Leave {
            /// Serial number of the leave event,
            serial: u32,
            /// Surface the pad is no longer focused on,
            surface: WlSurfaceId,
        },
        /// Pad removed event
        ///
        /// Sent when the pad has been removed from the system. When a tablet
        /// is removed its pad(s) will be removed too.
        ///
        /// When this event is received, the client must destroy all rings, strips
        /// and groups that were offered by this pad, and issue zwp_tablet_pad_v2.destroy
        /// the pad itself.
        Removed,
    }
    impl EventWriter for crate::wayland_objects::zwp_tablet_pad_v2::ZwpTabletPadV2 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Group { pad_group } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.id(pad_group);
                    raw
                }
                Path { path } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.string(path);
                    raw
                }
                Buttons { buttons } => {
                    let mut raw = RawEvent::new(self.id(), 2, 1);
                    raw.uint(buttons);
                    raw
                }
                Done {} => {
                    let mut raw = RawEvent::new(self.id(), 3, 0);
                    raw
                }
                Button { time, button, state } => {
                    let mut raw = RawEvent::new(self.id(), 4, 3);
                    raw.uint(time);
                    raw.uint(button);
                    raw.uint(state);
                    raw
                }
                Enter { serial, tablet, surface } => {
                    let mut raw = RawEvent::new(self.id(), 5, 3);
                    raw.uint(serial);
                    raw.id(tablet);
                    raw.id(surface);
                    raw
                }
                Leave { serial, surface } => {
                    let mut raw = RawEvent::new(self.id(), 6, 2);
                    raw.uint(serial);
                    raw.id(surface);
                    raw
                }
                Removed {} => {
                    let mut raw = RawEvent::new(self.id(), 7, 0);
                    raw
                }
            }
        }
    }
    /// Physical button state
    ///
    /// Describes the physical state of a button that caused the button
    /// event.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum ButtonState {
        /// The button is not pressed,
        Released = 0,
        /// The button is pressed,
        Pressed = 1,
    }
}

/// Pad dial
///
/// A rotary control, e.g. a dial or a wheel.
///
/// Events on a dial are logically grouped by the zwp_tablet_pad_dial_v2.frame
/// event.
pub mod zwp_tablet_pad_dial_v2 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_tablet_pad_dial_v2::ZwpTabletPadDialV2 {
        const INTERFACE_NAME: &'static str = "zwp_tablet_pad_dial_v2";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ZwpTabletPadDialV2Id {
        type Object = crate::wayland_objects::zwp_tablet_pad_dial_v2::ZwpTabletPadDialV2;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_tablet_pad_dial_v2::ZwpTabletPadDialV2
    {
        type Id = ZwpTabletPadDialV2Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Set compositor feedback
        ///
        /// Requests the compositor to use the provided feedback string
        /// associated with this dial. This request should be issued immediately
        /// after a zwp_tablet_pad_group_v2.mode_switch event from the corresponding
        /// group is received, or whenever the dial is mapped to a different
        /// action. See zwp_tablet_pad_group_v2.mode_switch for more details.
        ///
        /// Clients are encouraged to provide context-aware descriptions for
        /// the actions associated with the dial, and compositors may use this
        /// information to offer visual feedback about the button layout
        /// (eg. on-screen displays).
        ///
        /// The provided string 'description' is a UTF-8 encoded string to be
        /// associated with this ring, and is considered user-visible; general
        /// internationalization rules apply.
        ///
        /// The serial argument will be that of the last
        /// zwp_tablet_pad_group_v2.mode_switch event received for the group of this
        /// dial. Requests providing other serials than the most recent one will be
        /// ignored.
        SetFeedback {
            /// Dial description,
            description: String,
            /// Serial of the mode switch event,
            serial: u32,
        },
        /// Destroy the dial object
        ///
        /// This destroys the client's resource for this dial object.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwp_tablet_pad_dial_v2::ZwpTabletPadDialV2 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetFeedback { description: req.string()?, serial: req.uint()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Delta movement
        ///
        /// Sent whenever the position on a dial changes.
        ///
        /// This event carries the wheel delta as multiples or fractions
        /// of 120 with each multiple of 120 representing one logical wheel detent.
        /// For example, an axis_value120 of 30 is one quarter of
        /// a logical wheel step in the positive direction, a value120 of
        /// -240 are two logical wheel steps in the negative direction within the
        /// same hardware event. See the wl_pointer.axis_value120 for more details.
        ///
        /// The value120 must not be zero.
        Delta {
            /// Rotation distance as fraction of 120,
            value120: i32,
        },
        /// End of a dial event sequence
        ///
        /// Indicates the end of a set of events that represent one logical
        /// hardware dial event. A client is expected to accumulate the data
        /// in all events within the frame before proceeding.
        ///
        /// All zwp_tablet_pad_dial_v2 events before a zwp_tablet_pad_dial_v2.frame event belong
        /// logically together.
        ///
        /// A zwp_tablet_pad_dial_v2.frame event is sent for every logical event
        /// group, even if the group only contains a single zwp_tablet_pad_dial_v2
        /// event. Specifically, a client may get a sequence: delta, frame,
        /// delta, frame, etc.
        Frame {
            /// Timestamp with millisecond granularity,
            time: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::zwp_tablet_pad_dial_v2::ZwpTabletPadDialV2 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Delta { value120 } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.int(value120);
                    raw
                }
                Frame { time } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.uint(time);
                    raw
                }
            }
        }
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
    impl Constants for crate::wayland_objects::wp_presentation::WpPresentation {
        const INTERFACE_NAME: &'static str = "wp_presentation";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for WpPresentationId {
        type Object = crate::wayland_objects::wp_presentation::WpPresentation;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wp_presentation::WpPresentation {
        type Id = WpPresentationId;
    }
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
        /// The clock identifier is platform dependent. On POSIX platforms, the
        /// identifier value is one of the clockid_t values accepted by
        /// clock_gettime(). clock_gettime() is defined by POSIX.1-2001.
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::wp_presentation_feedback::WpPresentationFeedback {
        const INTERFACE_NAME: &'static str = "wp_presentation_feedback";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for WpPresentationFeedbackId {
        type Object = crate::wayland_objects::wp_presentation_feedback::WpPresentationFeedback;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_presentation_feedback::WpPresentationFeedback
    {
        type Id = WpPresentationFeedbackId;
    }
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
        /// For version 2 and later, if the output does not have a constant
        /// refresh rate, explicit video mode switches excluded, then the
        /// refresh argument must be either an appropriate rate picked by the
        /// compositor (e.g. fastest rate), or 0 if no such rate exists.
        /// For version 1, if the output does not have a constant refresh rate,
        /// the refresh argument must be zero.
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::wp_viewporter::WpViewporter {
        const INTERFACE_NAME: &'static str = "wp_viewporter";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpViewporterId {
        type Object = crate::wayland_objects::wp_viewporter::WpViewporter;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wp_viewporter::WpViewporter {
        type Id = WpViewporterId;
    }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
/// This state is double-buffered, see wl_surface.commit.
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
    impl Constants for crate::wayland_objects::wp_viewport::WpViewport {
        const INTERFACE_NAME: &'static str = "wp_viewport";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpViewportId {
        type Object = crate::wayland_objects::wp_viewport::WpViewport;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wp_viewport::WpViewport {
        type Id = WpViewportId;
    }
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
        /// The crop and scale state is double-buffered, see wl_surface.commit.
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
        /// The crop and scale state is double-buffered, see wl_surface.commit.
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::xdg_wm_base::XdgWmBase {
        const INTERFACE_NAME: &'static str = "xdg_wm_base";
        const INTERFACE_VERSION: u32 = 7;
    }
    impl crate::object::IdToObject for XdgWmBaseId {
        type Object = crate::wayland_objects::xdg_wm_base::XdgWmBase;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::xdg_wm_base::XdgWmBase {
        type Id = XdgWmBaseId;
    }
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
        /// This creates an xdg_surface for the given surface. An xdg_surface is
        /// used as basis to define a role to a given surface, such as xdg_toplevel
        /// or xdg_popup. It also manages functionality shared between xdg_surface
        /// based surface roles.
        ///
        /// While xdg_surface itself is not a role, the corresponding surface may
        /// only be assigned a role extending xdg_surface, such as xdg_toplevel or
        /// xdg_popup. It is illegal to create an xdg_surface for a wl_surface which
        /// already has anassigned role and this will result in a role error.
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::xdg_positioner::XdgPositioner {
        const INTERFACE_NAME: &'static str = "xdg_positioner";
        const INTERFACE_VERSION: u32 = 7;
    }
    impl crate::object::IdToObject for XdgPositionerId {
        type Object = crate::wayland_objects::xdg_positioner::XdgPositioner;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::xdg_positioner::XdgPositioner {
        type Id = XdgPositionerId;
    }
    /// Minimum interface version for the `set_reactive` message (XML `since` / `version`).
    pub const SET_REACTIVE_SINCE: u32 = 3;
    /// Minimum interface version for the `set_parent_size` message (XML `since` / `version`).
    pub const SET_PARENT_SIZE_SINCE: u32 = 3;
    /// Minimum interface version for the `set_parent_configure` message (XML `since` / `version`).
    pub const SET_PARENT_CONFIGURE_SINCE: u32 = 3;
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
        /// is used to derive an anchor point that the child surface will be
        /// positioned relative to. If a corner anchor is set (e.g. 'top_left' or
        /// 'bottom_right'), the anchor point will be at the specified corner;
        /// otherwise, the derived anchor point will be centered on the specified
        /// edge, or in the center of the anchor rectangle if no edge is specified.
        SetAnchor {
            /// Anchor point,
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
        /// Set parent size
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Invalid input provided,
        InvalidInput = 0,
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
/// After creating a role-specific object and setting it up (e.g. by sending
/// the title, app ID, size constraints, parent, etc), the client must
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
    impl Constants for crate::wayland_objects::xdg_surface::XdgSurface {
        const INTERFACE_NAME: &'static str = "xdg_surface";
        const INTERFACE_VERSION: u32 = 7;
    }
    impl crate::object::IdToObject for XdgSurfaceId {
        type Object = crate::wayland_objects::xdg_surface::XdgSurface;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::xdg_surface::XdgSurface {
        type Id = XdgSurfaceId;
    }
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
        GetPopup {
            id: XdgPopupId,
            /// Parent surface for this popup,
            parent: XdgSurfaceId,
            /// Positioner for this popup,
            positioner: XdgPositionerId,
        },
        /// Set the new window geometry
        ///
        /// The window geometry of a surface is its "visible bounds" from the
        /// user's perspective. Client-side decorations often have invisible
        /// portions like drop-shadows which should be ignored for the
        /// purposes of aligning, placing and constraining windows. Note that
        /// in some situations, compositors may clip rendering to the window
        /// geometry, so the client should avoid putting functional elements
        /// outside of it.
        ///
        /// The window geometry is double-buffered state, see wl_surface.commit.
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
        SetWindowGeometry {
            /// X coordinate of the top-left corner of the window inside this surface,
            x: i32,
            /// Y coordinate of the top-left corner of the window inside this surface,
            y: i32,
            /// Width of the window,
            width: i32,
            /// Height of the window,
            height: i32,
        },
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
/// An xdg_toplevel by default is responsible for providing the full intended
/// visual representation of the toplevel, which depending on the window
/// state, may mean things like a title bar, window controls and drop shadow.
///
/// Unmapping an xdg_toplevel means that the surface cannot be shown
/// by the compositor until it is explicitly mapped again.
/// All active operations (e.g., move, resize) are canceled and all
/// attributes (e.g. title, state, stacking, ...) are discarded for
/// an xdg_toplevel surface when it is unmapped. The xdg_toplevel returns to
/// the state it had right after xdg_surface.get_toplevel. The client
/// can re-map the toplevel by performing a commit without any buffer
/// attached, waiting for a configure event and handling it as usual (see
/// xdg_surface description).
///
/// Attaching a null buffer to a toplevel unmaps the surface.
pub mod xdg_toplevel {
    use super::*;
    impl Constants for crate::wayland_objects::xdg_toplevel::XdgToplevel {
        const INTERFACE_NAME: &'static str = "xdg_toplevel";
        const INTERFACE_VERSION: u32 = 7;
    }
    impl crate::object::IdToObject for XdgToplevelId {
        type Object = crate::wayland_objects::xdg_toplevel::XdgToplevel;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::xdg_toplevel::XdgToplevel {
        type Id = XdgToplevelId;
    }
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
        SetParent {
            /// Parent surface for this surface,
            parent: XdgToplevelId,
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
            /// Title of the surface,
            title: String,
        },
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
        SetAppId {
            /// Application identifier surface belongs to,
            app_id: String,
        },
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
        /// Values set in this way are double-buffered, see wl_surface.commit.
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
        /// strictly negative values for width or height will result in an
        /// invalid_size error.
        SetMaxSize {
            /// Maximum width of the window,
            width: i32,
            /// Maximum height of the window,
            height: i32,
        },
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
        /// Values set in this way are double-buffered, see wl_surface.commit.
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
        /// strictly negative values for width and height will result in an
        /// invalid_size error.
        SetMinSize {
            /// Minimum width of the window,
            width: i32,
            /// Minimum height of the window,
            height: i32,
        },
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
        /// border fill covering the rest of the output. The content of the
        /// border fill is undefined, but should be assumed to be in some way that
        /// attempts to blend into the surrounding area (e.g. solid black).
        ///
        /// If the fullscreened surface is not opaque, the compositor must make
        /// sure that other screen content not part of the same surface tree (made
        /// up of subsurfaces, popups or similarly coupled surfaces) are not
        /// visible below the fullscreened surface.
        SetFullscreen {
            /// Preferred output to place surface on,
            output: WlOutputId,
        },
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
    /// Minimum interface version for the `configure_bounds` message (XML `since` / `version`).
    pub const CONFIGURE_BOUNDS_SINCE: u32 = 4;
    /// Minimum interface version for the `wm_capabilities` message (XML `since` / `version`).
    pub const WM_CAPABILITIES_SINCE: u32 = 5;
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
        /// The states are sent as an array of 32-bit unsigned integers in
        /// native endianness. State values are defined in the state enum.
        ///
        /// Clients must send an ack_configure in response to this event. See
        /// xdg_surface.configure and xdg_surface.ack_configure for details.
        Configure {
            /// Suggested width of window,
            width: i32,
            /// Suggested height of window,
            height: i32,
            /// Suggested states of the window,
            states: Array,
        },
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
        ConfigureBounds {
            /// Suggested maximum width of surface,
            width: i32,
            /// Suggested maximum height of surface,
            height: i32,
        },
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
        /// native endianness. Capability values are defined in the wm_capabilities enum.
        WmCapabilities {
            /// Array of 32-bit capabilities,
            capabilities: Array,
        },
    }
    impl EventWriter for crate::wayland_objects::xdg_toplevel::XdgToplevel {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    /// States set in this way are double-buffered, see wl_surface.commit.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
        ///
        /// The client should draw without shadow or other decoration outside of
        /// the window geometry on the left edge.
        TiledLeft = 5,
        /// The surface’s right edge is tiled
        ///
        /// The window is currently in a tiled layout and the right edge is
        /// considered to be adjacent to another part of the tiling grid.
        ///
        /// The client should draw without shadow or other decoration outside of
        /// the window geometry on the right edge.
        TiledRight = 6,
        /// The surface’s top edge is tiled
        ///
        /// The window is currently in a tiled layout and the top edge is
        /// considered to be adjacent to another part of the tiling grid.
        ///
        /// The client should draw without shadow or other decoration outside of
        /// the window geometry on the top edge.
        TiledTop = 7,
        /// The surface’s bottom edge is tiled
        ///
        /// The window is currently in a tiled layout and the bottom edge is
        /// considered to be adjacent to another part of the tiling grid.
        ///
        /// The client should draw without shadow or other decoration outside of
        /// the window geometry on the bottom edge.
        TiledBottom = 8,
        /// Surface repaint is suspended
        ///
        /// The surface is currently not ordinarily being repainted; for
        /// example because its content is occluded by another window, or its
        /// outputs are switched off due to screen locking.
        Suspended = 9,
        /// The surface’s left edge is constrained
        ///
        /// The left edge of the window is currently constrained, meaning it
        /// shouldn't attempt to resize from that edge. It can for example mean
        /// it's tiled next to a monitor edge on the constrained side of the
        /// window.
        ConstrainedLeft = 10,
        /// The surface’s right edge is constrained
        ///
        /// The right edge of the window is currently constrained, meaning it
        /// shouldn't attempt to resize from that edge. It can for example mean
        /// it's tiled next to a monitor edge on the constrained side of the
        /// window.
        ConstrainedRight = 11,
        /// The surface’s top edge is constrained
        ///
        /// The top edge of the window is currently constrained, meaning it
        /// shouldn't attempt to resize from that edge. It can for example mean
        /// it's tiled next to a monitor edge on the constrained side of the
        /// window.
        ConstrainedTop = 12,
        /// The surface’s bottom edge is constrained
        ///
        /// The bottom edge of the window is currently constrained, meaning it
        /// shouldn't attempt to resize from that edge. It can for example mean
        /// it's tiled next to a monitor edge on the constrained side of the
        /// window.
        ConstrainedBottom = 13,
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::xdg_popup::XdgPopup {
        const INTERFACE_NAME: &'static str = "xdg_popup";
        const INTERFACE_VERSION: u32 = 7;
    }
    impl crate::object::IdToObject for XdgPopupId {
        type Object = crate::wayland_objects::xdg_popup::XdgPopup;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::xdg_popup::XdgPopup {
        type Id = XdgPopupId;
    }
    /// Minimum interface version for the `reposition` message (XML `since` / `version`).
    pub const REPOSITION_SINCE: u32 = 3;
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
    /// Minimum interface version for the `repositioned` message (XML `since` / `version`).
    pub const REPOSITIONED_SINCE: u32 = 3;
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::wp_content_type_manager_v1::WpContentTypeManagerV1 {
        const INTERFACE_NAME: &'static str = "wp_content_type_manager_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpContentTypeManagerV1Id {
        type Object = crate::wayland_objects::wp_content_type_manager_v1::WpContentTypeManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_content_type_manager_v1::WpContentTypeManagerV1
    {
        type Id = WpContentTypeManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the content type manager object
        ///
        /// Destroy the content type manager. This doesn't destroy objects created
        /// with the manager.
        Destroy,
        /// Create a new content type object
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::wp_content_type_v1::WpContentTypeV1 {
        const INTERFACE_NAME: &'static str = "wp_content_type_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpContentTypeV1Id {
        type Object = crate::wayland_objects::wp_content_type_v1::WpContentTypeV1;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wp_content_type_v1::WpContentTypeV1 {
        type Id = WpContentTypeV1Id;
    }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
/// Rendering Manager (DRM) masters to lease DRM resources to Wayland
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
    impl Constants for crate::wayland_objects::wp_drm_lease_device_v1::WpDrmLeaseDeviceV1 {
        const INTERFACE_NAME: &'static str = "wp_drm_lease_device_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpDrmLeaseDeviceV1Id {
        type Object = crate::wayland_objects::wp_drm_lease_device_v1::WpDrmLeaseDeviceV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_drm_lease_device_v1::WpDrmLeaseDeviceV1
    {
        type Id = WpDrmLeaseDeviceV1Id;
    }
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    impl Constants for crate::wayland_objects::wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1 {
        const INTERFACE_NAME: &'static str = "wp_drm_lease_connector_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpDrmLeaseConnectorV1Id {
        type Object = crate::wayland_objects::wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1
    {
        type Id = WpDrmLeaseConnectorV1Id;
    }
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
        ///
        /// If a client holds a lease for the connector, the status of the lease
        /// remains the same. The client should destroy the object after receiving
        /// this event.
        Withdrawn,
    }
    impl EventWriter for crate::wayland_objects::wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    impl Constants for crate::wayland_objects::wp_drm_lease_request_v1::WpDrmLeaseRequestV1 {
        const INTERFACE_NAME: &'static str = "wp_drm_lease_request_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpDrmLeaseRequestV1Id {
        type Object = crate::wayland_objects::wp_drm_lease_request_v1::WpDrmLeaseRequestV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_drm_lease_request_v1::WpDrmLeaseRequestV1
    {
        type Id = WpDrmLeaseRequestV1Id;
    }
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
        /// object, issuing any more requests will cause a wl_display error.
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::wp_drm_lease_v1::WpDrmLeaseV1 {
        const INTERFACE_NAME: &'static str = "wp_drm_lease_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpDrmLeaseV1Id {
        type Object = crate::wayland_objects::wp_drm_lease_v1::WpDrmLeaseV1;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::wp_drm_lease_v1::WpDrmLeaseV1 {
        type Id = WpDrmLeaseV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroys the lease object
        ///
        /// The client should send this to indicate that it no longer wishes to use
        /// this lease. The compositor should use drmModeRevokeLease on the
        /// appropriate file descriptor, if necessary.
        ///
        /// Upon destruction, the compositor should advertise the connector for
        /// leasing again by sending the connector event through the
        /// wp_drm_lease_device_v1 interface.
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
        /// compositor loses DRM master. Compositors may advertise the connector
        /// for leasing again, if the resource is available, by sending the
        /// connector event through the wp_drm_lease_device_v1 interface.
        Finished,
    }
    impl EventWriter for crate::wayland_objects::wp_drm_lease_v1::WpDrmLeaseV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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

/// Manager to control data devices
///
/// This interface is a manager that allows creating per-seat data device
/// controls.
pub mod ext_data_control_manager_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::ext_data_control_manager_v1::ExtDataControlManagerV1 {
        const INTERFACE_NAME: &'static str = "ext_data_control_manager_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtDataControlManagerV1Id {
        type Object = crate::wayland_objects::ext_data_control_manager_v1::ExtDataControlManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_data_control_manager_v1::ExtDataControlManagerV1
    {
        type Id = ExtDataControlManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Create a new data source
        ///
        /// Create a new data source.
        CreateDataSource {
            /// Data source to create,
            id: ExtDataControlSourceV1Id,
        },
        /// Get a data device for a seat
        ///
        /// Create a data device that can be used to manage a seat's selection.
        GetDataDevice { id: ExtDataControlDeviceV1Id, seat: WlSeatId },
        /// Destroy the manager
        ///
        /// All objects created by the manager will still remain valid, until their
        /// appropriate destroy request has been called.
        Destroy,
    }
    impl Parser for crate::wayland_objects::ext_data_control_manager_v1::ExtDataControlManagerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => CreateDataSource { id: req.id()? },
                1 => GetDataDevice { id: req.id()?, seat: req.id()? },
                2 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// Manage a data device for a seat
///
/// This interface allows a client to manage a seat's selection.
///
/// When the seat is destroyed, this object becomes inert.
pub mod ext_data_control_device_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::ext_data_control_device_v1::ExtDataControlDeviceV1 {
        const INTERFACE_NAME: &'static str = "ext_data_control_device_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtDataControlDeviceV1Id {
        type Object = crate::wayland_objects::ext_data_control_device_v1::ExtDataControlDeviceV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_data_control_device_v1::ExtDataControlDeviceV1
    {
        type Id = ExtDataControlDeviceV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Copy data to the selection
        ///
        /// This request asks the compositor to set the selection to the data from
        /// the source on behalf of the client.
        ///
        /// The given source may not be used in any further set_selection or
        /// set_primary_selection requests. Attempting to use a previously used
        /// source triggers the used_source protocol error.
        ///
        /// To unset the selection, set the source to NULL.
        SetSelection { source: ExtDataControlSourceV1Id },
        /// Destroy this data device
        ///
        /// Destroys the data device object.
        Destroy,
        /// Copy data to the primary selection
        ///
        /// This request asks the compositor to set the primary selection to the
        /// data from the source on behalf of the client.
        ///
        /// The given source may not be used in any further set_selection or
        /// set_primary_selection requests. Attempting to use a previously used
        /// source triggers the used_source protocol error.
        ///
        /// To unset the primary selection, set the source to NULL.
        ///
        /// The compositor will ignore this request if it does not support primary
        /// selection.
        SetPrimarySelection { source: ExtDataControlSourceV1Id },
    }
    impl Parser for crate::wayland_objects::ext_data_control_device_v1::ExtDataControlDeviceV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetSelection { source: req.id()? },
                1 => Destroy,
                2 => SetPrimarySelection { source: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Introduce a new ext_data_control_offer
        ///
        /// The data_offer event introduces a new ext_data_control_offer object,
        /// which will subsequently be used in either the
        /// ext_data_control_device.selection event (for the regular clipboard
        /// selections) or the ext_data_control_device.primary_selection event (for
        /// the primary clipboard selections). Immediately following the
        /// ext_data_control_device.data_offer event, the new data_offer object
        /// will send out ext_data_control_offer.offer events to describe the MIME
        /// types it offers.
        DataOffer { id: ExtDataControlOfferV1Id },
        /// Advertise new selection
        ///
        /// The selection event is sent out to notify the client of a new
        /// ext_data_control_offer for the selection for this device. The
        /// ext_data_control_device.data_offer and the ext_data_control_offer.offer
        /// events are sent out immediately before this event to introduce the data
        /// offer object. The selection event is sent to a client when a new
        /// selection is set. The ext_data_control_offer is valid until a new
        /// ext_data_control_offer or NULL is received. The client must destroy the
        /// previous selection ext_data_control_offer, if any, upon receiving this
        /// event. Regardless, the previous selection will be ignored once a new
        /// selection ext_data_control_offer is received.
        ///
        /// The first selection event is sent upon binding the
        /// ext_data_control_device object.
        Selection { id: ExtDataControlOfferV1Id },
        /// This data control is no longer valid
        ///
        /// This data control object is no longer valid and should be destroyed by
        /// the client.
        Finished,
        /// Advertise new primary selection
        ///
        /// The primary_selection event is sent out to notify the client of a new
        /// ext_data_control_offer for the primary selection for this device. The
        /// ext_data_control_device.data_offer and the ext_data_control_offer.offer
        /// events are sent out immediately before this event to introduce the data
        /// offer object. The primary_selection event is sent to a client when a
        /// new primary selection is set. The ext_data_control_offer is valid until
        /// a new ext_data_control_offer or NULL is received. The client must
        /// destroy the previous primary selection ext_data_control_offer, if any,
        /// upon receiving this event. Regardless, the previous primary selection
        /// will be ignored once a new primary selection ext_data_control_offer is
        /// received.
        ///
        /// If the compositor supports primary selection, the first
        /// primary_selection event is sent upon binding the
        /// ext_data_control_device object.
        PrimarySelection { id: ExtDataControlOfferV1Id },
    }
    impl EventWriter for crate::wayland_objects::ext_data_control_device_v1::ExtDataControlDeviceV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                DataOffer { id } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.id(id);
                    raw
                }
                Selection { id } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.id(id);
                    raw
                }
                Finished {} => {
                    let mut raw = RawEvent::new(self.id(), 2, 0);
                    raw
                }
                PrimarySelection { id } => {
                    let mut raw = RawEvent::new(self.id(), 3, 1);
                    raw.id(id);
                    raw
                }
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Source given to set_selection or set_primary_selection was already used before,
        UsedSource = 1,
    }
}

/// Offer to transfer data
///
/// The ext_data_control_source object is the source side of a
/// ext_data_control_offer. It is created by the source client in a data
/// transfer and provides a way to describe the offered data and a way to
/// respond to requests to transfer the data.
pub mod ext_data_control_source_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::ext_data_control_source_v1::ExtDataControlSourceV1 {
        const INTERFACE_NAME: &'static str = "ext_data_control_source_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtDataControlSourceV1Id {
        type Object = crate::wayland_objects::ext_data_control_source_v1::ExtDataControlSourceV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_data_control_source_v1::ExtDataControlSourceV1
    {
        type Id = ExtDataControlSourceV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Add an offered MIME type
        ///
        /// This request adds a MIME type to the set of MIME types advertised to
        /// targets. Can be called several times to offer multiple types.
        ///
        /// Calling this after ext_data_control_device.set_selection is a protocol
        /// error.
        Offer {
            /// MIME type offered by the data source,
            mime_type: String,
        },
        /// Destroy this source
        ///
        /// Destroys the data source object.
        Destroy,
    }
    impl Parser for crate::wayland_objects::ext_data_control_source_v1::ExtDataControlSourceV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Offer { mime_type: req.string()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Send the data
        ///
        /// Request for data from the client. Send the data as the specified MIME
        /// type over the passed file descriptor, then close it.
        Send {
            /// MIME type for the data,
            mime_type: String,
            /// File descriptor for the data,
            fd: RawFd,
        },
        /// Selection was cancelled
        ///
        /// This data source is no longer valid. The data source has been replaced
        /// by another data source.
        ///
        /// The client should clean up and destroy this data source.
        Cancelled,
    }
    impl EventWriter for crate::wayland_objects::ext_data_control_source_v1::ExtDataControlSourceV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Send { mime_type, fd } => {
                    let mut raw = RawEvent::new(self.id(), 0, 2);
                    raw.string(mime_type);
                    raw.fd(fd);
                    raw
                }
                Cancelled {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Offer sent after ext_data_control_device.set_selection,
        InvalidOffer = 1,
    }
}

/// Offer to transfer data
///
/// A ext_data_control_offer represents a piece of data offered for transfer
/// by another client (the source client). The offer describes the different
/// MIME types that the data can be converted to and provides the mechanism
/// for transferring the data directly from the source client.
pub mod ext_data_control_offer_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::ext_data_control_offer_v1::ExtDataControlOfferV1 {
        const INTERFACE_NAME: &'static str = "ext_data_control_offer_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtDataControlOfferV1Id {
        type Object = crate::wayland_objects::ext_data_control_offer_v1::ExtDataControlOfferV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_data_control_offer_v1::ExtDataControlOfferV1
    {
        type Id = ExtDataControlOfferV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Request that the data is transferred
        ///
        /// To transfer the offered data, the client issues this request and
        /// indicates the MIME type it wants to receive. The transfer happens
        /// through the passed file descriptor (typically created with the pipe
        /// system call). The source client writes the data in the MIME type
        /// representation requested and then closes the file descriptor.
        ///
        /// The receiving client reads from the read end of the pipe until EOF and
        /// then closes its end, at which point the transfer is complete.
        ///
        /// This request may happen multiple times for different MIME types.
        Receive {
            /// MIME type desired by receiver,
            mime_type: String,
            /// File descriptor for data transfer,
            fd: RawFd,
        },
        /// Destroy this offer
        ///
        /// Destroys the data offer object.
        Destroy,
    }
    impl Parser for crate::wayland_objects::ext_data_control_offer_v1::ExtDataControlOfferV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Receive { mime_type: req.string()?, fd: req.fd()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Advertise offered MIME type
        ///
        /// Sent immediately after creating the ext_data_control_offer object.
        /// One event per offered MIME type.
        Offer {
            /// Offered MIME type,
            mime_type: String,
        },
    }
    impl EventWriter for crate::wayland_objects::ext_data_control_offer_v1::ExtDataControlOfferV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Offer { mime_type } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.string(mime_type);
                    raw
                }
            }
        }
    }
}

/// List toplevels
///
/// A toplevel is defined as a surface with a role similar to xdg_toplevel.
/// XWayland surfaces may be treated like toplevels in this protocol.
///
/// After a client binds the ext_foreign_toplevel_list_v1, each mapped
/// toplevel window will be sent using the ext_foreign_toplevel_list_v1.toplevel
/// event.
///
/// Clients which only care about the current state can perform a roundtrip after
/// binding this global.
///
/// For each instance of ext_foreign_toplevel_list_v1, the compositor must
/// create a new ext_foreign_toplevel_handle_v1 object for each mapped toplevel.
///
/// If a compositor implementation sends the ext_foreign_toplevel_list_v1.finished
/// event after the global is bound, the compositor must not send any
/// ext_foreign_toplevel_list_v1.toplevel events.
pub mod ext_foreign_toplevel_list_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1 {
        const INTERFACE_NAME: &'static str = "ext_foreign_toplevel_list_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtForeignToplevelListV1Id {
        type Object =
            crate::wayland_objects::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1
    {
        type Id = ExtForeignToplevelListV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Stop sending events
        ///
        /// This request indicates that the client no longer wishes to receive
        /// events for new toplevels.
        ///
        /// The Wayland protocol is asynchronous, meaning the compositor may send
        /// further toplevel events until the stop request is processed.
        /// The client should wait for a ext_foreign_toplevel_list_v1.finished
        /// event before destroying this object.
        Stop,
        /// Destroy the ext_foreign_toplevel_list_v1 object
        ///
        /// This request should be called either when the client will no longer
        /// use the ext_foreign_toplevel_list_v1 or after the finished event
        /// has been received to allow destruction of the object.
        ///
        /// If a client wishes to destroy this object it should send a
        /// ext_foreign_toplevel_list_v1.stop request and wait for a ext_foreign_toplevel_list_v1.finished
        /// event, then destroy the handles and then this object.
        Destroy,
    }
    impl Parser for crate::wayland_objects::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Stop,
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// A toplevel has been created
        ///
        /// This event is emitted whenever a new toplevel window is created. It is
        /// emitted for all toplevels, regardless of the app that has created them.
        ///
        /// All initial properties of the toplevel (identifier, title, app_id) will be sent
        /// immediately after this event using the corresponding events for
        /// ext_foreign_toplevel_handle_v1. The compositor will use the
        /// ext_foreign_toplevel_handle_v1.done event to indicate when all data has
        /// been sent.
        Toplevel { toplevel: ExtForeignToplevelHandleV1Id },
        /// The compositor has finished with the toplevel manager
        ///
        /// This event indicates that the compositor is done sending events
        /// to this object. The client should destroy the object.
        /// See ext_foreign_toplevel_list_v1.destroy for more information.
        ///
        /// The compositor must not send any more toplevel events after this event.
        Finished,
    }
    impl EventWriter
        for crate::wayland_objects::ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1
    {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Toplevel { toplevel } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.id(toplevel);
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

/// A mapped toplevel
///
/// A ext_foreign_toplevel_handle_v1 object represents a mapped toplevel
/// window. A single app may have multiple mapped toplevels.
pub mod ext_foreign_toplevel_handle_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1
    {
        const INTERFACE_NAME: &'static str = "ext_foreign_toplevel_handle_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtForeignToplevelHandleV1Id {
        type Object =
            crate::wayland_objects::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1
    {
        type Id = ExtForeignToplevelHandleV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the ext_foreign_toplevel_handle_v1 object
        ///
        /// This request should be used when the client will no longer use the handle
        /// or after the closed event has been received to allow destruction of the
        /// object.
        ///
        /// When a handle is destroyed, a new handle may not be created by the server
        /// until the toplevel is unmapped and then remapped. Destroying a toplevel handle
        /// is not recommended unless the client is cleaning up child objects
        /// before destroying the ext_foreign_toplevel_list_v1 object, the toplevel
        /// was closed or the toplevel handle will not be used in the future.
        ///
        /// Other protocols which extend the ext_foreign_toplevel_handle_v1
        /// interface should require destructors for extension interfaces be
        /// called before allowing the toplevel handle to be destroyed.
        Destroy,
    }
    impl Parser for crate::wayland_objects::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1 {
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
        /// The toplevel has been closed
        ///
        /// The server will emit no further events on the ext_foreign_toplevel_handle_v1
        /// after this event. Any requests received aside from the destroy request must
        /// be ignored. Upon receiving this event, the client should destroy the handle.
        ///
        /// Other protocols which extend the ext_foreign_toplevel_handle_v1
        /// interface must also ignore requests other than destructors.
        Closed,
        /// All information about the toplevel has been sent
        ///
        /// This event is sent after all changes in the toplevel state have
        /// been sent.
        ///
        /// This allows changes to the ext_foreign_toplevel_handle_v1 properties
        /// to be atomically applied. Other protocols which extend the
        /// ext_foreign_toplevel_handle_v1 interface may use this event to also
        /// atomically apply any pending state.
        ///
        /// This event must not be sent after the ext_foreign_toplevel_handle_v1.closed
        /// event.
        Done,
        /// Title change
        ///
        /// The title of the toplevel has changed.
        ///
        /// The configured state must not be applied immediately. See
        /// ext_foreign_toplevel_handle_v1.done for details.
        Title { title: String },
        /// App_id change
        ///
        /// The app id of the toplevel has changed.
        ///
        /// The configured state must not be applied immediately. See
        /// ext_foreign_toplevel_handle_v1.done for details.
        AppId { app_id: String },
        /// A stable identifier for a toplevel
        ///
        /// This identifier is used to check if two or more toplevel handles belong
        /// to the same toplevel.
        ///
        /// The identifier is useful for command line tools or privileged clients
        /// which may need to reference an exact toplevel across processes or
        /// instances of the ext_foreign_toplevel_list_v1 global.
        ///
        /// The compositor must only send this event when the handle is created.
        ///
        /// The identifier must be unique per toplevel and its handles. Two different
        /// toplevels must not have the same identifier. The identifier is only valid
        /// as long as the toplevel is mapped. If the toplevel is unmapped the identifier
        /// must not be reused. An identifier must not be reused by the compositor to
        /// ensure there are no races when sharing identifiers between processes.
        ///
        /// An identifier is a string that contains up to 32 printable ASCII bytes.
        /// An identifier must not be an empty string. It is recommended that a
        /// compositor includes an opaque generation value in identifiers. How the
        /// generation value is used when generating the identifier is implementation
        /// dependent.
        Identifier { identifier: String },
    }
    impl EventWriter
        for crate::wayland_objects::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1
    {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Closed {} => {
                    let mut raw = RawEvent::new(self.id(), 0, 0);
                    raw
                }
                Done {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
                Title { title } => {
                    let mut raw = RawEvent::new(self.id(), 2, 1);
                    raw.string(title);
                    raw
                }
                AppId { app_id } => {
                    let mut raw = RawEvent::new(self.id(), 3, 1);
                    raw.string(app_id);
                    raw
                }
                Identifier { identifier } => {
                    let mut raw = RawEvent::new(self.id(), 4, 1);
                    raw.string(identifier);
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
    impl Constants for crate::wayland_objects::ext_idle_notifier_v1::ExtIdleNotifierV1 {
        const INTERFACE_NAME: &'static str = "ext_idle_notifier_v1";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ExtIdleNotifierV1Id {
        type Object = crate::wayland_objects::ext_idle_notifier_v1::ExtIdleNotifierV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_idle_notifier_v1::ExtIdleNotifierV1
    {
        type Id = ExtIdleNotifierV1Id;
    }
    /// Minimum interface version for the `get_input_idle_notification` message (XML `since` / `version`).
    pub const GET_INPUT_IDLE_NOTIFICATION_SINCE: u32 = 2;
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
        /// Create a notification object
        ///
        /// Create a new idle notification object to track input from the
        /// user, such as keyboard and mouse movement. Because this object is
        /// meant to track user input alone, it ignores idle inhibitors.
        ///
        /// The notification object has a minimum timeout duration and is tied to a
        /// seat. The client will be notified if the seat is inactive for at least
        /// the provided timeout. See ext_idle_notification_v1 for more details.
        ///
        /// A zero timeout is valid and means the client wants to be notified as
        /// soon as possible when the seat is inactive.
        GetInputIdleNotification {
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
                2 => GetInputIdleNotification {
                    id: req.id()?,
                    timeout: req.uint()?,
                    seat: req.id()?,
                },
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
/// compositor-specific.
///
/// How this notification responds to idle inhibitors depends on how
/// it was constructed. If constructed from the
/// get_idle_notification request, then if an idle inhibitor is
/// active (e.g. another client has created a zwp_idle_inhibitor_v1
/// on a visible surface), the compositor must not make the
/// notification object idle. However, if constructed from the
/// get_input_idle_notification request, then idle inhibitors are
/// ignored, and only input from the user, e.g. from a keyboard or
/// mouse, counts as activity.
///
/// When the notification object becomes idle, an idled event is sent. When
/// user activity starts again, the notification object stops being idle,
/// a resumed event is sent and the timeout is restarted.
pub mod ext_idle_notification_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::ext_idle_notification_v1::ExtIdleNotificationV1 {
        const INTERFACE_NAME: &'static str = "ext_idle_notification_v1";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ExtIdleNotificationV1Id {
        type Object = crate::wayland_objects::ext_idle_notification_v1::ExtIdleNotificationV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_idle_notification_v1::ExtIdleNotificationV1
    {
        type Id = ExtIdleNotificationV1Id;
    }
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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

/// Opaque image capture source object
///
/// The image capture source object is an opaque descriptor for a capturable
/// resource.  This resource may be any sort of entity from which an image
/// may be derived.
///
/// Note, because ext_image_capture_source_v1 objects are created from multiple
/// independent factory interfaces, the ext_image_capture_source_v1 interface is
/// frozen at version 1.
pub mod ext_image_capture_source_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::ext_image_capture_source_v1::ExtImageCaptureSourceV1 {
        const INTERFACE_NAME: &'static str = "ext_image_capture_source_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtImageCaptureSourceV1Id {
        type Object = crate::wayland_objects::ext_image_capture_source_v1::ExtImageCaptureSourceV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_image_capture_source_v1::ExtImageCaptureSourceV1
    {
        type Id = ExtImageCaptureSourceV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Delete this object
        ///
        /// Destroys the image capture source. This request may be sent at any time
        /// by the client.
        Destroy,
    }
    impl Parser for crate::wayland_objects::ext_image_capture_source_v1::ExtImageCaptureSourceV1 {
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
}

/// Image capture source manager for outputs
///
/// A manager for creating image capture source objects for wl_output objects.
pub mod ext_output_image_capture_source_manager_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1 {
    const INTERFACE_NAME: &'static str = "ext_output_image_capture_source_manager_v1";
    const INTERFACE_VERSION: u32 = 1;
  }
    impl crate::object::IdToObject for ExtOutputImageCaptureSourceManagerV1Id {
        type Object = crate::wayland_objects::ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1 {
    type Id = ExtOutputImageCaptureSourceManagerV1Id;
  }
    #[derive(Debug)]
    pub enum Request {
        /// Create source object for output
        ///
        /// Creates a source object for an output. Images captured from this source
        /// will show the same content as the output. Some elements may be omitted,
        /// such as cursors and overlays that have been marked as transparent to
        /// capturing.
        CreateSource { source: ExtImageCaptureSourceV1Id, output: WlOutputId },
        /// Delete this object
        ///
        /// Destroys the manager. This request may be sent at any time by the client
        /// and objects created by the manager will remain valid after its
        /// destruction.
        Destroy,
    }
    impl Parser for crate::wayland_objects::ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1 {
    type Request = Request;
    fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
      use Request::*;
      let request = match req.request_id {
        0 => CreateSource {
          source: req.id()?,
          output: req.id()?,
        },
        1 => Destroy,
        _ => return Err(ParserError::InvalidRequest),
      };
      Ok(request)
    }
  }
}

/// Image capture source manager for foreign toplevels
///
/// A manager for creating image capture source objects for
/// ext_foreign_toplevel_handle_v1 objects.
pub mod ext_foreign_toplevel_image_capture_source_manager_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1 {
    const INTERFACE_NAME: &'static str = "ext_foreign_toplevel_image_capture_source_manager_v1";
    const INTERFACE_VERSION: u32 = 1;
  }
    impl crate::object::IdToObject for ExtForeignToplevelImageCaptureSourceManagerV1Id {
        type Object = crate::wayland_objects::ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1 {
    type Id = ExtForeignToplevelImageCaptureSourceManagerV1Id;
  }
    #[derive(Debug)]
    pub enum Request {
        /// Create source object for foreign toplevel
        ///
        /// Creates a source object for a foreign toplevel handle. Images captured
        /// from this source will show the same content as the toplevel.
        CreateSource {
            source: ExtImageCaptureSourceV1Id,
            toplevel_handle: ExtForeignToplevelHandleV1Id,
        },
        /// Delete this object
        ///
        /// Destroys the manager. This request may be sent at any time by the client
        /// and objects created by the manager will remain valid after its
        /// destruction.
        Destroy,
    }
    impl Parser for crate::wayland_objects::ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1 {
    type Request = Request;
    fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
      use Request::*;
      let request = match req.request_id {
        0 => CreateSource {
          source: req.id()?,
          toplevel_handle: req.id()?,
        },
        1 => Destroy,
        _ => return Err(ParserError::InvalidRequest),
      };
      Ok(request)
    }
  }
}

/// Manager to inform clients and begin capturing
///
/// This object is a manager which offers requests to start capturing from a
/// source.
pub mod ext_image_copy_capture_manager_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1
    {
        const INTERFACE_NAME: &'static str = "ext_image_copy_capture_manager_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtImageCopyCaptureManagerV1Id {
        type Object =
            crate::wayland_objects::ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1
    {
        type Id = ExtImageCopyCaptureManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Capture an image capture source
        ///
        /// Create a capturing session for an image capture source.
        ///
        /// If the paint_cursors option is set, cursors shall be composited onto
        /// the captured frame. The cursor must not be composited onto the frame
        /// if this flag is not set.
        ///
        /// If the options bitfield is invalid, the invalid_option protocol error
        /// is sent.
        CreateSession {
            session: ExtImageCopyCaptureSessionV1Id,
            source: ExtImageCaptureSourceV1Id,
            options: u32,
        },
        /// Capture the pointer cursor of an image capture source
        ///
        /// Create a cursor capturing session for the pointer of an image capture
        /// source.
        CreatePointerCursorSession {
            session: ExtImageCopyCaptureCursorSessionV1Id,
            source: ExtImageCaptureSourceV1Id,
            pointer: WlPointerId,
        },
        /// Destroy the manager
        ///
        /// Destroy the manager object.
        ///
        /// Other objects created via this interface are unaffected.
        Destroy,
    }
    impl Parser
        for crate::wayland_objects::ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1
    {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => CreateSession { session: req.id()?, source: req.id()?, options: req.uint()? },
                1 => CreatePointerCursorSession {
                    session: req.id()?,
                    source: req.id()?,
                    pointer: req.id()?,
                },
                2 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Invalid option flag,
        InvalidOption = 1,
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Options {
        /// Paint cursors onto captured frames,
        PaintCursors = 1,
    }
}

/// Image copy capture session
///
/// This object represents an active image copy capture session.
///
/// After a capture session is created, buffer constraint events will be
/// emitted from the compositor to tell the client which buffer types and
/// formats are supported for reading from the session. The compositor may
/// re-send buffer constraint events whenever they change.
///
/// To advertise buffer constraints, the compositor must send in no
/// particular order: zero or more shm_format and dmabuf_format events, zero
/// or one dmabuf_device event, and exactly one buffer_size event. Then the
/// compositor must send a done event.
///
/// When the client has received all the buffer constraints, it can create a
/// buffer accordingly, attach it to the capture session using the
/// attach_buffer request, set the buffer damage using the damage_buffer
/// request and then send the capture request.
pub mod ext_image_copy_capture_session_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1
    {
        const INTERFACE_NAME: &'static str = "ext_image_copy_capture_session_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtImageCopyCaptureSessionV1Id {
        type Object =
            crate::wayland_objects::ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1
    {
        type Id = ExtImageCopyCaptureSessionV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Create a frame
        ///
        /// Create a capture frame for this session.
        ///
        /// At most one frame object can exist for a given session at any time. If
        /// a client sends a create_frame request before a previous frame object
        /// has been destroyed, the duplicate_frame protocol error is raised.
        CreateFrame { frame: ExtImageCopyCaptureFrameV1Id },
        /// Delete this object
        ///
        /// Destroys the session. This request can be sent at any time by the
        /// client.
        ///
        /// This request doesn't affect ext_image_copy_capture_frame_v1 objects created by
        /// this object.
        Destroy,
    }
    impl Parser
        for crate::wayland_objects::ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1
    {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => CreateFrame { frame: req.id()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Image capture source dimensions
        ///
        /// Provides the dimensions of the source image in buffer pixel coordinates.
        ///
        /// The client must attach buffers that match this size.
        BufferSize {
            /// Buffer width,
            width: u32,
            /// Buffer height,
            height: u32,
        },
        /// Shm buffer format
        ///
        /// Provides the format that must be used for shared-memory buffers.
        ///
        /// This event may be emitted multiple times, in which case the client may
        /// choose any given format.
        ShmFormat {
            /// Shm format,
            format: u32,
        },
        /// Dma-buf device
        ///
        /// This event advertises the device buffers must be allocated on for
        /// dma-buf buffers.
        ///
        /// In general the device is a DRM node. The DRM node type (primary vs.
        /// render) is unspecified. Clients must not rely on the compositor sending
        /// a particular node type. Clients cannot check two devices for equality
        /// by comparing the dev_t value.
        DmabufDevice {
            /// Device dev_t value,
            device: Array,
        },
        /// Dma-buf format
        ///
        /// Provides the format that must be used for dma-buf buffers.
        ///
        /// The client may choose any of the modifiers advertised in the array of
        /// 64-bit unsigned integers.
        ///
        /// This event may be emitted multiple times, in which case the client may
        /// choose any given format.
        DmabufFormat {
            /// Drm format code,
            format: u32,
            /// Drm format modifiers,
            modifiers: Array,
        },
        /// All constraints have been sent
        ///
        /// This event is sent once when all buffer constraint events have been
        /// sent.
        ///
        /// The compositor must always end a batch of buffer constraint events with
        /// this event, regardless of whether it sends the initial constraints or
        /// an update.
        Done,
        /// Session is no longer available
        ///
        /// This event indicates that the capture session has stopped and is no
        /// longer available. This can happen in a number of cases, e.g. when the
        /// underlying source is destroyed, if the user decides to end the image
        /// capture, or if an unrecoverable runtime error has occurred.
        ///
        /// The client should destroy the session after receiving this event.
        Stopped,
    }
    impl EventWriter
        for crate::wayland_objects::ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1
    {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                BufferSize { width, height } => {
                    let mut raw = RawEvent::new(self.id(), 0, 2);
                    raw.uint(width);
                    raw.uint(height);
                    raw
                }
                ShmFormat { format } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.uint(format);
                    raw
                }
                DmabufDevice { device } => {
                    let mut raw = RawEvent::new(self.id(), 2, 1);
                    raw.array(device);
                    raw
                }
                DmabufFormat { format, modifiers } => {
                    let mut raw = RawEvent::new(self.id(), 3, 2);
                    raw.uint(format);
                    raw.array(modifiers);
                    raw
                }
                Done {} => {
                    let mut raw = RawEvent::new(self.id(), 4, 0);
                    raw
                }
                Stopped {} => {
                    let mut raw = RawEvent::new(self.id(), 5, 0);
                    raw
                }
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Create_frame sent before destroying previous frame,
        DuplicateFrame = 1,
    }
}

/// Image capture frame
///
/// This object represents an image capture frame.
///
/// The client should attach a buffer, damage the buffer, and then send a
/// capture request.
///
/// If the capture is successful, the compositor must send the frame metadata
/// (transform, damage, presentation_time in any order) followed by the ready
/// event.
///
/// If the capture fails, the compositor must send the failed event.
pub mod ext_image_copy_capture_frame_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1
    {
        const INTERFACE_NAME: &'static str = "ext_image_copy_capture_frame_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtImageCopyCaptureFrameV1Id {
        type Object =
            crate::wayland_objects::ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1
    {
        type Id = ExtImageCopyCaptureFrameV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy this object
        ///
        /// Destroys the frame. This request can be sent at any time by the
        /// client.
        Destroy,
        /// Attach buffer to session
        ///
        /// Attach a buffer to the session.
        ///
        /// The wl_buffer.release request is unused.
        ///
        /// The new buffer replaces any previously attached buffer.
        ///
        /// This request must not be sent after capture, or else the
        /// already_captured protocol error is raised.
        AttachBuffer { buffer: WlBufferId },
        /// Damage buffer
        ///
        /// Apply damage to the buffer which is to be captured next. This request
        /// may be sent multiple times to describe a region.
        ///
        /// The client indicates the accumulated damage since this wl_buffer was
        /// last captured. During capture, the compositor will update the buffer
        /// with at least the union of the region passed by the client and the
        /// region advertised by ext_image_copy_capture_frame_v1.damage.
        ///
        /// When a wl_buffer is captured for the first time, or when the client
        /// doesn't track damage, the client must damage the whole buffer.
        ///
        /// This is for optimisation purposes. The compositor may use this
        /// information to reduce copying.
        ///
        /// These coordinates originate from the upper left corner of the buffer.
        ///
        /// If x or y are strictly negative, or if width or height are negative or
        /// zero, the invalid_buffer_damage protocol error is raised.
        ///
        /// This request must not be sent after capture, or else the
        /// already_captured protocol error is raised.
        DamageBuffer {
            /// Region x coordinate,
            x: i32,
            /// Region y coordinate,
            y: i32,
            /// Region width,
            width: i32,
            /// Region height,
            height: i32,
        },
        /// Capture a frame
        ///
        /// Capture a frame.
        ///
        /// Unless this is the first successful captured frame performed in this
        /// session, the compositor may wait an indefinite amount of time for the
        /// source content to change before performing the copy.
        ///
        /// This request may only be sent once, or else the already_captured
        /// protocol error is raised. A buffer must be attached before this request
        /// is sent, or else the no_buffer protocol error is raised.
        Capture,
    }
    impl Parser
        for crate::wayland_objects::ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1
    {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => AttachBuffer { buffer: req.id()? },
                2 => DamageBuffer {
                    x: req.int()?,
                    y: req.int()?,
                    width: req.int()?,
                    height: req.int()?,
                },
                3 => Capture,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Buffer transform
        ///
        /// This event is sent before the ready event and holds the transform that
        /// the compositor has applied to the buffer contents.
        Transform { transform: u32 },
        /// Buffer damaged region
        ///
        /// This event is sent before the ready event. It may be generated multiple
        /// times to describe a region.
        ///
        /// The first captured frame in a session will always carry full damage.
        /// Subsequent frames' damaged regions describe which parts of the buffer
        /// have changed since the last ready event.
        ///
        /// These coordinates originate in the upper left corner of the buffer.
        Damage {
            /// Damage x coordinate,
            x: i32,
            /// Damage y coordinate,
            y: i32,
            /// Damage width,
            width: i32,
            /// Damage height,
            height: i32,
        },
        /// Presentation time of the frame
        ///
        /// This event indicates the time at which the frame is presented to the
        /// output in system monotonic time. This event is sent before the ready
        /// event.
        ///
        /// The timestamp is expressed as tv_sec_hi, tv_sec_lo, tv_nsec triples,
        /// each component being an unsigned 32-bit value. Whole seconds are in
        /// tv_sec which is a 64-bit value combined from tv_sec_hi and tv_sec_lo,
        /// and the additional fractional part in tv_nsec as nanoseconds. Hence,
        /// for valid timestamps tv_nsec must be in [0, 999999999].
        PresentationTime {
            /// High 32 bits of the seconds part of the timestamp,
            tv_sec_hi: u32,
            /// Low 32 bits of the seconds part of the timestamp,
            tv_sec_lo: u32,
            /// Nanoseconds part of the timestamp,
            tv_nsec: u32,
        },
        /// Frame is available for reading
        ///
        /// Called as soon as the frame is copied, indicating it is available
        /// for reading.
        ///
        /// The buffer may be re-used by the client after this event.
        ///
        /// After receiving this event, the client must destroy the object.
        Ready,
        /// Capture failed
        ///
        /// This event indicates that the attempted frame copy has failed.
        ///
        /// After receiving this event, the client must destroy the object.
        Failed { reason: u32 },
    }
    impl EventWriter
        for crate::wayland_objects::ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1
    {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Transform { transform } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(transform);
                    raw
                }
                Damage { x, y, width, height } => {
                    let mut raw = RawEvent::new(self.id(), 1, 4);
                    raw.int(x);
                    raw.int(y);
                    raw.int(width);
                    raw.int(height);
                    raw
                }
                PresentationTime { tv_sec_hi, tv_sec_lo, tv_nsec } => {
                    let mut raw = RawEvent::new(self.id(), 2, 3);
                    raw.uint(tv_sec_hi);
                    raw.uint(tv_sec_lo);
                    raw.uint(tv_nsec);
                    raw
                }
                Ready {} => {
                    let mut raw = RawEvent::new(self.id(), 3, 0);
                    raw
                }
                Failed { reason } => {
                    let mut raw = RawEvent::new(self.id(), 4, 1);
                    raw.uint(reason);
                    raw
                }
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Capture sent without attach_buffer,
        NoBuffer = 1,
        /// Invalid buffer damage,
        InvalidBufferDamage = 2,
        /// Capture request has been sent,
        AlreadyCaptured = 3,
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum FailureReason {
        /// Unknown runtime error
        ///
        /// An unspecified runtime error has occurred. The client may retry.
        Unknown = 0,
        /// Buffer constraints mismatch
        ///
        /// The buffer submitted by the client doesn't match the latest session
        /// constraints. The client should re-allocate its buffers and retry.
        BufferConstraints = 1,
        /// Session is no longer available
        ///
        /// The session has stopped. See ext_image_copy_capture_session_v1.stopped.
        Stopped = 2,
    }
}

/// Cursor capture session
///
/// This object represents a cursor capture session. It extends the base
/// capture session with cursor-specific metadata.
pub mod ext_image_copy_capture_cursor_session_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1 {
    const INTERFACE_NAME: &'static str = "ext_image_copy_capture_cursor_session_v1";
    const INTERFACE_VERSION: u32 = 1;
  }
    impl crate::object::IdToObject for ExtImageCopyCaptureCursorSessionV1Id {
        type Object = crate::wayland_objects::ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1 {
    type Id = ExtImageCopyCaptureCursorSessionV1Id;
  }
    #[derive(Debug)]
    pub enum Request {
        /// Delete this object
        ///
        /// Destroys the session. This request can be sent at any time by the
        /// client.
        ///
        /// This request doesn't affect ext_image_copy_capture_frame_v1 objects created by
        /// this object.
        Destroy,
        /// Get image copy capturer session
        ///
        /// Gets the image copy capture session for this cursor session.
        ///
        /// The session will produce frames of the cursor image. The compositor may
        /// pause the session when the cursor leaves the captured area.
        ///
        /// This request must not be sent more than once, or else the
        /// duplicate_session protocol error is raised.
        GetCaptureSession { session: ExtImageCopyCaptureSessionV1Id },
    }
    impl Parser for crate::wayland_objects::ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1 {
    type Request = Request;
    fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
      use Request::*;
      let request = match req.request_id {
        0 => Destroy,
        1 => GetCaptureSession {
          session: req.id()?,
        },
        _ => return Err(ParserError::InvalidRequest),
      };
      Ok(request)
    }
  }
    #[derive(Debug)]
    pub enum Event {
        /// Cursor entered captured area
        ///
        /// Sent when a cursor enters the captured area. It shall be generated
        /// before the "position" and "hotspot" events when and only when a cursor
        /// enters the area.
        ///
        /// The cursor enters the captured area when the cursor image intersects
        /// with the captured area. Note, this is different from e.g.
        /// wl_pointer.enter.
        Enter,
        /// Cursor left captured area
        ///
        /// Sent when a cursor leaves the captured area. No "position" or "hotspot"
        /// event is generated for the cursor until the cursor enters the captured
        /// area again.
        Leave,
        /// Position changed
        ///
        /// Cursors outside the image capture source do not get captured and no
        /// event will be generated for them.
        ///
        /// The given position is the position of the cursor's hotspot and it is
        /// relative to the main buffer's top left corner in transformed buffer
        /// pixel coordinates. The coordinates may be negative or greater than the
        /// main buffer size.
        Position {
            /// Position x coordinates,
            x: i32,
            /// Position y coordinates,
            y: i32,
        },
        /// Hotspot changed
        ///
        /// The hotspot describes the offset between the cursor image and the
        /// position of the input device.
        ///
        /// The given coordinates are the hotspot's offset from the origin in
        /// buffer coordinates.
        ///
        /// Clients should not apply the hotspot immediately: the hotspot becomes
        /// effective when the next ext_image_copy_capture_frame_v1.ready event is received.
        ///
        /// Compositors may delay this event until the client captures a new frame.
        Hotspot {
            /// Hotspot x coordinates,
            x: i32,
            /// Hotspot y coordinates,
            y: i32,
        },
    }
    impl EventWriter for crate::wayland_objects::ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1 {
    type Event = Event;
    fn object_id(&self) -> WaylandId {
      use crate::object::Object;
      self.id()
    }
    fn to_raw_event(&self, event: Event) -> RawEvent {
      use Event::*;
      use crate::object::Object;
      match event {
        Enter {  } => {
          let mut raw = RawEvent::new(self.id(), 0, 0);
        raw
        },
        Leave {  } => {
          let mut raw = RawEvent::new(self.id(), 1, 0);
        raw
        },
        Position { x,y } => {
          let mut raw = RawEvent::new(self.id(), 2, 2);
          raw.int(x);
          raw.int(y);
        raw
        },
        Hotspot { x,y } => {
          let mut raw = RawEvent::new(self.id(), 3, 2);
          raw.int(x);
          raw.int(y);
        raw
        },
      }
    }
  }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Get_capture_session sent twice,
        DuplicateSession = 1,
    }
}

/// Used to lock the session
///
/// This interface is used to request that the session be locked.
pub mod ext_session_lock_manager_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::ext_session_lock_manager_v1::ExtSessionLockManagerV1 {
        const INTERFACE_NAME: &'static str = "ext_session_lock_manager_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtSessionLockManagerV1Id {
        type Object = crate::wayland_objects::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_session_lock_manager_v1::ExtSessionLockManagerV1
    {
        type Id = ExtSessionLockManagerV1Id;
    }
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
    impl Constants for crate::wayland_objects::ext_session_lock_v1::ExtSessionLockV1 {
        const INTERFACE_NAME: &'static str = "ext_session_lock_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtSessionLockV1Id {
        type Object = crate::wayland_objects::ext_session_lock_v1::ExtSessionLockV1;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::ext_session_lock_v1::ExtSessionLockV1 {
        type Id = ExtSessionLockV1Id;
    }
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1 {
        const INTERFACE_NAME: &'static str = "ext_session_lock_surface_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtSessionLockSurfaceV1Id {
        type Object = crate::wayland_objects::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1
    {
        type Id = ExtSessionLockSurfaceV1Id;
    }
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants
        for crate::wayland_objects::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1
    {
        const INTERFACE_NAME: &'static str = "wp_fractional_scale_manager_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpFractionalScaleManagerV1Id {
        type Object =
            crate::wayland_objects::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1
    {
        type Id = WpFractionalScaleManagerV1Id;
    }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::wp_fractional_scale_v1::WpFractionalScaleV1 {
        const INTERFACE_NAME: &'static str = "wp_fractional_scale_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpFractionalScaleV1Id {
        type Object = crate::wayland_objects::wp_fractional_scale_v1::WpFractionalScaleV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_fractional_scale_v1::WpFractionalScaleV1
    {
        type Id = WpFractionalScaleV1Id;
    }
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    impl Constants
        for crate::wayland_objects::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1
    {
        const INTERFACE_NAME: &'static str = "wp_single_pixel_buffer_manager_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpSinglePixelBufferManagerV1Id {
        type Object =
            crate::wayland_objects::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1
    {
        type Id = WpSinglePixelBufferManagerV1Id;
    }
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
        ///
        /// The r, g, b and a arguments valid range is from UINT32_MIN (0)
        /// to UINT32_MAX (0xffffffff).
        ///
        /// These arguments should be interpreted as a percentage, i.e.
        /// - UINT32_MIN = 0% of the given color component
        /// - UINT32_MAX = 100% of the given color component
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
    impl Constants
        for crate::wayland_objects::wp_tearing_control_manager_v1::WpTearingControlManagerV1
    {
        const INTERFACE_NAME: &'static str = "wp_tearing_control_manager_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpTearingControlManagerV1Id {
        type Object =
            crate::wayland_objects::wp_tearing_control_manager_v1::WpTearingControlManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_tearing_control_manager_v1::WpTearingControlManagerV1
    {
        type Id = WpTearingControlManagerV1Id;
    }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
///
/// If the associated wl_surface is destroyed, this object becomes inert and
/// should be destroyed.
pub mod wp_tearing_control_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::wp_tearing_control_v1::WpTearingControlV1 {
        const INTERFACE_NAME: &'static str = "wp_tearing_control_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpTearingControlV1Id {
        type Object = crate::wayland_objects::wp_tearing_control_v1::WpTearingControlV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_tearing_control_v1::WpTearingControlV1
    {
        type Id = WpTearingControlV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Set presentation hint
        ///
        /// Set the presentation hint for the associated wl_surface. This state is
        /// double-buffered, see wl_surface.commit.
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::xdg_activation_v1::XdgActivationV1 {
        const INTERFACE_NAME: &'static str = "xdg_activation_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for XdgActivationV1Id {
        type Object = crate::wayland_objects::xdg_activation_v1::XdgActivationV1;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::xdg_activation_v1::XdgActivationV1 {
        type Id = XdgActivationV1Id;
    }
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
    impl Constants for crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1 {
        const INTERFACE_NAME: &'static str = "xdg_activation_token_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for XdgActivationTokenV1Id {
        type Object = crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1
    {
        type Id = XdgActivationTokenV1Id;
    }
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
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::xwayland_shell_v1::XwaylandShellV1 {
        const INTERFACE_NAME: &'static str = "xwayland_shell_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for XwaylandShellV1Id {
        type Object = crate::wayland_objects::xwayland_shell_v1::XwaylandShellV1;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::xwayland_shell_v1::XwaylandShellV1 {
        type Id = XwaylandShellV1Id;
    }
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
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
    impl Constants for crate::wayland_objects::xwayland_surface_v1::XwaylandSurfaceV1 {
        const INTERFACE_NAME: &'static str = "xwayland_surface_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for XwaylandSurfaceV1Id {
        type Object = crate::wayland_objects::xwayland_surface_v1::XwaylandSurfaceV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::xwayland_surface_v1::XwaylandSurfaceV1
    {
        type Id = XwaylandSurfaceV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Associates a Xwayland window to a wl_surface
        ///
        /// Associates an Xwayland window to a wl_surface.
        /// The association state is double-buffered, see wl_surface.commit.
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
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Given wl_surface is already associated with an X11 window,
        AlreadyAssociated = 0,
        /// Serial was not valid,
        InvalidSerial = 1,
    }
}

/// Cursor shape manager
///
/// This global offers an alternative, optional way to set cursor images. This
/// new way uses enumerated cursors instead of a wl_surface like
/// wl_pointer.set_cursor does.
///
/// Warning! The protocol described in this file is currently in the testing
/// phase. Backward compatible changes may be added together with the
/// corresponding interface version bump. Backward incompatible changes can
/// only be done by creating a new major version of the extension.
pub mod wp_cursor_shape_manager_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1 {
        const INTERFACE_NAME: &'static str = "wp_cursor_shape_manager_v1";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for WpCursorShapeManagerV1Id {
        type Object = crate::wayland_objects::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1
    {
        type Id = WpCursorShapeManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the manager
        ///
        /// Destroy the cursor shape manager.
        Destroy,
        /// Manage the cursor shape of a pointer device
        ///
        /// Obtain a wp_cursor_shape_device_v1 for a wl_pointer object.
        ///
        /// When the pointer capability is removed from the wl_seat, the
        /// wp_cursor_shape_device_v1 object becomes inert.
        GetPointer { cursor_shape_device: WpCursorShapeDeviceV1Id, pointer: WlPointerId },
        /// Manage the cursor shape of a tablet tool device
        ///
        /// Obtain a wp_cursor_shape_device_v1 for a zwp_tablet_tool_v2 object.
        ///
        /// When the zwp_tablet_tool_v2 is removed, the wp_cursor_shape_device_v1
        /// object becomes inert.
        GetTabletToolV2 {
            cursor_shape_device: WpCursorShapeDeviceV1Id,
            tablet_tool: ZwpTabletToolV2Id,
        },
    }
    impl Parser for crate::wayland_objects::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetPointer { cursor_shape_device: req.id()?, pointer: req.id()? },
                2 => GetTabletToolV2 { cursor_shape_device: req.id()?, tablet_tool: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// Cursor shape for a device
///
/// This interface allows clients to set the cursor shape.
pub mod wp_cursor_shape_device_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1 {
        const INTERFACE_NAME: &'static str = "wp_cursor_shape_device_v1";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for WpCursorShapeDeviceV1Id {
        type Object = crate::wayland_objects::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1
    {
        type Id = WpCursorShapeDeviceV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the cursor shape device
        ///
        /// Destroy the cursor shape device.
        ///
        /// The device cursor shape remains unchanged.
        Destroy,
        /// Set device cursor to the shape
        ///
        /// Sets the device cursor to the specified shape. The compositor will
        /// change the cursor image based on the specified shape.
        ///
        /// The cursor actually changes only if the input device focus is one of
        /// the requesting client's surfaces. If any, the previous cursor image
        /// (surface or shape) is replaced.
        ///
        /// The "shape" argument must be a valid enum entry, otherwise the
        /// invalid_shape protocol error is raised.
        ///
        /// This is similar to the wl_pointer.set_cursor and
        /// zwp_tablet_tool_v2.set_cursor requests, but this request accepts a
        /// shape instead of contents in the form of a surface. Clients can mix
        /// set_cursor and set_shape requests.
        ///
        /// The serial parameter must match the latest wl_pointer.enter or
        /// zwp_tablet_tool_v2.proximity_in serial number sent to the client.
        /// Otherwise the request will be ignored.
        SetShape {
            /// Serial number of the enter event,
            serial: u32,
            shape: u32,
        },
    }
    impl Parser for crate::wayland_objects::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => SetShape { serial: req.uint()?, shape: req.uint()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    /// Cursor shapes
    ///
    /// This enum describes cursor shapes.
    ///
    /// The names are taken from the CSS W3C specification:
    /// https://w3c.github.io/csswg-drafts/css-ui/#cursor
    /// with a few additions.
    ///
    /// Note that there are some groups of cursor shapes that are related:
    /// The first group is drag-and-drop cursors which are used to indicate
    /// the selected action during dnd operations. The second group is resize
    /// cursors which are used to indicate resizing and moving possibilities
    /// on window borders. It is recommended that the shapes in these groups
    /// should use visually compatible images and metaphors.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Shape {
        /// Default cursor,
        Default = 1,
        /// A context menu is available for the object under the cursor,
        ContextMenu = 2,
        /// Help is available for the object under the cursor,
        Help = 3,
        /// Pointer that indicates a link or another interactive element,
        Pointer = 4,
        /// Progress indicator,
        Progress = 5,
        /// Program is busy, user should wait,
        Wait = 6,
        /// A cell or set of cells may be selected,
        Cell = 7,
        /// Simple crosshair,
        Crosshair = 8,
        /// Text may be selected,
        Text = 9,
        /// Vertical text may be selected,
        VerticalText = 10,
        /// Drag-and-drop: alias of/shortcut to something is to be created,
        Alias = 11,
        /// Drag-and-drop: something is to be copied,
        Copy = 12,
        /// Drag-and-drop: something is to be moved,
        Move = 13,
        /// Drag-and-drop: the dragged item cannot be dropped at the current cursor location,
        NoDrop = 14,
        /// Drag-and-drop: the requested action will not be carried out,
        NotAllowed = 15,
        /// Drag-and-drop: something can be grabbed,
        Grab = 16,
        /// Drag-and-drop: something is being grabbed,
        Grabbing = 17,
        /// Resizing: the east border is to be moved,
        EResize = 18,
        /// Resizing: the north border is to be moved,
        NResize = 19,
        /// Resizing: the north-east corner is to be moved,
        NeResize = 20,
        /// Resizing: the north-west corner is to be moved,
        NwResize = 21,
        /// Resizing: the south border is to be moved,
        SResize = 22,
        /// Resizing: the south-east corner is to be moved,
        SeResize = 23,
        /// Resizing: the south-west corner is to be moved,
        SwResize = 24,
        /// Resizing: the west border is to be moved,
        WResize = 25,
        /// Resizing: the east and west borders are to be moved,
        EwResize = 26,
        /// Resizing: the north and south borders are to be moved,
        NsResize = 27,
        /// Resizing: the north-east and south-west corners are to be moved,
        NeswResize = 28,
        /// Resizing: the north-west and south-east corners are to be moved,
        NwseResize = 29,
        /// Resizing: that the item/column can be resized horizontally,
        ColResize = 30,
        /// Resizing: that the item/row can be resized vertically,
        RowResize = 31,
        /// Something can be scrolled in any direction,
        AllScroll = 32,
        /// Something can be zoomed in,
        ZoomIn = 33,
        /// Something can be zoomed out,
        ZoomOut = 34,
        /// Drag-and-drop: the user will select which action will be carried out (non-css value),
        DndAsk = 35,
        /// Resizing: something can be moved or resized in any direction (non-css value),
        AllResize = 36,
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// The specified shape value is invalid,
        InvalidShape = 1,
    }
}

/// Global for providing explicit synchronization
///
/// This global is a factory interface, allowing clients to request
/// explicit synchronization for buffers on a per-surface basis.
///
/// See wp_linux_drm_syncobj_surface_v1 for more information.
pub mod wp_linux_drm_syncobj_manager_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1
    {
        const INTERFACE_NAME: &'static str = "wp_linux_drm_syncobj_manager_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpLinuxDrmSyncobjManagerV1Id {
        type Object =
            crate::wayland_objects::wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1
    {
        type Id = WpLinuxDrmSyncobjManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy explicit synchronization factory object
        ///
        /// Destroy this explicit synchronization factory object. Other objects
        /// shall not be affected by this request.
        Destroy,
        /// Extend surface interface for explicit synchronization
        ///
        /// Instantiate an interface extension for the given wl_surface to provide
        /// explicit synchronization.
        ///
        /// If the given wl_surface already has an explicit synchronization object
        /// associated, the surface_exists protocol error is raised.
        ///
        /// Graphics APIs, like EGL or Vulkan, that manage the buffer queue and
        /// commits of a wl_surface themselves, are likely to be using this
        /// extension internally. If a client is using such an API for a
        /// wl_surface, it should not directly use this extension on that surface,
        /// to avoid raising a surface_exists protocol error.
        GetSurface {
            /// The new synchronization surface object id,
            id: WpLinuxDrmSyncobjSurfaceV1Id,
            /// The surface,
            surface: WlSurfaceId,
        },
        /// Import a DRM syncobj timeline
        ///
        /// Import a DRM synchronization object timeline.
        ///
        /// If the FD cannot be imported, the invalid_timeline error is raised.
        ImportTimeline {
            id: WpLinuxDrmSyncobjTimelineV1Id,
            /// Drm_syncobj file descriptor,
            fd: RawFd,
        },
    }
    impl Parser
        for crate::wayland_objects::wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1
    {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetSurface { id: req.id()?, surface: req.id()? },
                2 => ImportTimeline { id: req.id()?, fd: req.fd()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// The surface already has a synchronization object associated,
        SurfaceExists = 0,
        /// The timeline object could not be imported,
        InvalidTimeline = 1,
    }
}

/// Synchronization object timeline
///
/// This object represents an explicit synchronization object timeline
/// imported by the client to the compositor.
pub mod wp_linux_drm_syncobj_timeline_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1
    {
        const INTERFACE_NAME: &'static str = "wp_linux_drm_syncobj_timeline_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpLinuxDrmSyncobjTimelineV1Id {
        type Object =
            crate::wayland_objects::wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1
    {
        type Id = WpLinuxDrmSyncobjTimelineV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the timeline
        ///
        /// Destroy the synchronization object timeline. Other objects are not
        /// affected by this request, in particular timeline points set by
        /// set_acquire_point and set_release_point are not unset.
        Destroy,
    }
    impl Parser
        for crate::wayland_objects::wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1
    {
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
}

/// Per-surface explicit synchronization
///
/// This object is an add-on interface for wl_surface to enable explicit
/// synchronization.
///
/// Each surface can be associated with only one object of this interface at
/// any time.
///
/// Explicit synchronization is guaranteed to be supported for buffers
/// created with any version of the linux-dmabuf protocol. Compositors are
/// free to support explicit synchronization for additional buffer types.
/// If at surface commit time the attached buffer does not support explicit
/// synchronization, an unsupported_buffer error is raised.
///
/// As long as the wp_linux_drm_syncobj_surface_v1 object is alive, the
/// compositor may ignore implicit synchronization for buffers attached and
/// committed to the wl_surface. The delivery of wl_buffer.release events
/// for buffers attached to the surface becomes undefined.
///
/// Clients must set both acquire and release points if and only if a
/// non-null buffer is attached in the same surface commit. See the
/// no_buffer, no_acquire_point and no_release_point protocol errors.
///
/// If at surface commit time the acquire and release DRM syncobj timelines
/// are identical, the acquire point value must be strictly less than the
/// release point value, or else the conflicting_points protocol error is
/// raised.
pub mod wp_linux_drm_syncobj_surface_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1
    {
        const INTERFACE_NAME: &'static str = "wp_linux_drm_syncobj_surface_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for WpLinuxDrmSyncobjSurfaceV1Id {
        type Object =
            crate::wayland_objects::wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1
    {
        type Id = WpLinuxDrmSyncobjSurfaceV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the surface synchronization object
        ///
        /// Destroy this surface synchronization object.
        ///
        /// Any timeline point set by this object with set_acquire_point or
        /// set_release_point since the last commit may be discarded by the
        /// compositor. Any timeline point set by this object before the last
        /// commit will not be affected.
        Destroy,
        /// Set the acquire timeline point
        ///
        /// Set the timeline point that must be signalled before the compositor may
        /// sample from the buffer attached with wl_surface.attach.
        ///
        /// The 64-bit unsigned value combined from point_hi and point_lo is the
        /// point value.
        ///
        /// The acquire point is double-buffered state, and will be applied on the
        /// next wl_surface.commit request for the associated surface. Thus, it
        /// applies only to the buffer that is attached to the surface at commit
        /// time.
        ///
        /// If an acquire point has already been attached during the same commit
        /// cycle, the new point replaces the old one.
        ///
        /// If the associated wl_surface was destroyed, a no_surface error is
        /// raised.
        ///
        /// If at surface commit time there is a pending acquire timeline point set
        /// but no pending buffer attached, a no_buffer error is raised. If at
        /// surface commit time there is a pending buffer attached but no pending
        /// acquire timeline point set, the no_acquire_point protocol error is
        /// raised.
        SetAcquirePoint {
            timeline: WpLinuxDrmSyncobjTimelineV1Id,
            /// High 32 bits of the point value,
            point_hi: u32,
            /// Low 32 bits of the point value,
            point_lo: u32,
        },
        /// Set the release timeline point
        ///
        /// Set the timeline point that must be signalled by the compositor when it
        /// has finished its usage of the buffer attached with wl_surface.attach
        /// for the relevant commit.
        ///
        /// Once the timeline point is signaled, and assuming the associated buffer
        /// is not pending release from other wl_surface.commit requests, no
        /// additional explicit or implicit synchronization with the compositor is
        /// required to safely re-use the buffer.
        ///
        /// Note that clients cannot rely on the release point being always
        /// signaled after the acquire point: compositors may release buffers
        /// without ever reading from them. In addition, the compositor may use
        /// different presentation paths for different commits, which may have
        /// different release behavior. As a result, the compositor may signal the
        /// release points in a different order than the client committed them.
        ///
        /// Because signaling a timeline point also signals every previous point,
        /// it is generally not safe to use the same timeline object for the
        /// release points of multiple buffers. The out-of-order signaling
        /// described above may lead to a release point being signaled before the
        /// compositor has finished reading. To avoid this, it is strongly
        /// recommended that each buffer should use a separate timeline for its
        /// release points.
        ///
        /// The 64-bit unsigned value combined from point_hi and point_lo is the
        /// point value.
        ///
        /// The release point is double-buffered state, and will be applied on the
        /// next wl_surface.commit request for the associated surface. Thus, it
        /// applies only to the buffer that is attached to the surface at commit
        /// time.
        ///
        /// If a release point has already been attached during the same commit
        /// cycle, the new point replaces the old one.
        ///
        /// If the associated wl_surface was destroyed, a no_surface error is
        /// raised.
        ///
        /// If at surface commit time there is a pending release timeline point set
        /// but no pending buffer attached, a no_buffer error is raised. If at
        /// surface commit time there is a pending buffer attached but no pending
        /// release timeline point set, the no_release_point protocol error is
        /// raised.
        SetReleasePoint {
            timeline: WpLinuxDrmSyncobjTimelineV1Id,
            /// High 32 bits of the point value,
            point_hi: u32,
            /// Low 32 bits of the point value,
            point_lo: u32,
        },
    }
    impl Parser
        for crate::wayland_objects::wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1
    {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => SetAcquirePoint {
                    timeline: req.id()?,
                    point_hi: req.uint()?,
                    point_lo: req.uint()?,
                },
                2 => SetReleasePoint {
                    timeline: req.id()?,
                    point_hi: req.uint()?,
                    point_lo: req.uint()?,
                },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// The associated wl_surface was destroyed,
        NoSurface = 1,
        /// The buffer does not support explicit synchronization,
        UnsupportedBuffer = 2,
        /// No buffer was attached,
        NoBuffer = 3,
        /// No acquire timeline point was set,
        NoAcquirePoint = 4,
        /// No release timeline point was set,
        NoReleasePoint = 5,
        /// Acquire and release timeline points are in conflict,
        ConflictingPoints = 6,
    }
}

/// Control behavior when display idles
///
/// This interface permits inhibiting the idle behavior such as screen
/// blanking, locking, and screensaving.  The client binds the idle manager
/// globally, then creates idle-inhibitor objects for each surface.
///
/// Warning! The protocol described in this file is experimental and
/// backward incompatible changes may be made. Backward compatible changes
/// may be added together with the corresponding interface version bump.
/// Backward incompatible changes are done by bumping the version number in
/// the protocol and interface names and resetting the interface version.
/// Once the protocol is to be declared stable, the 'z' prefix and the
/// version number in the protocol and interface names are removed and the
/// interface version number is reset.
pub mod zwp_idle_inhibit_manager_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1 {
        const INTERFACE_NAME: &'static str = "zwp_idle_inhibit_manager_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwpIdleInhibitManagerV1Id {
        type Object = crate::wayland_objects::zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1
    {
        type Id = ZwpIdleInhibitManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the idle inhibitor object
        ///
        /// Destroy the inhibit manager.
        Destroy,
        /// Create a new inhibitor object
        ///
        /// Create a new inhibitor object associated with the given surface.
        CreateInhibitor {
            id: ZwpIdleInhibitorV1Id,
            /// The surface that inhibits the idle behavior,
            surface: WlSurfaceId,
        },
    }
    impl Parser for crate::wayland_objects::zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => CreateInhibitor { id: req.id()?, surface: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// Context object for inhibiting idle behavior
///
/// An idle inhibitor prevents the output that the associated surface is
/// visible on from being set to a state where it is not visually usable due
/// to lack of user interaction (e.g. blanked, dimmed, locked, set to power
/// save, etc.)  Any screensaver processes are also blocked from displaying.
///
/// If the surface is destroyed, unmapped, becomes occluded, loses
/// visibility, or otherwise becomes not visually relevant for the user, the
/// idle inhibitor will not be honored by the compositor; if the surface
/// subsequently regains visibility the inhibitor takes effect once again.
/// Likewise, the inhibitor isn't honored if the system was already idled at
/// the time the inhibitor was established, although if the system later
/// de-idles and re-idles the inhibitor will take effect.
pub mod zwp_idle_inhibitor_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1 {
        const INTERFACE_NAME: &'static str = "zwp_idle_inhibitor_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwpIdleInhibitorV1Id {
        type Object = crate::wayland_objects::zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1
    {
        type Id = ZwpIdleInhibitorV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the idle inhibitor object
        ///
        /// Remove the inhibitor effect from the associated wl_surface.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1 {
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
}

/// Protocol for providing explicit synchronization
///
/// This global is a factory interface, allowing clients to request
/// explicit synchronization for buffers on a per-surface basis.
///
/// See zwp_linux_surface_synchronization_v1 for more information.
///
/// This interface is derived from Chromium's
/// zcr_linux_explicit_synchronization_v1.
///
/// Note: this protocol is superseded by linux-drm-syncobj.
///
/// Warning! The protocol described in this file is experimental and
/// backward incompatible changes may be made. Backward compatible changes
/// may be added together with the corresponding interface version bump.
/// Backward incompatible changes are done by bumping the version number in
/// the protocol and interface names and resetting the interface version.
/// Once the protocol is to be declared stable, the 'z' prefix and the
/// version number in the protocol and interface names are removed and the
/// interface version number is reset.
pub mod zwp_linux_explicit_synchronization_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_linux_explicit_synchronization_v1::ZwpLinuxExplicitSynchronizationV1 {
    const INTERFACE_NAME: &'static str = "zwp_linux_explicit_synchronization_v1";
    const INTERFACE_VERSION: u32 = 2;
  }
    impl crate::object::IdToObject for ZwpLinuxExplicitSynchronizationV1Id {
        type Object = crate::wayland_objects::zwp_linux_explicit_synchronization_v1::ZwpLinuxExplicitSynchronizationV1;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::zwp_linux_explicit_synchronization_v1::ZwpLinuxExplicitSynchronizationV1 {
    type Id = ZwpLinuxExplicitSynchronizationV1Id;
  }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy explicit synchronization factory object
        ///
        /// Destroy this explicit synchronization factory object. Other objects,
        /// including zwp_linux_surface_synchronization_v1 objects created by this
        /// factory, shall not be affected by this request.
        Destroy,
        /// Extend surface interface for explicit synchronization
        ///
        /// Instantiate an interface extension for the given wl_surface to provide
        /// explicit synchronization.
        ///
        /// If the given wl_surface already has an explicit synchronization object
        /// associated, the synchronization_exists protocol error is raised.
        ///
        /// Graphics APIs, like EGL or Vulkan, that manage the buffer queue and
        /// commits of a wl_surface themselves, are likely to be using this
        /// extension internally. If a client is using such an API for a
        /// wl_surface, it should not directly use this extension on that surface,
        /// to avoid raising a synchronization_exists protocol error.
        GetSynchronization {
            /// The new synchronization interface id,
            id: ZwpLinuxSurfaceSynchronizationV1Id,
            /// The surface,
            surface: WlSurfaceId,
        },
    }
    impl Parser for crate::wayland_objects::zwp_linux_explicit_synchronization_v1::ZwpLinuxExplicitSynchronizationV1 {
    type Request = Request;
    fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
      use Request::*;
      let request = match req.request_id {
        0 => Destroy,
        1 => GetSynchronization {
          id: req.id()?,
          surface: req.id()?,
        },
        _ => return Err(ParserError::InvalidRequest),
      };
      Ok(request)
    }
  }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// The surface already has a synchronization object associated,
        SynchronizationExists = 0,
    }
}

/// Per-surface explicit synchronization support
///
/// This object implements per-surface explicit synchronization.
///
/// Synchronization refers to co-ordination of pipelined operations performed
/// on buffers. Most GPU clients will schedule an asynchronous operation to
/// render to the buffer, then immediately send the buffer to the compositor
/// to be attached to a surface.
///
/// In implicit synchronization, ensuring that the rendering operation is
/// complete before the compositor displays the buffer is an implementation
/// detail handled by either the kernel or userspace graphics driver.
///
/// By contrast, in explicit synchronization, dma_fence objects mark when the
/// asynchronous operations are complete. When submitting a buffer, the
/// client provides an acquire fence which will be waited on before the
/// compositor accesses the buffer. The Wayland server, through a
/// zwp_linux_buffer_release_v1 object, will inform the client with an event
/// which may be accompanied by a release fence, when the compositor will no
/// longer access the buffer contents due to the specific commit that
/// requested the release event.
///
/// Each surface can be associated with only one object of this interface at
/// any time.
///
/// In version 1 of this interface, explicit synchronization is only
/// guaranteed to be supported for buffers created with any version of the
/// wp_linux_dmabuf buffer factory. Version 2 additionally guarantees
/// explicit synchronization support for opaque EGL buffers, which is a type
/// of platform specific buffers described in the EGL_WL_bind_wayland_display
/// extension. Compositors are free to support explicit synchronization for
/// additional buffer types.
pub mod zwp_linux_surface_synchronization_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_linux_surface_synchronization_v1::ZwpLinuxSurfaceSynchronizationV1 {
    const INTERFACE_NAME: &'static str = "zwp_linux_surface_synchronization_v1";
    const INTERFACE_VERSION: u32 = 2;
  }
    impl crate::object::IdToObject for ZwpLinuxSurfaceSynchronizationV1Id {
        type Object = crate::wayland_objects::zwp_linux_surface_synchronization_v1::ZwpLinuxSurfaceSynchronizationV1;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::zwp_linux_surface_synchronization_v1::ZwpLinuxSurfaceSynchronizationV1 {
    type Id = ZwpLinuxSurfaceSynchronizationV1Id;
  }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy synchronization object
        ///
        /// Destroy this explicit synchronization object.
        ///
        /// Any fence set by this object with set_acquire_fence since the last
        /// commit will be discarded by the server. Any fences set by this object
        /// before the last commit are not affected.
        ///
        /// zwp_linux_buffer_release_v1 objects created by this object are not
        /// affected by this request.
        Destroy,
        /// Set the acquire fence
        ///
        /// Set the acquire fence that must be signaled before the compositor
        /// may sample from the buffer attached with wl_surface.attach. The fence
        /// is a dma_fence kernel object.
        ///
        /// The acquire fence is double-buffered state, and will be applied on the
        /// next wl_surface.commit request for the associated surface. Thus, it
        /// applies only to the buffer that is attached to the surface at commit
        /// time.
        ///
        /// If the provided fd is not a valid dma_fence fd, then an INVALID_FENCE
        /// error is raised.
        ///
        /// If a fence has already been attached during the same commit cycle, a
        /// DUPLICATE_FENCE error is raised.
        ///
        /// If the associated wl_surface was destroyed, a NO_SURFACE error is
        /// raised.
        ///
        /// If at surface commit time the attached buffer does not support explicit
        /// synchronization, an UNSUPPORTED_BUFFER error is raised.
        ///
        /// If at surface commit time there is no buffer attached, a NO_BUFFER
        /// error is raised.
        SetAcquireFence {
            /// Acquire fence fd,
            fd: RawFd,
        },
        /// Release fence for last-attached buffer
        ///
        /// Create a listener for the release of the buffer attached by the
        /// client with wl_surface.attach. See zwp_linux_buffer_release_v1
        /// documentation for more information.
        ///
        /// The release object is double-buffered state, and will be associated
        /// with the buffer that is attached to the surface at wl_surface.commit
        /// time.
        ///
        /// If a zwp_linux_buffer_release_v1 object has already been requested for
        /// the surface in the same commit cycle, a DUPLICATE_RELEASE error is
        /// raised.
        ///
        /// If the associated wl_surface was destroyed, a NO_SURFACE error
        /// is raised.
        ///
        /// If at surface commit time there is no buffer attached, a NO_BUFFER
        /// error is raised.
        GetRelease {
            /// New zwp_linux_buffer_release_v1 object,
            release: ZwpLinuxBufferReleaseV1Id,
        },
    }
    impl Parser for crate::wayland_objects::zwp_linux_surface_synchronization_v1::ZwpLinuxSurfaceSynchronizationV1 {
    type Request = Request;
    fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
      use Request::*;
      let request = match req.request_id {
        0 => Destroy,
        1 => SetAcquireFence {
          fd: req.fd()?,
        },
        2 => GetRelease {
          release: req.id()?,
        },
        _ => return Err(ParserError::InvalidRequest),
      };
      Ok(request)
    }
  }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// The fence specified by the client could not be imported,
        InvalidFence = 0,
        /// Multiple fences added for a single surface commit,
        DuplicateFence = 1,
        /// Multiple releases added for a single surface commit,
        DuplicateRelease = 2,
        /// The associated wl_surface was destroyed,
        NoSurface = 3,
        /// The buffer does not support explicit synchronization,
        UnsupportedBuffer = 4,
        /// No buffer was attached,
        NoBuffer = 5,
    }
}

/// Buffer release explicit synchronization
///
/// This object is instantiated in response to a
/// zwp_linux_surface_synchronization_v1.get_release request.
///
/// It provides an alternative to wl_buffer.release events, providing a
/// unique release from a single wl_surface.commit request. The release event
/// also supports explicit synchronization, providing a fence FD for the
/// client to synchronize against.
///
/// Exactly one event, either a fenced_release or an immediate_release, will
/// be emitted for the wl_surface.commit request. The compositor can choose
/// release by release which event it uses.
///
/// This event does not replace wl_buffer.release events; servers are still
/// required to send those events.
///
/// Once a buffer release object has delivered a 'fenced_release' or an
/// 'immediate_release' event it is automatically destroyed.
pub mod zwp_linux_buffer_release_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_linux_buffer_release_v1::ZwpLinuxBufferReleaseV1 {
        const INTERFACE_NAME: &'static str = "zwp_linux_buffer_release_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwpLinuxBufferReleaseV1Id {
        type Object = crate::wayland_objects::zwp_linux_buffer_release_v1::ZwpLinuxBufferReleaseV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_linux_buffer_release_v1::ZwpLinuxBufferReleaseV1
    {
        type Id = ZwpLinuxBufferReleaseV1Id;
    }
    #[derive(Debug)]
    pub enum Event {
        /// Release buffer with fence
        ///
        /// Sent when the compositor has finalised its usage of the associated
        /// buffer for the relevant commit, providing a dma_fence which will be
        /// signaled when all operations by the compositor on that buffer for that
        /// commit have finished.
        ///
        /// Once the fence has signaled, and assuming the associated buffer is not
        /// pending release from other wl_surface.commit requests, no additional
        /// explicit or implicit synchronization is required to safely reuse or
        /// destroy the buffer.
        ///
        /// This event destroys the zwp_linux_buffer_release_v1 object.
        FencedRelease {
            /// Fence for last operation on buffer,
            fence: RawFd,
        },
        /// Release buffer immediately
        ///
        /// Sent when the compositor has finalised its usage of the associated
        /// buffer for the relevant commit, and either performed no operations
        /// using it, or has a guarantee that all its operations on that buffer for
        /// that commit have finished.
        ///
        /// Once this event is received, and assuming the associated buffer is not
        /// pending release from other wl_surface.commit requests, no additional
        /// explicit or implicit synchronization is required to safely reuse or
        /// destroy the buffer.
        ///
        /// This event destroys the zwp_linux_buffer_release_v1 object.
        ImmediateRelease,
    }
    impl EventWriter for crate::wayland_objects::zwp_linux_buffer_release_v1::ZwpLinuxBufferReleaseV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                FencedRelease { fence } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.fd(fence);
                    raw
                }
                ImmediateRelease {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
            }
        }
    }
}

/// Constrain the movement of a pointer
///
/// The global interface exposing pointer constraining functionality. It
/// exposes two requests: lock_pointer for locking the pointer to its
/// position, and confine_pointer for locking the pointer to a region.
///
/// The lock_pointer and confine_pointer requests create the objects
/// wp_locked_pointer and wp_confined_pointer respectively, and the client can
/// use these objects to interact with the lock.
///
/// For any surface, only one lock or confinement may be active across all
/// wl_pointer objects of the same seat. If a lock or confinement is requested
/// when another lock or confinement is active or requested on the same surface
/// and with any of the wl_pointer objects of the same seat, an
/// 'already_constrained' error will be raised.
pub mod zwp_pointer_constraints_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 {
        const INTERFACE_NAME: &'static str = "zwp_pointer_constraints_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwpPointerConstraintsV1Id {
        type Object = crate::wayland_objects::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1
    {
        type Id = ZwpPointerConstraintsV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the pointer constraints manager object
        ///
        /// Used by the client to notify the server that it will no longer use this
        /// pointer constraints object.
        Destroy,
        /// Lock pointer to a position
        ///
        /// The lock_pointer request lets the client request to disable movements of
        /// the virtual pointer (i.e. the cursor), effectively locking the pointer
        /// to a position. This request may not take effect immediately; in the
        /// future, when the compositor deems implementation-specific constraints
        /// are satisfied, the pointer lock will be activated and the compositor
        /// sends a locked event.
        ///
        /// The protocol provides no guarantee that the constraints are ever
        /// satisfied, and does not require the compositor to send an error if the
        /// constraints cannot ever be satisfied. It is thus possible to request a
        /// lock that will never activate.
        ///
        /// There may not be another pointer constraint of any kind requested or
        /// active on the surface for any of the wl_pointer objects of the seat of
        /// the passed pointer when requesting a lock. If there is, an error will be
        /// raised. See general pointer lock documentation for more details.
        ///
        /// The intersection of the region passed with this request and the input
        /// region of the surface is used to determine where the pointer must be
        /// in order for the lock to activate. It is up to the compositor whether to
        /// warp the pointer or require some kind of user interaction for the lock
        /// to activate. If the region is null the surface input region is used.
        ///
        /// A surface may receive pointer focus without the lock being activated.
        ///
        /// The request creates a new object wp_locked_pointer which is used to
        /// interact with the lock as well as receive updates about its state. See
        /// the the description of wp_locked_pointer for further information.
        ///
        /// Note that while a pointer is locked, the wl_pointer objects of the
        /// corresponding seat will not emit any wl_pointer.motion events, but
        /// relative motion events will still be emitted via wp_relative_pointer
        /// objects of the same seat. wl_pointer.axis and wl_pointer.button events
        /// are unaffected.
        LockPointer {
            id: ZwpLockedPointerV1Id,
            /// Surface to lock pointer to,
            surface: WlSurfaceId,
            /// The pointer that should be locked,
            pointer: WlPointerId,
            /// Region of surface,
            region: WlRegionId,
            /// Lock lifetime,
            lifetime: u32,
        },
        /// Confine pointer to a region
        ///
        /// The confine_pointer request lets the client request to confine the
        /// pointer cursor to a given region. This request may not take effect
        /// immediately; in the future, when the compositor deems implementation-
        /// specific constraints are satisfied, the pointer confinement will be
        /// activated and the compositor sends a confined event.
        ///
        /// The intersection of the region passed with this request and the input
        /// region of the surface is used to determine where the pointer must be
        /// in order for the confinement to activate. It is up to the compositor
        /// whether to warp the pointer or require some kind of user interaction for
        /// the confinement to activate. If the region is null the surface input
        /// region is used.
        ///
        /// The request will create a new object wp_confined_pointer which is used
        /// to interact with the confinement as well as receive updates about its
        /// state. See the the description of wp_confined_pointer for further
        /// information.
        ConfinePointer {
            id: ZwpConfinedPointerV1Id,
            /// Surface to lock pointer to,
            surface: WlSurfaceId,
            /// The pointer that should be confined,
            pointer: WlPointerId,
            /// Region of surface,
            region: WlRegionId,
            /// Confinement lifetime,
            lifetime: u32,
        },
    }
    impl Parser for crate::wayland_objects::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => LockPointer {
                    id: req.id()?,
                    surface: req.id()?,
                    pointer: req.id()?,
                    region: req.id()?,
                    lifetime: req.uint()?,
                },
                2 => ConfinePointer {
                    id: req.id()?,
                    surface: req.id()?,
                    pointer: req.id()?,
                    region: req.id()?,
                    lifetime: req.uint()?,
                },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    /// Wp_pointer_constraints error values
    ///
    /// These errors can be emitted in response to wp_pointer_constraints
    /// requests.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Pointer constraint already requested on that surface,
        AlreadyConstrained = 1,
    }
    /// Constraint lifetime
    ///
    /// These values represent different lifetime semantics. They are passed
    /// as arguments to the factory requests to specify how the constraint
    /// lifetimes should be managed.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Lifetime {
        /// The pointer constraint is defunct once deactivated
        ///
        /// A oneshot pointer constraint will never reactivate once it has been
        /// deactivated. See the corresponding deactivation event
        /// (wp_locked_pointer.unlocked and wp_confined_pointer.unconfined) for
        /// details.
        Oneshot = 1,
        /// The pointer constraint may reactivate
        ///
        /// A persistent pointer constraint may again reactivate once it has
        /// been deactivated. See the corresponding deactivation event
        /// (wp_locked_pointer.unlocked and wp_confined_pointer.unconfined) for
        /// details.
        Persistent = 2,
    }
}

/// Receive relative pointer motion events
///
/// The wp_locked_pointer interface represents a locked pointer state.
///
/// While the lock of this object is active, the wl_pointer objects of the
/// associated seat will not emit any wl_pointer.motion events.
///
/// This object will send the event 'locked' when the lock is activated.
/// Whenever the lock is activated, it is guaranteed that the locked surface
/// will already have received pointer focus and that the pointer will be
/// within the region passed to the request creating this object.
///
/// To unlock the pointer, send the destroy request. This will also destroy
/// the wp_locked_pointer object.
///
/// If the compositor decides to unlock the pointer the unlocked event is
/// sent. See wp_locked_pointer.unlock for details.
///
/// When unlocking, the compositor may warp the cursor position to the set
/// cursor position hint. If it does, it will not result in any relative
/// motion events emitted via wp_relative_pointer.
///
/// If the surface the lock was requested on is destroyed and the lock is not
/// yet activated, the wp_locked_pointer object is now defunct and must be
/// destroyed.
pub mod zwp_locked_pointer_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_locked_pointer_v1::ZwpLockedPointerV1 {
        const INTERFACE_NAME: &'static str = "zwp_locked_pointer_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwpLockedPointerV1Id {
        type Object = crate::wayland_objects::zwp_locked_pointer_v1::ZwpLockedPointerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_locked_pointer_v1::ZwpLockedPointerV1
    {
        type Id = ZwpLockedPointerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the locked pointer object
        ///
        /// Destroy the locked pointer object. If applicable, the compositor will
        /// unlock the pointer.
        Destroy,
        /// Set the pointer cursor position hint
        ///
        /// Set the cursor position hint relative to the top left corner of the
        /// surface.
        ///
        /// If the client is drawing its own cursor, it should update the position
        /// hint to the position of its own cursor. A compositor may use this
        /// information to warp the pointer upon unlock in order to avoid pointer
        /// jumps.
        ///
        /// The cursor position hint is double-buffered state, see
        /// wl_surface.commit.
        SetCursorPositionHint {
            /// Surface-local x coordinate,
            surface_x: f32,
            /// Surface-local y coordinate,
            surface_y: f32,
        },
        /// Set a new lock region
        ///
        /// Set a new region used to lock the pointer.
        ///
        /// The new lock region is double-buffered, see wl_surface.commit.
        ///
        /// For details about the lock region, see wp_locked_pointer.
        SetRegion {
            /// Region of surface,
            region: WlRegionId,
        },
    }
    impl Parser for crate::wayland_objects::zwp_locked_pointer_v1::ZwpLockedPointerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => SetCursorPositionHint { surface_x: req.fixed()?, surface_y: req.fixed()? },
                2 => SetRegion { region: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Lock activation event
        ///
        /// Notification that the pointer lock of the seat's pointer is activated.
        Locked,
        /// Lock deactivation event
        ///
        /// Notification that the pointer lock of the seat's pointer is no longer
        /// active. If this is a oneshot pointer lock (see
        /// wp_pointer_constraints.lifetime) this object is now defunct and should
        /// be destroyed. If this is a persistent pointer lock (see
        /// wp_pointer_constraints.lifetime) this pointer lock may again
        /// reactivate in the future.
        Unlocked,
    }
    impl EventWriter for crate::wayland_objects::zwp_locked_pointer_v1::ZwpLockedPointerV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Locked {} => {
                    let mut raw = RawEvent::new(self.id(), 0, 0);
                    raw
                }
                Unlocked {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
            }
        }
    }
}

/// Confined pointer object
///
/// The wp_confined_pointer interface represents a confined pointer state.
///
/// This object will send the event 'confined' when the confinement is
/// activated. Whenever the confinement is activated, it is guaranteed that
/// the surface the pointer is confined to will already have received pointer
/// focus and that the pointer will be within the region passed to the request
/// creating this object. It is up to the compositor to decide whether this
/// requires some user interaction and if the pointer will warp to within the
/// passed region if outside.
///
/// To unconfine the pointer, send the destroy request. This will also destroy
/// the wp_confined_pointer object.
///
/// If the compositor decides to unconfine the pointer the unconfined event is
/// sent. The wp_confined_pointer object is at this point defunct and should
/// be destroyed.
pub mod zwp_confined_pointer_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_confined_pointer_v1::ZwpConfinedPointerV1 {
        const INTERFACE_NAME: &'static str = "zwp_confined_pointer_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwpConfinedPointerV1Id {
        type Object = crate::wayland_objects::zwp_confined_pointer_v1::ZwpConfinedPointerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_confined_pointer_v1::ZwpConfinedPointerV1
    {
        type Id = ZwpConfinedPointerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the confined pointer object
        ///
        /// Destroy the confined pointer object. If applicable, the compositor will
        /// unconfine the pointer.
        Destroy,
        /// Set a new confine region
        ///
        /// Set a new region used to confine the pointer.
        ///
        /// The new confine region is double-buffered, see wl_surface.commit.
        ///
        /// If the confinement is active when the new confinement region is applied
        /// and the pointer ends up outside of newly applied region, the pointer may
        /// warped to a position within the new confinement region. If warped, a
        /// wl_pointer.motion event will be emitted, but no
        /// wp_relative_pointer.relative_motion event.
        ///
        /// The compositor may also, instead of using the new region, unconfine the
        /// pointer.
        ///
        /// For details about the confine region, see wp_confined_pointer.
        SetRegion {
            /// Region of surface,
            region: WlRegionId,
        },
    }
    impl Parser for crate::wayland_objects::zwp_confined_pointer_v1::ZwpConfinedPointerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => SetRegion { region: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Pointer confined
        ///
        /// Notification that the pointer confinement of the seat's pointer is
        /// activated.
        Confined,
        /// Pointer unconfined
        ///
        /// Notification that the pointer confinement of the seat's pointer is no
        /// longer active. If this is a oneshot pointer confinement (see
        /// wp_pointer_constraints.lifetime) this object is now defunct and should
        /// be destroyed. If this is a persistent pointer confinement (see
        /// wp_pointer_constraints.lifetime) this pointer confinement may again
        /// reactivate in the future.
        Unconfined,
    }
    impl EventWriter for crate::wayland_objects::zwp_confined_pointer_v1::ZwpConfinedPointerV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Confined {} => {
                    let mut raw = RawEvent::new(self.id(), 0, 0);
                    raw
                }
                Unconfined {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
            }
        }
    }
}

/// X primary selection emulation
///
/// The primary selection device manager is a singleton global object that
/// provides access to the primary selection. It allows to create
/// wp_primary_selection_source objects, as well as retrieving the per-seat
/// wp_primary_selection_device objects.
pub mod zwp_primary_selection_device_manager_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 {
    const INTERFACE_NAME: &'static str = "zwp_primary_selection_device_manager_v1";
    const INTERFACE_VERSION: u32 = 1;
  }
    impl crate::object::IdToObject for ZwpPrimarySelectionDeviceManagerV1Id {
        type Object = crate::wayland_objects::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 {
    type Id = ZwpPrimarySelectionDeviceManagerV1Id;
  }
    #[derive(Debug)]
    pub enum Request {
        /// Create a new primary selection source
        ///
        /// Create a new primary selection source.
        CreateSource { id: ZwpPrimarySelectionSourceV1Id },
        /// Create a new primary selection device
        ///
        /// Create a new data device for a given seat.
        GetDevice { id: ZwpPrimarySelectionDeviceV1Id, seat: WlSeatId },
        /// Destroy the primary selection device manager
        ///
        /// Destroy the primary selection device manager.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1 {
    type Request = Request;
    fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
      use Request::*;
      let request = match req.request_id {
        0 => CreateSource {
          id: req.id()?,
        },
        1 => GetDevice {
          id: req.id()?,
          seat: req.id()?,
        },
        2 => Destroy,
        _ => return Err(ParserError::InvalidRequest),
      };
      Ok(request)
    }
  }
}

pub mod zwp_primary_selection_device_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1
    {
        const INTERFACE_NAME: &'static str = "zwp_primary_selection_device_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwpPrimarySelectionDeviceV1Id {
        type Object =
            crate::wayland_objects::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1
    {
        type Id = ZwpPrimarySelectionDeviceV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Set the primary selection
        ///
        /// Replaces the current selection. The previous owner of the primary
        /// selection will receive a wp_primary_selection_source.cancelled event.
        ///
        /// To unset the selection, set the source to NULL.
        SetSelection {
            source: ZwpPrimarySelectionSourceV1Id,
            /// Serial of the event that triggered this request,
            serial: u32,
        },
        /// Destroy the primary selection device
        ///
        /// Destroy the primary selection device.
        Destroy,
    }
    impl Parser
        for crate::wayland_objects::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1
    {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetSelection { source: req.id()?, serial: req.uint()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Introduce a new wp_primary_selection_offer
        ///
        /// Introduces a new wp_primary_selection_offer object that may be used
        /// to receive the current primary selection. Immediately following this
        /// event, the new wp_primary_selection_offer object will send
        /// wp_primary_selection_offer.offer events to describe the offered mime
        /// types.
        DataOffer { offer: ZwpPrimarySelectionOfferV1Id },
        /// Advertise a new primary selection
        ///
        /// The wp_primary_selection_device.selection event is sent to notify the
        /// client of a new primary selection. This event is sent after the
        /// wp_primary_selection.data_offer event introducing this object, and after
        /// the offer has announced its mimetypes through
        /// wp_primary_selection_offer.offer.
        ///
        /// The data_offer is valid until a new offer or NULL is received
        /// or until the client loses keyboard focus. The client must destroy the
        /// previous selection data_offer, if any, upon receiving this event.
        Selection { id: ZwpPrimarySelectionOfferV1Id },
    }
    impl EventWriter
        for crate::wayland_objects::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1
    {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                DataOffer { offer } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.id(offer);
                    raw
                }
                Selection { id } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.id(id);
                    raw
                }
            }
        }
    }
}

/// Offer to transfer primary selection contents
///
/// A wp_primary_selection_offer represents an offer to transfer the contents
/// of the primary selection clipboard to the client. Similar to
/// wl_data_offer, the offer also describes the mime types that the data can
/// be converted to and provides the mechanisms for transferring the data
/// directly to the client.
pub mod zwp_primary_selection_offer_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1
    {
        const INTERFACE_NAME: &'static str = "zwp_primary_selection_offer_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwpPrimarySelectionOfferV1Id {
        type Object =
            crate::wayland_objects::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1
    {
        type Id = ZwpPrimarySelectionOfferV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Request that the data is transferred
        ///
        /// To transfer the contents of the primary selection clipboard, the client
        /// issues this request and indicates the mime type that it wants to
        /// receive. The transfer happens through the passed file descriptor
        /// (typically created with the pipe system call). The source client writes
        /// the data in the mime type representation requested and then closes the
        /// file descriptor.
        ///
        /// The receiving client reads from the read end of the pipe until EOF and
        /// closes its end, at which point the transfer is complete.
        Receive { mime_type: String, fd: RawFd },
        /// Destroy the primary selection offer
        ///
        /// Destroy the primary selection offer.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Receive { mime_type: req.string()?, fd: req.fd()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Advertise offered mime type
        ///
        /// Sent immediately after creating announcing the
        /// wp_primary_selection_offer through
        /// wp_primary_selection_device.data_offer. One event is sent per offered
        /// mime type.
        Offer { mime_type: String },
    }
    impl EventWriter
        for crate::wayland_objects::zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1
    {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Offer { mime_type } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.string(mime_type);
                    raw
                }
            }
        }
    }
}

/// Offer to replace the contents of the primary selection
///
/// The source side of a wp_primary_selection_offer, it provides a way to
/// describe the offered data and respond to requests to transfer the
/// requested contents of the primary selection clipboard.
pub mod zwp_primary_selection_source_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1
    {
        const INTERFACE_NAME: &'static str = "zwp_primary_selection_source_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwpPrimarySelectionSourceV1Id {
        type Object =
            crate::wayland_objects::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1
    {
        type Id = ZwpPrimarySelectionSourceV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Add an offered mime type
        ///
        /// This request adds a mime type to the set of mime types advertised to
        /// targets. Can be called several times to offer multiple types.
        Offer { mime_type: String },
        /// Destroy the primary selection source
        ///
        /// Destroy the primary selection source.
        Destroy,
    }
    impl Parser
        for crate::wayland_objects::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1
    {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Offer { mime_type: req.string()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Send the primary selection contents
        ///
        /// Request for the current primary selection contents from the client.
        /// Send the specified mime type over the passed file descriptor, then
        /// close it.
        Send { mime_type: String, fd: RawFd },
        /// Request for primary selection contents was canceled
        ///
        /// This primary selection source is no longer valid. The client should
        /// clean up and destroy this primary selection source.
        Cancelled,
    }
    impl EventWriter
        for crate::wayland_objects::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1
    {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Send { mime_type, fd } => {
                    let mut raw = RawEvent::new(self.id(), 0, 2);
                    raw.string(mime_type);
                    raw.fd(fd);
                    raw
                }
                Cancelled {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
            }
        }
    }
}

/// Get relative pointer objects
///
/// A global interface used for getting the relative pointer object for a
/// given pointer.
pub mod zwp_relative_pointer_manager_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1
    {
        const INTERFACE_NAME: &'static str = "zwp_relative_pointer_manager_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwpRelativePointerManagerV1Id {
        type Object =
            crate::wayland_objects::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1
    {
        type Id = ZwpRelativePointerManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the relative pointer manager object
        ///
        /// Used by the client to notify the server that it will no longer use this
        /// relative pointer manager object.
        Destroy,
        /// Get a relative pointer object
        ///
        /// Create a relative pointer interface given a wl_pointer object. See the
        /// wp_relative_pointer interface for more details.
        GetRelativePointer { id: ZwpRelativePointerV1Id, pointer: WlPointerId },
    }
    impl Parser
        for crate::wayland_objects::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1
    {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetRelativePointer { id: req.id()?, pointer: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// Relative pointer object
///
/// A wp_relative_pointer object is an extension to the wl_pointer interface
/// used for emitting relative pointer events. It shares the same focus as
/// wl_pointer objects of the same seat and will only emit events when it has
/// focus.
pub mod zwp_relative_pointer_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwp_relative_pointer_v1::ZwpRelativePointerV1 {
        const INTERFACE_NAME: &'static str = "zwp_relative_pointer_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwpRelativePointerV1Id {
        type Object = crate::wayland_objects::zwp_relative_pointer_v1::ZwpRelativePointerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwp_relative_pointer_v1::ZwpRelativePointerV1
    {
        type Id = ZwpRelativePointerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Release the relative pointer object
        ///
        ///
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwp_relative_pointer_v1::ZwpRelativePointerV1 {
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
        /// Relative pointer motion
        ///
        /// Relative x/y pointer motion from the pointer of the seat associated with
        /// this object.
        ///
        /// A relative motion is in the same dimension as regular wl_pointer motion
        /// events, except they do not represent an absolute position. For example,
        /// moving a pointer from (x, y) to (x', y') would have the equivalent
        /// relative motion (x' - x, y' - y). If a pointer motion caused the
        /// absolute pointer position to be clipped by for example the edge of the
        /// monitor, the relative motion is unaffected by the clipping and will
        /// represent the unclipped motion.
        ///
        /// This event also contains non-accelerated motion deltas. The
        /// non-accelerated delta is, when applicable, the regular pointer motion
        /// delta as it was before having applied motion acceleration and other
        /// transformations such as normalization.
        ///
        /// Note that the non-accelerated delta does not represent 'raw' events as
        /// they were read from some device. Pointer motion acceleration is device-
        /// and configuration-specific and non-accelerated deltas and accelerated
        /// deltas may have the same value on some devices.
        ///
        /// Relative motions are not coupled to wl_pointer.motion events, and can be
        /// sent in combination with such events, but also independently. There may
        /// also be scenarios where wl_pointer.motion is sent, but there is no
        /// relative motion. The order of an absolute and relative motion event
        /// originating from the same physical motion is not guaranteed.
        ///
        /// If the client needs button events or focus state, it can receive them
        /// from a wl_pointer object of the same seat that the wp_relative_pointer
        /// object is associated with.
        RelativeMotion {
            /// High 32 bits of a 64 bit timestamp with microsecond granularity,
            utime_hi: u32,
            /// Low 32 bits of a 64 bit timestamp with microsecond granularity,
            utime_lo: u32,
            /// The x component of the motion vector,
            dx: f32,
            /// The y component of the motion vector,
            dy: f32,
            /// The x component of the unaccelerated motion vector,
            dx_unaccel: f32,
            /// The y component of the unaccelerated motion vector,
            dy_unaccel: f32,
        },
    }
    impl EventWriter for crate::wayland_objects::zwp_relative_pointer_v1::ZwpRelativePointerV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                RelativeMotion { utime_hi, utime_lo, dx, dy, dx_unaccel, dy_unaccel } => {
                    let mut raw = RawEvent::new(self.id(), 0, 6);
                    raw.uint(utime_hi);
                    raw.uint(utime_lo);
                    raw.fixed(dx);
                    raw.fixed(dy);
                    raw.fixed(dx_unaccel);
                    raw.fixed(dy_unaccel);
                    raw
                }
            }
        }
    }
}

/// Window decoration manager
///
/// This interface allows a compositor to announce support for server-side
/// decorations.
///
/// A window decoration is a set of window controls as deemed appropriate by
/// the party managing them, such as user interface components used to move,
/// resize and change a window's state.
///
/// A client can use this protocol to request being decorated by a supporting
/// compositor.
///
/// If compositor and client do not negotiate the use of a server-side
/// decoration using this protocol, clients continue to self-decorate as they
/// see fit.
///
/// Warning! The protocol described in this file is experimental and
/// backward incompatible changes may be made. Backward compatible changes
/// may be added together with the corresponding interface version bump.
/// Backward incompatible changes are done by bumping the version number in
/// the protocol and interface names and resetting the interface version.
/// Once the protocol is to be declared stable, the 'z' prefix and the
/// version number in the protocol and interface names are removed and the
/// interface version number is reset.
pub mod zxdg_decoration_manager_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 {
        const INTERFACE_NAME: &'static str = "zxdg_decoration_manager_v1";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ZxdgDecorationManagerV1Id {
        type Object = crate::wayland_objects::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1
    {
        type Id = ZxdgDecorationManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the decoration manager object
        ///
        /// Destroy the decoration manager. This doesn't destroy objects created
        /// with the manager.
        Destroy,
        /// Create a new toplevel decoration object
        ///
        /// Create a new decoration object associated with the given toplevel.
        ///
        /// For objects of version 1, creating an xdg_toplevel_decoration from an
        /// xdg_toplevel which has a buffer attached or committed is a client
        /// error, and any attempts by a client to attach or manipulate a buffer
        /// prior to the first xdg_toplevel_decoration.configure event must also be
        /// treated as errors.
        ///
        /// For objects of version 2 or newer, creating an xdg_toplevel_decoration
        /// from an xdg_toplevel which has a buffer attached or committed is
        /// allowed. The initial decoration mode of the surface if a buffer is
        /// already attached depends on whether a xdg_toplevel_decoration object
        /// has been associated with the surface or not prior to this request.
        ///
        /// If an xdg_toplevel_decoration was associated with the surface, then
        /// destroyed without a surface commit, the previous decoration mode is
        /// retained.
        ///
        /// If no xdg_toplevel_decoration was associated with the surface prior to
        /// this request, or if a surface commit has been performed after a previous
        /// xdg_toplevel_decoration object associated with the surface was
        /// destroyed, the decoration mode is assumed to be client-side.
        GetToplevelDecoration { id: ZxdgToplevelDecorationV1Id, toplevel: XdgToplevelId },
    }
    impl Parser for crate::wayland_objects::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetToplevelDecoration { id: req.id()?, toplevel: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// Decoration object for a toplevel surface
///
/// The decoration object allows the compositor to toggle server-side window
/// decorations for a toplevel surface. The client can request to switch to
/// another mode.
///
/// The xdg_toplevel_decoration object must be destroyed before its
/// xdg_toplevel.
pub mod zxdg_toplevel_decoration_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1 {
        const INTERFACE_NAME: &'static str = "zxdg_toplevel_decoration_v1";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ZxdgToplevelDecorationV1Id {
        type Object = crate::wayland_objects::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1
    {
        type Id = ZxdgToplevelDecorationV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the decoration object
        ///
        /// Switch back to a mode without any server-side decorations at the next
        /// commit, unless a new xdg_toplevel_decoration is created for the surface
        /// first.
        Destroy,
        /// Set the decoration mode
        ///
        /// Set the toplevel surface decoration mode. This informs the compositor
        /// that the client prefers the provided decoration mode.
        ///
        /// After requesting a decoration mode, the compositor will respond by
        /// emitting an xdg_surface.configure event. The client should then update
        /// its content, drawing it without decorations if the received mode is
        /// server-side decorations. The client must also acknowledge the configure
        /// when committing the new content (see xdg_surface.ack_configure).
        ///
        /// The compositor can decide not to use the client's mode and enforce a
        /// different mode instead.
        ///
        /// Clients whose decoration mode depend on the xdg_toplevel state may send
        /// a set_mode request in response to an xdg_surface.configure event and wait
        /// for the next xdg_surface.configure event to prevent unwanted state.
        /// Such clients are responsible for preventing configure loops and must
        /// make sure not to send multiple successive set_mode requests with the
        /// same decoration mode.
        ///
        /// If an invalid mode is supplied by the client, the invalid_mode protocol
        /// error is raised by the compositor.
        SetMode {
            /// The decoration mode,
            mode: u32,
        },
        /// Unset the decoration mode
        ///
        /// Unset the toplevel surface decoration mode. This informs the compositor
        /// that the client doesn't prefer a particular decoration mode.
        ///
        /// This request has the same semantics as set_mode.
        UnsetMode,
    }
    impl Parser for crate::wayland_objects::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => SetMode { mode: req.uint()? },
                2 => UnsetMode,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Notify a decoration mode change
        ///
        /// The configure event configures the effective decoration mode. The
        /// configured state should not be applied immediately. Clients must send an
        /// ack_configure in response to this event. See xdg_surface.configure and
        /// xdg_surface.ack_configure for details.
        ///
        /// A configure event can be sent at any time. The specified mode must be
        /// obeyed by the client.
        Configure {
            /// The decoration mode,
            mode: u32,
        },
    }
    impl EventWriter for crate::wayland_objects::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Configure { mode } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(mode);
                    raw
                }
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Xdg_toplevel has a buffer attached before configure,
        UnconfiguredBuffer = 0,
        /// Xdg_toplevel already has a decoration object,
        AlreadyConstructed = 1,
        /// Xdg_toplevel destroyed before the decoration object,
        Orphaned = 2,
        /// Invalid mode,
        InvalidMode = 3,
    }
    /// Window decoration modes
    ///
    /// These values describe window decoration modes.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Mode {
        /// No server-side window decoration,
        ClientSide = 1,
        /// Server-side window decoration,
        ServerSide = 2,
    }
}

/// Manage xdg_output objects
///
/// A global factory interface for xdg_output objects.
pub mod zxdg_output_manager_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zxdg_output_manager_v1::ZxdgOutputManagerV1 {
        const INTERFACE_NAME: &'static str = "zxdg_output_manager_v1";
        const INTERFACE_VERSION: u32 = 3;
    }
    impl crate::object::IdToObject for ZxdgOutputManagerV1Id {
        type Object = crate::wayland_objects::zxdg_output_manager_v1::ZxdgOutputManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zxdg_output_manager_v1::ZxdgOutputManagerV1
    {
        type Id = ZxdgOutputManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the xdg_output_manager object
        ///
        /// Using this request a client can tell the server that it is not
        /// going to use the xdg_output_manager object anymore.
        ///
        /// Any objects already created through this instance are not affected.
        Destroy,
        /// Create an xdg output from a wl_output
        ///
        /// This creates a new xdg_output object for the given wl_output.
        GetXdgOutput { id: ZxdgOutputV1Id, output: WlOutputId },
    }
    impl Parser for crate::wayland_objects::zxdg_output_manager_v1::ZxdgOutputManagerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => GetXdgOutput { id: req.id()?, output: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// Compositor logical output region
///
/// An xdg_output describes part of the compositor geometry.
///
/// This typically corresponds to a monitor that displays part of the
/// compositor space.
///
/// For objects version 3 onwards, after all xdg_output properties have been
/// sent (when the object is created and when properties are updated), a
/// wl_output.done event is sent. This allows changes to the output
/// properties to be seen as atomic, even if they happen via multiple events.
pub mod zxdg_output_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zxdg_output_v1::ZxdgOutputV1 {
        const INTERFACE_NAME: &'static str = "zxdg_output_v1";
        const INTERFACE_VERSION: u32 = 3;
    }
    impl crate::object::IdToObject for ZxdgOutputV1Id {
        type Object = crate::wayland_objects::zxdg_output_v1::ZxdgOutputV1;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::zxdg_output_v1::ZxdgOutputV1 {
        type Id = ZxdgOutputV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the xdg_output object
        ///
        /// Using this request a client can tell the server that it is not
        /// going to use the xdg_output object anymore.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zxdg_output_v1::ZxdgOutputV1 {
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
    /// Minimum interface version for the `name` message (XML `since` / `version`).
    pub const NAME_SINCE: u32 = 2;
    /// Minimum interface version for the `description` message (XML `since` / `version`).
    pub const DESCRIPTION_SINCE: u32 = 2;
    #[derive(Debug)]
    pub enum Event {
        /// Position of the output within the global compositor space
        ///
        /// The position event describes the location of the wl_output within
        /// the global compositor space.
        ///
        /// The logical_position event is sent after creating an xdg_output
        /// (see xdg_output_manager.get_xdg_output) and whenever the location
        /// of the output changes within the global compositor space.
        LogicalPosition {
            /// X position within the global compositor space,
            x: i32,
            /// Y position within the global compositor space,
            y: i32,
        },
        /// Size of the output in the global compositor space
        ///
        /// The logical_size event describes the size of the output in the
        /// global compositor space.
        ///
        /// Most regular Wayland clients should not pay attention to the
        /// logical size and would rather rely on xdg_shell interfaces.
        ///
        /// Some clients such as Xwayland, however, need this to configure
        /// their surfaces in the global compositor space as the compositor
        /// may apply a different scale from what is advertised by the output
        /// scaling property (to achieve fractional scaling, for example).
        ///
        /// For example, for a wl_output mode 3840×2160 and a scale factor 2:
        ///
        /// - A compositor not scaling the monitor viewport in its compositing space
        /// will advertise a logical size of 3840×2160,
        ///
        /// - A compositor scaling the monitor viewport with scale factor 2 will
        /// advertise a logical size of 1920×1080,
        ///
        /// - A compositor scaling the monitor viewport using a fractional scale of
        /// 1.5 will advertise a logical size of 2560×1440.
        ///
        /// For example, for a wl_output mode 1920×1080 and a 90 degree rotation,
        /// the compositor will advertise a logical size of 1080x1920.
        ///
        /// The logical_size event is sent after creating an xdg_output
        /// (see xdg_output_manager.get_xdg_output) and whenever the logical
        /// size of the output changes, either as a result of a change in the
        /// applied scale or because of a change in the corresponding output
        /// mode(see wl_output.mode) or transform (see wl_output.transform).
        LogicalSize {
            /// Width in global compositor space,
            width: i32,
            /// Height in global compositor space,
            height: i32,
        },
        /// All information about the output have been sent
        ///
        /// This event is sent after all other properties of an xdg_output
        /// have been sent.
        ///
        /// This allows changes to the xdg_output properties to be seen as
        /// atomic, even if they happen via multiple events.
        ///
        /// For objects version 3 onwards, this event is deprecated. Compositors
        /// are not required to send it anymore and must send wl_output.done
        /// instead.
        Done,
        /// Name of this output
        ///
        /// Many compositors will assign names to their outputs, show them to the
        /// user, allow them to be configured by name, etc. The client may wish to
        /// know this name as well to offer the user similar behaviors.
        ///
        /// The naming convention is compositor defined, but limited to
        /// alphanumeric characters and dashes (-). Each name is unique among all
        /// wl_output globals, but if a wl_output global is destroyed the same name
        /// may be reused later. The names will also remain consistent across
        /// sessions with the same hardware and software configuration.
        ///
        /// Examples of names include 'HDMI-A-1', 'WL-1', 'X11-1', etc. However, do
        /// not assume that the name is a reflection of an underlying DRM
        /// connector, X11 connection, etc.
        ///
        /// The name event is sent after creating an xdg_output (see
        /// xdg_output_manager.get_xdg_output). This event is only sent once per
        /// xdg_output, and the name does not change over the lifetime of the
        /// wl_output global.
        ///
        /// This event is deprecated, instead clients should use wl_output.name.
        /// Compositors must still support this event.
        Name {
            /// Output name,
            name: String,
        },
        /// Human-readable description of this output
        ///
        /// Many compositors can produce human-readable descriptions of their
        /// outputs.  The client may wish to know this description as well, to
        /// communicate the user for various purposes.
        ///
        /// The description is a UTF-8 string with no convention defined for its
        /// contents. Examples might include 'Foocorp 11" Display' or 'Virtual X11
        /// output via :1'.
        ///
        /// The description event is sent after creating an xdg_output (see
        /// xdg_output_manager.get_xdg_output) and whenever the description
        /// changes. The description is optional, and may not be sent at all.
        ///
        /// For objects of version 2 and lower, this event is only sent once per
        /// xdg_output, and the description does not change over the lifetime of
        /// the wl_output global.
        ///
        /// This event is deprecated, instead clients should use
        /// wl_output.description. Compositors must still support this event.
        Description {
            /// Output description,
            description: String,
        },
    }
    impl EventWriter for crate::wayland_objects::zxdg_output_v1::ZxdgOutputV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                LogicalPosition { x, y } => {
                    let mut raw = RawEvent::new(self.id(), 0, 2);
                    raw.int(x);
                    raw.int(y);
                    raw
                }
                LogicalSize { width, height } => {
                    let mut raw = RawEvent::new(self.id(), 1, 2);
                    raw.int(width);
                    raw.int(height);
                    raw
                }
                Done {} => {
                    let mut raw = RawEvent::new(self.id(), 2, 0);
                    raw
                }
                Name { name } => {
                    let mut raw = RawEvent::new(self.id(), 3, 1);
                    raw.string(name);
                    raw
                }
                Description { description } => {
                    let mut raw = RawEvent::new(self.id(), 4, 1);
                    raw.string(description);
                    raw
                }
            }
        }
    }
}

/// List and control workspaces
///
/// Workspaces, also called virtual desktops, are groups of surfaces. A
/// compositor with a concept of workspaces may only show some such groups of
/// surfaces (those of 'active' workspaces) at a time. 'Activating' a
/// workspace is a request for the compositor to display that workspace's
/// surfaces as normal, whereas the compositor may hide or otherwise
/// de-emphasise surfaces that are associated only with 'inactive' workspaces.
/// Workspaces are grouped by which sets of outputs they correspond to, and
/// may contain surfaces only from those outputs. In this way, it is possible
/// for each output to have its own set of workspaces, or for all outputs (or
/// any other arbitrary grouping) to share workspaces. Compositors may
/// optionally conceptually arrange each group of workspaces in an
/// N-dimensional grid.
///
/// The purpose of this protocol is to enable the creation of taskbars and
/// docks by providing them with a list of workspaces and their properties,
/// and allowing them to activate and deactivate workspaces.
///
/// After a client binds the ext_workspace_manager_v1, each workspace will be
/// sent via the workspace event.
pub mod ext_workspace_manager_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::ext_workspace_manager_v1::ExtWorkspaceManagerV1 {
        const INTERFACE_NAME: &'static str = "ext_workspace_manager_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtWorkspaceManagerV1Id {
        type Object = crate::wayland_objects::ext_workspace_manager_v1::ExtWorkspaceManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_workspace_manager_v1::ExtWorkspaceManagerV1
    {
        type Id = ExtWorkspaceManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// All requests about the workspaces have been sent
        ///
        /// The client must send this request after it has finished sending other
        /// requests. The compositor must process a series of requests preceding a
        /// commit request atomically.
        ///
        /// This allows changes to the workspace properties to be seen as atomic,
        /// even if they happen via multiple events, and even if they involve
        /// multiple ext_workspace_handle_v1 objects, for example, deactivating one
        /// workspace and activating another.
        Commit,
        /// Stop sending events
        ///
        /// Indicates the client no longer wishes to receive events for new
        /// workspace groups. However the compositor may emit further workspace
        /// events, until the finished event is emitted. The compositor is expected
        /// to send the finished event eventually once the stop request has been processed.
        ///
        /// The client must not send any requests after this one, doing so will raise a wl_display
        /// invalid_object error.
        Stop,
    }
    impl Parser for crate::wayland_objects::ext_workspace_manager_v1::ExtWorkspaceManagerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Commit,
                1 => Stop,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// A workspace group has been created
        ///
        /// This event is emitted whenever a new workspace group has been created.
        ///
        /// All initial details of the workspace group (outputs) will be
        /// sent immediately after this event via the corresponding events in
        /// ext_workspace_group_handle_v1 and ext_workspace_handle_v1.
        WorkspaceGroup { workspace_group: ExtWorkspaceGroupHandleV1Id },
        /// Workspace has been created
        ///
        /// This event is emitted whenever a new workspace has been created.
        ///
        /// All initial details of the workspace (name, coordinates, state) will
        /// be sent immediately after this event via the corresponding events in
        /// ext_workspace_handle_v1.
        ///
        /// Workspaces start off unassigned to any workspace group.
        Workspace { workspace: ExtWorkspaceHandleV1Id },
        /// All information about the workspaces and workspace groups has been sent
        ///
        /// This event is sent after all changes in all workspaces and workspace groups have been
        /// sent.
        ///
        /// This allows changes to one or more ext_workspace_group_handle_v1
        /// properties and ext_workspace_handle_v1 properties
        /// to be seen as atomic, even if they happen via multiple events.
        /// In particular, an output moving from one workspace group to
        /// another sends an output_enter event and an output_leave event to the two
        /// ext_workspace_group_handle_v1 objects in question. The compositor sends
        /// the done event only after updating the output information in both
        /// workspace groups.
        Done,
        /// The compositor has finished with the workspace_manager
        ///
        /// This event indicates that the compositor is done sending events to the
        /// ext_workspace_manager_v1. The server will destroy the object
        /// immediately after sending this request.
        Finished,
    }
    impl EventWriter for crate::wayland_objects::ext_workspace_manager_v1::ExtWorkspaceManagerV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                WorkspaceGroup { workspace_group } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.id(workspace_group);
                    raw
                }
                Workspace { workspace } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.id(workspace);
                    raw
                }
                Done {} => {
                    let mut raw = RawEvent::new(self.id(), 2, 0);
                    raw
                }
                Finished {} => {
                    let mut raw = RawEvent::new(self.id(), 3, 0);
                    raw
                }
            }
        }
    }
}

/// A workspace group assigned to a set of outputs
///
/// A ext_workspace_group_handle_v1 object represents a workspace group
/// that is assigned a set of outputs and contains a number of workspaces.
///
/// The set of outputs assigned to the workspace group is conveyed to the client via
/// output_enter and output_leave events, and its workspaces are conveyed with
/// workspace events.
///
/// For example, a compositor which has a set of workspaces for each output may
/// advertise a workspace group (and its workspaces) per output, whereas a compositor
/// where a workspace spans all outputs may advertise a single workspace group for all
/// outputs.
pub mod ext_workspace_group_handle_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1
    {
        const INTERFACE_NAME: &'static str = "ext_workspace_group_handle_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtWorkspaceGroupHandleV1Id {
        type Object =
            crate::wayland_objects::ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1
    {
        type Id = ExtWorkspaceGroupHandleV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Create a new workspace
        ///
        /// Request that the compositor create a new workspace with the given name
        /// and assign it to this group.
        ///
        /// There is no guarantee that the compositor will create a new workspace,
        /// or that the created workspace will have the provided name.
        CreateWorkspace { workspace: String },
        /// Destroy the ext_workspace_group_handle_v1 object
        ///
        /// Destroys the ext_workspace_group_handle_v1 object.
        ///
        /// This request should be send either when the client does not want to
        /// use the workspace group object any more or after the removed event to finalize
        /// the destruction of the object.
        Destroy,
    }
    impl Parser for crate::wayland_objects::ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => CreateWorkspace { workspace: req.string()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Compositor capabilities
        ///
        /// This event advertises the capabilities supported by the compositor. If
        /// a capability isn't supported, clients should hide or disable the UI
        /// elements that expose this functionality. For instance, if the
        /// compositor doesn't advertise support for creating workspaces, a button
        /// triggering the create_workspace request should not be displayed.
        ///
        /// The compositor will ignore requests it doesn't support. For instance,
        /// a compositor which doesn't advertise support for creating workspaces will ignore
        /// create_workspace requests.
        ///
        /// Compositors must send this event once after creation of an
        /// ext_workspace_group_handle_v1. When the capabilities change, compositors
        /// must send this event again.
        Capabilities {
            /// Capabilities,
            capabilities: u32,
        },
        /// Output assigned to workspace group
        ///
        /// This event is emitted whenever an output is assigned to the workspace
        /// group or a new `wl_output` object is bound by the client, which was already
        /// assigned to this workspace_group.
        OutputEnter { output: WlOutputId },
        /// Output removed from workspace group
        ///
        /// This event is emitted whenever an output is removed from the workspace
        /// group.
        OutputLeave { output: WlOutputId },
        /// Workspace added to workspace group
        ///
        /// This event is emitted whenever a workspace is assigned to this group.
        /// A workspace may only ever be assigned to a single group at a single point
        /// in time, but can be re-assigned during its lifetime.
        WorkspaceEnter { workspace: ExtWorkspaceHandleV1Id },
        /// Workspace removed from workspace group
        ///
        /// This event is emitted whenever a workspace is removed from this group.
        WorkspaceLeave { workspace: ExtWorkspaceHandleV1Id },
        /// This workspace group has been removed
        ///
        /// This event is send when the group associated with the ext_workspace_group_handle_v1
        /// has been removed. After sending this request the compositor will immediately consider
        /// the object inert. Any requests will be ignored except the destroy request.
        /// It is guaranteed there won't be any more events referencing this
        /// ext_workspace_group_handle_v1.
        ///
        /// The compositor must remove all workspaces belonging to a workspace group
        /// via a workspace_leave event before removing the workspace group.
        Removed,
    }
    impl EventWriter
        for crate::wayland_objects::ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1
    {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Capabilities { capabilities } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(capabilities);
                    raw
                }
                OutputEnter { output } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.id(output);
                    raw
                }
                OutputLeave { output } => {
                    let mut raw = RawEvent::new(self.id(), 2, 1);
                    raw.id(output);
                    raw
                }
                WorkspaceEnter { workspace } => {
                    let mut raw = RawEvent::new(self.id(), 3, 1);
                    raw.id(workspace);
                    raw
                }
                WorkspaceLeave { workspace } => {
                    let mut raw = RawEvent::new(self.id(), 4, 1);
                    raw.id(workspace);
                    raw
                }
                Removed {} => {
                    let mut raw = RawEvent::new(self.id(), 5, 0);
                    raw
                }
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum GroupCapabilities {
        /// Create_workspace request is available,
        CreateWorkspace = 1,
    }
}

/// A workspace handing a group of surfaces
///
/// A ext_workspace_handle_v1 object represents a workspace that handles a
/// group of surfaces.
///
/// Each workspace has:
/// - a name, conveyed to the client with the name event
/// - potentially an id conveyed with the id event
/// - a list of states, conveyed to the client with the state event
/// - and optionally a set of coordinates, conveyed to the client with the
/// coordinates event
///
/// The client may request that the compositor activate or deactivate the workspace.
///
/// Each workspace can belong to only a single workspace group.
/// Depending on the compositor policy, there might be workspaces with
/// the same name in different workspace groups, but these workspaces are still
/// separate (e.g. one of them might be active while the other is not).
pub mod ext_workspace_handle_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::ext_workspace_handle_v1::ExtWorkspaceHandleV1 {
        const INTERFACE_NAME: &'static str = "ext_workspace_handle_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ExtWorkspaceHandleV1Id {
        type Object = crate::wayland_objects::ext_workspace_handle_v1::ExtWorkspaceHandleV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::ext_workspace_handle_v1::ExtWorkspaceHandleV1
    {
        type Id = ExtWorkspaceHandleV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Destroy the ext_workspace_handle_v1 object
        ///
        /// Destroys the ext_workspace_handle_v1 object.
        ///
        /// This request should be made either when the client does not want to
        /// use the workspace object any more or after the remove event to finalize
        /// the destruction of the object.
        Destroy,
        /// Activate the workspace
        ///
        /// Request that this workspace be activated.
        ///
        /// There is no guarantee the workspace will be actually activated, and
        /// behaviour may be compositor-dependent. For example, activating a
        /// workspace may or may not deactivate all other workspaces in the same
        /// group.
        Activate,
        /// Deactivate the workspace
        ///
        /// Request that this workspace be deactivated.
        ///
        /// There is no guarantee the workspace will be actually deactivated.
        Deactivate,
        /// Assign workspace to group
        ///
        /// Requests that this workspace is assigned to the given workspace group.
        ///
        /// There is no guarantee the workspace will be assigned.
        Assign { workspace_group: ExtWorkspaceGroupHandleV1Id },
        /// Remove the workspace
        ///
        /// Request that this workspace be removed.
        ///
        /// There is no guarantee the workspace will be actually removed.
        Remove,
    }
    impl Parser for crate::wayland_objects::ext_workspace_handle_v1::ExtWorkspaceHandleV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Destroy,
                1 => Activate,
                2 => Deactivate,
                3 => Assign { workspace_group: req.id()? },
                4 => Remove,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Workspace id
        ///
        /// If this event is emitted, it will be send immediately after the
        /// ext_workspace_handle_v1 is created or when an id is assigned to
        /// a workspace (at most once during its lifetime).
        ///
        /// An id will never change during the lifetime of the `ext_workspace_handle_v1`
        /// and is guaranteed to be unique during its lifetime.
        ///
        /// Ids are not human-readable and shouldn't be displayed, use `name` for that purpose.
        ///
        /// Compositors are expected to only send ids for workspaces likely stable across multiple
        /// sessions and can be used by clients to store preferences for workspaces. Workspaces without
        /// ids should be considered temporary and any data associated with them should be deleted once
        /// the respective object is lost.
        Id { id: String },
        /// Workspace name changed
        ///
        /// This event is emitted immediately after the ext_workspace_handle_v1 is
        /// created and whenever the name of the workspace changes.
        ///
        /// A name is meant to be human-readable and can be displayed to a user.
        /// Unlike the id it is neither stable nor unique.
        Name { name: String },
        /// Workspace coordinates changed
        ///
        /// This event is used to organize workspaces into an N-dimensional grid
        /// within a workspace group, and if supported, is emitted immediately after
        /// the ext_workspace_handle_v1 is created and whenever the coordinates of
        /// the workspace change. Compositors may not send this event if they do not
        /// conceptually arrange workspaces in this way. If compositors simply
        /// number workspaces, without any geometric interpretation, they may send
        /// 1D coordinates, which clients should not interpret as implying any
        /// geometry. Sending an empty array means that the compositor no longer
        /// orders the workspace geometrically.
        ///
        /// Coordinates have an arbitrary number of dimensions N with an uint32
        /// position along each dimension. By convention if N > 1, the first
        /// dimension is X, the second Y, the third Z, and so on. The compositor may
        /// chose to utilize these events for a more novel workspace layout
        /// convention, however. No guarantee is made about the grid being filled or
        /// bounded; there may be a workspace at coordinate 1 and another at
        /// coordinate 1000 and none in between. Within a workspace group, however,
        /// workspaces must have unique coordinates of equal dimensionality.
        Coordinates { coordinates: Array },
        /// The state of the workspace changed
        ///
        /// This event is emitted immediately after the ext_workspace_handle_v1 is
        /// created and each time the workspace state changes, either because of a
        /// compositor action or because of a request in this protocol.
        ///
        /// Missing states convey the opposite meaning, e.g. an unset active bit
        /// means the workspace is currently inactive.
        State { state: u32 },
        /// Compositor capabilities
        ///
        /// This event advertises the capabilities supported by the compositor. If
        /// a capability isn't supported, clients should hide or disable the UI
        /// elements that expose this functionality. For instance, if the
        /// compositor doesn't advertise support for removing workspaces, a button
        /// triggering the remove request should not be displayed.
        ///
        /// The compositor will ignore requests it doesn't support. For instance,
        /// a compositor which doesn't advertise support for remove will ignore
        /// remove requests.
        ///
        /// Compositors must send this event once after creation of an
        /// ext_workspace_handle_v1 . When the capabilities change, compositors
        /// must send this event again.
        Capabilities {
            /// Capabilities,
            capabilities: u32,
        },
        /// This workspace has been removed
        ///
        /// This event is send when the workspace associated with the ext_workspace_handle_v1
        /// has been removed. After sending this request, the compositor will immediately consider
        /// the object inert. Any requests will be ignored except the destroy request.
        ///
        /// It is guaranteed there won't be any more events referencing this
        /// ext_workspace_handle_v1.
        ///
        /// The compositor must only remove a workspaces not currently belonging to any
        /// workspace_group.
        Removed,
    }
    impl EventWriter for crate::wayland_objects::ext_workspace_handle_v1::ExtWorkspaceHandleV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Id { id } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.string(id);
                    raw
                }
                Name { name } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.string(name);
                    raw
                }
                Coordinates { coordinates } => {
                    let mut raw = RawEvent::new(self.id(), 2, 1);
                    raw.array(coordinates);
                    raw
                }
                State { state } => {
                    let mut raw = RawEvent::new(self.id(), 3, 1);
                    raw.uint(state);
                    raw
                }
                Capabilities { capabilities } => {
                    let mut raw = RawEvent::new(self.id(), 4, 1);
                    raw.uint(capabilities);
                    raw
                }
                Removed {} => {
                    let mut raw = RawEvent::new(self.id(), 5, 0);
                    raw
                }
            }
        }
    }
    /// Types of states on the workspace
    ///
    /// The different states that a workspace can have.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum State {
        /// The workspace is active,
        Active = 1,
        /// The workspace requests attention,
        Urgent = 2,
        /// The workspace is not visible
        ///
        /// The workspace is not visible in its workspace group, and clients
        /// attempting to visualize the compositor workspace state should not
        /// display such workspaces.
        Hidden = 4,
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum WorkspaceCapabilities {
        /// Activate request is available,
        Activate = 1,
        /// Deactivate request is available,
        Deactivate = 2,
        /// Remove request is available,
        Remove = 4,
        /// Assign request is available,
        Assign = 8,
    }
}

/// Manager to control data devices
///
/// This interface is a manager that allows creating per-seat data device
/// controls.
pub mod zwlr_data_control_manager_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1 {
        const INTERFACE_NAME: &'static str = "zwlr_data_control_manager_v1";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ZwlrDataControlManagerV1Id {
        type Object =
            crate::wayland_objects::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1
    {
        type Id = ZwlrDataControlManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Create a new data source
        ///
        /// Create a new data source.
        CreateDataSource {
            /// Data source to create,
            id: ZwlrDataControlSourceV1Id,
        },
        /// Get a data device for a seat
        ///
        /// Create a data device that can be used to manage a seat's selection.
        GetDataDevice { id: ZwlrDataControlDeviceV1Id, seat: WlSeatId },
        /// Destroy the manager
        ///
        /// All objects created by the manager will still remain valid, until their
        /// appropriate destroy request has been called.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => CreateDataSource { id: req.id()? },
                1 => GetDataDevice { id: req.id()?, seat: req.id()? },
                2 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// Manage a data device for a seat
///
/// This interface allows a client to manage a seat's selection.
///
/// When the seat is destroyed, this object becomes inert.
pub mod zwlr_data_control_device_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1 {
        const INTERFACE_NAME: &'static str = "zwlr_data_control_device_v1";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for ZwlrDataControlDeviceV1Id {
        type Object = crate::wayland_objects::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1
    {
        type Id = ZwlrDataControlDeviceV1Id;
    }
    /// Minimum interface version for the `set_primary_selection` message (XML `since` / `version`).
    pub const SET_PRIMARY_SELECTION_SINCE: u32 = 2;
    #[derive(Debug)]
    pub enum Request {
        /// Copy data to the selection
        ///
        /// This request asks the compositor to set the selection to the data from
        /// the source on behalf of the client.
        ///
        /// The given source may not be used in any further set_selection or
        /// set_primary_selection requests. Attempting to use a previously used
        /// source is a protocol error.
        ///
        /// To unset the selection, set the source to NULL.
        SetSelection { source: ZwlrDataControlSourceV1Id },
        /// Destroy this data device
        ///
        /// Destroys the data device object.
        Destroy,
        /// Copy data to the primary selection
        ///
        /// This request asks the compositor to set the primary selection to the
        /// data from the source on behalf of the client.
        ///
        /// The given source may not be used in any further set_selection or
        /// set_primary_selection requests. Attempting to use a previously used
        /// source is a protocol error.
        ///
        /// To unset the primary selection, set the source to NULL.
        ///
        /// The compositor will ignore this request if it does not support primary
        /// selection.
        SetPrimarySelection { source: ZwlrDataControlSourceV1Id },
    }
    impl Parser for crate::wayland_objects::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetSelection { source: req.id()? },
                1 => Destroy,
                2 => SetPrimarySelection { source: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    /// Minimum interface version for the `primary_selection` message (XML `since` / `version`).
    pub const PRIMARY_SELECTION_SINCE: u32 = 2;
    #[derive(Debug)]
    pub enum Event {
        /// Introduce a new wlr_data_control_offer
        ///
        /// The data_offer event introduces a new wlr_data_control_offer object,
        /// which will subsequently be used in either the
        /// wlr_data_control_device.selection event (for the regular clipboard
        /// selections) or the wlr_data_control_device.primary_selection event (for
        /// the primary clipboard selections). Immediately following the
        /// wlr_data_control_device.data_offer event, the new data_offer object
        /// will send out wlr_data_control_offer.offer events to describe the MIME
        /// types it offers.
        DataOffer { id: ZwlrDataControlOfferV1Id },
        /// Advertise new selection
        ///
        /// The selection event is sent out to notify the client of a new
        /// wlr_data_control_offer for the selection for this device. The
        /// wlr_data_control_device.data_offer and the wlr_data_control_offer.offer
        /// events are sent out immediately before this event to introduce the data
        /// offer object. The selection event is sent to a client when a new
        /// selection is set. The wlr_data_control_offer is valid until a new
        /// wlr_data_control_offer or NULL is received. The client must destroy the
        /// previous selection wlr_data_control_offer, if any, upon receiving this
        /// event.
        ///
        /// The first selection event is sent upon binding the
        /// wlr_data_control_device object.
        Selection { id: ZwlrDataControlOfferV1Id },
        /// This data control is no longer valid
        ///
        /// This data control object is no longer valid and should be destroyed by
        /// the client.
        Finished,
        /// Advertise new primary selection
        ///
        /// The primary_selection event is sent out to notify the client of a new
        /// wlr_data_control_offer for the primary selection for this device. The
        /// wlr_data_control_device.data_offer and the wlr_data_control_offer.offer
        /// events are sent out immediately before this event to introduce the data
        /// offer object. The primary_selection event is sent to a client when a
        /// new primary selection is set. The wlr_data_control_offer is valid until
        /// a new wlr_data_control_offer or NULL is received. The client must
        /// destroy the previous primary selection wlr_data_control_offer, if any,
        /// upon receiving this event.
        ///
        /// If the compositor supports primary selection, the first
        /// primary_selection event is sent upon binding the
        /// wlr_data_control_device object.
        PrimarySelection { id: ZwlrDataControlOfferV1Id },
    }
    impl EventWriter for crate::wayland_objects::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                DataOffer { id } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.id(id);
                    raw
                }
                Selection { id } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.id(id);
                    raw
                }
                Finished {} => {
                    let mut raw = RawEvent::new(self.id(), 2, 0);
                    raw
                }
                PrimarySelection { id } => {
                    let mut raw = RawEvent::new(self.id(), 3, 1);
                    raw.id(id);
                    raw
                }
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Source given to set_selection or set_primary_selection was already used before,
        UsedSource = 1,
    }
}

/// Offer to transfer data
///
/// The wlr_data_control_source object is the source side of a
/// wlr_data_control_offer. It is created by the source client in a data
/// transfer and provides a way to describe the offered data and a way to
/// respond to requests to transfer the data.
pub mod zwlr_data_control_source_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwlr_data_control_source_v1::ZwlrDataControlSourceV1 {
        const INTERFACE_NAME: &'static str = "zwlr_data_control_source_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwlrDataControlSourceV1Id {
        type Object = crate::wayland_objects::zwlr_data_control_source_v1::ZwlrDataControlSourceV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwlr_data_control_source_v1::ZwlrDataControlSourceV1
    {
        type Id = ZwlrDataControlSourceV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Add an offered MIME type
        ///
        /// This request adds a MIME type to the set of MIME types advertised to
        /// targets. Can be called several times to offer multiple types.
        ///
        /// Calling this after wlr_data_control_device.set_selection is a protocol
        /// error.
        Offer {
            /// MIME type offered by the data source,
            mime_type: String,
        },
        /// Destroy this source
        ///
        /// Destroys the data source object.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwlr_data_control_source_v1::ZwlrDataControlSourceV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Offer { mime_type: req.string()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Send the data
        ///
        /// Request for data from the client. Send the data as the specified MIME
        /// type over the passed file descriptor, then close it.
        Send {
            /// MIME type for the data,
            mime_type: String,
            /// File descriptor for the data,
            fd: RawFd,
        },
        /// Selection was cancelled
        ///
        /// This data source is no longer valid. The data source has been replaced
        /// by another data source.
        ///
        /// The client should clean up and destroy this data source.
        Cancelled,
    }
    impl EventWriter for crate::wayland_objects::zwlr_data_control_source_v1::ZwlrDataControlSourceV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Send { mime_type, fd } => {
                    let mut raw = RawEvent::new(self.id(), 0, 2);
                    raw.string(mime_type);
                    raw.fd(fd);
                    raw
                }
                Cancelled {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Offer sent after wlr_data_control_device.set_selection,
        InvalidOffer = 1,
    }
}

/// Offer to transfer data
///
/// A wlr_data_control_offer represents a piece of data offered for transfer
/// by another client (the source client). The offer describes the different
/// MIME types that the data can be converted to and provides the mechanism
/// for transferring the data directly from the source client.
pub mod zwlr_data_control_offer_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwlr_data_control_offer_v1::ZwlrDataControlOfferV1 {
        const INTERFACE_NAME: &'static str = "zwlr_data_control_offer_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwlrDataControlOfferV1Id {
        type Object = crate::wayland_objects::zwlr_data_control_offer_v1::ZwlrDataControlOfferV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwlr_data_control_offer_v1::ZwlrDataControlOfferV1
    {
        type Id = ZwlrDataControlOfferV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Request that the data is transferred
        ///
        /// To transfer the offered data, the client issues this request and
        /// indicates the MIME type it wants to receive. The transfer happens
        /// through the passed file descriptor (typically created with the pipe
        /// system call). The source client writes the data in the MIME type
        /// representation requested and then closes the file descriptor.
        ///
        /// The receiving client reads from the read end of the pipe until EOF and
        /// then closes its end, at which point the transfer is complete.
        ///
        /// This request may happen multiple times for different MIME types.
        Receive {
            /// MIME type desired by receiver,
            mime_type: String,
            /// File descriptor for data transfer,
            fd: RawFd,
        },
        /// Destroy this offer
        ///
        /// Destroys the data offer object.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwlr_data_control_offer_v1::ZwlrDataControlOfferV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Receive { mime_type: req.string()?, fd: req.fd()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Advertise offered MIME type
        ///
        /// Sent immediately after creating the wlr_data_control_offer object.
        /// One event per offered MIME type.
        Offer {
            /// Offered MIME type,
            mime_type: String,
        },
    }
    impl EventWriter for crate::wayland_objects::zwlr_data_control_offer_v1::ZwlrDataControlOfferV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Offer { mime_type } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.string(mime_type);
                    raw
                }
            }
        }
    }
}

/// List and control opened apps
///
/// The purpose of this protocol is to enable the creation of taskbars
/// and docks by providing them with a list of opened applications and
/// letting them request certain actions on them, like maximizing, etc.
///
/// After a client binds the zwlr_foreign_toplevel_manager_v1, each opened
/// toplevel window will be sent via the toplevel event
pub mod zwlr_foreign_toplevel_manager_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1
    {
        const INTERFACE_NAME: &'static str = "zwlr_foreign_toplevel_manager_v1";
        const INTERFACE_VERSION: u32 = 3;
    }
    impl crate::object::IdToObject for ZwlrForeignToplevelManagerV1Id {
        type Object =
            crate::wayland_objects::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1
    {
        type Id = ZwlrForeignToplevelManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Stop sending events
        ///
        /// Indicates the client no longer wishes to receive events for new toplevels.
        /// However the compositor may emit further toplevel_created events, until
        /// the finished event is emitted.
        ///
        /// The client must not send any more requests after this one.
        Stop,
    }
    impl Parser
        for crate::wayland_objects::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1
    {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Stop,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// A toplevel has been created
        ///
        /// This event is emitted whenever a new toplevel window is created. It
        /// is emitted for all toplevels, regardless of the app that has created
        /// them.
        ///
        /// All initial details of the toplevel(title, app_id, states, etc.) will
        /// be sent immediately after this event via the corresponding events in
        /// zwlr_foreign_toplevel_handle_v1.
        Toplevel { toplevel: ZwlrForeignToplevelHandleV1Id },
        /// The compositor has finished with the toplevel manager
        ///
        /// This event indicates that the compositor is done sending events to the
        /// zwlr_foreign_toplevel_manager_v1. The server will destroy the object
        /// immediately after sending this request, so it will become invalid and
        /// the client should free any resources associated with it.
        Finished,
    }
    impl EventWriter
        for crate::wayland_objects::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1
    {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Toplevel { toplevel } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.id(toplevel);
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

/// An opened toplevel
///
/// A zwlr_foreign_toplevel_handle_v1 object represents an opened toplevel
/// window. Each app may have multiple opened toplevels.
///
/// Each toplevel has a list of outputs it is visible on, conveyed to the
/// client with the output_enter and output_leave events.
pub mod zwlr_foreign_toplevel_handle_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1
    {
        const INTERFACE_NAME: &'static str = "zwlr_foreign_toplevel_handle_v1";
        const INTERFACE_VERSION: u32 = 3;
    }
    impl crate::object::IdToObject for ZwlrForeignToplevelHandleV1Id {
        type Object =
            crate::wayland_objects::zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1
    {
        type Id = ZwlrForeignToplevelHandleV1Id;
    }
    /// Minimum interface version for the `set_fullscreen` message (XML `since` / `version`).
    pub const SET_FULLSCREEN_SINCE: u32 = 2;
    /// Minimum interface version for the `unset_fullscreen` message (XML `since` / `version`).
    pub const UNSET_FULLSCREEN_SINCE: u32 = 2;
    #[derive(Debug)]
    pub enum Request {
        /// Requests that the toplevel be maximized
        ///
        /// Requests that the toplevel be maximized. If the maximized state actually
        /// changes, this will be indicated by the state event.
        SetMaximized,
        /// Requests that the toplevel be unmaximized
        ///
        /// Requests that the toplevel be unmaximized. If the maximized state actually
        /// changes, this will be indicated by the state event.
        UnsetMaximized,
        /// Requests that the toplevel be minimized
        ///
        /// Requests that the toplevel be minimized. If the minimized state actually
        /// changes, this will be indicated by the state event.
        SetMinimized,
        /// Requests that the toplevel be unminimized
        ///
        /// Requests that the toplevel be unminimized. If the minimized state actually
        /// changes, this will be indicated by the state event.
        UnsetMinimized,
        /// Activate the toplevel
        ///
        /// Request that this toplevel be activated on the given seat.
        /// There is no guarantee the toplevel will be actually activated.
        Activate { seat: WlSeatId },
        /// Request that the toplevel be closed
        ///
        /// Send a request to the toplevel to close itself. The compositor would
        /// typically use a shell-specific method to carry out this request, for
        /// example by sending the xdg_toplevel.close event. However, this gives
        /// no guarantees the toplevel will actually be destroyed. If and when
        /// this happens, the zwlr_foreign_toplevel_handle_v1.closed event will
        /// be emitted.
        Close,
        /// The rectangle which represents the toplevel
        ///
        /// The rectangle of the surface specified in this request corresponds to
        /// the place where the app using this protocol represents the given toplevel.
        /// It can be used by the compositor as a hint for some operations, e.g
        /// minimizing. The client is however not required to set this, in which
        /// case the compositor is free to decide some default value.
        ///
        /// If the client specifies more than one rectangle, only the last one is
        /// considered.
        ///
        /// The dimensions are given in surface-local coordinates.
        /// Setting width=height=0 removes the already-set rectangle.
        SetRectangle { surface: WlSurfaceId, x: i32, y: i32, width: i32, height: i32 },
        /// Destroy the zwlr_foreign_toplevel_handle_v1 object
        ///
        /// Destroys the zwlr_foreign_toplevel_handle_v1 object.
        ///
        /// This request should be called either when the client does not want to
        /// use the toplevel anymore or after the closed event to finalize the
        /// destruction of the object.
        Destroy,
        /// Request that the toplevel be fullscreened
        ///
        /// Requests that the toplevel be fullscreened on the given output. If the
        /// fullscreen state and/or the outputs the toplevel is visible on actually
        /// change, this will be indicated by the state and output_enter/leave
        /// events.
        ///
        /// The output parameter is only a hint to the compositor. Also, if output
        /// is NULL, the compositor should decide which output the toplevel will be
        /// fullscreened on, if at all.
        SetFullscreen { output: WlOutputId },
        /// Request that the toplevel be unfullscreened
        ///
        /// Requests that the toplevel be unfullscreened. If the fullscreen state
        /// actually changes, this will be indicated by the state event.
        UnsetFullscreen,
    }
    impl Parser
        for crate::wayland_objects::zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1
    {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetMaximized,
                1 => UnsetMaximized,
                2 => SetMinimized,
                3 => UnsetMinimized,
                4 => Activate { seat: req.id()? },
                5 => Close,
                6 => SetRectangle {
                    surface: req.id()?,
                    x: req.int()?,
                    y: req.int()?,
                    width: req.int()?,
                    height: req.int()?,
                },
                7 => Destroy,
                8 => SetFullscreen { output: req.id()? },
                9 => UnsetFullscreen,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    /// Minimum interface version for the `parent` message (XML `since` / `version`).
    pub const PARENT_SINCE: u32 = 3;
    #[derive(Debug)]
    pub enum Event {
        /// Title change
        ///
        /// This event is emitted whenever the title of the toplevel changes.
        Title { title: String },
        /// App-id change
        ///
        /// This event is emitted whenever the app-id of the toplevel changes.
        AppId { app_id: String },
        /// Toplevel entered an output
        ///
        /// This event is emitted whenever the toplevel becomes visible on
        /// the given output. A toplevel may be visible on multiple outputs.
        OutputEnter { output: WlOutputId },
        /// Toplevel left an output
        ///
        /// This event is emitted whenever the toplevel stops being visible on
        /// the given output. It is guaranteed that an entered-output event
        /// with the same output has been emitted before this event.
        OutputLeave { output: WlOutputId },
        /// The toplevel state changed
        ///
        /// This event is emitted immediately after the zlw_foreign_toplevel_handle_v1
        /// is created and each time the toplevel state changes, either because of a
        /// compositor action or because of a request in this protocol.
        State { state: Array },
        /// All information about the toplevel has been sent
        ///
        /// This event is sent after all changes in the toplevel state have been
        /// sent.
        ///
        /// This allows changes to the zwlr_foreign_toplevel_handle_v1 properties
        /// to be seen as atomic, even if they happen via multiple events.
        Done,
        /// This toplevel has been destroyed
        ///
        /// This event means the toplevel has been destroyed. It is guaranteed there
        /// won't be any more events for this zwlr_foreign_toplevel_handle_v1. The
        /// toplevel itself becomes inert so any requests will be ignored except the
        /// destroy request.
        Closed,
        /// Parent change
        ///
        /// This event is emitted whenever the parent of the toplevel changes.
        ///
        /// No event is emitted when the parent handle is destroyed by the client.
        Parent { parent: ZwlrForeignToplevelHandleV1Id },
    }
    impl EventWriter
        for crate::wayland_objects::zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1
    {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Title { title } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.string(title);
                    raw
                }
                AppId { app_id } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.string(app_id);
                    raw
                }
                OutputEnter { output } => {
                    let mut raw = RawEvent::new(self.id(), 2, 1);
                    raw.id(output);
                    raw
                }
                OutputLeave { output } => {
                    let mut raw = RawEvent::new(self.id(), 3, 1);
                    raw.id(output);
                    raw
                }
                State { state } => {
                    let mut raw = RawEvent::new(self.id(), 4, 1);
                    raw.array(state);
                    raw
                }
                Done {} => {
                    let mut raw = RawEvent::new(self.id(), 5, 0);
                    raw
                }
                Closed {} => {
                    let mut raw = RawEvent::new(self.id(), 6, 0);
                    raw
                }
                Parent { parent } => {
                    let mut raw = RawEvent::new(self.id(), 7, 1);
                    raw.id(parent);
                    raw
                }
            }
        }
    }
    /// Types of states on the toplevel
    ///
    /// The different states that a toplevel can have. These have the same meaning
    /// as the states with the same names defined in xdg-toplevel
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum State {
        /// The toplevel is maximized,
        Maximized = 0,
        /// The toplevel is minimized,
        Minimized = 1,
        /// The toplevel is active,
        Activated = 2,
        /// The toplevel is fullscreen,
        Fullscreen = 3,
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// The provided rectangle is invalid,
        InvalidRectangle = 0,
    }
}

/// Manager to inform clients and begin capturing
///
/// This object is a manager which offers requests to start capturing from a
/// source.
pub mod zwlr_screencopy_manager_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1 {
        const INTERFACE_NAME: &'static str = "zwlr_screencopy_manager_v1";
        const INTERFACE_VERSION: u32 = 3;
    }
    impl crate::object::IdToObject for ZwlrScreencopyManagerV1Id {
        type Object = crate::wayland_objects::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1
    {
        type Id = ZwlrScreencopyManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Capture an output
        ///
        /// Capture the next frame of an entire output.
        CaptureOutput {
            frame: ZwlrScreencopyFrameV1Id,
            /// Composite cursor onto the frame,
            overlay_cursor: i32,
            output: WlOutputId,
        },
        /// Capture an output's region
        ///
        /// Capture the next frame of an output's region.
        ///
        /// The region is given in output logical coordinates, see
        /// xdg_output.logical_size. The region will be clipped to the output's
        /// extents.
        CaptureOutputRegion {
            frame: ZwlrScreencopyFrameV1Id,
            /// Composite cursor onto the frame,
            overlay_cursor: i32,
            output: WlOutputId,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
        },
        /// Destroy the manager
        ///
        /// All objects created by the manager will still remain valid, until their
        /// appropriate destroy request has been called.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => CaptureOutput {
                    frame: req.id()?,
                    overlay_cursor: req.int()?,
                    output: req.id()?,
                },
                1 => CaptureOutputRegion {
                    frame: req.id()?,
                    overlay_cursor: req.int()?,
                    output: req.id()?,
                    x: req.int()?,
                    y: req.int()?,
                    width: req.int()?,
                    height: req.int()?,
                },
                2 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// A frame ready for copy
///
/// This object represents a single frame.
///
/// When created, a series of buffer events will be sent, each representing a
/// supported buffer type. The "buffer_done" event is sent afterwards to
/// indicate that all supported buffer types have been enumerated. The client
/// will then be able to send a "copy" request. If the capture is successful,
/// the compositor will send a "flags" event followed by a "ready" event.
///
/// For objects version 2 or lower, wl_shm buffers are always supported, ie.
/// the "buffer" event is guaranteed to be sent.
///
/// If the capture failed, the "failed" event is sent. This can happen anytime
/// before the "ready" event.
///
/// Once either a "ready" or a "failed" event is received, the client should
/// destroy the frame.
pub mod zwlr_screencopy_frame_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1 {
        const INTERFACE_NAME: &'static str = "zwlr_screencopy_frame_v1";
        const INTERFACE_VERSION: u32 = 3;
    }
    impl crate::object::IdToObject for ZwlrScreencopyFrameV1Id {
        type Object = crate::wayland_objects::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1
    {
        type Id = ZwlrScreencopyFrameV1Id;
    }
    /// Minimum interface version for the `copy_with_damage` message (XML `since` / `version`).
    pub const COPY_WITH_DAMAGE_SINCE: u32 = 2;
    #[derive(Debug)]
    pub enum Request {
        /// Copy the frame
        ///
        /// Copy the frame to the supplied buffer. The buffer must have the
        /// correct size, see zwlr_screencopy_frame_v1.buffer and
        /// zwlr_screencopy_frame_v1.linux_dmabuf. The buffer needs to have a
        /// supported format.
        ///
        /// If the frame is successfully copied, "flags" and "ready" events are
        /// sent. Otherwise, a "failed" event is sent.
        Copy { buffer: WlBufferId },
        /// Delete this object, used or not
        ///
        /// Destroys the frame. This request can be sent at any time by the client.
        Destroy,
        /// Copy the frame when it's damaged
        ///
        /// Same as copy, except it waits until there is damage to copy.
        CopyWithDamage { buffer: WlBufferId },
    }
    impl Parser for crate::wayland_objects::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => Copy { buffer: req.id()? },
                1 => Destroy,
                2 => CopyWithDamage { buffer: req.id()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    /// Minimum interface version for the `damage` message (XML `since` / `version`).
    pub const DAMAGE_SINCE: u32 = 2;
    /// Minimum interface version for the `linux_dmabuf` message (XML `since` / `version`).
    pub const LINUX_DMABUF_SINCE: u32 = 3;
    /// Minimum interface version for the `buffer_done` message (XML `since` / `version`).
    pub const BUFFER_DONE_SINCE: u32 = 3;
    #[derive(Debug)]
    pub enum Event {
        /// Wl_shm buffer information
        ///
        /// Provides information about wl_shm buffer parameters that need to be
        /// used for this frame. This event is sent once after the frame is created
        /// if wl_shm buffers are supported.
        Buffer {
            /// Buffer format,
            format: u32,
            /// Buffer width,
            width: u32,
            /// Buffer height,
            height: u32,
            /// Buffer stride,
            stride: u32,
        },
        /// Frame flags
        ///
        /// Provides flags about the frame. This event is sent once before the
        /// "ready" event.
        Flags {
            /// Frame flags,
            flags: u32,
        },
        /// Indicates frame is available for reading
        ///
        /// Called as soon as the frame is copied, indicating it is available
        /// for reading. This event includes the time at which the presentation took place.
        ///
        /// The timestamp is expressed as tv_sec_hi, tv_sec_lo, tv_nsec triples,
        /// each component being an unsigned 32-bit value. Whole seconds are in
        /// tv_sec which is a 64-bit value combined from tv_sec_hi and tv_sec_lo,
        /// and the additional fractional part in tv_nsec as nanoseconds. Hence,
        /// for valid timestamps tv_nsec must be in [0, 999999999]. The seconds part
        /// may have an arbitrary offset at start.
        ///
        /// After receiving this event, the client should destroy the object.
        Ready {
            /// High 32 bits of the seconds part of the timestamp,
            tv_sec_hi: u32,
            /// Low 32 bits of the seconds part of the timestamp,
            tv_sec_lo: u32,
            /// Nanoseconds part of the timestamp,
            tv_nsec: u32,
        },
        /// Frame copy failed
        ///
        /// This event indicates that the attempted frame copy has failed.
        ///
        /// After receiving this event, the client should destroy the object.
        Failed,
        /// Carries the coordinates of the damaged region
        ///
        /// This event is sent right before the ready event when copy_with_damage is
        /// requested. It may be generated multiple times for each copy_with_damage
        /// request.
        ///
        /// The arguments describe a box around an area that has changed since the
        /// last copy request that was derived from the current screencopy manager
        /// instance.
        ///
        /// The union of all regions received between the call to copy_with_damage
        /// and a ready event is the total damage since the prior ready event.
        Damage {
            /// Damaged x coordinates,
            x: u32,
            /// Damaged y coordinates,
            y: u32,
            /// Current width,
            width: u32,
            /// Current height,
            height: u32,
        },
        /// Linux-dmabuf buffer information
        ///
        /// Provides information about linux-dmabuf buffer parameters that need to
        /// be used for this frame. This event is sent once after the frame is
        /// created if linux-dmabuf buffers are supported.
        LinuxDmabuf {
            /// Fourcc pixel format,
            format: u32,
            /// Buffer width,
            width: u32,
            /// Buffer height,
            height: u32,
        },
        /// All buffer types reported
        ///
        /// This event is sent once after all buffer events have been sent.
        ///
        /// The client should proceed to create a buffer of one of the supported
        /// types, and send a "copy" request.
        BufferDone,
    }
    impl EventWriter for crate::wayland_objects::zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Buffer { format, width, height, stride } => {
                    let mut raw = RawEvent::new(self.id(), 0, 4);
                    raw.uint(format);
                    raw.uint(width);
                    raw.uint(height);
                    raw.uint(stride);
                    raw
                }
                Flags { flags } => {
                    let mut raw = RawEvent::new(self.id(), 1, 1);
                    raw.uint(flags);
                    raw
                }
                Ready { tv_sec_hi, tv_sec_lo, tv_nsec } => {
                    let mut raw = RawEvent::new(self.id(), 2, 3);
                    raw.uint(tv_sec_hi);
                    raw.uint(tv_sec_lo);
                    raw.uint(tv_nsec);
                    raw
                }
                Failed {} => {
                    let mut raw = RawEvent::new(self.id(), 3, 0);
                    raw
                }
                Damage { x, y, width, height } => {
                    let mut raw = RawEvent::new(self.id(), 4, 4);
                    raw.uint(x);
                    raw.uint(y);
                    raw.uint(width);
                    raw.uint(height);
                    raw
                }
                LinuxDmabuf { format, width, height } => {
                    let mut raw = RawEvent::new(self.id(), 5, 3);
                    raw.uint(format);
                    raw.uint(width);
                    raw.uint(height);
                    raw
                }
                BufferDone {} => {
                    let mut raw = RawEvent::new(self.id(), 6, 0);
                    raw
                }
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// The object has already been used to copy a wl_buffer,
        AlreadyUsed = 0,
        /// Buffer attributes are invalid,
        InvalidBuffer = 1,
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Flags {
        /// Contents are y-inverted,
        YInvert = 1,
    }
}

/// Manager to create per-output gamma controls
///
/// This interface is a manager that allows creating per-output gamma
/// controls.
pub mod zwlr_gamma_control_manager_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1
    {
        const INTERFACE_NAME: &'static str = "zwlr_gamma_control_manager_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwlrGammaControlManagerV1Id {
        type Object =
            crate::wayland_objects::zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1
    {
        type Id = ZwlrGammaControlManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Get a gamma control for an output
        ///
        /// Create a gamma control that can be used to adjust gamma tables for the
        /// provided output.
        GetGammaControl { id: ZwlrGammaControlV1Id, output: WlOutputId },
        /// Destroy the manager
        ///
        /// All objects created by the manager will still remain valid, until their
        /// appropriate destroy request has been called.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => GetGammaControl { id: req.id()?, output: req.id()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
}

/// Adjust gamma tables for an output
///
/// This interface allows a client to adjust gamma tables for a particular
/// output.
///
/// The client will receive the gamma size, and will then be able to set gamma
/// tables. At any time the compositor can send a failed event indicating that
/// this object is no longer valid.
///
/// There can only be at most one gamma control object per output, which
/// has exclusive access to this particular output. When the gamma control
/// object is destroyed, the gamma table is restored to its original value.
pub mod zwlr_gamma_control_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwlr_gamma_control_v1::ZwlrGammaControlV1 {
        const INTERFACE_NAME: &'static str = "zwlr_gamma_control_v1";
        const INTERFACE_VERSION: u32 = 1;
    }
    impl crate::object::IdToObject for ZwlrGammaControlV1Id {
        type Object = crate::wayland_objects::zwlr_gamma_control_v1::ZwlrGammaControlV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwlr_gamma_control_v1::ZwlrGammaControlV1
    {
        type Id = ZwlrGammaControlV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Set the gamma table
        ///
        /// Set the gamma table. The file descriptor can be memory-mapped to provide
        /// the raw gamma table, which contains successive gamma ramps for the red,
        /// green and blue channels. Each gamma ramp is an array of 16-byte unsigned
        /// integers which has the same length as the gamma size.
        ///
        /// The file descriptor data must have the same length as three times the
        /// gamma size.
        SetGamma {
            /// Gamma table file descriptor,
            fd: RawFd,
        },
        /// Destroy this control
        ///
        /// Destroys the gamma control object. If the object is still valid, this
        /// restores the original gamma tables.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwlr_gamma_control_v1::ZwlrGammaControlV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetGamma { fd: req.fd()? },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Size of gamma ramps
        ///
        /// Advertise the size of each gamma ramp.
        ///
        /// This event is sent immediately when the gamma control object is created.
        GammaSize {
            /// Number of elements in a ramp,
            size: u32,
        },
        /// Object no longer valid
        ///
        /// This event indicates that the gamma control is no longer valid. This
        /// can happen for a number of reasons, including:
        /// - The output doesn't support gamma tables
        /// - Setting the gamma tables failed
        /// - Another client already has exclusive gamma control for this output
        /// - The compositor has transferred gamma control to another client
        ///
        /// Upon receiving this event, the client should destroy this object.
        Failed,
    }
    impl EventWriter for crate::wayland_objects::zwlr_gamma_control_v1::ZwlrGammaControlV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                GammaSize { size } => {
                    let mut raw = RawEvent::new(self.id(), 0, 1);
                    raw.uint(size);
                    raw
                }
                Failed {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Invalid gamma tables,
        InvalidGamma = 1,
    }
}

/// Create surfaces that are layers of the desktop
///
/// Clients can use this interface to assign the surface_layer role to
/// wl_surfaces. Such surfaces are assigned to a "layer" of the output and
/// rendered with a defined z-depth respective to each other. They may also be
/// anchored to the edges and corners of a screen and specify input handling
/// semantics. This interface should be suitable for the implementation of
/// many desktop shell components, and a broad number of other applications
/// that interact with the desktop.
pub mod zwlr_layer_shell_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwlr_layer_shell_v1::ZwlrLayerShellV1 {
        const INTERFACE_NAME: &'static str = "zwlr_layer_shell_v1";
        const INTERFACE_VERSION: u32 = 5;
    }
    impl crate::object::IdToObject for ZwlrLayerShellV1Id {
        type Object = crate::wayland_objects::zwlr_layer_shell_v1::ZwlrLayerShellV1;
    }
    impl crate::object::ObjectWithId for crate::wayland_objects::zwlr_layer_shell_v1::ZwlrLayerShellV1 {
        type Id = ZwlrLayerShellV1Id;
    }
    /// Minimum interface version for the `destroy` message (XML `since` / `version`).
    pub const DESTROY_SINCE: u32 = 3;
    #[derive(Debug)]
    pub enum Request {
        /// Create a layer_surface from a surface
        ///
        /// Create a layer surface for an existing surface. This assigns the role of
        /// layer_surface, or raises a protocol error if another role is already
        /// assigned.
        ///
        /// Creating a layer surface from a wl_surface which has a buffer attached
        /// or committed is a client error, and any attempts by a client to attach
        /// or manipulate a buffer prior to the first layer_surface.configure call
        /// must also be treated as errors.
        ///
        /// After creating a layer_surface object and setting it up, the client
        /// must perform an initial commit without any buffer attached.
        /// The compositor will reply with a layer_surface.configure event.
        /// The client must acknowledge it and is then allowed to attach a buffer
        /// to map the surface.
        ///
        /// You may pass NULL for output to allow the compositor to decide which
        /// output to use. Generally this will be the one that the user most
        /// recently interacted with.
        ///
        /// Clients can specify a namespace that defines the purpose of the layer
        /// surface.
        GetLayerSurface {
            id: ZwlrLayerSurfaceV1Id,
            surface: WlSurfaceId,
            output: WlOutputId,
            /// Layer to add this surface to,
            layer: u32,
            /// Namespace for the layer surface,
            namespace: String,
        },
        /// Destroy the layer_shell object
        ///
        /// This request indicates that the client will not use the layer_shell
        /// object any more. Objects that have been created through this instance
        /// are not affected.
        Destroy,
    }
    impl Parser for crate::wayland_objects::zwlr_layer_shell_v1::ZwlrLayerShellV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => GetLayerSurface {
                    id: req.id()?,
                    surface: req.id()?,
                    output: req.id()?,
                    layer: req.uint()?,
                    namespace: req.string()?,
                },
                1 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Wl_surface has another role,
        Role = 0,
        /// Layer value is invalid,
        InvalidLayer = 1,
        /// Wl_surface has a buffer attached or committed,
        AlreadyConstructed = 2,
    }
    /// Available layers for surfaces
    ///
    /// These values indicate which layers a surface can be rendered in. They
    /// are ordered by z depth, bottom-most first. Traditional shell surfaces
    /// will typically be rendered between the bottom and top layers.
    /// Fullscreen shell surfaces are typically rendered at the top layer.
    /// Multiple surfaces can share a single layer, and ordering within a
    /// single layer is undefined.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Layer {
        Background = 0,
        Bottom = 1,
        Top = 2,
        Overlay = 3,
    }
}

/// Layer metadata interface
///
/// An interface that may be implemented by a wl_surface, for surfaces that
/// are designed to be rendered as a layer of a stacked desktop-like
/// environment.
///
/// Layer surface state (layer, size, anchor, exclusive zone,
/// margin, interactivity) is double-buffered, and will be applied at the
/// time wl_surface.commit of the corresponding wl_surface is called.
///
/// Attaching a null buffer to a layer surface unmaps it.
///
/// Unmapping a layer_surface means that the surface cannot be shown by the
/// compositor until it is explicitly mapped again. The layer_surface
/// returns to the state it had right after layer_shell.get_layer_surface.
/// The client can re-map the surface by performing a commit without any
/// buffer attached, waiting for a configure event and handling it as usual.
pub mod zwlr_layer_surface_v1 {
    use super::*;
    impl Constants for crate::wayland_objects::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1 {
        const INTERFACE_NAME: &'static str = "zwlr_layer_surface_v1";
        const INTERFACE_VERSION: u32 = 5;
    }
    impl crate::object::IdToObject for ZwlrLayerSurfaceV1Id {
        type Object = crate::wayland_objects::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1
    {
        type Id = ZwlrLayerSurfaceV1Id;
    }
    /// Minimum interface version for the `set_layer` message (XML `since` / `version`).
    pub const SET_LAYER_SINCE: u32 = 2;
    /// Minimum interface version for the `set_exclusive_edge` message (XML `since` / `version`).
    pub const SET_EXCLUSIVE_EDGE_SINCE: u32 = 5;
    #[derive(Debug)]
    pub enum Request {
        /// Sets the size of the surface
        ///
        /// Sets the size of the surface in surface-local coordinates. The
        /// compositor will display the surface centered with respect to its
        /// anchors.
        ///
        /// If you pass 0 for either value, the compositor will assign it and
        /// inform you of the assignment in the configure event. You must set your
        /// anchor to opposite edges in the dimensions you omit; not doing so is a
        /// protocol error. Both values are 0 by default.
        ///
        /// Size is double-buffered, see wl_surface.commit.
        SetSize { width: u32, height: u32 },
        /// Configures the anchor point of the surface
        ///
        /// Requests that the compositor anchor the surface to the specified edges
        /// and corners. If two orthogonal edges are specified (e.g. 'top' and
        /// 'left'), then the anchor point will be the intersection of the edges
        /// (e.g. the top left corner of the output); otherwise the anchor point
        /// will be centered on that edge, or in the center if none is specified.
        ///
        /// Anchor is double-buffered, see wl_surface.commit.
        SetAnchor { anchor: u32 },
        /// Configures the exclusive geometry of this surface
        ///
        /// Requests that the compositor avoids occluding an area with other
        /// surfaces. The compositor's use of this information is
        /// implementation-dependent - do not assume that this region will not
        /// actually be occluded.
        ///
        /// A positive value is only meaningful if the surface is anchored to one
        /// edge or an edge and both perpendicular edges. If the surface is not
        /// anchored, anchored to only two perpendicular edges (a corner), anchored
        /// to only two parallel edges or anchored to all edges, a positive value
        /// will be treated the same as zero.
        ///
        /// A positive zone is the distance from the edge in surface-local
        /// coordinates to consider exclusive.
        ///
        /// Surfaces that do not wish to have an exclusive zone may instead specify
        /// how they should interact with surfaces that do. If set to zero, the
        /// surface indicates that it would like to be moved to avoid occluding
        /// surfaces with a positive exclusive zone. If set to -1, the surface
        /// indicates that it would not like to be moved to accommodate for other
        /// surfaces, and the compositor should extend it all the way to the edges
        /// it is anchored to.
        ///
        /// For example, a panel might set its exclusive zone to 10, so that
        /// maximized shell surfaces are not shown on top of it. A notification
        /// might set its exclusive zone to 0, so that it is moved to avoid
        /// occluding the panel, but shell surfaces are shown underneath it. A
        /// wallpaper or lock screen might set their exclusive zone to -1, so that
        /// they stretch below or over the panel.
        ///
        /// The default value is 0.
        ///
        /// Exclusive zone is double-buffered, see wl_surface.commit.
        SetExclusiveZone { zone: i32 },
        /// Sets a margin from the anchor point
        ///
        /// Requests that the surface be placed some distance away from the anchor
        /// point on the output, in surface-local coordinates. Setting this value
        /// for edges you are not anchored to has no effect.
        ///
        /// The exclusive zone includes the margin.
        ///
        /// Margin is double-buffered, see wl_surface.commit.
        SetMargin { top: i32, right: i32, bottom: i32, left: i32 },
        /// Requests keyboard events
        ///
        /// Set how keyboard events are delivered to this surface. By default,
        /// layer shell surfaces do not receive keyboard events; this request can
        /// be used to change this.
        ///
        /// This setting is inherited by child surfaces set by the get_popup
        /// request.
        ///
        /// Layer surfaces receive pointer, touch, and tablet events normally. If
        /// you do not want to receive them, set the input region on your surface
        /// to an empty region.
        ///
        /// Keyboard interactivity is double-buffered, see wl_surface.commit.
        SetKeyboardInteractivity { keyboard_interactivity: u32 },
        /// Assign this layer_surface as an xdg_popup parent
        ///
        /// This assigns an xdg_popup's parent to this layer_surface.  This popup
        /// should have been created via xdg_surface::get_popup with the parent set
        /// to NULL, and this request must be invoked before committing the popup's
        /// initial state.
        ///
        /// See the documentation of xdg_popup for more details about what an
        /// xdg_popup is and how it is used.
        GetPopup { popup: XdgPopupId },
        /// Ack a configure event
        ///
        /// When a configure event is received, if a client commits the
        /// surface in response to the configure event, then the client
        /// must make an ack_configure request sometime before the commit
        /// request, passing along the serial of the configure event.
        ///
        /// If the client receives multiple configure events before it
        /// can respond to one, it only has to ack the last configure event.
        ///
        /// A client is not required to commit immediately after sending
        /// an ack_configure request - it may even ack_configure several times
        /// before its next surface commit.
        ///
        /// A client may send multiple ack_configure requests before committing, but
        /// only the last request sent before a commit indicates which configure
        /// event the client really is responding to.
        AckConfigure {
            /// The serial from the configure event,
            serial: u32,
        },
        /// Destroy the layer_surface
        ///
        /// This request destroys the layer surface.
        Destroy,
        /// Change the layer of the surface
        ///
        /// Change the layer that the surface is rendered on.
        ///
        /// Layer is double-buffered, see wl_surface.commit.
        SetLayer {
            /// Layer to move this surface to,
            layer: u32,
        },
        /// Set the edge the exclusive zone will be applied to
        ///
        /// Requests an edge for the exclusive zone to apply. The exclusive
        /// edge will be automatically deduced from anchor points when possible,
        /// but when the surface is anchored to a corner, it will be necessary
        /// to set it explicitly to disambiguate, as it is not possible to deduce
        /// which one of the two corner edges should be used.
        ///
        /// The edge must be one the surface is anchored to, otherwise the
        /// invalid_exclusive_edge protocol error will be raised.
        SetExclusiveEdge { edge: u32 },
    }
    impl Parser for crate::wayland_objects::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1 {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetSize { width: req.uint()?, height: req.uint()? },
                1 => SetAnchor { anchor: req.uint()? },
                2 => SetExclusiveZone { zone: req.int()? },
                3 => SetMargin {
                    top: req.int()?,
                    right: req.int()?,
                    bottom: req.int()?,
                    left: req.int()?,
                },
                4 => SetKeyboardInteractivity { keyboard_interactivity: req.uint()? },
                5 => GetPopup { popup: req.id()? },
                6 => AckConfigure { serial: req.uint()? },
                7 => Destroy,
                8 => SetLayer { layer: req.uint()? },
                9 => SetExclusiveEdge { edge: req.uint()? },
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    #[derive(Debug)]
    pub enum Event {
        /// Suggest a surface change
        ///
        /// The configure event asks the client to resize its surface.
        ///
        /// Clients should arrange their surface for the new states, and then send
        /// an ack_configure request with the serial sent in this configure event at
        /// some point before committing the new surface.
        ///
        /// The client is free to dismiss all but the last configure event it
        /// received.
        ///
        /// The width and height arguments specify the size of the window in
        /// surface-local coordinates.
        ///
        /// The size is a hint, in the sense that the client is free to ignore it if
        /// it doesn't resize, pick a smaller size (to satisfy aspect ratio or
        /// resize in steps of NxM pixels). If the client picks a smaller size and
        /// is anchored to two opposite anchors (e.g. 'top' and 'bottom'), the
        /// surface will be centered on this axis.
        ///
        /// If the width or height arguments are zero, it means the client should
        /// decide its own window dimension.
        Configure { serial: u32, width: u32, height: u32 },
        /// Surface should be closed
        ///
        /// The closed event is sent by the compositor when the surface will no
        /// longer be shown. The output may have been destroyed or the user may
        /// have asked for it to be removed. Further changes to the surface will be
        /// ignored. The client should destroy the resource after receiving this
        /// event, and create a new surface if they so choose.
        Closed,
    }
    impl EventWriter for crate::wayland_objects::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1 {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
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
                Closed {} => {
                    let mut raw = RawEvent::new(self.id(), 1, 0);
                    raw
                }
            }
        }
    }
    /// Types of keyboard interaction possible for a layer shell surface
    ///
    /// Types of keyboard interaction possible for layer shell surfaces. The
    /// rationale for this is twofold: (1) some applications are not interested
    /// in keyboard events and not allowing them to be focused can improve the
    /// desktop experience; (2) some applications will want to take exclusive
    /// keyboard focus.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum KeyboardInteractivity {
        /// No keyboard focus is possible
        ///
        /// This value indicates that this surface is not interested in keyboard
        /// events and the compositor should never assign it the keyboard focus.
        ///
        /// This is the default value, set for newly created layer shell surfaces.
        ///
        /// This is useful for e.g. desktop widgets that display information or
        /// only have interaction with non-keyboard input devices.
        None = 0,
        /// Request exclusive keyboard focus
        ///
        /// Request exclusive keyboard focus if this surface is above the shell surface layer.
        ///
        /// For the top and overlay layers, the seat will always give
        /// exclusive keyboard focus to the top-most layer which has keyboard
        /// interactivity set to exclusive. If this layer contains multiple
        /// surfaces with keyboard interactivity set to exclusive, the compositor
        /// determines the one receiving keyboard events in an implementation-
        /// defined manner. In this case, no guarantee is made when this surface
        /// will receive keyboard focus (if ever).
        ///
        /// For the bottom and background layers, the compositor is allowed to use
        /// normal focus semantics.
        ///
        /// This setting is mainly intended for applications that need to ensure
        /// they receive all keyboard events, such as a lock screen or a password
        /// prompt.
        Exclusive = 1,
        /// Request regular keyboard focus semantics
        ///
        /// This requests the compositor to allow this surface to be focused and
        /// unfocused by the user in an implementation-defined manner. The user
        /// should be able to unfocus this surface even regardless of the layer
        /// it is on.
        ///
        /// Typically, the compositor will want to use its normal mechanism to
        /// manage keyboard focus between layer shell surfaces with this setting
        /// and regular toplevels on the desktop layer (e.g. click to focus).
        /// Nevertheless, it is possible for a compositor to require a special
        /// interaction to focus or unfocus layer shell surfaces (e.g. requiring
        /// a click even if focus follows the mouse normally, or providing a
        /// keybinding to switch focus between layers).
        ///
        /// This setting is mainly intended for desktop shell components (e.g.
        /// panels) that allow keyboard interaction. Using this option can allow
        /// implementing a desktop shell that can be fully usable without the
        /// mouse.
        OnDemand = 2,
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// Provided surface state is invalid,
        InvalidSurfaceState = 0,
        /// Size is invalid,
        InvalidSize = 1,
        /// Anchor bitfield is invalid,
        InvalidAnchor = 2,
        /// Keyboard interactivity is invalid,
        InvalidKeyboardInteractivity = 3,
        /// Exclusive edge is invalid given the surface anchors,
        InvalidExclusiveEdge = 4,
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Anchor {
        /// The top edge of the anchor rectangle,
        Top = 1,
        /// The bottom edge of the anchor rectangle,
        Bottom = 2,
        /// The left edge of the anchor rectangle,
        Left = 4,
        /// The right edge of the anchor rectangle,
        Right = 8,
    }
}

/// Manager to control CTMs
///
/// This object is a manager which offers requests to control CTMs.
///
/// If any changes are done, once this object is destroyed, CTMs are reset back to
/// an identity matrix.
pub mod hyprland_ctm_control_manager_v1 {
    use super::*;
    impl Constants
        for crate::wayland_objects::hyprland_ctm_control_manager_v1::HyprlandCtmControlManagerV1
    {
        const INTERFACE_NAME: &'static str = "hyprland_ctm_control_manager_v1";
        const INTERFACE_VERSION: u32 = 2;
    }
    impl crate::object::IdToObject for HyprlandCtmControlManagerV1Id {
        type Object =
            crate::wayland_objects::hyprland_ctm_control_manager_v1::HyprlandCtmControlManagerV1;
    }
    impl crate::object::ObjectWithId
        for crate::wayland_objects::hyprland_ctm_control_manager_v1::HyprlandCtmControlManagerV1
    {
        type Id = HyprlandCtmControlManagerV1Id;
    }
    #[derive(Debug)]
    pub enum Request {
        /// Set the CTM of an output
        ///
        /// Set a CTM for a wl_output.
        ///
        /// This state is not applied immediately; clients must call .commit to
        /// apply any pending changes.
        ///
        /// The provided values describe a 3x3 Row-Major CTM with values in the range of [0, ∞)
        ///
        /// Passing values outside of the range will raise an invalid_matrix error.
        ///
        /// The default value of the CTM is an identity matrix.
        ///
        /// If an output doesn't get a CTM set with set_ctm_for_output and commit is called,
        /// that output will get its CTM reset to an identity matrix.
        SetCtmForOutput {
            output: WlOutputId,
            mat0: f32,
            mat1: f32,
            mat2: f32,
            mat3: f32,
            mat4: f32,
            mat5: f32,
            mat6: f32,
            mat7: f32,
            mat8: f32,
        },
        /// Commit the pending state
        ///
        /// Commits the pending state(s) set by set_ctm_for_output.
        Commit,
        /// Destroy the manager
        ///
        /// All objects created by the manager will still remain valid, until their
        /// appropriate destroy request has been called.
        ///
        /// The CTMs of all outputs will be reset to an identity matrix.
        Destroy,
    }
    impl Parser
        for crate::wayland_objects::hyprland_ctm_control_manager_v1::HyprlandCtmControlManagerV1
    {
        type Request = Request;
        fn parse_request(mut req: RawRequest) -> Result<Self::Request, ParserError> {
            use Request::*;
            let request = match req.request_id {
                0 => SetCtmForOutput {
                    output: req.id()?,
                    mat0: req.fixed()?,
                    mat1: req.fixed()?,
                    mat2: req.fixed()?,
                    mat3: req.fixed()?,
                    mat4: req.fixed()?,
                    mat5: req.fixed()?,
                    mat6: req.fixed()?,
                    mat7: req.fixed()?,
                    mat8: req.fixed()?,
                },
                1 => Commit,
                2 => Destroy,
                _ => return Err(ParserError::InvalidRequest),
            };
            Ok(request)
        }
    }
    /// Minimum interface version for the `blocked` message (XML `since` / `version`).
    pub const BLOCKED_SINCE: u32 = 2;
    #[derive(Debug)]
    pub enum Event {
        ///
        ///
        /// This event is sent if another manager was bound by any client
        /// at the time the current manager was bound.
        /// Any set_ctm_for_output requests from a blocked manager will be
        /// silently ignored by the compositor.
        ///
        /// The client should destroy the manager after receiving this event.
        Blocked,
    }
    impl EventWriter
        for crate::wayland_objects::hyprland_ctm_control_manager_v1::HyprlandCtmControlManagerV1
    {
        type Event = Event;
        fn object_id(&self) -> WaylandId {
            use crate::object::Object;
            self.id()
        }
        fn to_raw_event(&self, event: Event) -> RawEvent {
            use {crate::object::Object, Event::*};
            match event {
                Blocked {} => {
                    let mut raw = RawEvent::new(self.id(), 0, 0);
                    raw
                }
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone)]
    pub enum Error {
        /// The matrix values are invalid.,
        InvalidMatrix = 0,
    }
}
