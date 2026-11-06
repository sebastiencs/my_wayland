use std::{
    num::NonZeroU32,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd},
};

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
    #[error("failed to create an independent GBM allocator: {0}")]
    FailedCreate(String),
}

/// Which independent DRM file description owns the GBM device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AllocatorOrigin {
    RenderNode,
    PrimaryLease,
    PrimaryReopen,
}

pub(crate) fn allocator_fallback_order() -> [AllocatorOrigin; 3] {
    [AllocatorOrigin::RenderNode, AllocatorOrigin::PrimaryLease, AllocatorOrigin::PrimaryReopen]
}

#[derive(Debug)]
pub struct GbmDevice {
    drm: DrmFd,
    pub gbm_device: *mut sys::gbm_device,
    origin: AllocatorOrigin,
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

impl GbmDevice {
    pub(crate) fn describe_node(&self) -> String {
        self.drm.describe_node()
    }

    pub(crate) fn allocator_fd(&self) -> DrmFd {
        self.drm.clone()
    }
}

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
    /// Create GBM on an independent DRM file description.
    ///
    /// The KMS master is only the source used to find that description. The
    /// render node is preferred. An empty primary lease, then a separately
    /// reopened primary node, are fallbacks. The caller's file description is
    /// never reused.
    pub fn new(kms: &DrmFd) -> Result<Self, GbmError> {
        let mut failures = Vec::new();
        for origin in allocator_fallback_order() {
            match Self::open_allocator(kms, origin) {
                Ok(device) => {
                    log::info!(
                        "GBM allocator selected {:?}: primary={} allocator={}",
                        device.origin,
                        kms.describe_node(),
                        device.drm.describe_node()
                    );
                    return Ok(device);
                }
                Err(error) => {
                    log::warn!("GBM allocator candidate {origin:?} failed: {error}");
                    failures.push(format!("{origin:?}: {error}"));
                }
            }
        }
        Err(GbmError::FailedCreate(failures.join("; ")))
    }

    fn open_allocator(kms: &DrmFd, origin: AllocatorOrigin) -> Result<Self, GbmError> {
        let drm = match origin {
            AllocatorOrigin::RenderNode => DrmFd::from_owned(
                kms.open_render_node()
                    .map_err(|error| GbmError::FailedCreate(error.to_string()))?,
            ),
            AllocatorOrigin::PrimaryLease => kms.try_create_lease()?,
            AllocatorOrigin::PrimaryReopen => DrmFd::from_owned(
                kms.open_primary_node()
                    .map_err(|error| GbmError::FailedCreate(error.to_string()))?,
            ),
        };
        let gbm_device = unsafe { sys::gbm_create_device(drm.as_raw_fd()) };
        if gbm_device.is_null() {
            return Err(GbmError::FailedCreate(format!("{origin:?}: gbm_create_device failed")));
        }
        if !probe_rendering_export(gbm_device) {
            unsafe { sys::gbm_device_destroy(gbm_device) };
            return Err(GbmError::FailedCreate(format!(
                "{origin:?}: rendering export probe failed"
            )));
        }
        Ok(Self { drm, gbm_device, origin })
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
        flags: UseFlags,
        modifiers: &[Modifier],
    ) -> BufferObject {
        let bo = self.allocate_bo(width, height, format, flags, modifiers);
        let dmabuf = make_dmabuf(bo);
        BufferObject { bo, dmabuf: Some(dmabuf) }
    }

    pub(crate) fn try_create_bo_with_size(
        &self,
        width: u32,
        height: u32,
        format: drm::buffer::DrmFourcc,
        flags: UseFlags,
    ) -> Option<BufferObject> {
        let bo = unsafe {
            sys::gbm_bo_create(self.gbm_device, width, height, format as u32, flags.bits())
        };
        exported_bo(bo)
    }

    /// Cursor scanout buffers are linear and renderer-owned. `explicit_flags`
    /// selects `gbm_bo_create_with_modifiers2([LINEAR], flags)`; `None` selects
    /// plain `gbm_bo_create` with scanout, rendering, and linear usage.
    pub(crate) fn try_create_linear_cursor_bo(
        &self,
        width: u32,
        height: u32,
        explicit_flags: Option<UseFlags>,
    ) -> Option<BufferObject> {
        let format = drm::buffer::DrmFourcc::Argb8888;
        match explicit_flags {
            Some(flags) => {
                log::debug!(
                    "Allocating linear cursor GBM BO with modifiers: size={width}x{height} flags={flags:?} modifiers=[LINEAR]"
                );
                self.try_create_bo_with_modifiers(width, height, format, flags, &[Modifier::LINEAR])
            }
            None => {
                let flags = UseFlags::SCANOUT | UseFlags::RENDERING | UseFlags::LINEAR;
                log::debug!(
                    "Allocating linear cursor GBM BO without modifiers: size={width}x{height} flags={flags:?}"
                );
                self.try_create_bo_with_size(width, height, format, flags)
            }
        }
    }

    pub(crate) fn try_create_bo_with_modifiers(
        &self,
        width: u32,
        height: u32,
        format: drm::buffer::DrmFourcc,
        flags: UseFlags,
        modifiers: &[Modifier],
    ) -> Option<BufferObject> {
        if modifiers.is_empty() {
            return None;
        }
        let flags = flags & !UseFlags::LINEAR;
        let bo = unsafe {
            sys::gbm_bo_create_with_modifiers2(
                self.gbm_device,
                width,
                height,
                format as u32,
                utils::ptr_or_null(modifiers) as _,
                modifiers.len() as _,
                flags.bits(),
            )
        };
        exported_bo(bo)
    }

    fn allocate_bo(
        &self,
        width: u32,
        height: u32,
        format: drm::buffer::DrmFourcc,
        mut flags: UseFlags,
        mut modifiers: &[Modifier],
    ) -> *mut gbm_bo {
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
        bo
    }
}

fn probe_rendering_export(device: *mut sys::gbm_device) -> bool {
    let bo = unsafe {
        sys::gbm_bo_create(
            device,
            1,
            1,
            drm::buffer::DrmFourcc::Xrgb8888 as u32,
            UseFlags::RENDERING.bits(),
        )
    };
    if bo.is_null() {
        return false;
    }
    let fd = unsafe { sys::gbm_bo_get_fd(bo) };
    unsafe { sys::gbm_bo_destroy(bo) };
    if fd < 0 {
        return false;
    }
    unsafe { libc::close(fd) };
    true
}

fn exported_bo(bo: *mut gbm_bo) -> Option<BufferObject> {
    if bo.is_null() {
        return None;
    }
    let Some(dmabuf) = try_export_dmabuf(bo) else {
        unsafe { sys::gbm_bo_destroy(bo) };
        return None;
    };
    Some(BufferObject { bo, dmabuf: Some(dmabuf) })
}

fn make_dmabuf(bo: *mut libc::c_void) -> DmaBuf {
    try_export_dmabuf(bo).expect("GBM buffer exported a dmabuf")
}

fn try_export_dmabuf(bo: *mut libc::c_void) -> Option<DmaBuf> {
    unsafe {
        let nplanes = sys::gbm_bo_get_plane_count(bo);
        if nplanes <= 0 || nplanes > 4 {
            return None;
        }
        let format = drm_fourcc::DrmFourcc::try_from(sys::gbm_bo_get_format(bo)).ok()?;
        let format = *FORMATS_MAP.get(&format)?;
        let mut planes = Vec::with_capacity(nplanes as usize);
        for index in 0..nplanes {
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
            if fd < 0 {
                return None;
            }
            planes.push(DmaBufPlane { offset, stride, fd: OwnedFd::from_raw_fd(fd) });
        }
        Some(DmaBuf {
            id: DmaBufId::next(),
            width: sys::gbm_bo_get_width(bo) as i32,
            height: sys::gbm_bo_get_height(bo) as i32,
            format,
            modifier: Modifier(sys::gbm_bo_get_modifier(bo)),
            planes: planes.into_boxed_slice(),
        })
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

impl Drop for GbmSurface {
    fn drop(&mut self) {
        if !self.surface.is_null() {
            unsafe { sys::gbm_surface_destroy(self.surface) };
            self.surface = std::ptr::null_mut();
        }
    }
}

#[derive(Debug)]
pub struct BufferObject {
    bo: *mut gbm_bo,
    dmabuf: Option<DmaBuf>,
}

impl BufferObject {
    pub fn raw(&self) -> *mut gbm_bo {
        self.bo
    }

    pub fn write(&self, buf: &[u8]) {
        let _res_write: i32 =
            unsafe { sys::gbm_bo_write(self.bo, buf.as_ptr() as *mut _, buf.len()) };
    }

    pub fn dmabuf(&self) -> Option<&DmaBuf> {
        self.dmabuf.as_ref()
    }

    pub fn plane_count(&self) -> i32 {
        unsafe { sys::gbm_bo_get_plane_count(self.bo) }
    }

    pub fn modifier(&self) -> Modifier {
        Modifier(unsafe { sys::gbm_bo_get_modifier(self.bo) })
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

impl drm::buffer::PlanarBuffer for BufferObject {
    fn size(&self) -> (u32, u32) {
        <Self as drm::buffer::Buffer>::size(self)
    }

    fn format(&self) -> drm::buffer::DrmFourcc {
        <Self as drm::buffer::Buffer>::format(self)
    }

    fn modifier(&self) -> Option<drm::buffer::DrmModifier> {
        match self.modifier() {
            Modifier::INVALID | Modifier::LINEAR | Modifier::ZERO => None,
            modifier => Some(drm::buffer::DrmModifier::from(modifier.0)),
        }
    }

    fn pitches(&self) -> [u32; 4] {
        let mut pitches = [0; 4];
        for index in 0..self.plane_count() {
            pitches[index as usize] = unsafe { sys::gbm_bo_get_stride_for_plane(self.bo, index) };
        }
        pitches
    }

    fn handles(&self) -> [Option<drm::buffer::Handle>; 4] {
        let mut handles = [None; 4];
        for index in 0..self.plane_count() {
            let handle = unsafe { sys::gbm_bo_get_handle_for_plane(self.bo, index) as u32 };
            let raw = NonZeroU32::new(handle).unwrap();
            handles[index as usize] = Some(raw.into());
        }
        handles
    }

    fn offsets(&self) -> [u32; 4] {
        let mut offsets = [0; 4];
        for index in 0..self.plane_count() {
            offsets[index as usize] = unsafe { sys::gbm_bo_get_offset(self.bo, index) };
        }
        offsets
    }
}

impl Drop for GbmDevice {
    fn drop(&mut self) {
        if !self.gbm_device.is_null() {
            unsafe { sys::gbm_device_destroy(self.gbm_device) };
            self.gbm_device = std::ptr::null_mut();
        }
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

#[cfg(test)]
mod tests {
    use std::os::fd::{FromRawFd, OwnedFd};

    use {
        super::{allocator_fallback_order, AllocatorOrigin, GbmDevice},
        crate::drm::DrmFd,
    };

    #[test]
    fn allocator_candidates_prefer_the_render_node() {
        assert_eq!(
            allocator_fallback_order(),
            [
                AllocatorOrigin::RenderNode,
                AllocatorOrigin::PrimaryLease,
                AllocatorOrigin::PrimaryReopen,
            ]
        );
    }

    #[test]
    fn a_non_drm_fd_does_not_become_the_allocator() {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let _write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        let error = GbmDevice::new(&DrmFd::from_owned(read)).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("RenderNode"), "{text}");
        assert!(text.contains("PrimaryLease"), "{text}");
        assert!(text.contains("PrimaryReopen"), "{text}");
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
