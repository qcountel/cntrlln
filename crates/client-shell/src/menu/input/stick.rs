//! Gamepad sticks in menus: the left stick steps focus like the D-pad, repeating while held,
//! and the right stick scrolls like a mouse wheel.

use bevy::input::gamepad::Gamepad;
use bevy::math::Vec2;
use std::time::Duration;

/// Deflection that starts a focus step.
const STEP_PRESS: f32 = 0.5;
/// Deflection the left stick must fall below before another direction can start.
const STEP_RELEASE: f32 = 0.3;
/// Hold before a held direction starts repeating, then the repeat interval.
const REPEAT_DELAY: Duration = Duration::from_millis(400);
const REPEAT_INTERVAL: Duration = Duration::from_millis(120);
/// Right-stick deflection below which nothing scrolls.
const SCROLL_DEADZONE: f32 = 0.2;
/// Scrolling at full deflection, in wheel lines per second.
const SCROLL_LINES_PER_SECOND: f32 = 14.0;

/// One focus step: `[x, y]` with exactly one non-zero component, `y` negative upward.
pub(super) type Step = [i32; 2];

/// Held direction and repeat timing of the left stick, and unspent right-stick scroll.
#[derive(Default)]
pub(super) struct StickNavigation {
    held: Option<(Step, Duration)>,
    scroll: f32,
}

impl StickNavigation {
    /// Advances by `elapsed` and returns the focus step to take this frame, if any, and the
    /// whole wheel lines to scroll (positive upward).
    pub(super) fn update<'a>(
        &mut self,
        pads: impl IntoIterator<Item = &'a Gamepad> + Clone,
        elapsed: Duration,
    ) -> (Option<Step>, f32) {
        let strongest = |stick: fn(&Gamepad) -> Vec2| {
            pads.clone()
                .into_iter()
                .map(stick)
                .filter(|value| value.is_finite())
                .max_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))
                .unwrap_or(Vec2::ZERO)
        };
        let step = self.step(strongest(Gamepad::left_stick), elapsed);
        (step, self.scroll(strongest(Gamepad::right_stick).y, elapsed))
    }

    fn step(&mut self, stick: Vec2, elapsed: Duration) -> Option<Step> {
        let direction = if stick.x.abs() >= stick.y.abs() {
            [stick.x.signum() as i32, 0]
        } else {
            [0, -stick.y.signum() as i32]
        };
        let magnitude = stick.x.abs().max(stick.y.abs());
        match &mut self.held {
            Some((held, _)) if magnitude < STEP_RELEASE || *held != direction => {
                self.held = None;
                None
            }
            Some((_, wait)) => {
                *wait = wait.saturating_sub(elapsed);
                if wait.is_zero() {
                    *wait = REPEAT_INTERVAL;
                    Some(direction)
                } else {
                    None
                }
            }
            None if magnitude >= STEP_PRESS => {
                self.held = Some((direction, REPEAT_DELAY));
                Some(direction)
            }
            None => None,
        }
    }

    fn scroll(&mut self, y: f32, elapsed: Duration) -> f32 {
        if !y.is_finite() || y.abs() <= SCROLL_DEADZONE {
            self.scroll = 0.0;
            return 0.0;
        }
        let travel = (y.abs().min(1.0) - SCROLL_DEADZONE) / (1.0 - SCROLL_DEADZONE);
        self.scroll += y.signum() * travel * SCROLL_LINES_PER_SECOND * elapsed.as_secs_f32();
        let lines = self.scroll.trunc();
        self.scroll -= lines;
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Duration = Duration::from_millis(16);

    #[test]
    fn a_held_direction_steps_once_then_repeats_after_the_delay() {
        let mut nav = StickNavigation::default();
        let down = Vec2::new(0.0, -0.9);
        assert_eq!(nav.step(down, FRAME), Some([0, 1]));
        assert_eq!(nav.step(down, Duration::from_millis(300)), None);
        assert_eq!(nav.step(down, Duration::from_millis(100)), Some([0, 1]));
        assert_eq!(nav.step(down, Duration::from_millis(60)), None);
        assert_eq!(nav.step(down, Duration::from_millis(60)), Some([0, 1]));
    }

    #[test]
    fn releasing_or_turning_the_stick_rearms_a_step() {
        let mut nav = StickNavigation::default();
        assert_eq!(nav.step(Vec2::new(0.9, 0.0), FRAME), Some([1, 0]));
        assert_eq!(nav.step(Vec2::new(0.1, 0.0), FRAME), None);
        assert_eq!(nav.step(Vec2::new(0.9, 0.0), FRAME), Some([1, 0]));
        assert_eq!(nav.step(Vec2::new(0.0, 0.9), FRAME), None);
        assert_eq!(nav.step(Vec2::new(0.0, 0.9), FRAME), Some([0, -1]));
    }

    #[test]
    fn the_right_stick_scrolls_whole_lines_and_rests_inside_its_dead_zone() {
        let mut nav = StickNavigation::default();
        assert_eq!(nav.scroll(0.1, Duration::from_secs(1)), 0.0);
        assert_eq!(nav.scroll(1.0, Duration::from_secs(1)), 14.0);
        assert_eq!(nav.scroll(-1.0, Duration::from_millis(500)), -7.0);
    }
}
