use std::rc::Rc;

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    drm::Format,
    egl::{GlTexture, RenderItem},
    object::{Object, Parser},
    protocol_generated::{wl_buffer, WaylandId, WlBufferId},
    state::State,
};

use super::wl_shm_pool::SharedMemory;

#[derive(Debug)]
pub struct WlBuffer {
    id: WlBufferId,
    offset: usize,
    width: usize,
    height: usize,
    stride: usize,
    format: &'static Format,

    mem: Rc<SharedMemory>,
    texture: Option<GlTexture>,
}

pub struct NewWlBufferParams {
    pub id: WlBufferId,
    pub offset: i32,
    pub width: i32,
    pub height: i32,
    pub stride: i32,
    pub format: &'static Format,
    pub mem: Rc<SharedMemory>,
}

#[derive(Debug)]
pub enum WlBufferError {
    InvalidParams,
}

impl WlBuffer {
    pub fn new(params: NewWlBufferParams) -> Result<Self, WlBufferError> {
        let NewWlBufferParams { id, offset, width, height, stride, format, mem } = params;

        Ok(Self {
            id,
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
            texture: None,
            mem,
        })
    }

    pub fn send_release(&mut self, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_buffer::Event::Release, dispatcher);
    }

    pub fn update_texture(&mut self) {
        let texture = GlTexture::import_shm(
            self.mem.clone().slice(),
            self.width,
            self.height,
            self.stride,
            self.format,
        )
        .unwrap();
        self.texture = Some(texture);
    }

    pub fn render_item(&self, x: i32, y: i32) -> Option<RenderItem> {
        let texture = match self.texture.as_ref() {
            Some(texture) => texture,
            None => {
                log::warn!("WlBuffer::render_item missing texture for buffer={:?}", self.id);
                return None;
            }
        };
        Some(RenderItem {
            texture: texture.texture_id(),
            x,
            y,
            width: self.width as i32,
            height: self.height as i32,
        })
    }
}

impl Object for WlBuffer {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        &mut self,
        client: &mut Client,
        _: &mut State,
        req: RawRequest,
        _: &mut ClientDispatcher,
    ) {
        use wl_buffer::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => todo!(),
        }
    }
}
