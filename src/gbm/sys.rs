use bitflags::bitflags;

bitflags! {
    /// Flags to indicate the intended use for the buffer - these are passed into
    /// gbm_bo_create(). The caller must set the union of all the flags that are
    /// appropriate
    ///
    /// Use gbm_device_is_format_supported() to check if the combination of format
    /// and use flags are supported
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct UseFlags: u32 {
        /// Buffer is going to be presented to the screen using an API such as KMS
        const SCANOUT = 1 << 0;
        /// Buffer is going to be used as cursor
        const CURSOR = 1 << 1;
        /// Buffer is to be used for rendering - for example it is going to be used
        /// as the storage for a color buffer
        const RENDERING = 1 << 2;
        /// Buffer can be used for gbm_bo_write.  This is guaranteed to work
        /// with GBM_BO_USE_CURSOR, but may not work for other combinations.
        const WRITE = 1 << 3;
        /// Buffer is linear, i.e. not tiled.
        const LINEAR = 1 << 4;
        /// Buffer is protected, i.e. encrypted and not readable by CPU or any
        /// other non-secure / non-trusted components nor by non-trusted OpenGL,
        /// OpenCL, and Vulkan applications.
        const PROTECTED = 1 << 5;
        /// The buffer will be used for front buffer rendering.  On some
        /// platforms this may (for example) disable framebuffer compression
        /// to avoid problems with compression flags data being out of sync
        /// with pixel data.
        const FRONT_RENDERING = 1 << 6;
    }

    /// Flags to indicate the type of mapping for the buffer - these are
    /// passed into gbm_bo_map(). The caller must set the union of all the
    /// flags that are appropriate.
    ///
    /// These flags are independent of the GBM_BO_USE_* creation flags. However,
    /// mapping the buffer may require copying to/from a staging buffer.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct TransferFlags: u32 {
        /// Buffer contents read back (or accessed directly) at transfer
        /// create time.
        const READ = 1 << 0;
        /// Buffer contents will be written back at unmap time
        /// (or modified as a result of being accessed directly).
        const WRITE = 1 << 1;
        /// Read/modify/write
        const READ_WRITE = Self::READ.bits() | Self::WRITE.bits();
    }
}

pub const GBM_BO_IMPORT_WL_BUFFER: usize = 0x5501;
pub const GBM_BO_IMPORT_EGL_IMAGE: usize = 0x5502;
pub const GBM_BO_IMPORT_FD: usize = 0x5503;
pub const GBM_BO_IMPORT_FD_MODIFIER: usize = 0x5504;

pub const GBM_MAX_PLANES: usize = 4;

#[allow(non_camel_case_types)]
pub(super) type gbm_device = libc::c_void;
#[allow(non_camel_case_types)]
pub(super) type gbm_bo = libc::c_void;
#[allow(non_camel_case_types)]
pub(super) type gbm_surface = libc::c_void;

#[allow(non_camel_case_types)]
#[repr(C)]
struct gbm_import_fd_modifier_data {
    width: u32,
    height: u32,
    format: u32,
    num_fds: u32,
    fds: [libc::c_int; GBM_MAX_PLANES],
    strides: [libc::c_int; GBM_MAX_PLANES],
    offsets: [libc::c_int; GBM_MAX_PLANES],
    modifier: u64,
}

#[link(name = "gbm")]
extern "C" {
    pub fn gbm_create_device(fd: libc::c_int) -> *mut gbm_device;
    pub fn gbm_bo_create(
        dev: *mut gbm_device,
        width: u32,
        height: u32,
        format: u32,
        flags: u32,
    ) -> *mut gbm_bo;
    pub fn gbm_bo_create_with_modifiers2(
        dev: *mut gbm_device,
        width: u32,
        height: u32,
        format: u32,
        modifiers: *const u64,
        count: libc::c_uint,
        flags: u32,
    ) -> *mut gbm_bo;
    pub fn gbm_device_destroy(dev: *mut gbm_device);
    pub fn gbm_surface_create(
        dev: *mut gbm_device,
        width: u32,
        height: u32,
        format: u32,
        flags: u32,
    ) -> *mut gbm_surface;
    pub fn gbm_bo_import(dev: *mut gbm_device, ty: u32, buffer: *mut u8, flags: u32)
        -> *mut gbm_bo;
    pub fn gbm_surface_lock_front_buffer(surface: *mut gbm_surface) -> *mut gbm_bo;
    pub fn gbm_bo_get_offset(bo: *mut gbm_bo, plane: libc::c_int) -> u32;
    pub fn gbm_bo_destroy(bo: *mut gbm_bo);
    pub fn gbm_bo_get_fd(bo: *mut gbm_bo) -> libc::c_int;
    pub fn gbm_bo_get_fd_for_plane(bo: *mut gbm_bo, plane: libc::c_int) -> libc::c_int;
    pub fn gbm_bo_get_handle_for_plane(bo: *mut gbm_bo, plane: libc::c_int) -> u64;
    pub fn gbm_bo_get_plane_count(bo: *mut gbm_bo) -> libc::c_int;
    pub fn gbm_bo_get_stride_for_plane(bo: *mut gbm_bo, plane: libc::c_int) -> u32;
    pub fn gbm_bo_get_width(bo: *mut gbm_bo) -> u32;
    pub fn gbm_bo_get_height(bo: *mut gbm_bo) -> u32;
    pub fn gbm_bo_get_handle(bo: *mut gbm_bo) -> u64;
    pub fn gbm_bo_get_stride(bo: *mut gbm_bo) -> u32;
    pub fn gbm_bo_get_bpp(bo: *mut gbm_bo) -> u32;
    pub fn gbm_bo_get_format(bo: *mut gbm_bo) -> u32;
    pub fn gbm_bo_get_modifier(bo: *mut gbm_bo) -> u64;
    pub fn gbm_bo_write(bo: *mut gbm_bo, buf: *mut libc::c_void, count: libc::size_t) -> i32;
    pub fn gbm_bo_unmap(bo: *mut gbm_bo, map_data: *mut u8);
    pub fn gbm_surface_release_buffer(gbm_surface: *mut gbm_surface, gbm_bo: *mut gbm_bo);
    pub fn gbm_surface_destroy(surface: *mut gbm_surface);
    pub fn gbm_bo_map(
        bo: *mut gbm_bo,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        flags: u32,
        strid: *mut u32,
        map_data: *mut *mut u8,
    ) -> *mut u8;
}

// void
// gbm_surface_release_buffer(struct gbm_surface *surface, struct gbm_bo *bo);

// int
// gbm_bo_write(struct gbm_bo *bo, const void *buf, size_t count);

// struct gbm_bo *
// gbm_surface_lock_front_buffer(struct gbm_surface *surface);

// struct gbm_surface *
// gbm_surface_create(struct gbm_device *gbm,
//                    uint32_t width, uint32_t height,
// 		   uint32_t format, uint32_t flags);

// struct gbm_surface *
// gbm_surface_create_with_modifiers(struct gbm_device *gbm,
//                                   uint32_t width, uint32_t height,
//                                   uint32_t format,
//                                   const uint64_t *modifiers,
//                                   const unsigned int count);

// struct gbm_surface *
// gbm_surface_create_with_modifiers2(struct gbm_device *gbm,
//                                    uint32_t width, uint32_t height,
//                                    uint32_t format,
//                                    const uint64_t *modifiers,
//                                    const unsigned int count,
//                                    uint32_t flags);
