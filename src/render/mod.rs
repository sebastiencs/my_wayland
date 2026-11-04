pub mod backend;
mod factory;
pub mod gles;
pub mod recipe;
pub mod visual;
mod vulkan;

/// Color written into output pixels that no surface covers. `#282828`.
pub const OUTPUT_BACKGROUND: [f32; 4] = [40.0 / 255.0, 40.0 / 255.0, 40.0 / 255.0, 1.0];

#[cfg(test)]
mod mock;

pub use factory::create_renderer;

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
        RootSurfaceVisualState, RoundedRing, ShadowVisualStyle, SurfaceEffectStyle, WindowClip,
    },
};
