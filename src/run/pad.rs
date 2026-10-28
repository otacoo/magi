//! SDL gamepad events mapped onto the vendor pad's raw event pipeline.

use sdl2::controller::{Axis as PadAxis, Button as PadButton};
use sdl2::event::Event;

use crate::input::{ev, joypad_bit, retro, RawEvent};

use super::prefs::TRIGGER_ON;
use super::STICK_DEADZONE;

/// D-pad and left-stick state for a Bluetooth/USB pad, combined into the
/// hat events the input pipeline already understands.
#[derive(Default)]
pub(super) struct PadHat {
    dpad: (i32, i32),
    stick: (i32, i32),
    last: (i32, i32),
}

/// SDL controller events as vendor-space raw events, so the shelf's action
/// table, chords, hold-exit and libretro bits need no second copy.
pub(super) fn pump_pads(
    pads: &mut crate::pads::Pads,
    events: &mut sdl2::EventPump,
    hat: &mut PadHat,
    stick_as_dpad: bool,
) -> (Vec<RawEvent>, bool) {
    let mut out = Vec::new();
    let mut changed = false;
    let mut removed = false;
    for ev in events.poll_iter() {
        match ev {
            Event::ControllerButtonDown { button, .. } => match button {
                PadButton::DPadUp => hat.dpad.1 = -1,
                PadButton::DPadDown => hat.dpad.1 = 1,
                PadButton::DPadLeft => hat.dpad.0 = -1,
                PadButton::DPadRight => hat.dpad.0 = 1,
                _ => {
                    if let Some(code) = pad_button_code(button) {
                        out.push(pad_key(code, 1));
                    }
                }
            },
            Event::ControllerButtonUp { button, .. } => match button {
                PadButton::DPadUp | PadButton::DPadDown => hat.dpad.1 = 0,
                PadButton::DPadLeft | PadButton::DPadRight => hat.dpad.0 = 0,
                _ => {
                    if let Some(code) = pad_button_code(button) {
                        out.push(pad_key(code, 0));
                    }
                }
            },
            Event::ControllerAxisMotion { axis, value, .. } => match axis {
                PadAxis::TriggerLeft => out.push(pad_axis(ev::ABS_Z, pad_trigger(value))),
                PadAxis::TriggerRight => out.push(pad_axis(ev::ABS_RZ, pad_trigger(value))),
                PadAxis::LeftX => {
                    hat.stick.0 = pad_stick(value);
                    out.push(pad_axis(ev::ABS_X, i32::from(value)));
                }
                PadAxis::LeftY => {
                    hat.stick.1 = pad_stick(value);
                    out.push(pad_axis(ev::ABS_Y, i32::from(value)));
                }
                PadAxis::RightX => out.push(pad_axis(ev::ABS_RX, i32::from(value))),
                PadAxis::RightY => out.push(pad_axis(ev::ABS_RY, i32::from(value))),
            },
            Event::ControllerDeviceAdded { which, .. } => {
                pads.added(which);
                changed = true;
            }
            Event::ControllerDeviceRemoved { which, .. } => {
                pads.removed(which);
                changed = true;
                removed = true;
            }
            _ => {}
        }
    }
    if removed {
        hat.dpad = (0, 0);
        hat.stick = (0, 0);
    }
    let x = if hat.dpad.0 != 0 {
        hat.dpad.0
    } else if stick_as_dpad {
        hat.stick.0
    } else {
        0
    };
    let y = if hat.dpad.1 != 0 {
        hat.dpad.1
    } else if stick_as_dpad {
        hat.stick.1
    } else {
        0
    };
    if (x, y) != hat.last {
        out.push(pad_axis(ev::ABS_HAT0X, x));
        out.push(pad_axis(ev::ABS_HAT0Y, y));
        hat.last = (x, y);
    }
    (out, changed)
}

fn pad_key(code: u16, value: i32) -> RawEvent {
    RawEvent {
        dev: 0,
        type_: ev::EV_KEY,
        code,
        value,
    }
}

fn pad_axis(code: u16, value: i32) -> RawEvent {
    RawEvent {
        dev: 0,
        type_: ev::EV_ABS,
        code,
        value,
    }
}

fn pad_button_code(b: PadButton) -> Option<u16> {
    use ev::*;
    match b {
        PadButton::A => Some(BTN_SOUTH),
        PadButton::B => Some(BTN_EAST),
        PadButton::X => Some(BTN_NORTH),
        PadButton::Y => Some(BTN_C),
        PadButton::LeftShoulder => Some(BTN_WEST),
        PadButton::RightShoulder => Some(BTN_Z),
        PadButton::Back => Some(BTN_TL),
        PadButton::Start => Some(BTN_TR),
        PadButton::Guide => Some(BTN_TL2),
        _ => None,
    }
}

fn pad_stick(value: i16) -> i32 {
    const DEADZONE: i16 = 16_000;
    match value {
        v if v <= -DEADZONE => -1,
        v if v >= DEADZONE => 1,
        _ => 0,
    }
}

/// SDL trigger axes span 0..32767; the pipeline reads 0..1023.
fn pad_trigger(value: i16) -> i32 {
    (value.max(0) as i32 * 1023) / 32767
}

#[derive(Default)]
pub(super) struct PadState {
    pub(super) bits: u16,
    hat_x: i8,
    hat_y: i8,
    pub(super) left: (i16, i16),
    pub(super) right: (i16, i16),
    pub(super) analog: bool,
}

impl PadState {
    pub(super) fn apply(&mut self, e: &RawEvent) {
        if e.type_ == ev::EV_ABS {
            match e.code {
                ev::ABS_X => {
                    self.left.0 = e.value as i16;
                    self.stick_bits();
                    return;
                }
                ev::ABS_Y => {
                    self.left.1 = e.value as i16;
                    self.stick_bits();
                    return;
                }
                ev::ABS_RX => {
                    self.right.0 = e.value as i16;
                    return;
                }
                ev::ABS_RY => {
                    self.right.1 = e.value as i16;
                    return;
                }
                ev::ABS_HAT0X => self.hat_x = e.value.clamp(-1, 1) as i8,
                ev::ABS_HAT0Y => self.hat_y = e.value.clamp(-1, 1) as i8,
                ev::ABS_Z => {
                    self.set_bit(retro::L2, e.value > TRIGGER_ON);
                    return;
                }
                ev::ABS_RZ => {
                    self.set_bit(retro::R2, e.value > TRIGGER_ON);
                    return;
                }
                _ => return,
            }
            self.set_bit(retro::LEFT, self.hat_x < 0);
            self.set_bit(retro::RIGHT, self.hat_x > 0);
            self.set_bit(retro::UP, self.hat_y < 0);
            self.set_bit(retro::DOWN, self.hat_y > 0);
            return;
        }
        if e.type_ != ev::EV_KEY {
            return;
        }
        if let Some(bit) = joypad_bit(e.code) {
            self.set_bit(bit, e.value != 0);
            return;
        }
        let dir = match e.code {
            ev::KEY_UP | ev::BTN_DPAD_UP => Some(retro::UP),
            ev::KEY_DOWN | ev::BTN_DPAD_DOWN => Some(retro::DOWN),
            ev::KEY_LEFT | ev::BTN_DPAD_LEFT => Some(retro::LEFT),
            ev::KEY_RIGHT | ev::BTN_DPAD_RIGHT => Some(retro::RIGHT),
            _ => None,
        };
        if let Some(bit) = dir {
            self.set_bit(bit, e.value != 0);
        }
    }

    /// JOYPAD-only cores get the left stick as a d-pad.
    fn stick_bits(&mut self) {
        if self.analog {
            return;
        }
        self.set_bit(retro::LEFT, self.left.0 < -STICK_DEADZONE);
        self.set_bit(retro::RIGHT, self.left.0 > STICK_DEADZONE);
        self.set_bit(retro::UP, self.left.1 < -STICK_DEADZONE);
        self.set_bit(retro::DOWN, self.left.1 > STICK_DEADZONE);
    }

    fn set_bit(&mut self, bit: u16, on: bool) {
        if on {
            self.bits |= 1 << bit;
        } else {
            self.bits &= !(1 << bit);
        }
    }

    pub(super) fn reset(&mut self) {
        self.bits = 0;
        self.hat_x = 0;
        self.hat_y = 0;
        self.left = (0, 0);
        self.right = (0, 0);
    }
}
