use std::os::fd::{FromRawFd, OwnedFd};

use {super::loader::EGL, khronos_types::egl};

pub(super) const EGL_SYNC_NATIVE_FENCE_ANDROID: egl::Enum = 0x3144;
pub(super) const EGL_SYNC_NATIVE_FENCE_FD_ANDROID: egl::Int = 0x3145;
pub(super) const EGL_NO_NATIVE_FENCE_FD_ANDROID: egl::Int = -1;

#[derive(Clone, Copy, Debug)]
pub(super) struct EglSyncKhr(*mut std::ffi::c_void);

impl EglSyncKhr {
    const NO_SYNC: Self = Self(std::ptr::null_mut());

    pub(super) fn is_null(self) -> bool {
        self.0.is_null()
    }
}

impl EGL {
    pub(super) fn create_sync_khr(
        &self,
        display: egl::Display,
        type_: egl::Enum,
        attrib_list: &[egl::Int],
    ) -> Result<EglSyncKhr, egl::Error> {
        type CreateSyncKhr = unsafe extern "system" fn(
            egl::Display,
            egl::Enum,
            *const egl::Int,
        ) -> *mut std::ffi::c_void;

        Self::assert_ends_with_none(attrib_list);
        let proc = unsafe { egl::GetProcAddress(c"eglCreateSyncKHR".as_ptr()) };
        if proc.is_null() {
            return Err(egl::Error::BadDisplay);
        }
        let create_sync: CreateSyncKhr = unsafe { std::mem::transmute(proc) };
        let sync = unsafe { create_sync(display, type_, attrib_list.as_ptr()) };
        let sync = EglSyncKhr(sync);
        if sync.is_null() {
            return Err(Self::last_error().unwrap_err());
        }
        Ok(sync)
    }

    pub(super) fn destroy_sync_khr(
        &self,
        display: egl::Display,
        sync: EglSyncKhr,
    ) -> Result<(), egl::Error> {
        type DestroySyncKhr =
            unsafe extern "system" fn(egl::Display, *mut std::ffi::c_void) -> egl::Boolean;

        if sync.is_null() {
            return Ok(());
        }

        let proc = unsafe { egl::GetProcAddress(c"eglDestroySyncKHR".as_ptr()) };
        if proc.is_null() {
            return Err(egl::Error::BadDisplay);
        }
        let destroy_sync: DestroySyncKhr = unsafe { std::mem::transmute(proc) };
        let result = unsafe { destroy_sync(display, sync.0) };
        Self::ok_on(result, egl::TRUE as _)?;
        Ok(())
    }

    pub(super) fn dup_native_fence_fd_android(
        &self,
        display: egl::Display,
        sync: EglSyncKhr,
    ) -> Result<OwnedFd, egl::Error> {
        type DupNativeFenceFdAndroid =
            unsafe extern "system" fn(egl::Display, *mut std::ffi::c_void) -> egl::Int;

        let proc = unsafe { egl::GetProcAddress(c"eglDupNativeFenceFDANDROID".as_ptr()) };
        if proc.is_null() {
            return Err(egl::Error::BadDisplay);
        }
        let dup_native_fence: DupNativeFenceFdAndroid = unsafe { std::mem::transmute(proc) };
        let fd = unsafe { dup_native_fence(display, sync.0) };
        if fd == EGL_NO_NATIVE_FENCE_FD_ANDROID {
            return Err(Self::last_error().unwrap_err());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    pub(super) fn wait_sync_khr(
        &self,
        display: egl::Display,
        sync: EglSyncKhr,
        flags: egl::Int,
    ) -> Result<(), egl::Error> {
        type WaitSyncKhr = unsafe extern "system" fn(
            egl::Display,
            *mut std::ffi::c_void,
            egl::Int,
        ) -> egl::Boolean;

        let proc = unsafe { egl::GetProcAddress(c"eglWaitSyncKHR".as_ptr()) };
        if proc.is_null() {
            return Err(egl::Error::BadDisplay);
        }
        let wait_sync: WaitSyncKhr = unsafe { std::mem::transmute(proc) };
        let result = unsafe { wait_sync(display, sync.0, flags) };
        Self::ok_on(result, egl::TRUE as _)?;
        Ok(())
    }
}
