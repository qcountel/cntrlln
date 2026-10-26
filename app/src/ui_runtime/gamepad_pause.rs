//! The gamepad's Start (Options) button is Escape, as in vanilla: it pauses gameplay, and every
//! screen that leaves or steps back on Escape does so on Start too.

use bevy::{
    input::{
        ButtonState,
        gamepad::{Gamepad, GamepadButton},
        keyboard::{Key, KeyCode, KeyboardInput},
    },
    prelude::{Entity, Local, MessageWriter, Query, With},
    window::{PrimaryWindow, Window},
};

/// Runs before Bevy's input systems, so the emitted Escape reaches `ButtonInput` this frame.
/// Gamepad state is the last processed frame's, so each Start press is seen exactly once.
pub(crate) fn press_escape_for_start(
    pads: Query<&Gamepad>,
    windows: Query<(Entity, &Window), With<PrimaryWindow>>,
    mut keys: MessageWriter<KeyboardInput>,
    mut held: Local<bool>,
) {
    let Ok((window, state)) = windows.single() else {
        return;
    };
    let pressed = state.focused
        && pads.iter().any(|pad| pad.pressed(GamepadButton::Start))
        && (*held || pads.iter().any(|pad| pad.just_pressed(GamepadButton::Start)));
    if pressed == *held {
        return;
    }
    *held = pressed;
    keys.write(KeyboardInput {
        key_code: KeyCode::Escape,
        logical_key: Key::Escape,
        state: if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        },
        text: None,
        repeat: false,
        window,
    });
}
