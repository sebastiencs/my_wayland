use std::{
    os::fd::RawFd,
    sync::{atomic::AtomicUsize, Arc},
};

use crate::{
    drm::{Format, Modifier},
    gbm::BufferObject,
};

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

// impl PlanarBuffer for DmaBuf {
// }

// impl DmaBuf {
//     pub fn from_bo(bo: &BufferObject) -> Self {

//         todo!()
//     }
// }

// unsafe fn export_bo(dmabuf_ids: &DmaBufIds, bo: *mut Bo) -> Result<DmaBuf, GbmError> {
//     Ok(DmaBuf {
//         id: dmabuf_ids.next(),
//         width: gbm_bo_get_width(bo) as _,
//         height: gbm_bo_get_height(bo) as _,
//         modifier: gbm_bo_get_modifier(bo),
//         format: {
//             let format = gbm_bo_get_format(bo);
//             match formats().get(&format).copied() {
//                 Some(f) => f,
//                 _ => return Err(GbmError::UnknownFormat),
//             }
//         },
//         planes: {
//             let mut planes = PlaneVec::new();
//             for plane in 0..gbm_bo_get_plane_count(bo) {
//                 let offset = gbm_bo_get_offset(bo, plane);
//                 let stride = gbm_bo_get_stride_for_plane(bo, plane);
//                 let fd = gbm_bo_get_fd_for_plane(bo, plane);
//                 if fd < 0 {
//                     return Err(GbmError::DrmFd);
//                 }
//                 planes.push(DmaBufPlane {
//                     offset,
//                     stride,
//                     fd: Rc::new(OwnedFd::new(fd)),
//                 })
//             }
//             planes
//         },
//     })
// }
