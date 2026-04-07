use std::rc::Rc;

use crate::{
    clients::{Client, ClientDispatcher, ClientId, EventWriter, RawRequest, Resolved},
    dmabuf::DmaBuf,
    drm::{Format, ARGB8888},
    egl::{EglContext, GlTexture},
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
        texture: Option<GlTexture>,
    },
    Dmabuf {
        dmabuf: DmaBuf,
        y_inverted: bool,
        texture: Option<GlTexture>,
    },
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
                texture: None,
            },
        })
    }

    pub fn new_dmabuf(params: NewDmabufWlBufferParams) -> Result<Self, WlBufferError> {
        let NewDmabufWlBufferParams { id, owner_client_id, dmabuf, y_inverted } = params;
        Ok(Self {
            id,
            owner_client_id,
            destroyed: false,
            backing: BufferBacking::Dmabuf { dmabuf, y_inverted, texture: None },
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

    pub fn prepare_texture(
        &mut self,
        egl: &EglContext,
        sync: &crate::sync::SurfaceSyncState,
    ) -> Option<u32> {
        match &mut self.backing {
            BufferBacking::Shm { offset, width, height, format, stride, mem, texture } => {
                // wl_shm contents can change between commits while the wl_buffer object stays alive,
                // so conservatively re-upload the texture every time we sample it.
                let bytes = stride.saturating_mul(*height);
                match GlTexture::import_shm(
                    mem.clone().slice_at(*offset, bytes),
                    *width,
                    *height,
                    *stride,
                    format,
                ) {
                    Ok(imported) => *texture = Some(imported),
                    Err(()) => {
                        log::warn!("failed to upload shm wl_buffer {:?}", self.id);
                        return None;
                    }
                }
                Some(texture.as_ref()?.texture_id())
            }
            BufferBacking::Dmabuf { dmabuf, y_inverted, texture } => {
                if !sync.acquire.is_none() {
                    log::debug!(
                        "applying explicit acquire wait before sampling dma-buf wl_buffer={:?}",
                        self.id
                    );
                    sync.acquire.wait_for_renderer(egl);
                }
                if texture.is_none() {
                    let image = egl.import_dmabuf(dmabuf);
                    match image.into_texture() {
                        Ok(imported) => *texture = Some(imported),
                        Err(()) => {
                            log::warn!("failed to import dma-buf wl_buffer {:?}", self.id);
                            return None;
                        }
                    }
                }
                let _ = y_inverted;
                Some(texture.as_ref()?.texture_id())
            }
        }
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
                texture: None,
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
        _: &mut ClientDispatcher,
    ) -> RequestResult {
        use wl_buffer::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                self.destroyed = true;
                client.remove_wayland_object(self.id);
            }
        }
        Ok(())
    }
}
