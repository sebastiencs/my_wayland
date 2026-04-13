//! Per-output scheduling split between a pure `FrameClock`, a thin `OutputScheduler`
//! runtime shell, and presentation bookkeeping. The shape follows the per-output
//! clock ownership used by compositors like Mutter while keeping Weston's explicit
//! completion semantics concentrated in the completion/presentation path.

pub mod completion;
pub mod frame_clock;
pub mod output_scheduler;
pub mod presentation;

pub use {
    completion::{complete_frame_for_timeline, discard_frame_for_timeline, FrameCompletionOutcome},
    frame_clock::{PresentFlags, PresentSample, RequestFrameReason},
    output_scheduler::{output_scheduler_id, OutputScheduler, OutputSchedulerAction},
    presentation::PresentationSubsystem,
};
