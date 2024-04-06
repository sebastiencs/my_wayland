use std::{
    os::fd::RawFd,
    sync::{atomic::AtomicUsize, Arc},
};

use crate::drm::{Format, Modifier};

#[derive(Debug, Clone)]
pub struct DmaBufId(usize);

impl DmaBufId {
    pub fn next() -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);
        let next = ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self(next)
    }
}

#[derive(Clone, Debug)]
pub struct DmaBufPlane {
    pub offset: u32,
    pub stride: u32,
    pub fd: Arc<RawFd>,
}

#[derive(Debug)]
pub struct DmaBuf {
    pub id: DmaBufId,
    pub width: i32,
    pub height: i32,
    pub format: &'static Format,
    pub modifier: Modifier,
    pub planes: Box<[DmaBufPlane]>,
}
