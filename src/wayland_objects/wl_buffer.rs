use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    drm::Format,
    egl::GlTexture,
    object::{Object, Parser},
    protocol_generated::{wl_buffer, WaylandId, WlBufferId},
    state::State,
};

#[derive(Debug)]
pub struct WlBuffer {
    id: WlBufferId,
    offset: usize,
    width: usize,
    height: usize,
    stride: usize,
    format: &'static Format,

    texture: Option<GlTexture>,
}

pub struct NewWlBufferParams {
    pub id: WlBufferId,
    pub offset: i32,
    pub width: i32,
    pub height: i32,
    pub stride: i32,
    pub format: &'static Format,
}

#[derive(Debug)]
pub enum WlBufferError {
    InvalidParams,
}

impl WlBuffer {
    pub fn new(params: NewWlBufferParams) -> Result<Self, WlBufferError> {
        let NewWlBufferParams { id, offset, width, height, stride, format } = params;

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
        })
    }
}

impl Object for WlBuffer {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, _: &mut Client, _: &mut State, req: RawRequest, _: &mut ClientDispatcher) {
        use wl_buffer::Request::*;

        match Self::parse(req).unwrap() {
            Destroy => todo!(),
        }
    }
}
