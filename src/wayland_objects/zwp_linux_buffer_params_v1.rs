use std::{
    collections::BTreeMap,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    dmabuf::{DmaBuf, DmaBufId, DmaBufPlane},
    drm::{Modifier, FORMATS_MAP},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_display, zwp_linux_buffer_params_v1, WaylandId, ZwpLinuxBufferParamsV1Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::wl_buffer::{NewDmabufWlBufferParams, WlBuffer},
};

#[derive(Debug)]
struct PendingPlane {
    fd: OwnedFd,
    offset: u32,
    stride: u32,
    modifier: Modifier,
}

#[derive(Debug)]
pub struct ZwpLinuxBufferParamsV1 {
    id: ZwpLinuxBufferParamsV1Id,
    used: bool,
    planes: BTreeMap<u32, PendingPlane>,
}

impl ZwpLinuxBufferParamsV1 {
    pub fn new(id: ZwpLinuxBufferParamsV1Id) -> Self {
        Self { id, used: false, planes: BTreeMap::new() }
    }

    fn already_used(
        &self,
        dispatcher: &mut ClientDispatcher,
    ) -> Result<(), zwp_linux_buffer_params_v1::Error> {
        if self.used {
            self.send_error(
                dispatcher,
                zwp_linux_buffer_params_v1::Error::AlreadyUsed as u32,
                "buffer params already used",
            );
            Err(zwp_linux_buffer_params_v1::Error::AlreadyUsed)
        } else {
            Ok(())
        }
    }

    fn build_dmabuf(
        &self,
        width: i32,
        height: i32,
        format: u32,
    ) -> Result<DmaBuf, zwp_linux_buffer_params_v1::Error> {
        if width <= 0 || height <= 0 {
            return Err(zwp_linux_buffer_params_v1::Error::InvalidDimensions);
        }
        let drm_format = drm::buffer::DrmFourcc::try_from(format)
            .map_err(|_| zwp_linux_buffer_params_v1::Error::InvalidFormat)?;
        let format = *FORMATS_MAP
            .get(&drm_format)
            .ok_or(zwp_linux_buffer_params_v1::Error::InvalidFormat)?;
        if self.planes.is_empty() {
            return Err(zwp_linux_buffer_params_v1::Error::Incomplete);
        }
        let max_plane = *self.planes.keys().last().unwrap();
        if self.planes.len() != (max_plane as usize + 1) {
            return Err(zwp_linux_buffer_params_v1::Error::Incomplete);
        }

        let mut modifier = None;
        let planes = self
            .planes
            .iter()
            .map(|(index, plane)| {
                if *index > 3 {
                    return Err(zwp_linux_buffer_params_v1::Error::PlaneIdx);
                }
                if let Some(existing) = modifier {
                    if existing != plane.modifier {
                        return Err(zwp_linux_buffer_params_v1::Error::InvalidFormat);
                    }
                } else {
                    modifier = Some(plane.modifier);
                }
                let fd = plane.fd.as_raw_fd();
                let fd = unsafe { OwnedFd::from_raw_fd(libc::dup(fd)) };
                Ok(DmaBufPlane { offset: plane.offset, stride: plane.stride, fd })
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(DmaBuf {
            id: DmaBufId::next(),
            width,
            height,
            format,
            modifier: modifier.unwrap_or(Modifier::INVALID),
            planes: planes.into_boxed_slice(),
        })
    }
}

impl Object for ZwpLinuxBufferParamsV1 {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        mut self: GrabbedObject<Self>,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        use zwp_linux_buffer_params_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            Add { fd, plane_idx, offset, stride, modifier_hi, modifier_lo } => {
                if self.already_used(dispatcher).is_err() {
                    return Ok(());
                }
                if plane_idx > 3 {
                    self.send_error(
                        dispatcher,
                        zwp_linux_buffer_params_v1::Error::PlaneIdx as u32,
                        "plane index out of bounds",
                    );
                    return Ok(());
                }
                if self.planes.contains_key(&plane_idx) {
                    self.send_error(
                        dispatcher,
                        zwp_linux_buffer_params_v1::Error::PlaneSet as u32,
                        "plane index already set",
                    );
                    return Ok(());
                }
                let modifier = Modifier(((modifier_hi as u64) << 32) | modifier_lo as u64);
                self.planes.insert(
                    plane_idx,
                    PendingPlane {
                        fd: unsafe { OwnedFd::from_raw_fd(fd) },
                        offset,
                        stride,
                        modifier,
                    },
                );
            }
            Create { width, height, format, flags } => {
                if self.already_used(dispatcher).is_err() {
                    return Ok(());
                }
                self.used = true;
                let y_inverted = (flags & zwp_linux_buffer_params_v1::Flags::YInvert as u32) != 0;
                match self.build_dmabuf(width, height, format) {
                    Ok(dmabuf) => match WlBuffer::new_dmabuf(NewDmabufWlBufferParams {
                        id: client.allocate_server_object_id(),
                        owner_client_id: client.client_id(),
                        dmabuf,
                        y_inverted,
                    }) {
                        Ok(buffer) => {
                            let (buffer, _) = client.insert_object(state, buffer);
                            let buffer_wayland_id: WaylandId = buffer.wayland_id().into();
                            self.send_event(
                                zwp_linux_buffer_params_v1::Event::Created {
                                    buffer: buffer_wayland_id.into(),
                                },
                                dispatcher,
                            );
                            log::debug!("created client dma-buf wl_buffer {:?}", buffer_wayland_id);
                        }
                        Err(_) => {
                            self.send_event(zwp_linux_buffer_params_v1::Event::Failed, dispatcher)
                        }
                    },
                    Err(error) => {
                        self.send_error(dispatcher, error as u32, "invalid dma-buf buffer params");
                    }
                }
            }
            CreateImmed { buffer_id, width, height, format, flags } => {
                if self.already_used(dispatcher).is_err() {
                    return Ok(());
                }
                self.used = true;
                let y_inverted = (flags & zwp_linux_buffer_params_v1::Flags::YInvert as u32) != 0;
                match self.build_dmabuf(width, height, format) {
                    Ok(dmabuf) => match WlBuffer::new_dmabuf(NewDmabufWlBufferParams {
                        id: buffer_id,
                        owner_client_id: client.client_id(),
                        dmabuf,
                        y_inverted,
                    }) {
                        Ok(buffer) => {
                            client.insert_object(state, buffer);
                            log::debug!("created immediate dma-buf wl_buffer {:?}", buffer_id);
                        }
                        Err(_) => self.send_error(
                            dispatcher,
                            zwp_linux_buffer_params_v1::Error::InvalidWlBuffer as u32,
                            "failed to create wl_buffer from dma-buf params",
                        ),
                    },
                    Err(error) => {
                        self.send_error(dispatcher, error as u32, "invalid dma-buf params")
                    }
                }
            }
            SetSamplingDevice { .. } => {
                self.send_error(
                    dispatcher,
                    wl_display::Error::InvalidMethod as u32,
                    "linux-dmabuf sampling device is not advertised by this compositor version",
                );
            }
        }
        Ok(())
    }
}
