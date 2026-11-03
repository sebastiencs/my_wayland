use crate::render::RendererError;

pub(super) fn readback() -> Result<(), RendererError> {
    Err(RendererError::NotImplemented("vulkan readback"))
}
