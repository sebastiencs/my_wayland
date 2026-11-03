pub mod backend;
pub mod gles;
pub mod recipe;
pub mod visual;

#[cfg(test)]
mod mock;

pub use {
    backend::{
        inert_texture, FormatSupport, GraphicsApi, InertTexture, OffscreenTarget, RenderCompletion,
        RenderDeviceId, RenderFrame, RenderSyncMode, RenderTarget, Renderer, RendererCapabilities,
        RendererError, ShmImage, Texture,
    },
    recipe::{
        EffectElement, FrameElement, FramePhase, OutputFrameRecipe, TextureElement, TextureMask,
        TextureSource,
    },
    visual::{
        map_layout_point, map_layout_rect, place_visual_frame, rounded_ring, shadow_geometry,
        unmap_layout_point, BlurVisualStyle, BorderGradient, BorderVisualStyle,
        RootSurfaceVisualState, RoundedRing, ShadowVisualStyle, SurfaceEffectStyle,
    },
};
