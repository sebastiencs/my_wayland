#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowAnimationValues {
    pub content_alpha: u8,
    pub dim_alpha: u8,
    pub shadow_alpha: u8,
    pub border_alpha: u8,
}

impl Default for WindowAnimationValues {
    fn default() -> Self {
        Self {
            content_alpha: u8::MAX,
            dim_alpha: 0,
            shadow_alpha: u8::MAX,
            border_alpha: u8::MAX,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowAnimationState {
    current: WindowAnimationValues,
    target: WindowAnimationValues,
    last_frame_msec: Option<u32>,
}

impl Default for WindowAnimationState {
    fn default() -> Self {
        let values = WindowAnimationValues::default();
        Self { current: values, target: values, last_frame_msec: None }
    }
}

impl WindowAnimationState {
    pub fn from_values(values: WindowAnimationValues) -> Self {
        Self { current: values, target: values, last_frame_msec: None }
    }

    pub fn current_values(self) -> WindowAnimationValues {
        self.current
    }

    pub fn advance_to(
        &mut self,
        target: WindowAnimationValues,
        duration_msec: u32,
        now_msec: u32,
    ) -> WindowAnimationValues {
        if self.last_frame_msec.is_none() {
            self.target = target;
            self.last_frame_msec = Some(now_msec);
            if duration_msec == 0 || self.current == target {
                self.current = target;
                return self.current;
            }
            let step = 1.min(duration_msec);
            self.current.content_alpha = advance_channel(
                self.current.content_alpha,
                target.content_alpha,
                step,
                duration_msec,
            );
            self.current.dim_alpha =
                advance_channel(self.current.dim_alpha, target.dim_alpha, step, duration_msec);
            self.current.shadow_alpha = advance_channel(
                self.current.shadow_alpha,
                target.shadow_alpha,
                step,
                duration_msec,
            );
            self.current.border_alpha = advance_channel(
                self.current.border_alpha,
                target.border_alpha,
                step,
                duration_msec,
            );
            return self.current;
        }

        let elapsed = now_msec.saturating_sub(self.last_frame_msec.unwrap());
        self.last_frame_msec = Some(now_msec);
        self.target = target;
        if duration_msec == 0 {
            self.current = target;
            return self.current;
        }

        let step = elapsed.min(duration_msec);
        self.current.content_alpha =
            advance_channel(self.current.content_alpha, target.content_alpha, step, duration_msec);
        self.current.dim_alpha =
            advance_channel(self.current.dim_alpha, target.dim_alpha, step, duration_msec);
        self.current.shadow_alpha =
            advance_channel(self.current.shadow_alpha, target.shadow_alpha, step, duration_msec);
        self.current.border_alpha =
            advance_channel(self.current.border_alpha, target.border_alpha, step, duration_msec);
        self.current
    }

    pub fn is_animating(self) -> bool {
        self.current != self.target
    }
}

fn advance_channel(current: u8, target: u8, step: u32, duration_msec: u32) -> u8 {
    if current == target || duration_msec == 0 {
        return target;
    }
    let current = i32::from(current);
    let target = i32::from(target);
    let delta = target - current;
    let applied = (delta * step as i32) / duration_msec as i32;
    let next = if applied == 0 { current + delta.signum() } else { current + applied };
    next.clamp(0, i32::from(u8::MAX)) as u8
}

#[cfg(test)]
mod tests {
    use super::{WindowAnimationState, WindowAnimationValues};

    #[test]
    fn initialized_state_starts_animating_on_first_target_change() {
        let mut animation = WindowAnimationState::from_values(WindowAnimationValues {
            content_alpha: 120,
            dim_alpha: 44,
            shadow_alpha: 120,
            border_alpha: 120,
        });

        let next = animation.advance_to(
            WindowAnimationValues {
                content_alpha: 255,
                dim_alpha: 0,
                shadow_alpha: 255,
                border_alpha: 255,
            },
            120,
            10,
        );

        assert!(next.content_alpha > 120);
        assert!(animation.is_animating());
    }

    #[test]
    fn zero_duration_animation_snaps_to_target() {
        let mut animation = WindowAnimationState::from_values(WindowAnimationValues {
            content_alpha: 64,
            dim_alpha: 32,
            shadow_alpha: 64,
            border_alpha: 64,
        });
        let target = WindowAnimationValues::default();

        let next = animation.advance_to(target, 0, 10);

        assert_eq!(next, target);
        assert!(!animation.is_animating());
    }
}
