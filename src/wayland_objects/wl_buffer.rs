use std::rc::Rc;

use crate::{
    clients::{Client, ClientDispatcher, ClientId, EventWriter, RawRequest, Resolved},
    dmabuf::DmaBuf,
    drm::{Format, ARGB8888},
    object::{Object, Parser, RequestResult},
    protocol_generated::{wl_buffer, WaylandId, WlBufferId},
    state::{GrabbedObject, ObjectId, State},
    sync::PlainRelease,
    transform,
};

use super::wl_shm_pool::SharedMemory;

#[derive(Debug)]
enum BufferBacking {
    Shm {
        offset: usize,
        width: usize,
        height: usize,
        stride: usize,
        format: &'static Format,
        mem: Rc<SharedMemory>,
    },
    Dmabuf {
        dmabuf: DmaBuf,
        y_inverted: bool,
    },
}

/// One shared-memory sample. The bytes are borrowed from the buffer's pool.
#[derive(Debug)]
pub(crate) struct ShmSample<'a> {
    pub bytes: &'a [u8],
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub fourcc: drm::buffer::DrmFourcc,
}

#[derive(Debug)]
pub struct WlBuffer {
    id: WlBufferId,
    owner_client_id: ClientId,
    destroyed: bool,
    backing: BufferBacking,
}

#[derive(Clone, Debug)]
pub struct CursorShmBuffer {
    mem: Rc<SharedMemory>,
    offset: usize,
    width: usize,
    height: usize,
    stride: usize,
}

impl CursorShmBuffer {
    pub(crate) fn stride(&self) -> usize {
        self.stride
    }

    pub(crate) fn argb_bytes(&self) -> Vec<u8> {
        let len = self.stride.saturating_mul(self.height);
        if self.offset.saturating_add(len) > self.mem.len() {
            return Vec::new();
        }
        self.mem.bytes_at(self.offset, len).to_vec()
    }
}

pub struct NewWlBufferParams {
    pub id: WlBufferId,
    pub owner_client_id: ClientId,
    pub offset: i32,
    pub width: i32,
    pub height: i32,
    pub stride: i32,
    pub format: &'static Format,
    pub mem: Rc<SharedMemory>,
}

pub struct NewDmabufWlBufferParams {
    pub id: WlBufferId,
    pub owner_client_id: ClientId,
    pub dmabuf: DmaBuf,
    pub y_inverted: bool,
}

#[derive(Debug)]
pub enum WlBufferError {
    InvalidParams,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureBufferError {
    Destroyed,
    NotShm,
    Format,
    Size,
    Stride,
    Overflow,
}

/// A checked writable view of one SHM buffer. Padding past each row's pixels stays untouched.
#[derive(Debug)]
pub struct CaptureShmWrite {
    mem: Rc<SharedMemory>,
    offset: usize,
    width: usize,
    height: usize,
    stride: usize,
}

impl CaptureShmWrite {
    pub(crate) fn write_top_left(
        &self,
        src: &[u8],
        src_stride: usize,
    ) -> Result<(), CaptureBufferError> {
        let row_bytes = self.width.saturating_mul(4);
        if src_stride < row_bytes {
            return Err(CaptureBufferError::Stride);
        }
        let src_bytes = src_stride
            .checked_mul(self.height)
            .ok_or(CaptureBufferError::Overflow)?;
        if src.len() < src_bytes {
            return Err(CaptureBufferError::Overflow);
        }
        let length = self.stride.saturating_mul(self.height);
        let dst = self.mem.bytes_at_mut(self.offset, length);
        for row in 0..self.height {
            let src_start = row.saturating_mul(src_stride);
            let dst_start = row.saturating_mul(self.stride);
            dst[dst_start..dst_start + row_bytes]
                .copy_from_slice(&src[src_start..src_start + row_bytes]);
        }
        Ok(())
    }

    pub(crate) fn stride_bytes(&self) -> usize {
        self.stride
    }

    #[cfg(test)]
    pub(crate) fn as_slice(&self) -> &[u8] {
        let length = self.stride.saturating_mul(self.height);
        self.mem.bytes_at(self.offset, length)
    }
}

impl WlBuffer {
    pub fn new(params: NewWlBufferParams) -> Result<Self, WlBufferError> {
        let NewWlBufferParams {
            id,
            owner_client_id,
            offset,
            width,
            height,
            stride,
            format,
            mem,
        } = params;

        Ok(Self {
            id,
            owner_client_id,
            destroyed: false,
            backing: BufferBacking::Shm {
                offset: offset
                    .try_into()
                    .map_err(|_| WlBufferError::InvalidParams)?,
                width: width.try_into().map_err(|_| WlBufferError::InvalidParams)?,
                height: height
                    .try_into()
                    .map_err(|_| WlBufferError::InvalidParams)?,
                stride: stride
                    .try_into()
                    .map_err(|_| WlBufferError::InvalidParams)?,
                format,
                mem,
            },
        })
    }

    pub fn new_dmabuf(params: NewDmabufWlBufferParams) -> Result<Self, WlBufferError> {
        let NewDmabufWlBufferParams { id, owner_client_id, dmabuf, y_inverted } = params;
        Ok(Self {
            id,
            owner_client_id,
            destroyed: false,
            backing: BufferBacking::Dmabuf { dmabuf, y_inverted },
        })
    }

    pub fn send_release(&mut self, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_buffer::Event::Release, dispatcher);
    }

    pub fn plain_release_target(&self, object_id: ObjectId) -> Option<PlainRelease> {
        if self.destroyed {
            return None;
        }
        Some(Resolved::new(self.owner_client_id, self.id, object_id))
    }

    pub fn mark_destroyed(&mut self) {
        self.destroyed = true;
    }

    pub fn is_destroyed(&self) -> bool {
        self.destroyed
    }

    pub fn dimensions(&self) -> (i32, i32) {
        match &self.backing {
            BufferBacking::Shm { width, height, .. } => (*width as i32, *height as i32),
            BufferBacking::Dmabuf { dmabuf, .. } => (dmabuf.width, dmabuf.height),
        }
    }

    pub fn format(&self) -> &'static Format {
        match &self.backing {
            BufferBacking::Shm { format, .. } => format,
            BufferBacking::Dmabuf { dmabuf, .. } => dmabuf.format,
        }
    }

    pub fn dmabuf(&self) -> Option<&DmaBuf> {
        match &self.backing {
            BufferBacking::Shm { .. } => None,
            BufferBacking::Dmabuf { dmabuf, .. } => Some(dmabuf),
        }
    }

    pub fn y_inverted(&self) -> bool {
        match &self.backing {
            BufferBacking::Shm { .. } => true,
            BufferBacking::Dmabuf { y_inverted, .. } => *y_inverted,
        }
    }

    pub fn supports_explicit_sync(&self) -> bool {
        matches!(self.backing, BufferBacking::Dmabuf { .. })
    }

    pub fn base_tex_transform(&self) -> transform::Mat3 {
        match &self.backing {
            BufferBacking::Shm { .. } => transform::Mat3::identity(),
            BufferBacking::Dmabuf { y_inverted, .. } => {
                if *y_inverted {
                    transform::normalized_texcoord_flip_y()
                } else {
                    transform::Mat3::identity()
                }
            }
        }
    }

    /// Shared-memory pixels for one sample. The client can rewrite them between commits.
    pub(crate) fn shm_sample(&self) -> Option<ShmSample<'_>> {
        let BufferBacking::Shm { offset, width, height, stride, format, mem } = &self.backing
        else {
            return None;
        };
        let bytes = stride.saturating_mul(*height);
        let end = offset.checked_add(bytes)?;
        if end > mem.len() {
            return None;
        }
        Some(ShmSample {
            bytes: mem.bytes_at(*offset, bytes),
            width: *width as i32,
            height: *height as i32,
            stride: *stride,
            fourcc: format.drm,
        })
    }

    /// Accept an SHM `XRGB8888` or `ARGB8888` destination for one capture.
    ///
    /// The buffer size must match the capture, the stride must hold each row, and the mapped
    /// pool must contain `stride * height` bytes from the buffer offset.
    pub fn begin_capture_write(
        &self,
        width: i32,
        height: i32,
    ) -> Result<CaptureShmWrite, CaptureBufferError> {
        if self.destroyed {
            return Err(CaptureBufferError::Destroyed);
        }
        let BufferBacking::Shm {
            offset,
            width: buffer_width,
            height: buffer_height,
            stride,
            format,
            mem,
            ..
        } = &self.backing
        else {
            return Err(CaptureBufferError::NotShm);
        };
        if format.drm != ARGB8888.drm && format.drm != crate::drm::XRGB8888.drm {
            return Err(CaptureBufferError::Format);
        }
        if width <= 0
            || height <= 0
            || *buffer_width != width as usize
            || *buffer_height != height as usize
        {
            return Err(CaptureBufferError::Size);
        }
        let row_bytes = buffer_width.saturating_mul(4);
        if *stride < row_bytes || stride % 4 != 0 {
            return Err(CaptureBufferError::Stride);
        }
        let bytes = stride
            .checked_mul(*buffer_height)
            .ok_or(CaptureBufferError::Overflow)?;
        let end = offset
            .checked_add(bytes)
            .ok_or(CaptureBufferError::Overflow)?;
        if end > mem.len() {
            return Err(CaptureBufferError::Overflow);
        }
        Ok(CaptureShmWrite {
            mem: mem.clone(),
            offset: *offset,
            width: *buffer_width,
            height: *buffer_height,
            stride: *stride,
        })
    }

    pub fn shm_cursor_argb8888(&self) -> Option<CursorShmBuffer> {
        let BufferBacking::Shm { offset, width, height, stride, format, mem, .. } = &self.backing
        else {
            return None;
        };
        if format.drm != ARGB8888.drm {
            return None;
        }
        Some(CursorShmBuffer {
            mem: mem.clone(),
            offset: *offset,
            width: *width,
            height: *height,
            stride: *stride,
        })
    }

    #[cfg(test)]
    pub(crate) fn test_argb8888_shm(
        id: WlBufferId,
        owner_client_id: ClientId,
        width: i32,
        height: i32,
        pixels: Vec<u8>,
    ) -> Self {
        let stride = (width as usize).saturating_mul(ARGB8888.bpp as usize);
        let mem = super::wl_shm_pool::shared_memory_from_bytes(&pixels);
        Self {
            id,
            owner_client_id,
            destroyed: false,
            backing: BufferBacking::Shm {
                offset: 0,
                width: width as usize,
                height: height as usize,
                stride,
                format: ARGB8888,
                mem,
            },
        }
    }
}

impl CursorShmBuffer {
    pub fn width(&self) -> i32 {
        self.width as i32
    }

    pub fn height(&self) -> i32 {
        self.height as i32
    }

    pub fn copy_to_drm_argb8888_cursor_bytes(
        &self,
        output_pitch: usize,
        output_height: usize,
    ) -> Vec<u8> {
        let mut dst = vec![0; output_pitch.saturating_mul(output_height)];
        let row_bytes = self.width.saturating_mul(4);
        let src = self
            .mem
            .clone()
            .slice_at(self.offset, self.stride.saturating_mul(self.height));
        let src = src.as_slice();
        for row in 0..self.height {
            let src_start = row.saturating_mul(self.stride);
            let dst_start = row.saturating_mul(output_pitch);
            let src_row = &src[src_start..src_start + row_bytes];
            let dst_row = &mut dst[dst_start..dst_start + row_bytes];
            dst_row.copy_from_slice(src_row);
        }
        dst
    }
}

impl Object for WlBuffer {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        mut self: GrabbedObject<Self>,
        client: &mut Client,
        _state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        use wl_buffer::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                self.destroyed = true;
                client.remove_wayland_object(self.id);
            }
        }
        Ok(())
    }
}
