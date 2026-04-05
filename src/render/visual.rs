use crate::{
    config::{RgbaColor, ShadowPower},
    region::RectI,
    state::RootSurfaceClip,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BorderGradient {
    pub start_color: RgbaColor,
    pub end_color: RgbaColor,
    pub angle_degrees: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BorderVisualStyle {
    pub gradient: BorderGradient,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShadowVisualStyle {
    pub radius: i32,
    pub offset_x: i32,
    pub offset_y: i32,
    pub color: RgbaColor,
    pub power: ShadowPower,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlurVisualStyle {
    pub radius: i32,
    pub passes: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoundedRing {
    pub outer: RootSurfaceClip,
    pub inner: RootSurfaceClip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceEffectStyle {
    pub blur: Option<BlurVisualStyle>,
    pub opacity_alpha: u8,
    pub dim_alpha: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootSurfaceVisualState {
    pub outer_rect: RectI,
    pub content_rect: RectI,
    pub clip: Option<RootSurfaceClip>,
    pub border: Option<BorderVisualStyle>,
    pub border_alpha: u8,
    pub shadow: Option<ShadowVisualStyle>,
    pub shadow_alpha: u8,
    pub effects: SurfaceEffectStyle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShadowGeometry {
    pub bounds: RectI,
    pub inner_rect: RectI,
    pub shadow_range: i32,
}

pub fn rounded_ring(outer: RootSurfaceClip, inner_rect: RectI) -> RoundedRing {
    let left_inset = inner_rect.x.saturating_sub(outer.outer_rect.x);
    let top_inset = inner_rect.y.saturating_sub(outer.outer_rect.y);
    let right_inset = outer.outer_rect.x2().saturating_sub(inner_rect.x2());
    let bottom_inset = outer.outer_rect.y2().saturating_sub(inner_rect.y2());
    let inset = left_inset
        .min(top_inset)
        .min(right_inset)
        .min(bottom_inset)
        .max(0);
    let inner = RootSurfaceClip::new(
        inner_rect,
        outer.corner_radius.saturating_sub(inset),
        outer.rounding_power,
    );
    RoundedRing { outer, inner }
}

pub fn shadow_geometry(clip: RootSurfaceClip, style: ShadowVisualStyle) -> Option<ShadowGeometry> {
    let shadow_range = style.radius.max(1);
    let bounds = RectI::new(
        clip.outer_rect
            .x
            .saturating_add(style.offset_x.saturating_sub(shadow_range)),
        clip.outer_rect
            .y
            .saturating_add(style.offset_y.saturating_sub(shadow_range)),
        clip.outer_rect
            .width
            .saturating_add(shadow_range.saturating_mul(2)),
        clip.outer_rect
            .height
            .saturating_add(shadow_range.saturating_mul(2)),
    )?;
    let inner_rect = RectI::new(
        clip.outer_rect.x.saturating_sub(bounds.x),
        clip.outer_rect.y.saturating_sub(bounds.y),
        clip.outer_rect.width,
        clip.outer_rect.height,
    )?;
    Some(ShadowGeometry { bounds, inner_rect, shadow_range })
}

impl RootSurfaceVisualState {
    pub fn has_visible_decoration_ring(self) -> bool {
        self.outer_rect != self.content_rect
    }

    pub fn influence_bounds(self) -> RectI {
        let mut x1 = self.outer_rect.x;
        let mut y1 = self.outer_rect.y;
        let mut x2 = self.outer_rect.x2();
        let mut y2 = self.outer_rect.y2();
        let shadow_clip = self.clip.unwrap_or_else(|| {
            RootSurfaceClip::new(self.outer_rect, 0, crate::config::RoundingPower::DEFAULT)
        });
        if let Some(shadow) = self.shadow {
            if let Some(shadow_geometry) = shadow_geometry(shadow_clip, shadow) {
                x1 = x1.min(shadow_geometry.bounds.x);
                y1 = y1.min(shadow_geometry.bounds.y);
                x2 = x2.max(shadow_geometry.bounds.x2());
                y2 = y2.max(shadow_geometry.bounds.y2());
            }
        }
        RectI::from_extents(x1, y1, x2, y2).unwrap_or(self.outer_rect)
    }
}

#[cfg(test)]
mod tests {
    use {
        super::{
            rounded_ring, shadow_geometry, BlurVisualStyle, RootSurfaceVisualState, RoundedRing,
            ShadowVisualStyle, SurfaceEffectStyle,
        },
        crate::{
            config::{RgbaColor, RoundingPower, ShadowPower},
            region::RectI,
            state::RootSurfaceClip,
        },
    };

    #[test]
    fn shadow_geometry_offsets_inner_rect_within_shifted_shadow_bounds() {
        let clip = RootSurfaceClip::new(
            RectI::new(100, 200, 300, 150).unwrap(),
            16,
            RoundingPower::DEFAULT,
        );
        let style = ShadowVisualStyle {
            radius: 12,
            offset_x: 4,
            offset_y: 8,
            color: RgbaColor::new(0, 0, 0, 128),
            power: ShadowPower::DEFAULT,
        };

        let geometry = shadow_geometry(clip, style).unwrap();

        assert_eq!(geometry.bounds, RectI::new(92, 196, 324, 174).unwrap());
        assert_eq!(geometry.inner_rect, RectI::new(8, 4, 300, 150).unwrap());
        assert_eq!(geometry.shadow_range, 12);
    }

    #[test]
    fn rounded_ring_derives_inner_clip_geometry_once() {
        let outer =
            RootSurfaceClip::new(RectI::new(40, 50, 120, 80).unwrap(), 18, RoundingPower::DEFAULT);

        assert_eq!(
            rounded_ring(outer, RectI::new(46, 56, 108, 68).unwrap()),
            RoundedRing {
                outer,
                inner: RootSurfaceClip::new(
                    RectI::new(46, 56, 108, 68).unwrap(),
                    12,
                    RoundingPower::DEFAULT,
                ),
            }
        );
    }

    #[test]
    fn influence_bounds_include_shadow_extent_without_clip() {
        let visual = RootSurfaceVisualState {
            outer_rect: RectI::new(100, 200, 300, 150).unwrap(),
            content_rect: RectI::new(100, 200, 300, 150).unwrap(),
            clip: None,
            border: None,
            border_alpha: u8::MAX,
            shadow: Some(ShadowVisualStyle {
                radius: 10,
                offset_x: -3,
                offset_y: 6,
                color: RgbaColor::new(0, 0, 0, 128),
                power: ShadowPower::DEFAULT,
            }),
            shadow_alpha: u8::MAX,
            effects: SurfaceEffectStyle {
                blur: Some(BlurVisualStyle { radius: 8, passes: 2 }),
                opacity_alpha: u8::MAX,
                dim_alpha: 0,
            },
        };

        assert_eq!(visual.influence_bounds(), RectI::new(87, 196, 320, 170).unwrap());
    }

    #[test]
    fn visible_decoration_ring_detects_asymmetric_frame_growth() {
        let visual = RootSurfaceVisualState {
            outer_rect: RectI::new(100, 172, 300, 178).unwrap(),
            content_rect: RectI::new(100, 200, 300, 150).unwrap(),
            clip: None,
            border: None,
            border_alpha: u8::MAX,
            shadow: None,
            shadow_alpha: u8::MAX,
            effects: SurfaceEffectStyle { blur: None, opacity_alpha: u8::MAX, dim_alpha: 0 },
        };

        assert!(visual.has_visible_decoration_ring());
    }
}
