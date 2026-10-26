//! A gamepad drives the mouse pointer while the top screen is one reached only through the
//! pointer (`Scene::reads_only_pointer`), such as the inventory and containers, which have no
//! controller focus of their own.
//!
//! Those screens already read the cursor, buttons, Shift and the wheel, so the gamepad only
//! emits those, laid out as in vanilla: the left stick moves the pointer, the D-pad steps it by
//! one slot, A takes or places a stack (click), X takes half (right-click), Y moves a stack
//! across (Shift-click), B leaves the screen (mouse Back) and the right stick scrolls.

use bevy::{
    input::{
        ButtonState,
        gamepad::{Gamepad, GamepadButton},
        keyboard::{Key, KeyCode, KeyboardInput},
        mouse::{MouseButton, MouseButtonInput, MouseScrollUnit, MouseWheel},
        touch::TouchPhase,
    },
    math::Vec2,
    prelude::{Entity, Local, MessageWriter, Query, Res, With},
    time::{Real, Time},
    window::{PrimaryWindow, Window},
};
use client_shell::{
    menu::MenuRuntime,
    ui_resources::{UiPresentationResource, UiRuntimeResource},
};
use client_ui::ui_runtime::presentation::forms::scene_policy::MenuScene;
use ui::UiPoint;

/// Stick deflection below which the pointer rests; travel past it is remapped from zero.
const STICK_DEADZONE: f32 = 0.2;
/// Pointer speed at full deflection, in window heights per second.
const POINTER_HEIGHTS_PER_SECOND: f32 = 1.2;
/// One inventory slot in GUI units: a D-pad press moves the pointer this far.
const SLOT_GUI_UNITS: f32 = 18.0;
/// D-pad step on a screen without inventory geometry, in window heights.
const FALLBACK_STEP_HEIGHTS: f32 = 0.05;
/// Scrolling at full right-stick deflection, in wheel lines per second.
const SCROLL_LINES_PER_SECOND: f32 = 12.0;

/// Takes or places a whole stack.
const TAKE: GamepadButton = GamepadButton::South;
/// Takes half a stack or places one item.
const TAKE_HALF: GamepadButton = GamepadButton::West;
/// Moves a stack to the other part of the screen. The default Inventory binding shares this
/// button, so it does not close a screen this module drives (see [`claims_press`]).
const QUICK_MOVE: GamepadButton = GamepadButton::North;
/// Leaves the screen.
const LEAVE: GamepadButton = GamepadButton::East;

/// Gamepad buttons the pointer holds, each from a press made while the screen was shown.
#[derive(Clone, Copy, Default)]
struct Holds {
    take: bool,
    take_half: bool,
    quick_move: bool,
    leave: bool,
}

/// Buttons and wheel travel the gamepad holds on the emulated pointer.
#[derive(Default)]
pub(crate) struct GamepadPointer {
    holds: Holds,
    scroll: f32,
}

/// Whether a pointer button pressed this frame belongs to an open screen rather than to its
/// binding: Y moves a stack on the inventory instead of closing it.
pub(crate) fn claims_press(pads: &Query<&Gamepad>) -> bool {
    pads.iter().any(|pad| pad.just_pressed(QUICK_MOVE))
}

/// A stick's deflection past the dead zone, remapped to `0..=1` and eased for fine aiming.
fn stick_travel(stick: Vec2) -> Vec2 {
    let magnitude = stick.length();
    if !magnitude.is_finite() || magnitude <= STICK_DEADZONE {
        return Vec2::ZERO;
    }
    let travel = ((magnitude.min(1.0) - STICK_DEADZONE) / (1.0 - STICK_DEADZONE)).powi(2);
    stick / magnitude * travel
}

/// The stick with the most travel across every gamepad.
fn strongest(pads: &Query<&Gamepad>, stick: fn(&Gamepad) -> Vec2) -> Vec2 {
    pads.iter()
        .map(stick)
        .max_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))
        .unwrap_or(Vec2::ZERO)
}

/// Window pixels per inventory slot, measured from the open screen's GUI scale.
fn slot_pixels(window: &Window, at: Vec2, presentation: Option<&UiPresentationResource>) -> f32 {
    let fallback = window.height() * FALLBACK_STEP_HEIGHTS;
    let size = [window.physical_width(), window.physical_height()];
    let gui = |x: f32| {
        let point = UiPoint::new(x, at.y).ok()?;
        presentation?.inventory_gui_point(point, size, window.scale_factor())
    };
    match (gui(at.x), gui(at.x + 100.0)) {
        (Some(from), Some(to)) if to[0] > from[0] => SLOT_GUI_UNITS * 100.0 / (to[0] - from[0]),
        _ => fallback,
    }
}

/// Runs before Bevy's input systems, so emitted edges reach `ButtonInput` this frame. Gamepad
/// state is the last processed frame's, so each D-pad press is seen exactly once.
#[allow(
    clippy::too_many_arguments,
    reason = "The Bevy system reads pads, screen ownership and writes three message streams."
)]
pub(crate) fn drive_ui_pointer(
    pads: Query<&Gamepad>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    ui: Option<Res<UiRuntimeResource>>,
    player: Option<Res<client_state::PlayerRuntime>>,
    menu: Option<Res<MenuRuntime>>,
    presentation: Option<Res<UiPresentationResource>>,
    time: Res<Time<Real>>,
    mut buttons: MessageWriter<MouseButtonInput>,
    mut keys: MessageWriter<KeyboardInput>,
    mut wheels: MessageWriter<MouseWheel>,
    mut state: Local<GamepadPointer>,
) {
    let Ok((window_entity, mut window)) = windows.single_mut() else {
        return;
    };
    let pointer_screen = window.focused
        && (ui.zip(player).is_some_and(|(ui, player)| {
            ui.top_scene_reads_only_pointer(&player, menu.as_deref().map(|m| m as &dyn MenuScene))
        }) || presentation
            .as_deref()
            .is_some_and(|presentation| presentation.mod_ui().mod_panel_open()));
    let state = &mut *state;
    let was = state.holds;
    // A hold starts only on a press made over the screen, so Y still held from opening the
    // inventory, or A from a jump, does nothing until pressed again.
    let held = |button, holding: bool| {
        pointer_screen
            && pads.iter().any(|pad| pad.pressed(button))
            && (holding || pads.iter().any(|pad| pad.just_pressed(button)))
    };
    let now = Holds {
        take: held(TAKE, was.take),
        take_half: held(TAKE_HALF, was.take_half),
        quick_move: held(QUICK_MOVE, was.quick_move),
        leave: held(LEAVE, was.leave),
    };
    state.holds = now;

    // Shift goes down before and up after its click, so the screen sees a Shift-click.
    let mut key = |pressed: bool| {
        keys.write(KeyboardInput {
            key_code: KeyCode::ShiftLeft,
            logical_key: Key::Shift,
            state: if pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            },
            text: None,
            repeat: false,
            window: window_entity,
        });
    };
    if now.quick_move && !was.quick_move {
        key(true);
    }
    for (button, now, was) in [
        (
            MouseButton::Left,
            now.take || now.quick_move,
            was.take || was.quick_move,
        ),
        (MouseButton::Right, now.take_half, was.take_half),
        (MouseButton::Back, now.leave, was.leave),
    ] {
        if now != was {
            buttons.write(MouseButtonInput {
                button,
                state: if now {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                },
                window: window_entity,
            });
        }
    }
    if !now.quick_move && was.quick_move {
        key(false);
    }
    if !pointer_screen {
        state.scroll = 0.0;
        return;
    }

    let seconds = time.delta_secs().clamp(0.0, 0.1);
    let travel = stick_travel(strongest(&pads, Gamepad::left_stick));
    let center = Vec2::new(window.width(), window.height()) / 2.0;
    let from = window.cursor_position().unwrap_or(center);
    // Window coordinates grow downward; stick Y grows upward.
    let mut to = from
        + Vec2::new(travel.x, -travel.y) * window.height() * POINTER_HEIGHTS_PER_SECOND * seconds;
    let step = pads
        .iter()
        .map(|pad| {
            let pressed = |button| f32::from(u8::from(pad.just_pressed(button)));
            Vec2::new(
                pressed(GamepadButton::DPadRight) - pressed(GamepadButton::DPadLeft),
                pressed(GamepadButton::DPadDown) - pressed(GamepadButton::DPadUp),
            )
        })
        .sum::<Vec2>()
        .clamp(Vec2::splat(-1.0), Vec2::ONE);
    if step != Vec2::ZERO {
        to += step * slot_pixels(&window, from, presentation.as_deref());
    }
    let to = to.clamp(
        Vec2::ZERO,
        (Vec2::new(window.width(), window.height()) - 1.0).max(Vec2::ZERO),
    );
    if to != from {
        // Changed so the window backend warps the OS cursor to match.
        window.set_cursor_position(Some(to));
    }

    let scroll = stick_travel(strongest(&pads, Gamepad::right_stick)).y;
    state.scroll = if scroll == 0.0 {
        0.0
    } else {
        state.scroll + scroll * SCROLL_LINES_PER_SECOND * seconds
    };
    let lines = state.scroll.trunc();
    if lines != 0.0 {
        state.scroll -= lines;
        wheels.write(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: lines,
            window: window_entity,
            phase: TouchPhase::Moved,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stick_travel_rests_inside_the_dead_zone_and_reaches_full_speed() {
        assert_eq!(stick_travel(Vec2::new(0.15, 0.1)), Vec2::ZERO);
        assert_eq!(stick_travel(Vec2::new(f32::NAN, 0.0)), Vec2::ZERO);
        let full = stick_travel(Vec2::new(0.0, -1.5));
        assert!((full - Vec2::new(0.0, -1.0)).length() < 1e-6);
        let half = stick_travel(Vec2::new(0.6, 0.0)).x;
        assert!(half > 0.0 && half < 0.5);
    }
}
