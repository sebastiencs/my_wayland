//! Compositor-owned visual animation state.
//!
//! The scheduler remains the only timing authority. This module samples absolute time and describes
//! draw modifiers; it does not arm timers, choose refresh cadence, or mutate workspace layout.
//! `FocusVisualAnimation` owns the focus fade and `WindowLifecycleAnimation` owns opening.
//! `VisualAnimationPump::{advance_and_schedule, advance_and_schedule_output}` is the sole phase
//! that mutates that progress. Render resolution reads the cached samples and calls the pure
//! helpers here.
//!
//! Visibility stays in the scene participation policy. Input, protocol, and render eligibility are
//! not derived from these visual modifiers. A workspace group owns one live slide-and-fade, and
//! the same animation pump samples it. Closing toplevels keep a retained snapshot.

mod closing;
mod track;
mod window;
mod workspace;

pub use {
    closing::{FrozenClosingBlur, RetainedClosingSnapshot},
    track::{CubicBezier, Easing, UnitValue},
    window::{
        lifecycle_pose, resolve_window_visual, FocusVisualAnimation, GeometryTiming,
        OpeningIntroduction, WindowGeometryAnimation, WindowGeometryMotion, WindowLifecycleAnim,
        WindowLifecycleAnimation, WindowVisualBase,
    },
    workspace::{
        WorkspaceGroupTransition, WorkspaceMotion, WorkspaceTransitionAdvance,
        WorkspaceTransitionDirection,
    },
};
