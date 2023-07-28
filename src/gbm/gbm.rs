use std::os::fd::AsRawFd;

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

pub struct GbmDevice {
    drm: DrmFd,
    pub gbm_device: *mut sys::gbm_device,
}

impl GbmDevice {
    pub fn new(drm: &DrmFd) -> Result<Self, GbmError> {
        let drm = drm.create_lease()?;
        let gbm_device = unsafe { sys::gbm_create_device(drm.as_raw_fd()) };
        if gbm_device.is_null() {
            return Err(GbmError::FailedCreate);
        }
        Ok(Self { drm, gbm_device })
    }
}
