//! Gamepad controls, virtual-key labels, and prompt-label maps.
//!
//! Input bindings (`victory-hle`) and on-screen keycap prompts (`victory-game`)
//! both speak in [`PadControl`] and [`PromptLabels`] so both agree without
//! creating a circular dependency between those crates.

/// The Windows virtual key of the left mouse button.
pub const VK_LBUTTON: u8 = 0x01;
/// The Windows virtual key of the right mouse button.
pub const VK_RBUTTON: u8 = 0x02;
/// The Windows virtual key of the middle mouse button.
pub const VK_MBUTTON: u8 = 0x04;
/// The Windows virtual key of Mouse4 (back).
pub const VK_XBUTTON1: u8 = 0x05;
/// The Windows virtual key of Mouse5 (forward).
pub const VK_XBUTTON2: u8 = 0x06;
/// The Windows virtual key of Backspace.
pub const VK_BACK: u8 = 0x08;
/// The Windows virtual key of Tab.
pub const VK_TAB: u8 = 0x09;
/// The Windows virtual key of Enter.
pub const VK_RETURN: u8 = 0x0D;
/// Synthetic virtual key used by `[controls]` for mouse wheel up.
pub const VK_WHEEL_UP: u8 = 0x0E;
/// Synthetic virtual key used by `[controls]` for mouse wheel down.
pub const VK_WHEEL_DOWN: u8 = 0x0F;
/// The Windows virtual key of Caps Lock.
pub const VK_CAPITAL: u8 = 0x14;
/// The Windows virtual key of Esc.
pub const VK_ESCAPE: u8 = 0x1B;
/// The Windows virtual key of Space.
pub const VK_SPACE: u8 = 0x20;
/// The Windows virtual key of the left arrow.
pub const VK_LEFT: u8 = 0x25;
/// The Windows virtual key of the up arrow.
pub const VK_UP: u8 = 0x26;
/// The Windows virtual key of the right arrow.
pub const VK_RIGHT: u8 = 0x27;
/// The Windows virtual key of the down arrow.
pub const VK_DOWN: u8 = 0x28;
/// The Windows virtual key of Left Shift.
pub const VK_LSHIFT: u8 = 0xA0;
/// The Windows virtual key of Right Shift.
pub const VK_RSHIFT: u8 = 0xA1;
/// The Windows virtual key of Left Ctrl.
pub const VK_LCONTROL: u8 = 0xA2;
/// The Windows virtual key of Right Ctrl.
pub const VK_RCONTROL: u8 = 0xA3;
/// The Windows virtual key of Left Alt.
pub const VK_LMENU: u8 = 0xA4;
/// The Windows virtual key of Right Alt.
pub const VK_RMENU: u8 = 0xA5;
/// The Windows virtual key of the grave/tilde key (`~`).
pub const VK_OEM_3: u8 = 0xC0;

/// Whether button prompts keep the title's gamepad icons or redraw them as
/// keyboard/mouse keycaps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PromptMode {
    #[default]
    Gamepad,
    Keys,
}

impl PromptMode {
    /// Parses `"gamepad"` or `"keys"` (ASCII case-insensitive).
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.eq_ignore_ascii_case("gamepad") {
            Some(Self::Gamepad)
        } else if trimmed.eq_ignore_ascii_case("keys") {
            Some(Self::Keys)
        } else {
            None
        }
    }
}

/// Declares the gamepad's controls and the list of all of them from one
/// list, so that the list cannot miss a control.
macro_rules! pad_controls {
    ($($control:ident),* $(,)?) => {
        /// A control of a gamepad.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u8)]
        pub enum PadControl {
            $($control),*
        }

        impl PadControl {
            /// Every control, in the order declared.
            pub const ALL: &[Self] = &[$(Self::$control),*];

            /// Index of this control in [`Self::ALL`].
            #[must_use]
            pub const fn index(self) -> usize {
                self as usize
            }
        }
    };
}

pad_controls!(
    Menu,
    View,
    A,
    B,
    X,
    Y,
    Up,
    Down,
    Left,
    Right,
    LeftShoulder,
    RightShoulder,
    LeftStick,
    RightStick,
    LeftTrigger,
    RightTrigger,
    LeftStickUp,
    LeftStickDown,
    LeftStickLeft,
    LeftStickRight,
    RightStickUp,
    RightStickDown,
    RightStickLeft,
    RightStickRight,
);

/// The virtual key that drives `control` on the legacy keyboard stand-in
/// gamepad (used when `[controls]` is disabled).
#[must_use]
pub const fn fallback_key(control: PadControl) -> u8 {
    match control {
        PadControl::Menu => VK_ESCAPE,
        PadControl::View => VK_TAB,
        PadControl::A => VK_RETURN,
        PadControl::B => VK_BACK,
        PadControl::X => b'X',
        PadControl::Y => b'Y',
        PadControl::Up => VK_UP,
        PadControl::Down => VK_DOWN,
        PadControl::Left => VK_LEFT,
        PadControl::Right => VK_RIGHT,
        PadControl::LeftShoulder => b'Q',
        PadControl::RightShoulder => b'E',
        PadControl::LeftStick => b'F',
        PadControl::RightStick => b'H',
        PadControl::LeftTrigger => b'Z',
        PadControl::RightTrigger => b'C',
        PadControl::LeftStickUp => b'W',
        PadControl::LeftStickDown => b'S',
        PadControl::LeftStickLeft => b'A',
        PadControl::LeftStickRight => b'D',
        PadControl::RightStickUp => b'I',
        PadControl::RightStickDown => b'K',
        PadControl::RightStickLeft => b'J',
        PadControl::RightStickRight => b'L',
    }
}

/// What a key cap shows for a virtual key; `None` for a key without one.
#[must_use]
pub fn key_label(virtual_key: u8) -> Option<&'static str> {
    const LETTERS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const DIGITS: &str = "0123456789";
    const FKEYS: [&str; 12] = [
        "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
    ];
    Some(match virtual_key {
        VK_LBUTTON => "LMB",
        VK_RBUTTON => "RMB",
        VK_MBUTTON => "MMB",
        VK_XBUTTON1 => "M4",
        VK_XBUTTON2 => "M5",
        VK_BACK => "\u{232B}",
        VK_TAB => "Tab",
        VK_RETURN => "\u{21B5}",
        VK_WHEEL_UP => "Wh\u{2191}",
        VK_WHEEL_DOWN => "Wh\u{2193}",
        VK_CAPITAL => "Caps",
        VK_ESCAPE => "Esc",
        VK_SPACE => "Spc",
        0x21 => "PgUp",
        0x22 => "PgDn",
        0x23 => "End",
        0x24 => "Home",
        VK_LEFT => "\u{2190}",
        VK_UP => "\u{2191}",
        VK_RIGHT => "\u{2192}",
        VK_DOWN => "\u{2193}",
        0x2D => "Ins",
        0x2E => "Del",
        b'0'..=b'9' => {
            let at = usize::from(virtual_key - b'0');
            DIGITS.get(at..=at)?
        }
        b'A'..=b'Z' => {
            let at = usize::from(virtual_key - b'A');
            LETTERS.get(at..=at)?
        }
        0x70..=0x7B => FKEYS[usize::from(virtual_key - 0x70)],
        VK_LSHIFT | VK_RSHIFT => "Shift",
        VK_LCONTROL | VK_RCONTROL => "Ctrl",
        VK_LMENU | VK_RMENU => "Alt",
        0xBA => ";",
        0xBB => "=",
        0xBC => ",",
        0xBD => "-",
        0xBE => ".",
        0xBF => "/",
        VK_OEM_3 => "~",
        0xDB => "[",
        0xDC => "\\",
        0xDD => "]",
        0xDE => "'",
        _ => return None,
    })
}

/// Keycap text for each gamepad control, passed from the active input layer
/// to the prompt redrawer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptLabels {
    labels: [Option<String>; PadControl::ALL.len()],
    /// True when the mouse drives the right stick (so right-stick icons draw
    /// `"Mouse"` / `"M↕"` / `"M↔"` / `"M↑"` etc. instead of keyboard keys).
    pub right_stick_is_mouse: bool,
}

impl Default for PromptLabels {
    fn default() -> Self {
        Self::legacy()
    }
}

impl PromptLabels {
    /// Labels matching the legacy stand-in gamepad ([`fallback_key`]).
    #[must_use]
    pub fn legacy() -> Self {
        Self::from_bindings(|control| Some(fallback_key(control)), false)
    }

    /// Builds a label table from a mapping of each [`PadControl`] to its
    /// primary virtual key.
    #[must_use]
    pub fn from_bindings(
        vk_for: impl Fn(PadControl) -> Option<u8>,
        right_stick_is_mouse: bool,
    ) -> Self {
        let mut labels: [Option<String>; PadControl::ALL.len()] =
            std::array::from_fn(|_| None);
        for &control in PadControl::ALL {
            if let Some(text) = vk_for(control).and_then(key_label) {
                labels[control.index()] = Some(text.to_owned());
            }
        }
        Self {
            labels,
            right_stick_is_mouse,
        }
    }

    /// Sets an explicit label for `control`.
    pub fn set(&mut self, control: PadControl, label: impl Into<String>) {
        self.labels[control.index()] = Some(label.into());
    }

    /// Returns the label for `control`, if bound.
    #[must_use]
    pub fn label(&self, control: PadControl) -> Option<&str> {
        self.labels[control.index()].as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PadControl, PromptLabels, PromptMode, VK_LBUTTON, VK_LCONTROL, VK_LMENU, VK_LSHIFT,
        VK_MBUTTON, VK_OEM_3, VK_RBUTTON, VK_SPACE, VK_WHEEL_DOWN, VK_WHEEL_UP, VK_XBUTTON1,
        VK_XBUTTON2, fallback_key, key_label,
    };

    #[test]
    fn every_control_has_a_key_of_its_own() {
        let mut keys: Vec<u8> = PadControl::ALL.iter().map(|&c| fallback_key(c)).collect();
        keys.sort_unstable();
        keys.dedup();

        assert_eq!(keys.len(), 24);
    }

    #[test]
    fn key_labels_cover_pc_keys_and_mouse_buttons() {
        assert_eq!(key_label(VK_SPACE), Some("Spc"));
        assert_eq!(key_label(VK_LSHIFT), Some("Shift"));
        assert_eq!(key_label(VK_LCONTROL), Some("Ctrl"));
        assert_eq!(key_label(VK_LMENU), Some("Alt"));
        assert_eq!(key_label(VK_LBUTTON), Some("LMB"));
        assert_eq!(key_label(VK_RBUTTON), Some("RMB"));
        assert_eq!(key_label(VK_MBUTTON), Some("MMB"));
        assert_eq!(key_label(VK_XBUTTON1), Some("M4"));
        assert_eq!(key_label(VK_XBUTTON2), Some("M5"));
        assert_eq!(key_label(VK_WHEEL_UP), Some("Wh\u{2191}"));
        assert_eq!(key_label(VK_WHEEL_DOWN), Some("Wh\u{2193}"));
        assert_eq!(key_label(VK_OEM_3), Some("~"));
        assert_eq!(key_label(b'5'), Some("5"));
        assert_eq!(key_label(0x70), Some("F1"));
        assert_eq!(key_label(0x7B), Some("F12"));
        assert_eq!(key_label(0xFF), None);
    }

    #[test]
    fn legacy_labels_populate_every_control() {
        let labels = PromptLabels::legacy();
        assert!(!labels.right_stick_is_mouse);
        for &control in PadControl::ALL {
            assert!(labels.label(control).is_some(), "{control:?}");
        }
    }

    #[test]
    fn prompt_mode_parses_case_insensitively() {
        assert_eq!(PromptMode::parse("keys"), Some(PromptMode::Keys));
        assert_eq!(PromptMode::parse("KEYS"), Some(PromptMode::Keys));
        assert_eq!(PromptMode::parse("gamepad"), Some(PromptMode::Gamepad));
        assert_eq!(PromptMode::parse("other"), None);
    }
}
