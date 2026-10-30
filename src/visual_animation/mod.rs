//! Compositor-owned visual animation state.
//!
//! The scheduler remains the only timing authority. This module samples absolute time and describes
//! draw modifiers; it does not arm timers, choose refresh cadence, or mutate workspace layout.
//! `FocusVisualAnimation` owns the focus fade. `WindowAnimationManager::{advance_and_schedule,
//! advance_and_schedule_output}` is the sole phase that mutates that progress. Render resolution
//! reads the cached sample and calls the pure helpers here.
//!
//! Visibility stays in the scene participation policy. Input, protocol, and render eligibility are
//! not derived from these visual modifiers. Workspace transitions and close records defined here
//! are dormant until later phases attach them to workspace groups and surface teardown.

mod closing;
mod track;
mod window;
mod workspace;

pub use {
    closing::{ClosingFrame, DetachedClosingWindow, RetainedClosingSnapshot},
    track::{
        apply_easing, unit_progress_between, Easing, RetargetableTrack, TrackInterrupt, TrackMode,
        TrackSample, TrackStatus, UnitValue,
    },
    window::{
        resolve_window_visual, FocusVisualAnimation, ResolvedWindowVisual, WindowLifecycleAnim,
        WindowVisualBase,
    },
    workspace::{
        WorkspaceGroupTransition, WorkspaceTransitionDirection, WorkspaceTransitionSample,
    },
};
