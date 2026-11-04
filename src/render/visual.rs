use crate::{
    config::{RgbaColor, RoundingPower, ShadowPower},
    region::RectI,
    state::RootSurfaceClip,
};

/// Rounded-rect clip applied to a window's root-surface tree.
///
/// The kind decides how pixels outside the rounded box are treated:
/// - [`WindowClip::Frame`] hard-clips to the compositor-owned (SSD) frame.
/// - [`WindowClip::Corners`] only discards pixels inside the window box that
///   fall outside a rounded corner, so pixels outside the box — for example
///   client-drawn CSD shadows — are kept. This matches how Hyprland rounds
///   corners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowClip {
    Frame(RootSurfaceClip),
    Corners(RootSurfaceClip),
}

impl WindowClip {
    pub fn clip(self) -> RootSurfaceClip {
        match self {
            Self::Frame(clip) | Self::Corners(clip) => clip,
        }
    }

    /// Move the clip to `outer_rect`, keeping the kind and the configured
    /// radius/power. Used when a geometry animation resizes the visual box.
    pub fn reposition(
        self,
        outer_rect: RectI,
        corner_radius: i32,
        rounding_power: RoundingPower,
    ) -> Self {
        let clip = RootSurfaceClip::new(outer_rect, corner_radius, rounding_power);
        match self {
            Self::Frame(_) => Self::Frame(clip),
            Self::Corners(_) => Self::Corners(clip),
        }
    }
}

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
    /// Client-surface clip. The radius is `decorations.rounding` on the content rect.
    pub clip: Option<WindowClip>,
    /// Outer frame, with Hyprland's outer corner radius. Shadow and dim use it.
    pub frame_clip: RootSurfaceClip,
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

/// Outer corner radius from `CHyprBorderDecoration`.
///
/// `rounding + border_size - border_size * (sqrt(2) - 1) * max(2 - power, 0)`.
/// With the default power of 2 the correction is zero.
pub fn border_outer_radius(rounding: i32, border_size: i32, rounding_power: RoundingPower) -> i32 {
    let rounding = rounding.max(0);
    let border_size = border_size.max(0);
    let correction = f64::from(border_size)
        * (std::f64::consts::SQRT_2 - 1.0)
        * (2.0 - f64::from(rounding_power.get())).max(0.0);
    round_to_i32(f64::from(rounding.saturating_add(border_size)) - correction).max(0)
}

/// Shadow corner radius. Hyprland uses the outer radius only when `rounding` is positive.
pub fn shadow_corner_radius(rounding: i32, border_size: i32, rounding_power: RoundingPower) -> i32 {
    if rounding <= 0 {
        0
    } else {
        border_outer_radius(rounding, border_size, rounding_power)
    }
}

/// Border ring. The inner radius is `rounding`; the outer radius is [`border_outer_radius`].
pub fn rounded_ring(
    outer_rect: RectI,
    inner_rect: RectI,
    rounding: i32,
    border_size: i32,
    rounding_power: RoundingPower,
) -> RoundedRing {
    RoundedRing {
        outer: RootSurfaceClip::new(
            outer_rect,
            border_outer_radius(rounding, border_size, rounding_power),
            rounding_power,
        ),
        inner: RootSurfaceClip::new(inner_rect, rounding.max(0), rounding_power),
    }
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

/// Map `rect` from the logical layout rectangle into the sampled visual rectangle.
///
/// The layout rectangle itself maps exactly. Other rectangles, including popups that stick out,
/// scale around its origin.
pub fn map_layout_rect(logical: RectI, visual: RectI, rect: RectI) -> RectI {
    if logical.width <= 0 || logical.height <= 0 || rect == logical {
        return if rect == logical { visual } else { rect };
    }
    let scale_x = f64::from(visual.width) / f64::from(logical.width);
    let scale_y = f64::from(visual.height) / f64::from(logical.height);
    let x = f64::from(visual.x) + (f64::from(rect.x) - f64::from(logical.x)) * scale_x;
    let y = f64::from(visual.y) + (f64::from(rect.y) - f64::from(logical.y)) * scale_y;
    RectI::new(
        round_to_i32(x),
        round_to_i32(y),
        round_to_i32(f64::from(rect.width) * scale_x).max(0),
        round_to_i32(f64::from(rect.height) * scale_y).max(0),
    )
    .unwrap_or(rect)
}

/// Map a global point from the logical layout rectangle into the sampled visual rectangle.
pub fn map_layout_point(logical: RectI, visual: RectI, point: (f64, f64)) -> (f64, f64) {
    let Some((scale_x, scale_y)) = layout_scale(logical, visual) else {
        return point;
    };
    (
        f64::from(visual.x) + (point.0 - f64::from(logical.x)) * scale_x,
        f64::from(visual.y) + (point.1 - f64::from(logical.y)) * scale_y,
    )
}

/// Map a global point from the sampled visual rectangle back into the logical layout rectangle.
pub fn unmap_layout_point(logical: RectI, visual: RectI, point: (f64, f64)) -> (f64, f64) {
    let Some((scale_x, scale_y)) = layout_scale(logical, visual) else {
        return point;
    };
    if scale_x == 0.0 || scale_y == 0.0 {
        return point;
    }
    (
        f64::from(logical.x) + (point.0 - f64::from(visual.x)) / scale_x,
        f64::from(logical.y) + (point.1 - f64::from(visual.y)) / scale_y,
    )
}

fn layout_scale(logical: RectI, visual: RectI) -> Option<(f64, f64)> {
    if logical.width <= 0 || logical.height <= 0 || logical == visual {
        return None;
    }
    Some((
        f64::from(visual.width) / f64::from(logical.width),
        f64::from(visual.height) / f64::from(logical.height),
    ))
}

/// Move a window's decorations onto `visual_layout` without scaling their pixel thickness.
///
/// The client surfaces use [`map_layout_rect`]. The border inset, corner radius, and shadow spread
/// stay in pixels, matching the way Hyprland redraws chrome around `m_realSize`.
pub fn place_visual_frame(
    visual: &mut RootSurfaceVisualState,
    logical_layout: RectI,
    visual_layout: RectI,
) {
    if logical_layout.width <= 0 || logical_layout.height <= 0 || logical_layout == visual_layout {
        return;
    }
    let outer = visual.outer_rect;
    let content = visual.content_rect;
    let visual_outer = map_layout_rect(logical_layout, visual_layout, outer);
    let left = content.x.saturating_sub(outer.x);
    let top = content.y.saturating_sub(outer.y);
    let right = outer.x2().saturating_sub(content.x2());
    let bottom = outer.y2().saturating_sub(content.y2());
    let content_inside = content.x >= outer.x
        && content.y >= outer.y
        && content.x2() <= outer.x2()
        && content.y2() <= outer.y2();
    visual.outer_rect = visual_outer;
    visual.content_rect = if content_inside {
        inset_rect(visual_outer, left, top, right, bottom).unwrap_or(visual_outer)
    } else {
        map_layout_rect(logical_layout, visual_layout, content)
    };
    if let Some(clip) = visual.clip {
        let radius = clip.clip().corner_radius;
        let power = clip.clip().rounding_power;
        visual.clip = Some(clip.reposition(visual.content_rect, radius, power));
    }
    let frame_radius = visual.frame_clip.corner_radius;
    let frame_power = visual.frame_clip.rounding_power;
    visual.frame_clip = RootSurfaceClip::new(visual.outer_rect, frame_radius, frame_power);
}

fn inset_rect(outer: RectI, left: i32, top: i32, right: i32, bottom: i32) -> Option<RectI> {
    let width = outer.width.saturating_sub(left.saturating_add(right));
    let height = outer.height.saturating_sub(top.saturating_add(bottom));
    if width <= 0 || height <= 0 {
        return None;
    }
    RectI::new(outer.x.saturating_add(left), outer.y.saturating_add(top), width, height)
}

fn round_to_i32(value: f64) -> i32 {
    let rounded = value.round();
    if rounded >= f64::from(i32::MAX) {
        i32::MAX
    } else if rounded <= f64::from(i32::MIN) {
        i32::MIN
    } else {
        rounded as i32
    }
}

impl RootSurfaceVisualState {
    /// Clip for the drop shadow. A square shadow when `rounding` is zero, otherwise the outer frame.
    pub fn shadow_clip(self) -> RootSurfaceClip {
        if self.clip.is_some() {
            self.frame_clip
        } else {
            RootSurfaceClip::new(self.frame_clip.outer_rect, 0, self.frame_clip.rounding_power)
        }
    }

    pub fn has_visible_decoration_ring(self) -> bool {
        self.outer_rect != self.content_rect
    }

    pub fn influence_bounds(self) -> RectI {
        let mut x1 = self.outer_rect.x;
        let mut y1 = self.outer_rect.y;
        let mut x2 = self.outer_rect.x2();
        let mut y2 = self.outer_rect.y2();
        let shadow_clip = self.shadow_clip();
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
            border_outer_radius, rounded_ring, shadow_geometry, BlurVisualStyle,
            RootSurfaceVisualState, RoundedRing, ShadowVisualStyle, SurfaceEffectStyle, WindowClip,
        },
        crate::{
            config::{RgbaColor, RoundingPower, ShadowPower},
            region::RectI,
            state::RootSurfaceClip,
        },
    };

    #[test]
    fn layout_point_mapping_round_trips_through_a_scaled_visual_rect() {
        let logical = RectI::new(10, 20, 100, 80).unwrap();
        let visual = RectI::new(40, 30, 50, 40).unwrap();
        let point = (60.0, 60.0);
        let mapped = super::map_layout_point(logical, visual, point);
        let restored = super::unmap_layout_point(logical, visual, mapped);
        assert!((restored.0 - point.0).abs() < 1e-9);
        assert!((restored.1 - point.1).abs() < 1e-9);
        assert!((mapped.0 - 65.0).abs() < 1e-9);
        assert!((mapped.1 - 50.0).abs() < 1e-9);
    }

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
    fn rounded_ring_uses_hyprland_inner_and_outer_radii() {
        let power = RoundingPower::DEFAULT;
        let outer = RectI::new(40, 50, 120, 80).unwrap();
        let inner = RectI::new(44, 54, 112, 72).unwrap();

        assert_eq!(
            rounded_ring(outer, inner, 10, 4, power),
            RoundedRing {
                outer: RootSurfaceClip::new(outer, 14, power),
                inner: RootSurfaceClip::new(inner, 10, power),
            }
        );
        assert_eq!(border_outer_radius(0, 4, power), 4);
        let tighter = RoundingPower::sanitize(1.0);
        let correction = 4.0 * (std::f64::consts::SQRT_2 - 1.0);
        assert_eq!(border_outer_radius(10, 4, tighter), (14.0 - correction).round() as i32);
    }

    #[test]
    fn influence_bounds_include_shadow_extent_without_clip() {
        let visual = RootSurfaceVisualState {
            outer_rect: RectI::new(100, 200, 300, 150).unwrap(),
            content_rect: RectI::new(100, 200, 300, 150).unwrap(),
            clip: None,
            frame_clip: RootSurfaceClip::new(
                RectI::new(100, 200, 300, 150).unwrap(),
                0,
                RoundingPower::DEFAULT,
            ),
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
            frame_clip: RootSurfaceClip::new(
                RectI::new(100, 200, 300, 150).unwrap(),
                0,
                RoundingPower::DEFAULT,
            ),
            border: None,
            border_alpha: u8::MAX,
            shadow: None,
            shadow_alpha: u8::MAX,
            effects: SurfaceEffectStyle { blur: None, opacity_alpha: u8::MAX, dim_alpha: 0 },
        };

        assert!(visual.has_visible_decoration_ring());
    }

    #[test]
    fn layout_mapping_scales_attached_popups_and_keeps_the_frame_exact() {
        let logical = RectI::new(10, 20, 100, 80).unwrap();
        let visual = RectI::new(30, 40, 50, 40).unwrap();
        assert_eq!(super::map_layout_rect(logical, visual, logical), visual);
        let popup = RectI::new(110, 20, 20, 16).unwrap();
        assert_eq!(
            super::map_layout_rect(logical, visual, popup),
            RectI::new(80, 40, 10, 8).unwrap()
        );
    }

    #[test]
    fn placed_frame_keeps_border_thickness_and_corner_radius() {
        let mut visual = RootSurfaceVisualState {
            outer_rect: RectI::new(0, 0, 100, 80).unwrap(),
            content_rect: RectI::new(4, 6, 92, 70).unwrap(),
            clip: Some(WindowClip::Frame(RootSurfaceClip::new(
                RectI::new(4, 6, 92, 70).unwrap(),
                12,
                RoundingPower::DEFAULT,
            ))),
            frame_clip: RootSurfaceClip::new(
                RectI::new(0, 0, 100, 80).unwrap(),
                16,
                RoundingPower::DEFAULT,
            ),
            border: None,
            border_alpha: u8::MAX,
            shadow: None,
            shadow_alpha: u8::MAX,
            effects: SurfaceEffectStyle { blur: None, opacity_alpha: u8::MAX, dim_alpha: 0 },
        };
        let logical = visual.outer_rect;
        super::place_visual_frame(&mut visual, logical, RectI::new(10, 20, 40, 30).unwrap());
        assert_eq!(visual.outer_rect, RectI::new(10, 20, 40, 30).unwrap());
        assert_eq!(visual.content_rect, RectI::new(14, 26, 32, 20).unwrap());
        let clip = visual.clip.unwrap();
        assert_eq!(
            clip,
            WindowClip::Frame(RootSurfaceClip::new(
                RectI::new(14, 26, 32, 20).unwrap(),
                12,
                RoundingPower::DEFAULT,
            ))
        );
        assert_eq!(
            visual.frame_clip,
            RootSurfaceClip::new(RectI::new(10, 20, 40, 30).unwrap(), 16, RoundingPower::DEFAULT,)
        );
    }

    #[test]
    fn placed_frame_keeps_corner_clip_kind_for_csd_windows() {
        let mut visual = RootSurfaceVisualState {
            outer_rect: RectI::new(0, 0, 100, 80).unwrap(),
            content_rect: RectI::new(0, 0, 100, 80).unwrap(),
            clip: Some(WindowClip::Corners(RootSurfaceClip::new(
                RectI::new(0, 0, 100, 80).unwrap(),
                12,
                RoundingPower::DEFAULT,
            ))),
            frame_clip: RootSurfaceClip::new(
                RectI::new(0, 0, 100, 80).unwrap(),
                12,
                RoundingPower::DEFAULT,
            ),
            border: None,
            border_alpha: u8::MAX,
            shadow: None,
            shadow_alpha: u8::MAX,
            effects: SurfaceEffectStyle { blur: None, opacity_alpha: u8::MAX, dim_alpha: 0 },
        };
        let logical = visual.outer_rect;
        super::place_visual_frame(&mut visual, logical, RectI::new(10, 20, 40, 30).unwrap());
        let clip = visual.clip.unwrap();
        assert_eq!(
            clip,
            WindowClip::Corners(RootSurfaceClip::new(
                RectI::new(10, 20, 40, 30).unwrap(),
                12,
                RoundingPower::DEFAULT,
            ))
        );
        assert_eq!(
            visual.frame_clip,
            RootSurfaceClip::new(RectI::new(10, 20, 40, 30).unwrap(), 12, RoundingPower::DEFAULT,)
        );
    }
}
