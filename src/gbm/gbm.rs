use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};

use crate::{
    dmabuf::{DmaBuf, DmaBufId, DmaBufPlane},
    drm::{Modifier, FORMATS_MAP},
    utils,
};

use super::sys::{gbm_bo, gbm_surface, UseFlags};

use {
    crate::{drm::DrmFd, gbm::sys},
    thiserror::Error,
};

#[derive(Error, Debug)]
pub enum GbmError {
    #[error("IO Error: `{0}`")]
    IO(#[from] std::io::Error),
    #[error("Failed to create gbm device")]
    FailedCreate,
}

#[derive(Debug)]
pub struct GbmDevice {
    drm: DrmFd,
    pub gbm_device: *mut sys::gbm_device,
}

impl AsFd for GbmDevice {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.drm.as_fd()
    }
}

impl AsRawFd for GbmDevice {
    fn as_raw_fd(&self) -> RawFd {
        self.drm.as_raw_fd()
    }
}

impl drm::Device for GbmDevice {}

impl drm::control::Device for GbmDevice {}

// impl Device

//  * struct drm_mode_rect - Two dimensional rectangle.
//  * @x1: Horizontal starting coordinate (inclusive).
//  * @y1: Vertical starting coordinate (inclusive).
//  * @x2: Horizontal ending coordinate (exclusive).
//  * @y2: Vertical ending coordinate (exclusive).
//  *
//  * With drm subsystem using struct drm_rect to manage rectangular area this
//  * export it to user-space.
//  *
//  * Currently used by drm_mode_atomic blob property FB_DAMAGE_CLIPS.
//  */
// struct drm_mode_rect {
// 	__s32 x1;
// 	__s32 y1;
// 	__s32 x2;
// 	__s32 y2;
// };

impl GbmDevice {
    /// Create GBM on the same DRM master fd used for KMS (not a lease fd).
    pub fn new(drm: &DrmFd) -> Result<Self, GbmError> {
        let gbm_device = unsafe { sys::gbm_create_device(drm.as_raw_fd()) };
        if gbm_device.is_null() {
            return Err(GbmError::FailedCreate);
        }
        Ok(Self { drm: drm.clone(), gbm_device })
    }

    pub fn create_surface(&self, mode: &drm::control::Mode) -> GbmSurface {
        let (width, height) = mode.size();
        let (width, height) = (width as u32, height as u32);

        let format = drm::buffer::DrmFourcc::Xrgb8888 as u32;
        let flags = UseFlags::SCANOUT.bits() | UseFlags::RENDERING.bits();
        // let flags = UseFlags::SCANOUT.bits() | UseFlags::RENDERING.bits() | UseFlags::WRITE.bits();
        // let flags = UseFlags::WRITE.bits();

        let surface =
            unsafe { sys::gbm_surface_create(self.gbm_device, width, height, format, flags) };
        assert!(!surface.is_null());

        GbmSurface { surface }
    }

    pub fn create_bo(
        &self,
        mode: &drm::control::Mode,
        flags: UseFlags,
        modifiers: &[Modifier],
    ) -> BufferObject {
        let (width, height) = mode.size();
        self.create_bo_with_size(
            width as u32,
            height as u32,
            drm::buffer::DrmFourcc::Xrgb8888,
            flags,
            modifiers,
        )
    }

    pub fn create_bo_with_size(
        &self,
        width: u32,
        height: u32,
        format: drm::buffer::DrmFourcc,
        mut flags: UseFlags,
        mut modifiers: &[Modifier],
    ) -> BufferObject {
        let format = format as u32;
        let use_plain_create = modifiers.is_empty()
            || modifiers == [Modifier::INVALID]
            || modifiers
                .iter()
                .all(|m| *m == Modifier::LINEAR || *m == Modifier::ZERO);

        let bo = if use_plain_create {
            log::debug!(
                "Allocating GBM BO without modifiers: size={}x{} flags={:?}",
                width,
                height,
                flags
            );
            unsafe { sys::gbm_bo_create(self.gbm_device, width, height, format, flags.bits()) }
        } else {
            flags &= !UseFlags::LINEAR;
            log::debug!(
                "Allocating GBM BO with modifiers: size={}x{} flags={:?} modifiers={:?}",
                width,
                height,
                flags,
                modifiers
            );
            unsafe {
                sys::gbm_bo_create_with_modifiers2(
                    self.gbm_device,
                    width,
                    height,
                    format,
                    utils::ptr_or_null(modifiers) as _,
                    modifiers.len() as _,
                    flags.bits(),
                )
            }
        };
        assert!(!bo.is_null());

        let dmabuf = make_dmabuf(bo);
        BufferObject { bo, dmabuf }
    }
}

fn make_dmabuf(bo: *mut libc::c_void) -> DmaBuf {
    unsafe {
        let nplanes = sys::gbm_bo_get_plane_count(bo);
        DmaBuf {
            id: DmaBufId::next(),
            width: sys::gbm_bo_get_width(bo) as i32,
            height: sys::gbm_bo_get_height(bo) as i32,
            format: {
                let format = sys::gbm_bo_get_format(bo);
                let format = drm_fourcc::DrmFourcc::try_from(format).unwrap();
                FORMATS_MAP.get(&format).unwrap()
            },
            modifier: Modifier(sys::gbm_bo_get_modifier(bo)),
            planes: {
                (0..nplanes)
                    .map(|index| {
                        let offset = sys::gbm_bo_get_offset(bo, index);
                        let stride = sys::gbm_bo_get_stride_for_plane(bo, index);
                        let mut fd = sys::gbm_bo_get_fd_for_plane(bo, index);
                        if fd < 0 && nplanes == 1 {
                            fd = sys::gbm_bo_get_fd(bo);
                            if fd >= 0 {
                                log::debug!(
                                    "gbm_bo_get_fd_for_plane failed for single-plane BO, falling back to gbm_bo_get_fd"
                                );
                            }
                        }
                        assert!(fd >= 0);
                        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
                        DmaBufPlane { offset, stride, fd }
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice()
            },
        }
    }
}

#[derive(Debug)]
pub struct GbmSurface {
    surface: *mut gbm_surface,
}

impl GbmSurface {
    pub fn raw(&self) -> *mut gbm_surface {
        self.surface
    }

    pub fn lock_front_buffer(&self) -> BufferObject {
        todo!()
        // let bo = unsafe { sys::gbm_surface_lock_front_buffer(self.surface) };
        // assert!(!bo.is_null());
        // BufferObject { bo }
    }

    pub fn release_buffer(&self, bo: BufferObject) {
        unsafe { sys::gbm_surface_release_buffer(self.surface, bo.raw()) }
    }
}

#[derive(Debug)]
pub struct BufferObject {
    bo: *mut gbm_bo,
    dmabuf: DmaBuf,
}

impl BufferObject {
    pub fn raw(&self) -> *mut gbm_bo {
        self.bo
    }

    pub fn write(&self, buf: &[u8]) {
        let _res_write: i32 =
            unsafe { sys::gbm_bo_write(self.bo, buf.as_ptr() as *mut _, buf.len()) };
    }

    pub fn dmabuf(&self) -> &DmaBuf {
        &self.dmabuf
    }
}

impl drm::buffer::Buffer for BufferObject {
    fn size(&self) -> (u32, u32) {
        let width = unsafe { sys::gbm_bo_get_width(self.bo) };
        let height = unsafe { sys::gbm_bo_get_height(self.bo) };

        (width, height)
    }

    fn format(&self) -> drm::buffer::DrmFourcc {
        let format = unsafe { sys::gbm_bo_get_format(self.bo) };
        drm::buffer::DrmFourcc::try_from(format).unwrap()
    }

    fn pitch(&self) -> u32 {
        unsafe { sys::gbm_bo_get_stride(self.bo) }
    }

    fn handle(&self) -> drm::buffer::Handle {
        let handle = unsafe { sys::gbm_bo_get_handle(self.bo) };
        let handle = handle as u32;

        unsafe { std::mem::transmute(handle) }
    }
}

impl Drop for BufferObject {
    fn drop(&mut self) {
        if !self.bo.is_null() {
            unsafe {
                sys::gbm_bo_destroy(self.bo);
            }
            self.bo = std::ptr::null_mut();
        }
    }
}

// union gbm_bo_handle
// gbm_bo_get_handle(struct gbm_bo *bo);

// union gbm_bo_handle {
//    void *ptr;
//    int32_t s32;
//    uint32_t u32;
//    int64_t s64;
//    uint64_t u64;
// };
