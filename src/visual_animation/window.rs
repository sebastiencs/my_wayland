use super::track::UnitValue;

/// Appearance sampled from logical window state before lifecycle scaling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowVisualBase {
    pub content_alpha: u8,
    pub dim_alpha: u8,
    pub shadow_alpha: u8,
    pub border_alpha: u8,
}

/// Lifecycle contribution composed onto [`WindowVisualBase`] at sample time.
///
/// Phase 3 resolves production windows as [`WindowLifecycleAnim::Idle`]. Opening and closing exist
/// so later phases can scale the same channels without a second visual model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowLifecycleAnim {
    Idle,
    Opening { progress: UnitValue },
    Closing { progress: UnitValue },
}

/// Draw modifiers consumed by render assembly.
///
/// `translation` is reserved for later workspace and window motion. Phase 3 resolution keeps it at
/// the origin and does not move layout or scene geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedWindowVisual {
    pub content_alpha: u8,
    pub dim_alpha: u8,
    pub shadow_alpha: u8,
    pub border_alpha: u8,
    pub translation: (i32, i32),
}

pub fn resolve_window_visual(
    base: WindowVisualBase,
    lifecycle: WindowLifecycleAnim,
) -> ResolvedWindowVisual {
    let factor = match lifecycle {
        WindowLifecycleAnim::Idle => UnitValue::ONE,
        WindowLifecycleAnim::Opening { progress } => progress,
        WindowLifecycleAnim::Closing { progress } => UnitValue::ONE.saturating_sub(progress),
    };
    ResolvedWindowVisual {
        content_alpha: scale_alpha(base.content_alpha, factor),
        dim_alpha: scale_alpha(base.dim_alpha, factor),
        shadow_alpha: scale_alpha(base.shadow_alpha, factor),
        border_alpha: scale_alpha(base.border_alpha, factor),
        translation: (0, 0),
    }
}

fn scale_alpha(alpha: u8, factor: UnitValue) -> u8 {
    factor.lerp_u8(0, alpha)
}

#[cfg(test)]
mod tests {
    use {
        super::{resolve_window_visual, WindowLifecycleAnim, WindowVisualBase},
        crate::visual_animation::UnitValue,
    };

    fn base() -> WindowVisualBase {
        WindowVisualBase {
            content_alpha: 200,
            dim_alpha: 40,
            shadow_alpha: 180,
            border_alpha: 160,
        }
    }

    #[test]
    fn idle_resolution_preserves_alpha_and_origin() {
        let resolved = resolve_window_visual(base(), WindowLifecycleAnim::Idle);

        assert_eq!(resolved.content_alpha, 200);
        assert_eq!(resolved.dim_alpha, 40);
        assert_eq!(resolved.shadow_alpha, 180);
        assert_eq!(resolved.border_alpha, 160);
        assert_eq!(resolved.translation, (0, 0));
    }

    #[test]
    fn opening_and_closing_scale_every_alpha_channel() {
        let hidden = resolve_window_visual(
            base(),
            WindowLifecycleAnim::Opening { progress: UnitValue::ZERO },
        );
        assert_eq!(hidden.content_alpha, 0);
        assert_eq!(hidden.dim_alpha, 0);
        assert_eq!(hidden.shadow_alpha, 0);
        assert_eq!(hidden.border_alpha, 0);

        let shown = resolve_window_visual(
            base(),
            WindowLifecycleAnim::Opening { progress: UnitValue::ONE },
        );
        assert_eq!(shown.content_alpha, base().content_alpha);
        assert_eq!(shown.dim_alpha, base().dim_alpha);

        let closing_start = resolve_window_visual(
            base(),
            WindowLifecycleAnim::Closing { progress: UnitValue::ZERO },
        );
        assert_eq!(closing_start.content_alpha, base().content_alpha);
        let closing_end = resolve_window_visual(
            base(),
            WindowLifecycleAnim::Closing { progress: UnitValue::ONE },
        );
        assert_eq!(closing_end.content_alpha, 0);
        assert_eq!(closing_end.shadow_alpha, 0);
        assert_eq!(closing_end.border_alpha, 0);
        assert_eq!(closing_end.translation, (0, 0));
    }
}
