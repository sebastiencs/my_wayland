use std::rc::Rc;

use crate::{region::DamageRegion, state::RootSurfaceClip, transform::Mat3};

use super::Texture;

use super::visual::{BlurVisualStyle, BorderGradient, RoundedRing, ShadowVisualStyle};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureSource {
    Texture,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TextureMask {
    None,
    RoundedClip { clip: RootSurfaceClip },
    FrameRing { ring: RoundedRing },
}

#[derive(Clone, Debug)]
pub struct TextureElement {
    pub source: TextureSource,
    pub texture: Rc<dyn Texture>,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub tex_transform: Mat3,
    pub damage: DamageRegion,
    pub blend: bool,
    pub alpha: u8,
    pub mask: TextureMask,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FramePhase {
    UnderWindow,
    Content,
    OverWindow,
    Overlay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectElement {
    WindowBorder { ring: RoundedRing, gradient: BorderGradient, alpha: u8 },
    WindowShadow { clip: RootSurfaceClip, style: ShadowVisualStyle, alpha: u8 },
    WindowBlurBackdrop { ring: RoundedRing, style: BlurVisualStyle, alpha: u8 },
    DimOverlay { clip: RootSurfaceClip, alpha: u8 },
}

#[derive(Clone, Debug)]
pub enum FrameElement {
    Texture { phase: FramePhase, element: TextureElement },
    Effect { phase: FramePhase, element: EffectElement },
}

#[derive(Clone, Debug, Default)]
pub struct OutputFrameRecipe {
    pub elements: Vec<FrameElement>,
}

impl OutputFrameRecipe {
    pub fn has_effects(&self) -> bool {
        self.elements
            .iter()
            .any(|element| matches!(element, FrameElement::Effect { .. }))
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }
}
