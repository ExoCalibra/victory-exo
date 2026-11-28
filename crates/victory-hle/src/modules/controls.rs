//! Keyboard and mouse played the way PC games are: keys to pad buttons and
//! sticks, the mouse to the right stick, mouse buttons to the triggers.
//!
//! The title reads gamepads and keyboards only (it registers no
//! mouse), and turns its right stick into a turn rate. Mouse counts are a
//! distance, so they become a rate when the title reads the pad: the counts
//! since its last read over the time since then. The camera then turns the
//! same angle for the same hand movement at any frame rate.
//!
//! All of this is plain data: the host side of `gameinput` feeds it and the
//! title's reads drain it. `[controls]` in victory.toml sets and rebinds it.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU64, Ordering};

use serde::Deserialize;
use victory_core::pad::{
    PadControl, PromptLabels, PromptMode, VK_BACK, VK_DOWN, VK_ESCAPE, VK_LBUTTON, VK_LCONTROL,
    VK_LEFT, VK_LMENU, VK_LSHIFT, VK_MBUTTON, VK_RBUTTON, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB,
    VK_UP, VK_XBUTTON1, VK_XBUTTON2, key_label,
};
use victory_gpu::controls_ui::{self, BindingEntry, Command as OverlayCommand, Snapshot as OverlaySnapshot};

use super::gameinput::{
    A, B, DOWN, LEFT, LEFT_SHOULDER, LEFT_STICK, MENU, PadState, RIGHT, RIGHT_SHOULDER,
    RIGHT_STICK, UP, VIEW, X, Y,
};

/// Key codes are Windows virtual keys; mouse buttons are theirs too (1, 2,
/// 4, 5, 6) and the wheel takes two codes Windows leaves unassigned.
pub(super) const WHEEL_UP: u8 = 0x0E;
pub(super) const WHEEL_DOWN: u8 = 0x0F;
/// Mouse buttons by virtual key: left, right, middle, back, forward.
pub(super) const MOUSE_BUTTONS: [u8; 5] = [0x01, 0x02, 0x04, 0x05, 0x06];

/// Mouse counts per second that make a full stick at sensitivity 1.
const FULL_RATE: f64 = 2000.0;
/// Title reads closer together than this see the same mouse stick.
const MIN_TAKE_US: u64 = 2_000;
/// The first read's time span, and the longest one (a title that paused;
/// the title's world frames take 1-2 s here for now, measured).
const FIRST_TAKE_US: u64 = 16_667;
const MAX_TAKE_US: u64 = 1_000_000;
/// Counts past a full stick wait at most this long (seconds of full stick).
const CARRY_S: f64 = 0.1;
/// A pad stick past this takes its stick back from the keyboard and mouse.
/// Detection only: the pad's own value is passed on as it is.
const PAD_TAKES_OVER: f32 = 0.2;
/// Wheel steps waiting to be pressed.
const WHEEL_QUEUE: usize = 8;

/// What a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Output {
    Button(u32),
    LeftTrigger,
    RightTrigger,
    /// One stick direction.
    Stick {
        right: bool,
        x: bool,
        positive: bool,
    },
    /// While held, the left stick moves at the walk speed.
    Walk,
    /// Switches between the on-foot and the vehicle layout.
    ToggleVehicle,
    /// Frees the mouse cursor, or captures it again.
    ReleaseCursor,
}

const fn stick(right: bool, x: bool, positive: bool) -> Output {
    Output::Stick { right, x, positive }
}

const OUTPUTS: [(&str, Output); 27] = [
    ("A", Output::Button(A)),
    ("B", Output::Button(B)),
    ("X", Output::Button(X)),
    ("Y", Output::Button(Y)),
    ("LB", Output::Button(LEFT_SHOULDER)),
    ("RB", Output::Button(RIGHT_SHOULDER)),
    ("LT", Output::LeftTrigger),
    ("RT", Output::RightTrigger),
    ("LS", Output::Button(LEFT_STICK)),
    ("RS", Output::Button(RIGHT_STICK)),
    ("View", Output::Button(VIEW)),
    ("Menu", Output::Button(MENU)),
    ("DpadUp", Output::Button(UP)),
    ("DpadDown", Output::Button(DOWN)),
    ("DpadLeft", Output::Button(LEFT)),
    ("DpadRight", Output::Button(RIGHT)),
    ("LeftUp", stick(false, false, true)),
    ("LeftDown", stick(false, false, false)),
    ("LeftLeft", stick(false, true, false)),
    ("LeftRight", stick(false, true, true)),
    ("RightUp", stick(true, false, true)),
    ("RightDown", stick(true, false, false)),
    ("RightLeft", stick(true, true, false)),
    ("RightRight", stick(true, true, true)),
    ("Walk", Output::Walk),
    ("ToggleVehicle", Output::ToggleVehicle),
    ("ReleaseCursor", Output::ReleaseCursor),
];

/// Key names besides single letters and digits, `F1`-`F24` and `Num0`-`Num9`.
const KEYS: [(&str, u8); 48] = [
    ("MouseLeft", 0x01),
    ("MouseRight", 0x02),
    ("MouseMiddle", 0x04),
    ("Mouse4", 0x05),
    ("Mouse5", 0x06),
    ("WheelUp", WHEEL_UP),
    ("WheelDown", WHEEL_DOWN),
    ("Backspace", 0x08),
    ("Tab", 0x09),
    ("Enter", 0x0D),
    ("Pause", 0x13),
    ("CapsLock", 0x14),
    ("Esc", 0x1B),
    ("Space", 0x20),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("NumMultiply", 0x6A),
    ("NumAdd", 0x6B),
    ("NumSubtract", 0x6D),
    ("NumDecimal", 0x6E),
    ("NumDivide", 0x6F),
    ("LShift", 0xA0),
    ("RShift", 0xA1),
    ("LCtrl", 0xA2),
    ("RCtrl", 0xA3),
    ("LAlt", 0xA4),
    ("RAlt", 0xA5),
    ("Semicolon", 0xBA),
    ("Equals", 0xBB),
    ("Comma", 0xBC),
    ("Minus", 0xBD),
    ("Period", 0xBE),
    ("Slash", 0xBF),
    ("Grave", 0xC0),
    ("LBracket", 0xDB),
    ("Backslash", 0xDC),
    ("RBracket", 0xDD),
    ("Quote", 0xDE),
    ("Shift", 0xA0),
    ("Ctrl", 0xA2),
];

/// The virtual key a key name stands for (case does not matter).
pub(super) fn key_code(name: &str) -> Option<u8> {
    let name = name.trim();
    if let [c] = name.as_bytes()
        && c.is_ascii_alphanumeric()
    {
        return Some(c.to_ascii_uppercase());
    }
    if let Some((_, vk)) = KEYS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        return Some(*vk);
    }
    if name.eq_ignore_ascii_case("Alt") {
        return Some(0xA4);
    }
    let numbered = |prefix: &str, first: u8, count: u8| {
        let rest = name.get(prefix.len()..)?;
        if !name.get(..prefix.len())?.eq_ignore_ascii_case(prefix) {
            return None;
        }
        let n: u8 = rest.parse().ok()?;
        (n < count).then(|| first + n)
    };
    numbered("Num", 0x60, 10).or_else(|| {
        numbered("F", 0x6F, 25).filter(|&vk| vk > 0x6F) // F1 = 0x70
    })
}

/// The key a US keyboard has at the place of scan code `scan` (letters,
/// digits and punctuation; `None` for keys that sit at the same place on
/// every layout, which keep their virtual key). Bindings name keys by that
/// place, so the hand position is the same on every layout: the keys of
/// W A S D are Z Q S D on an AZERTY keyboard.
pub(super) fn us_key(scan: u32) -> Option<u8> {
    // Rows of a US keyboard from their first scan code: digits to `=`,
    // Q to `]`, A to the grave accent, `\` to `/`.
    const ROWS: [(u32, &[u8]); 4] = [
        (0x02, &[b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', 0xBD, 0xBB]),
        (0x10, &[b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', b'O', b'P', 0xDB, 0xDD]),
        (0x1E, &[b'A', b'S', b'D', b'F', b'G', b'H', b'J', b'K', b'L', 0xBA, 0xDE, 0xC0]),
        (0x2B, &[0xDC, b'Z', b'X', b'C', b'V', b'B', b'N', b'M', 0xBC, 0xBE, 0xBF]),
    ];
    ROWS.iter().find_map(|&(first, keys)| {
        let at = scan.checked_sub(first)?;
        keys.get(at as usize).copied()
    })
}

/// A key's name, for messages.
pub(super) fn key_name(vk: u8) -> String {
    if vk.is_ascii_uppercase() || vk.is_ascii_digit() {
        return char::from(vk).to_string();
    }
    match vk {
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => KEYS
            .iter()
            .find(|(_, k)| *k == vk)
            .map_or_else(|| format!("{vk:#04x}"), |(n, _)| (*n).to_string()),
    }
}

fn output(name: &str) -> Option<Output> {
    OUTPUTS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name.trim()))
        .map(|(_, o)| *o)
}

/// Keys and what each does, for one situation (on foot, in a vehicle).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Layout(BTreeMap<u8, Vec<Output>>);

impl Layout {
    fn outputs(&self, vk: u8) -> &[Output] {
        self.0.get(&vk).map_or(&[], Vec::as_slice)
    }

    /// Binds `key` to `outputs` (none: unbinds it).
    fn bind(&mut self, key: &str, outputs: &[&str]) -> Result<(), String> {
        let vk = key_code(key).ok_or_else(|| format!("unknown key `{key}`"))?;
        let outs = outputs
            .iter()
            .filter(|o| !o.trim().is_empty())
            .map(|o| output(o).ok_or_else(|| format!("unknown pad output `{o}` for key `{key}`")))
            .collect::<Result<Vec<_>, _>>()?;
        if outs.is_empty() {
            self.0.remove(&vk);
        } else {
            self.0.insert(vk, outs);
        }
        Ok(())
    }

    fn of(table: &[(&str, &[&str])]) -> Layout {
        let mut layout = Layout::default();
        for (key, outs) in table {
            layout.bind(key, outs).expect("default bindings use known names");
        }
        layout
    }
}

/// On foot: the series' usual PC keys, each on the pad button that does the
/// same on a console (the previous game's console layout; this title's is
/// unverified).
const ON_FOOT: &[(&str, &[&str])] = &[
    ("W", &["LeftUp"]),
    ("S", &["LeftDown"]),
    ("A", &["LeftLeft"]),
    ("D", &["LeftRight"]),
    ("LAlt", &["Walk"]),
    ("LShift", &["A"]),
    ("Space", &["X"]),
    ("F", &["Y", "ToggleVehicle"]),
    ("G", &["ToggleVehicle"]),
    ("R", &["B"]),
    ("Q", &["RB"]),
    ("E", &["DpadRight"]),
    ("LCtrl", &["LS"]),
    ("C", &["RS"]),
    ("V", &["View"]),
    ("M", &["View"]),
    ("Tab", &["LB"]),
    ("WheelUp", &["DpadLeft"]),
    ("WheelDown", &["DpadRight"]),
    ("MouseLeft", &["RT"]),
    ("MouseRight", &["LT"]),
    ("MouseMiddle", &["RS"]),
    ("Up", &["DpadUp"]),
    ("Down", &["DpadDown"]),
    ("Left", &["DpadLeft"]),
    ("Right", &["DpadRight"]),
    ("Enter", &["A"]),
    ("Backspace", &["B"]),
    ("Esc", &["Menu", "ReleaseCursor"]),
    ("P", &["Menu"]),
];

/// In a vehicle: W/S throttle and brake on the triggers, A/D steer.
const VEHICLE: &[(&str, &[&str])] = &[
    ("W", &["RT"]),
    ("S", &["LT"]),
    ("A", &["LeftLeft"]),
    ("D", &["LeftRight"]),
    ("LShift", &["A"]),
    ("Space", &["RB"]),
    ("E", &["LS"]),
    ("F", &["Y", "ToggleVehicle"]),
    ("G", &["ToggleVehicle"]),
    ("R", &["B"]),
    ("C", &["RS"]),
    ("V", &["View"]),
    ("M", &["View"]),
    ("Tab", &["LB"]),
    ("WheelUp", &["DpadLeft"]),
    ("WheelDown", &["DpadRight"]),
    ("MouseLeft", &["RT"]),
    ("MouseRight", &["LT"]),
    ("MouseMiddle", &["RS"]),
    ("Up", &["DpadUp"]),
    ("Down", &["DpadDown"]),
    ("Left", &["DpadLeft"]),
    ("Right", &["DpadRight"]),
    ("Enter", &["A"]),
    ("Backspace", &["B"]),
    ("Esc", &["Menu", "ReleaseCursor"]),
    ("P", &["Menu"]),
];

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Settings {
    /// Off: the old stand-in pad (README's table) and nothing else.
    pub enabled: bool,
    /// `"gamepad"` (default) keeps Xbox icons; `"keys"` redraws them as keycaps.
    pub prompts: PromptMode,
    pub sensitivity: f32,
    /// Vertical sensitivity relative to horizontal.
    pub y_scale: f32,
    pub invert_y: bool,
    /// Capture (hide and hold) the cursor while the title window has focus.
    pub capture: bool,
    /// Counts past a full stick are turned later instead of dropped.
    pub carry: bool,
    /// The title's own stick dead zone and response exponent, undone so the
    /// smallest movement turns and the response is linear.
    pub deadzone: f32,
    pub curve: f32,
    pub walk_speed: f32,
    /// Letters, digits and punctuation are the keys at those places on a
    /// US keyboard ([`us_key`]), whatever the layout prints on them.
    pub by_position: bool,
    /// Show the top-center `ON FOOT` / `VEHICLE` mode badge on screen.
    pub show_badge: bool,
    pub on_foot: Layout,
    pub vehicle: Layout,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            enabled: true,
            prompts: PromptMode::Gamepad,
            sensitivity: 1.0,
            y_scale: 1.0,
            invert_y: false,
            capture: true,
            carry: true,
            deadzone: 0.3,
            curve: 1.0,
            walk_speed: 0.5,
            by_position: true,
            show_badge: true,
            on_foot: Layout::of(ON_FOOT),
            vehicle: Layout::of(VEHICLE),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Binding {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    enabled: Option<bool>,
    prompts: Option<String>,
    mouse_sensitivity: Option<f32>,
    mouse_y_scale: Option<f32>,
    mouse_invert_y: Option<bool>,
    mouse_capture: Option<bool>,
    mouse_carry: Option<bool>,
    stick_deadzone: Option<f32>,
    stick_curve: Option<f32>,
    walk_speed: Option<f32>,
    keys_by_position: Option<bool>,
    show_badge: Option<bool>,
    #[serde(default)]
    on_foot: BTreeMap<String, Binding>,
    #[serde(default)]
    vehicle: BTreeMap<String, Binding>,
}

impl Settings {
    /// The defaults with `[controls]` applied; a layout table replaces only
    /// the keys it names.
    pub(super) fn parse(table: &toml::Table) -> Result<Settings, String> {
        let file: File = toml::Value::Table(table.clone())
            .try_into()
            .map_err(|e| format!("[controls]: {e}"))?;
        let mut s = Settings::default();
        if let Some(raw) = file.prompts.as_deref() {
            s.prompts = PromptMode::parse(raw).ok_or_else(|| {
                format!("[controls] prompts = {raw:?}: expected \"gamepad\" or \"keys\"")
            })?;
        }
        let range = |name: &str, v: Option<f32>, lo: f32, hi: f32, into: &mut f32| match v {
            Some(v) if !(lo..=hi).contains(&v) => {
                Err(format!("[controls] {name} = {v} is outside {lo} to {hi}"))
            }
            Some(v) => {
                *into = v;
                Ok(())
            }
            None => Ok(()),
        };
        range("mouse_sensitivity", file.mouse_sensitivity, 0.01, 100.0, &mut s.sensitivity)?;
        range("mouse_y_scale", file.mouse_y_scale, 0.01, 100.0, &mut s.y_scale)?;
        range("stick_deadzone", file.stick_deadzone, 0.0, 0.9, &mut s.deadzone)?;
        range("stick_curve", file.stick_curve, 0.2, 5.0, &mut s.curve)?;
        range("walk_speed", file.walk_speed, 0.05, 1.0, &mut s.walk_speed)?;
        s.enabled = file.enabled.unwrap_or(s.enabled);
        s.invert_y = file.mouse_invert_y.unwrap_or(s.invert_y);
        s.capture = file.mouse_capture.unwrap_or(s.capture);
        s.carry = file.mouse_carry.unwrap_or(s.carry);
        s.by_position = file.keys_by_position.unwrap_or(s.by_position);
        s.show_badge = file.show_badge.unwrap_or(s.show_badge);
        for (layout, binds, name) in [
            (&mut s.on_foot, &file.on_foot, "on_foot"),
            (&mut s.vehicle, &file.vehicle, "vehicle"),
        ] {
            for (key, binding) in binds {
                let outs: Vec<&str> = match binding {
                    Binding::One(o) => vec![o.as_str()],
                    Binding::Many(os) => os.iter().map(String::as_str).collect(),
                };
                layout
                    .bind(key, &outs)
                    .map_err(|e| format!("[controls.{name}]: {e}"))?;
            }
        }
        Ok(s)
    }
}

static SETTINGS: OnceLock<Settings> = OnceLock::new();
static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();
static SNAPSHOT_GEN: AtomicU64 = AtomicU64::new(1);

/// Takes `[controls]` from the config file (none: the defaults). Call it
/// before the title starts; later calls change nothing.
pub fn configure(table: Option<&toml::Table>) -> Result<(), String> {
    configure_with_path(table, None)
}

/// Same as [`configure`], remembering the config file path so edits made in
/// the in-game `F2` controls overlay save back to the same `victory.toml`.
pub fn configure_with_path(table: Option<&toml::Table>, path: Option<&Path>) -> Result<(), String> {
    let s = match table {
        Some(t) => Settings::parse(t)?,
        None => Settings::default(),
    };
    let save_path = path
        .map(Path::to_path_buf)
        .or_else(|| {
            std::env::var_os("VICTORY_CONFIG")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("victory.toml"));
    let _ = CONFIG_PATH.set(save_path);
    tracing::info!(
        enabled = s.enabled,
        sensitivity = s.sensitivity,
        invert_y = s.invert_y,
        capture = s.capture,
        deadzone = s.deadzone,
        curve = s.curve,
        show_badge = s.show_badge,
        "keyboard and mouse controls"
    );
    controls_ui::publish(build_snapshot(&s, false, String::new()));
    let _ = SETTINGS.set(s);
    Ok(())
}

/// Actions shown in the in-game `F2` controls overlay (output id + uppercase label).
const ON_FOOT_ROWS: &[(&str, &str)] = &[
    ("LeftUp", "MOVE FORWARD (L-UP)"),
    ("LeftDown", "MOVE BACK (L-DOWN)"),
    ("LeftLeft", "MOVE LEFT (L-LEFT)"),
    ("LeftRight", "MOVE RIGHT (L-RIGHT)"),
    ("Walk", "WALK MODIFIER"),
    ("A", "SPRINT / CONFIRM (A)"),
    ("B", "MELEE / BACK (B)"),
    ("X", "JUMP / CLIMB (X)"),
    ("Y", "INTERACT / CAR (Y)"),
    ("LB", "WEAPON WHEEL (LB)"),
    ("RB", "COVER (RB)"),
    ("LT", "AIM (LT)"),
    ("RT", "FIRE / ATTACK (RT)"),
    ("LS", "CROUCH / STEALTH (LS)"),
    ("RS", "LOOK BACK (RS)"),
    ("DpadUp", "PHONE / DPAD UP"),
    ("DpadDown", "DPAD DOWN"),
    ("DpadLeft", "DPAD LEFT"),
    ("DpadRight", "INTERACT / DPAD RIGHT"),
    ("View", "CAMERA / MAP (VIEW)"),
    ("Menu", "PAUSE MENU (MENU)"),
    ("ToggleVehicle", "TOGGLE VEHICLE MODE"),
    ("ReleaseCursor", "RELEASE CURSOR"),
];

const VEHICLE_ROWS: &[(&str, &str)] = &[
    ("RT", "GAS / THROTTLE (RT)"),
    ("LT", "BRAKE / REVERSE (LT)"),
    ("LeftLeft", "STEER LEFT (L-LEFT)"),
    ("LeftRight", "STEER RIGHT (L-RIGHT)"),
    ("LeftUp", "LEAN FORWARD (L-UP)"),
    ("LeftDown", "LEAN BACK (L-DOWN)"),
    ("RB", "HANDBRAKE (RB)"),
    ("LS", "HORN (LS)"),
    ("Y", "EXIT VEHICLE (Y)"),
    ("A", "DUCK / BOOST (A)"),
    ("B", "CINEMATIC CAM (B)"),
    ("X", "SWITCH WEAPON (X)"),
    ("LB", "FIRE DRIVE-BY (LB)"),
    ("RS", "LOOK BACK (RS)"),
    ("DpadUp", "PHONE / DPAD UP"),
    ("DpadDown", "DPAD DOWN"),
    ("DpadLeft", "RADIO PREV (DPAD L)"),
    ("DpadRight", "HEADLIGHTS (DPAD R)"),
    ("View", "CAMERA / MAP (VIEW)"),
    ("Menu", "PAUSE MENU (MENU)"),
    ("ToggleVehicle", "TOGGLE ON-FOOT MODE"),
    ("ReleaseCursor", "RELEASE CURSOR"),
];

fn output_name(out: Output) -> Option<&'static str> {
    OUTPUTS.iter().find(|(_, o)| *o == out).map(|(n, _)| *n)
}

fn layout_entries(layout: &Layout, rows: &[(&'static str, &'static str)]) -> Vec<BindingEntry> {
    rows.iter()
        .map(|&(id, label)| {
            let target = output(id);
            let mut names: Vec<String> = layout
                .0
                .iter()
                .filter(|(_, outs)| target.is_some_and(|t| outs.contains(&t)))
                .map(|(&vk, _)| key_name(vk).to_ascii_uppercase())
                .collect();
            names.sort();
            let keys = if names.is_empty() {
                "UNBOUND".to_owned()
            } else {
                names.join(", ")
            };
            BindingEntry { id, label, keys }
        })
        .collect()
}

fn build_snapshot(s: &Settings, vehicle: bool, note: String) -> OverlaySnapshot {
    OverlaySnapshot {
        enabled: s.enabled,
        vehicle,
        show_badge: s.show_badge,
        sensitivity: s.sensitivity,
        y_scale: s.y_scale,
        deadzone: s.deadzone,
        curve: s.curve,
        invert_y: s.invert_y,
        on_foot: layout_entries(&s.on_foot, ON_FOOT_ROWS),
        vehicle_binds: layout_entries(&s.vehicle, VEHICLE_ROWS),
        note,
        generation: SNAPSHOT_GEN.fetch_add(1, Ordering::Relaxed),
    }
}

fn layout_diff_table(current: &Layout, defaults: &Layout) -> toml::Table {
    let mut table = toml::Table::new();
    let mut all_vks: Vec<u8> = current
        .0
        .keys()
        .chain(defaults.0.keys())
        .copied()
        .collect();
    all_vks.sort_unstable();
    all_vks.dedup();
    for vk in all_vks {
        let cur = current.0.get(&vk);
        let def = defaults.0.get(&vk);
        if cur == def {
            continue;
        }
        let key = key_name(vk);
        let val = match cur {
            None => toml::Value::String(String::new()),
            Some(outs) if outs.len() == 1 => {
                toml::Value::String(output_name(outs[0]).unwrap_or_default().to_owned())
            }
            Some(outs) => toml::Value::Array(
                outs.iter()
                    .filter_map(|&o| output_name(o).map(|n| toml::Value::String(n.to_owned())))
                    .collect(),
            ),
        };
        table.insert(key, val);
    }
    table
}

fn round2(v: f32) -> f64 {
    (f64::from(v) * 100.0).round() / 100.0
}

fn save_settings_to(s: &Settings, path: &Path) -> Result<(), String> {
    let mut root: toml::Table = match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => return Err(format!("reading {}: {e}", path.display())),
    };
    let defaults = Settings::default();
    let mut controls = toml::Table::new();
    controls.insert("enabled".to_owned(), toml::Value::Boolean(s.enabled));
    controls.insert(
        "prompts".to_owned(),
        toml::Value::String(
            match s.prompts {
                PromptMode::Gamepad => "gamepad",
                PromptMode::Keys => "keys",
            }
            .to_owned(),
        ),
    );
    controls.insert(
        "mouse_sensitivity".to_owned(),
        toml::Value::Float(round2(s.sensitivity)),
    );
    controls.insert(
        "mouse_y_scale".to_owned(),
        toml::Value::Float(round2(s.y_scale)),
    );
    controls.insert("mouse_invert_y".to_owned(), toml::Value::Boolean(s.invert_y));
    controls.insert("mouse_capture".to_owned(), toml::Value::Boolean(s.capture));
    controls.insert("mouse_carry".to_owned(), toml::Value::Boolean(s.carry));
    controls.insert(
        "stick_deadzone".to_owned(),
        toml::Value::Float(round2(s.deadzone)),
    );
    controls.insert("stick_curve".to_owned(), toml::Value::Float(round2(s.curve)));
    controls.insert(
        "walk_speed".to_owned(),
        toml::Value::Float(round2(s.walk_speed)),
    );
    controls.insert(
        "keys_by_position".to_owned(),
        toml::Value::Boolean(s.by_position),
    );
    controls.insert("show_badge".to_owned(), toml::Value::Boolean(s.show_badge));
    let on_foot = layout_diff_table(&s.on_foot, &defaults.on_foot);
    if !on_foot.is_empty() {
        controls.insert("on_foot".to_owned(), toml::Value::Table(on_foot));
    }
    let vehicle = layout_diff_table(&s.vehicle, &defaults.vehicle);
    if !vehicle.is_empty() {
        controls.insert("vehicle".to_owned(), toml::Value::Table(vehicle));
    }
    root.insert("controls".to_owned(), toml::Value::Table(controls));
    let rendered =
        toml::to_string_pretty(&root).map_err(|e| format!("serialising controls: {e}"))?;
    std::fs::write(path, rendered).map_err(|e| format!("writing {}: {e}", path.display()))
}

pub(super) fn settings() -> Settings {
    SETTINGS.get_or_init(Settings::default).clone()
}

/// The `[controls] prompts` setting (`Gamepad` by default).
pub fn configured_prompts() -> PromptMode {
    settings().prompts
}

/// Keycap labels for the active `[controls]` configuration (or the legacy
/// stand-in pad when `[controls] enabled = false`).
pub fn prompt_labels() -> PromptLabels {
    settings().prompt_labels()
}

/// Maps a `[controls]` `Output` to the [`PadControl`] it drives.
fn output_to_pad_control(output: Output) -> Option<PadControl> {
    Some(match output {
        Output::Button(MENU) => PadControl::Menu,
        Output::Button(VIEW) => PadControl::View,
        Output::Button(A) => PadControl::A,
        Output::Button(B) => PadControl::B,
        Output::Button(X) => PadControl::X,
        Output::Button(Y) => PadControl::Y,
        Output::Button(UP) => PadControl::Up,
        Output::Button(DOWN) => PadControl::Down,
        Output::Button(LEFT) => PadControl::Left,
        Output::Button(RIGHT) => PadControl::Right,
        Output::Button(LEFT_SHOULDER) => PadControl::LeftShoulder,
        Output::Button(RIGHT_SHOULDER) => PadControl::RightShoulder,
        Output::Button(LEFT_STICK) => PadControl::LeftStick,
        Output::Button(RIGHT_STICK) => PadControl::RightStick,
        Output::LeftTrigger => PadControl::LeftTrigger,
        Output::RightTrigger => PadControl::RightTrigger,
        Output::Stick {
            right: false,
            x: false,
            positive: true,
        } => PadControl::LeftStickUp,
        Output::Stick {
            right: false,
            x: false,
            positive: false,
        } => PadControl::LeftStickDown,
        Output::Stick {
            right: false,
            x: true,
            positive: false,
        } => PadControl::LeftStickLeft,
        Output::Stick {
            right: false,
            x: true,
            positive: true,
        } => PadControl::LeftStickRight,
        Output::Stick {
            right: true,
            x: false,
            positive: true,
        } => PadControl::RightStickUp,
        Output::Stick {
            right: true,
            x: false,
            positive: false,
        } => PadControl::RightStickDown,
        Output::Stick {
            right: true,
            x: true,
            positive: false,
        } => PadControl::RightStickLeft,
        Output::Stick {
            right: true,
            x: true,
            positive: true,
        } => PadControl::RightStickRight,
        Output::Button(_) | Output::Walk | Output::ToggleVehicle | Output::ReleaseCursor => {
            return None;
        }
    })
}

/// Priority score for choosing the primary on-screen keycap when multiple keys
/// bind the same [`PadControl`] (lower score wins). Prefers gameplay PC keys
/// over menu/arrow/scroll-wheel duplicates.
fn prompt_key_priority(control: PadControl, vk: u8) -> (u8, u8) {
    let tier = match (control, vk) {
        (PadControl::Menu, VK_ESCAPE) => 0,
        (PadControl::A, VK_RETURN) | (PadControl::B, VK_BACK) => 50,
        (PadControl::View, b'M') => 40,
        (PadControl::RightStick, VK_MBUTTON) => 40,
        (_, WHEEL_UP | WHEEL_DOWN) => 60,
        (_, VK_LEFT | VK_UP | VK_RIGHT | VK_DOWN) => 30,
        (_, VK_LBUTTON | VK_RBUTTON | VK_XBUTTON1 | VK_XBUTTON2) => 15,
        (_, VK_LSHIFT | VK_LCONTROL | VK_LMENU | VK_SPACE | VK_TAB) => 10,
        _ => 10,
    };
    (tier, vk)
}

impl Settings {
    /// Builds [`PromptLabels`] from this configuration.
    pub(super) fn prompt_labels(&self) -> PromptLabels {
        if !self.enabled {
            return PromptLabels::legacy();
        }
        let mut candidates: [Vec<u8>; PadControl::ALL.len()] = std::array::from_fn(|_| Vec::new());
        for (&vk, outputs) in &self.on_foot.0 {
            if key_label(vk).is_none() {
                continue;
            }
            for &out in outputs {
                if let Some(control) = output_to_pad_control(out) {
                    candidates[control.index()].push(vk);
                }
            }
        }
        let has_right_stick_keys = [
            PadControl::RightStickUp,
            PadControl::RightStickDown,
            PadControl::RightStickLeft,
            PadControl::RightStickRight,
        ]
        .into_iter()
        .any(|c| !candidates[c.index()].is_empty());

        PromptLabels::from_bindings(
            |control| {
                candidates[control.index()]
                    .iter()
                    .copied()
                    .min_by_key(|&vk| prompt_key_priority(control, vk))
            },
            !has_right_stick_keys,
        )
    }
}

/// Mouse motion and wheel steps since they were last drained, added from
/// the host's raw input thread (only while the cursor is captured).
#[derive(Default)]
pub(super) struct Counts {
    x: AtomicI64,
    y: AtomicI64,
    wheel: AtomicI32,
}

impl Counts {
    pub(super) fn add(&self, dx: i64, dy: i64) {
        self.x.fetch_add(dx, Ordering::Relaxed);
        self.y.fetch_add(dy, Ordering::Relaxed);
    }
    /// Steps away from the user are positive, as Windows reports them.
    pub(super) fn add_wheel(&self, steps: i32) {
        self.wheel.fetch_add(steps, Ordering::Relaxed);
    }
    fn drain(&self) -> (i64, i64) {
        (
            self.x.swap(0, Ordering::Relaxed),
            self.y.swap(0, Ordering::Relaxed),
        )
    }
    fn drain_wheel(&self) -> i32 {
        self.wheel.swap(0, Ordering::Relaxed)
    }
}

/// Which device moved a stick last, or was used last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Owner {
    Pad,
    Kbm,
}

/// What a set of held keys does.
#[derive(Clone, Copy, Default, PartialEq)]
struct Derived {
    buttons: u32,
    left_trigger: bool,
    right_trigger: bool,
    left: (f32, f32),
    right: (f32, f32),
    left_moved: bool,
    right_moved: bool,
}

/// A wheel step is a press the title has read once, then a release it has
/// read once, so steps survive any frame rate.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pulse {
    Idle,
    Down(u8),
    Gap,
}

/// The mouse as a right stick.
#[derive(Default)]
struct Look {
    /// Counts not yet turned (y as Windows reports it: down positive).
    pending: (f64, f64),
    last_take: Option<u64>,
    value: (f32, f32),
    moved: bool,
}

impl Look {
    fn take(&mut self, now: u64, (dx, dy): (i64, i64), s: &Settings) -> (f32, f32) {
        self.pending.0 += dx as f64;
        self.pending.1 += dy as f64;
        if let Some(last) = self.last_take
            && now.saturating_sub(last) < MIN_TAKE_US
        {
            return self.value;
        }
        let span = self
            .last_take
            .map_or(FIRST_TAKE_US, |last| now.saturating_sub(last).min(MAX_TAKE_US));
        self.last_take = Some(now);
        let dt = span as f64 / 1e6;
        let full = FULL_RATE / f64::from(s.sensitivity);
        let mut ux = self.pending.0 / dt / full;
        let mut uy = -self.pending.1 * f64::from(s.y_scale) / dt / full;
        let mut m = ux.hypot(uy);
        self.moved = m > 0.0;
        if m <= 1.0 {
            self.pending = (0.0, 0.0);
        } else {
            // Full stick in the direction moved; the rest waits (capped).
            let keep = if s.carry { 1.0 - 1.0 / m } else { 0.0 };
            self.pending.0 *= keep;
            self.pending.1 *= keep;
            let cap = CARRY_S * full;
            let left = self.pending.0.hypot(self.pending.1);
            if left > cap {
                self.pending.0 *= cap / left;
                self.pending.1 *= cap / left;
            }
            ux /= m;
            uy /= m;
            m = 1.0;
        }
        if m > 0.0 {
            let d = f64::from(s.deadzone);
            let shaped = d + (1.0 - d) * m.powf(1.0 / f64::from(s.curve));
            ux *= shaped / m;
            uy *= shaped / m;
        }
        if s.invert_y {
            uy = -uy;
        }
        self.value = (ux as f32, uy as f32);
        self.value
    }
}

/// Keyboard and mouse state between host polls and title reads.
pub(super) struct Controls {
    pub(super) s: Settings,
    vehicle: bool,
    held: Vec<u8>,
    derived: Derived,
    /// Buttons and triggers pressed since the title's last read: they stay
    /// down until it has read them, so a tap shorter than a frame counts.
    latched: u32,
    latched_triggers: (bool, bool),
    wheel: VecDeque<u8>,
    pulse: Pulse,
    look: Look,
    left_owner: Owner,
    right_owner: Owner,
    last_used: Option<Owner>,
    /// The release key was pressed: the host frees or captures the cursor.
    pub(super) toggle_capture: bool,
}

impl Controls {
    pub(super) fn new(s: Settings) -> Controls {
        Controls {
            s,
            vehicle: false,
            held: Vec::new(),
            derived: Derived::default(),
            latched: 0,
            latched_triggers: (false, false),
            wheel: VecDeque::new(),
            pulse: Pulse::Idle,
            look: Look::default(),
            left_owner: Owner::Kbm,
            right_owner: Owner::Kbm,
            last_used: None,
            toggle_capture: false,
        }
    }

    fn layout(&self) -> &Layout {
        if self.vehicle {
            &self.s.vehicle
        } else {
            &self.s.on_foot
        }
    }

    #[cfg(test)]
    fn in_vehicle(&self) -> bool {
        self.vehicle
    }

    fn derive(&self, keys: impl Iterator<Item = u8>) -> Derived {
        let mut d = Derived::default();
        let (mut l, mut r) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
        let mut walk = false;
        for vk in keys {
            for out in self.layout().outputs(vk) {
                match *out {
                    Output::Button(b) => d.buttons |= b,
                    Output::LeftTrigger => d.left_trigger = true,
                    Output::RightTrigger => d.right_trigger = true,
                    Output::Stick { right, x, positive } => {
                        let stick = if right { &mut r } else { &mut l };
                        let axis = if x { &mut stick.0 } else { &mut stick.1 };
                        *axis += if positive { 1.0 } else { -1.0 };
                        if right {
                            d.right_moved = true;
                        } else {
                            d.left_moved = true;
                        }
                    }
                    Output::Walk => walk = true,
                    Output::ToggleVehicle | Output::ReleaseCursor => {}
                }
            }
        }
        let round = |(x, y): (f32, f32)| {
            let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));
            let m = x.hypot(y);
            if m > 1.0 { (x / m, y / m) } else { (x, y) }
        };
        let speed = if walk { self.s.walk_speed } else { 1.0 };
        let (lx, ly) = round(l);
        Derived {
            left: (lx * speed, ly * speed),
            right: round(r),
            ..d
        }
    }

    /// Drains any live edits queued from the in-game `F2` controls overlay and
    /// publishes the updated snapshot back to `victory-gpu`.
    pub(super) fn poll_overlay(&mut self) {
        if !controls_ui::has_commands() {
            return;
        }
        let cmds = controls_ui::take_commands();
        let mut last_note = String::new();
        for cmd in cmds {
            last_note = self.apply_overlay_command(cmd, true);
        }
        controls_ui::publish(build_snapshot(&self.s, self.vehicle, last_note));
    }

    /// Applies one `F2` controls overlay command live and optionally auto-saves
    /// to `victory.toml`. Returns the status note to show in the overlay.
    fn apply_overlay_command(&mut self, cmd: OverlayCommand, persist: bool) -> String {
        let mut should_save = false;
        let mut note = String::new();
        match cmd {
            OverlayCommand::SetVehicle(v) => {
                self.vehicle = v;
                self.derived = self.derive(self.held.iter().copied());
                tracing::info!(vehicle = self.vehicle, "controls layout (overlay)");
                note = if v {
                    "SWITCHED TO VEHICLE LAYOUT (LIVE)".to_owned()
                } else {
                    "SWITCHED TO ON-FOOT LAYOUT (LIVE)".to_owned()
                };
            }
            OverlayCommand::SetShowBadge(show) => {
                self.s.show_badge = show;
                should_save = true;
            }
            OverlayCommand::StepSensitivity(dir) => {
                let next = (self.s.sensitivity + f32::from(dir) * 0.05).clamp(0.10, 5.0);
                self.s.sensitivity = (next * 100.0).round() / 100.0;
                should_save = true;
            }
            OverlayCommand::StepYScale(dir) => {
                let next = (self.s.y_scale + f32::from(dir) * 0.05).clamp(0.25, 3.0);
                self.s.y_scale = (next * 100.0).round() / 100.0;
                should_save = true;
            }
            OverlayCommand::StepDeadzone(dir) => {
                let next = (self.s.deadzone + f32::from(dir) * 0.05).clamp(0.0, 0.60);
                self.s.deadzone = (next * 100.0).round() / 100.0;
                should_save = true;
            }
            OverlayCommand::StepCurve(dir) => {
                let next = (self.s.curve + f32::from(dir) * 0.10).clamp(0.50, 3.0);
                self.s.curve = (next * 100.0).round() / 100.0;
                should_save = true;
            }
            OverlayCommand::ToggleInvertY => {
                self.s.invert_y = !self.s.invert_y;
                should_save = true;
            }
            OverlayCommand::Rebind {
                vehicle,
                output_id,
                vk,
            } => {
                if let Some(target) = output(output_id) {
                    let layout = if vehicle {
                        &mut self.s.vehicle
                    } else {
                        &mut self.s.on_foot
                    };
                    // Remove this output from previously bound keys in the layout.
                    layout.0.retain(|_, outs| {
                        outs.retain(|&o| o != target);
                        !outs.is_empty()
                    });
                    // Bind the chosen key directly to this output.
                    layout.0.insert(vk, vec![target]);
                    self.derived = self.derive(self.held.iter().copied());
                    note = format!(
                        "BOUND {} TO {} (LIVE)",
                        output_id.to_ascii_uppercase(),
                        key_name(vk).to_ascii_uppercase()
                    );
                    should_save = true;
                }
            }
            OverlayCommand::Unbind { vehicle, output_id } => {
                if let Some(target) = output(output_id) {
                    let layout = if vehicle {
                        &mut self.s.vehicle
                    } else {
                        &mut self.s.on_foot
                    };
                    layout.0.retain(|_, outs| {
                        outs.retain(|&o| o != target);
                        !outs.is_empty()
                    });
                    self.derived = self.derive(self.held.iter().copied());
                    note = format!("CLEARED {} (LIVE)", output_id.to_ascii_uppercase());
                    should_save = true;
                }
            }
            OverlayCommand::ResetDefaults => {
                let prompts = self.s.prompts;
                let mut reset = Settings::default();
                reset.prompts = prompts;
                // Keep F = "Y" only on foot so on-foot Y interactions don't toggle vehicle mode.
                let _ = reset.on_foot.bind("F", &["Y"]);
                self.s = reset;
                self.vehicle = false;
                self.derived = self.derive(self.held.iter().copied());
                note = "RESET TO PC DEFAULTS (LIVE)".to_owned();
                should_save = true;
            }
            OverlayCommand::SaveNow => {
                should_save = true;
            }
        }
        if should_save && persist {
            let path = CONFIG_PATH
                .get()
                .cloned()
                .unwrap_or_else(|| PathBuf::from("victory.toml"));
            match save_settings_to(&self.s, &path) {
                Ok(()) => {
                    tracing::info!(path = %path.display(), "controls: saved to config");
                    if note.is_empty() {
                        note = "SAVED TO VICTORY.TOML (LIVE)".to_owned();
                    } else {
                        note.push_str(" - SAVED");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "controls: could not save config");
                    note = "APPLIED LIVE (COULD NOT SAVE VICTORY.TOML)".to_owned();
                }
            }
        }
        note
    }

    /// A host poll: the keys and mouse buttons held now, wheel steps since
    /// the last poll.
    pub(super) fn feed(&mut self, held: &[u8], counts: &Counts) {
        let steps = counts.drain_wheel();
        let key = if steps > 0 { WHEEL_UP } else { WHEEL_DOWN };
        for _ in 0..steps.unsigned_abs() {
            if self.wheel.len() < WHEEL_QUEUE {
                self.wheel.push_back(key);
            }
        }
        let pressed: Vec<u8> = held
            .iter()
            .copied()
            .filter(|vk| !self.held.contains(vk))
            .collect();
        let mut mode_changed = false;
        for &vk in &pressed {
            let outs = self.layout().outputs(vk).to_vec();
            if outs.contains(&Output::ToggleVehicle) {
                self.vehicle = !self.vehicle;
                mode_changed = true;
                tracing::info!(vehicle = self.vehicle, key = %key_name(vk), "controls layout");
            }
            if outs.contains(&Output::ReleaseCursor) {
                self.toggle_capture = true;
            }
        }
        if mode_changed {
            controls_ui::publish(build_snapshot(&self.s, self.vehicle, String::new()));
        }
        let before = self.derived;
        self.held = held.to_vec();
        self.derived = self.derive(self.held.iter().copied());
        self.latched |= self.derived.buttons & !before.buttons;
        self.latched_triggers.0 |= self.derived.left_trigger && !before.left_trigger;
        self.latched_triggers.1 |= self.derived.right_trigger && !before.right_trigger;
        if self.pulse == Pulse::Idle
            && let Some(key) = self.wheel.pop_front()
        {
            self.pulse = Pulse::Down(key);
        }
    }

    /// The keyboard and mouse pad as it is now (a host poll's reading).
    fn kbm(&self) -> PadState {
        let mut d = self.derived;
        if let Pulse::Down(key) = self.pulse {
            let p = self.derive(std::iter::once(key));
            d.buttons |= p.buttons;
            d.left_trigger |= p.left_trigger;
            d.right_trigger |= p.right_trigger;
        }
        let on = |b: bool| if b { 1.0 } else { 0.0 };
        let (rx, ry) = (
            d.right.0 + self.look.value.0,
            d.right.1 + self.look.value.1,
        );
        let m = rx.hypot(ry);
        let (rx, ry) = if m > 1.0 { (rx / m, ry / m) } else { (rx, ry) };
        PadState {
            buttons: d.buttons | self.latched,
            left_trigger: on(d.left_trigger || self.latched_triggers.0),
            right_trigger: on(d.right_trigger || self.latched_triggers.1),
            left_x: d.left.0,
            left_y: d.left.1,
            right_x: rx,
            right_y: ry,
        }
    }

    /// The pad a host poll publishes: `pad` (a host pad, if any) with the
    /// keyboard and mouse on it.
    pub(super) fn peek(&mut self, pad: Option<PadState>) -> PadState {
        self.merge(pad)
    }

    /// The pad the title reads now: the mouse counts since its last read
    /// become the right stick, and what it has now seen is let go.
    pub(super) fn take(&mut self, now: u64, pad: Option<PadState>, counts: &Counts) -> PadState {
        let before = self.look.last_take;
        self.look.take(now, counts.drain(), &self.s);
        let state = self.merge(pad);
        if self.look.last_take != before {
            self.latched = 0;
            self.latched_triggers = (false, false);
            self.pulse = match self.pulse {
                Pulse::Down(_) => Pulse::Gap,
                Pulse::Gap | Pulse::Idle => Pulse::Idle,
            };
        }
        state
    }

    fn merge(&mut self, pad: Option<PadState>) -> PadState {
        let kbm = self.kbm();
        let kbm_used = !self.held.is_empty() || self.look.moved || self.pulse != Pulse::Idle;
        let Some(pad) = pad else {
            self.used(kbm_used.then_some(Owner::Kbm));
            return kbm;
        };
        let pushed = |x: f32, y: f32| x.hypot(y) > PAD_TAKES_OVER;
        let pad_left = pushed(pad.left_x, pad.left_y);
        let pad_right = pushed(pad.right_x, pad.right_y);
        if self.derived.left_moved {
            self.left_owner = Owner::Kbm;
        } else if pad_left {
            self.left_owner = Owner::Pad;
        }
        if self.derived.right_moved || self.look.moved {
            self.right_owner = Owner::Kbm;
        } else if pad_right {
            self.right_owner = Owner::Pad;
        }
        let pad_used = pad.buttons != 0
            || pad_left
            || pad_right
            || pad.left_trigger > 0.1
            || pad.right_trigger > 0.1;
        self.used(if kbm_used {
            Some(Owner::Kbm)
        } else {
            pad_used.then_some(Owner::Pad)
        });
        let (lx, ly) = match self.left_owner {
            Owner::Kbm => (kbm.left_x, kbm.left_y),
            Owner::Pad => (pad.left_x, pad.left_y),
        };
        let (rx, ry) = match self.right_owner {
            Owner::Kbm => (kbm.right_x, kbm.right_y),
            Owner::Pad => (pad.right_x, pad.right_y),
        };
        PadState {
            buttons: pad.buttons | kbm.buttons,
            left_trigger: pad.left_trigger.max(kbm.left_trigger),
            right_trigger: pad.right_trigger.max(kbm.right_trigger),
            left_x: lx,
            left_y: ly,
            right_x: rx,
            right_y: ry,
        }
    }

    fn used(&mut self, now: Option<Owner>) {
        if let Some(owner) = now
            && self.last_used != Some(owner)
        {
            self.last_used = Some(owner);
            tracing::info!(
                device = match owner {
                    Owner::Pad => "gamepad",
                    Owner::Kbm => "keyboard and mouse",
                },
                "input: now using"
            );
        }
    }

    /// The device used last (for prompts, once they can change).
    #[allow(dead_code)]
    pub(super) fn last_used(&self) -> Option<Owner> {
        self.last_used
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vk(name: &str) -> u8 {
        key_code(name).unwrap()
    }

    fn linear() -> Settings {
        Settings {
            deadzone: 0.0,
            curve: 1.0,
            ..Settings::default()
        }
    }

    fn controls() -> Controls {
        Controls::new(linear())
    }

    fn held(c: &mut Controls, keys: &[&str]) -> PadState {
        let keys: Vec<u8> = keys.iter().map(|k| vk(k)).collect();
        c.feed(&keys, &Counts::default());
        c.peek(None)
    }

    #[test]
    fn key_names_round_trip() {
        for (name, code) in [
            ("w", b'W'),
            ("7", b'7'),
            ("Space", 0x20),
            ("lshift", 0xA0),
            ("F1", 0x70),
            ("F24", 0x87),
            ("Num5", 0x65),
            ("MouseLeft", 0x01),
            ("WheelDown", WHEEL_DOWN),
            ("Alt", 0xA4),
        ] {
            assert_eq!(key_code(name), Some(code), "{name}");
        }
        assert_eq!(key_code("F0"), None);
        assert_eq!(key_code("F25"), None);
        assert_eq!(key_code("Num10"), None);
        assert_eq!(key_code("Banana"), None);
        assert_eq!(key_name(0x70), "F1");
        assert_eq!(key_name(0xA0), "LShift");
        assert_eq!(key_name(b'Q'), "Q");
    }

    #[test]
    fn letters_are_named_by_their_place_on_a_us_keyboard() {
        for (scan, key) in [
            (0x11, b'W'),
            (0x1E, b'A'),
            (0x1F, b'S'),
            (0x20, b'D'),
            (0x10, b'Q'),
            (0x2C, b'Z'),
            (0x32, b'M'),
            (0x02, b'1'),
            (0x0B, b'0'),
            (0x29, 0xC0),
            (0x2B, 0xDC),
            (0x35, 0xBF),
        ] {
            assert_eq!(us_key(scan), Some(key), "{scan:#x}");
        }
        // Esc, Tab, Enter, Left Ctrl, Space, Left Shift, the extended keys.
        for scan in [0x01, 0x0F, 0x1C, 0x1D, 0x39, 0x2A, 0xE035, 0xE048] {
            assert_eq!(us_key(scan), None, "{scan:#x}");
        }
        let off: toml::Table = toml::from_str("keys_by_position = false").unwrap();
        assert!(Settings::default().by_position);
        assert!(!Settings::parse(&off).unwrap().by_position);
    }

    #[test]
    fn wasd_moves_the_left_stick_round_and_walk_slows_it() {
        let mut c = controls();
        let p = held(&mut c, &["W"]);
        assert_eq!((p.left_x, p.left_y), (0.0, 1.0));
        let p = held(&mut c, &["W", "D"]);
        let r = std::f32::consts::FRAC_1_SQRT_2;
        assert!((p.left_x - r).abs() < 1e-6 && (p.left_y - r).abs() < 1e-6);
        let p = held(&mut c, &["A", "D"]);
        assert_eq!((p.left_x, p.left_y), (0.0, 0.0));
        let p = held(&mut c, &["S", "LAlt"]);
        assert_eq!((p.left_x, p.left_y), (0.0, -0.5));
    }

    #[test]
    fn keys_and_mouse_buttons_press_pad_buttons_and_triggers() {
        let mut c = controls();
        let p = held(&mut c, &["Space", "LShift", "MouseLeft", "MouseRight", "Tab"]);
        assert_eq!(p.buttons, X | A | LEFT_SHOULDER);
        assert_eq!((p.left_trigger, p.right_trigger), (1.0, 1.0));
        let p = held(&mut c, &["Esc"]);
        assert!(p.buttons & MENU != 0);
        assert!(c.toggle_capture, "Esc frees the cursor");
    }

    #[test]
    fn f_switches_to_the_vehicle_layout_and_g_puts_it_right() {
        let mut c = controls();
        held(&mut c, &["F"]);
        assert!(c.in_vehicle());
        held(&mut c, &[]);
        let p = held(&mut c, &["W", "A"]);
        assert_eq!(p.right_trigger, 1.0, "W is throttle");
        assert_eq!((p.left_x, p.left_y), (-1.0, 0.0), "A steers, W does not move the stick");
        // Holding F does not switch again.
        held(&mut c, &["F"]);
        held(&mut c, &["F"]);
        assert!(!c.in_vehicle());
        held(&mut c, &[]);
        held(&mut c, &["G"]);
        assert!(c.in_vehicle());
    }

    #[test]
    fn a_tap_between_two_reads_is_read_once() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        c.feed(&[vk("Space")], &counts);
        c.feed(&[], &counts);
        assert_eq!(c.peek(None).buttons, X, "held for the title until it reads");
        assert_eq!(c.take(10_000, None, &counts).buttons, X);
        assert_eq!(c.take(20_000, None, &counts).buttons, 0);
        // A quick click shoots once.
        c.feed(&[vk("MouseLeft")], &counts);
        c.feed(&[], &counts);
        assert_eq!(c.take(30_000, None, &counts).right_trigger, 1.0);
        assert_eq!(c.take(40_000, None, &counts).right_trigger, 0.0);
    }

    #[test]
    fn wheel_steps_are_each_a_press_the_title_reads() {
        let mut c = controls();
        let counts = Counts::default();
        counts.add_wheel(2);
        let mut seen = Vec::new();
        for i in 0..6 {
            c.feed(&[], &counts);
            seen.push(c.take(i * 10_000, None, &counts).buttons);
        }
        assert_eq!(seen, [LEFT, 0, LEFT, 0, 0, 0]);
        counts.add_wheel(-1);
        c.feed(&[], &counts);
        assert_eq!(c.take(100_000, None, &counts).buttons, RIGHT);
    }

    #[test]
    fn mouse_counts_become_a_rate_at_each_read() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        // 2,000 counts/s = full stick at sensitivity 1: 10 counts in 10 ms is half.
        counts.add(10, 0);
        let p = c.take(10_000, None, &counts);
        assert!((p.right_x - 0.5).abs() < 1e-6, "{}", p.right_x);
        // The same distance over a 4x longer frame is a 4x smaller rate: the
        // title turns rate x frame time, so the same angle.
        counts.add(10, 0);
        let p = c.take(50_000, None, &counts);
        assert!((p.right_x - 0.125).abs() < 1e-6, "{}", p.right_x);
        // Mouse up looks up; no motion lets go.
        counts.add(0, -5);
        let p = c.take(60_000, None, &counts);
        assert!((p.right_y - 0.25).abs() < 1e-6 && p.right_x == 0.0);
        assert_eq!(c.take(70_000, None, &counts).right_y, 0.0);
    }

    #[test]
    fn slow_frames_get_the_rate_over_their_whole_span() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        // 1,000 counts over an 800 ms frame: half a stick, not saturated.
        counts.add(1000, 0);
        let p = c.take(800_000, None, &counts);
        assert!((p.right_x - 0.625).abs() < 1e-6, "{}", p.right_x);
        // Past the longest span the title counts as paused.
        counts.add(1000, 0);
        let p = c.take(3_800_000, None, &counts);
        assert!((p.right_x - 0.5).abs() < 1e-6, "{}", p.right_x);
    }

    #[test]
    fn reads_close_together_see_the_same_mouse_stick() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        counts.add(10, 0);
        let a = c.take(10_000, None, &counts);
        let b = c.take(10_500, None, &counts);
        assert_eq!(a, b);
        counts.add(4, 0);
        let p = c.take(11_000, None, &counts);
        assert_eq!(p, a, "counts wait for the next read");
        let p = c.take(20_000, None, &counts);
        assert!((p.right_x - 0.2).abs() < 1e-6, "{}", p.right_x);
    }

    #[test]
    fn dead_zone_curve_and_inversion_shape_the_mouse_stick() {
        let s = Settings {
            deadzone: 0.2,
            curve: 2.0,
            invert_y: true,
            ..Settings::default()
        };
        let mut c = Controls::new(s);
        let counts = Counts::default();
        c.take(0, None, &counts);
        counts.add(0, -5); // a quarter stick up
        let p = c.take(10_000, None, &counts);
        // 0.2 + 0.8 * sqrt(0.25) = 0.6, then inverted.
        assert!((p.right_y + 0.6).abs() < 1e-6, "{}", p.right_y);
        // One count still clears the dead zone.
        counts.add(1, 0);
        let p = c.take(20_000, None, &counts);
        assert!(p.right_x > 0.2);
    }

    #[test]
    fn a_fast_flick_is_turned_in_full_later() {
        let mut c = controls();
        let counts = Counts::default();
        c.take(0, None, &counts);
        counts.add(50, 0); // 2.5 full sticks in 10 ms
        let xs: Vec<f32> = (1..=4)
            .map(|i| c.take(i * 10_000, None, &counts).right_x)
            .collect();
        assert_eq!(&xs[..2], &[1.0, 1.0]);
        assert!((xs[2] - 0.5).abs() < 1e-6, "{xs:?}");
        assert_eq!(xs[3], 0.0);
        let mut c = Controls::new(Settings {
            carry: false,
            ..linear()
        });
        c.take(0, None, &counts);
        counts.add(50, 0);
        assert_eq!(c.take(10_000, None, &counts).right_x, 1.0);
        assert_eq!(c.take(20_000, None, &counts).right_x, 0.0);
    }

    #[test]
    fn a_resting_pad_never_overrides_the_mouse_and_a_pushed_one_does() {
        let mut c = controls();
        let counts = Counts::default();
        let drift = PadState {
            right_x: 0.1,
            left_y: -0.05,
            buttons: B,
            right_trigger: 0.4,
            ..Default::default()
        };
        c.take(0, Some(drift), &counts);
        counts.add(10, 0);
        c.feed(&[vk("W"), vk("MouseLeft")], &counts);
        let p = c.take(10_000, Some(drift), &counts);
        assert!((p.right_x - 0.5).abs() < 1e-6);
        assert_eq!((p.left_y, p.buttons, p.right_trigger), (1.0, B, 1.0));
        c.feed(&[], &counts);
        assert_eq!(c.take(20_000, Some(drift), &counts).right_x, 0.0, "drift stays out");
        let pushed = PadState {
            right_x: 0.7,
            left_x: -0.9,
            ..Default::default()
        };
        let p = c.take(30_000, Some(pushed), &counts);
        assert_eq!((p.right_x, p.left_x), (0.7, -0.9));
        assert_eq!(c.last_used(), Some(Owner::Pad));
    }

    #[test]
    fn config_rebinds_unbinds_and_refuses_typos() {
        let table: toml::Table = toml::from_str(
            r#"
            mouse_sensitivity = 2.5
            mouse_invert_y = true
            stick_deadzone = 0.24
            [on_foot]
            Space = "A"
            LShift = ""
            Mouse4 = ["LB", "RB"]
            [vehicle]
            W = "RightTrigger_typo"
            "#,
        )
        .unwrap();
        let e = Settings::parse(&table).unwrap_err();
        assert!(e.contains("RightTrigger_typo"), "{e}");
        let mut table = table;
        table.remove("vehicle");
        let s = Settings::parse(&table).unwrap();
        assert_eq!((s.sensitivity, s.invert_y, s.deadzone), (2.5, true, 0.24));
        assert_eq!(s.on_foot.outputs(0x20), &[Output::Button(A)]);
        assert!(s.on_foot.outputs(0xA0).is_empty());
        assert_eq!(
            s.on_foot.outputs(0x05),
            &[Output::Button(LEFT_SHOULDER), Output::Button(RIGHT_SHOULDER)]
        );
        assert_eq!(s.on_foot.outputs(b'W'), Settings::default().on_foot.outputs(b'W'));
        for bad in ["mouse_sensitivty = 1.0", "stick_deadzone = 1.5", "[on_foot]\nBanana = \"A\""] {
            let t: toml::Table = toml::from_str(bad).unwrap();
            assert!(Settings::parse(&t).is_err(), "{bad}");
        }
        let off: toml::Table = toml::from_str("enabled = false").unwrap();
        assert!(!Settings::parse(&off).unwrap().enabled);
        let keys: toml::Table = toml::from_str("prompts = \"keys\"").unwrap();
        assert_eq!(Settings::parse(&keys).unwrap().prompts, PromptMode::Keys);
        let bad_prompts: toml::Table = toml::from_str("prompts = \"invalid\"").unwrap();
        assert!(Settings::parse(&bad_prompts).is_err());
    }

    #[test]
    fn prompt_labels_match_pc_defaults_and_reflect_rebinds() {
        let labels = Settings::default().prompt_labels();
        assert!(labels.right_stick_is_mouse);
        assert_eq!(labels.label(PadControl::A), Some("Shift"));
        assert_eq!(labels.label(PadControl::B), Some("R"));
        assert_eq!(labels.label(PadControl::X), Some("Spc"));
        assert_eq!(labels.label(PadControl::Y), Some("F"));
        assert_eq!(labels.label(PadControl::LeftShoulder), Some("Tab"));
        assert_eq!(labels.label(PadControl::RightShoulder), Some("Q"));
        assert_eq!(labels.label(PadControl::LeftTrigger), Some("RMB"));
        assert_eq!(labels.label(PadControl::RightTrigger), Some("LMB"));
        assert_eq!(labels.label(PadControl::LeftStick), Some("Ctrl"));
        assert_eq!(labels.label(PadControl::RightStick), Some("C"));
        assert_eq!(labels.label(PadControl::Right), Some("E"));
        assert_eq!(labels.label(PadControl::Up), Some("\u{2191}"));
        assert_eq!(labels.label(PadControl::Down), Some("\u{2193}"));
        assert_eq!(labels.label(PadControl::Left), Some("\u{2190}"));
        assert_eq!(labels.label(PadControl::View), Some("V"));
        assert_eq!(labels.label(PadControl::Menu), Some("Esc"));
        assert_eq!(labels.label(PadControl::LeftStickUp), Some("W"));
        assert_eq!(labels.label(PadControl::LeftStickLeft), Some("A"));
        assert_eq!(labels.label(PadControl::LeftStickDown), Some("S"));
        assert_eq!(labels.label(PadControl::LeftStickRight), Some("D"));

        // Disabled `[controls]` falls back to legacy labels.
        let legacy: toml::Table = toml::from_str("enabled = false").unwrap();
        assert_eq!(
            Settings::parse(&legacy).unwrap().prompt_labels(),
            PromptLabels::legacy()
        );

        // Custom `[controls.on_foot]` rebinds update prompt labels, and binding
        // right-stick keys switches right-stick prompts off mouse mode.
        let rebound: toml::Table = toml::from_str(
            r#"
            [on_foot]
            Q = ""
            Mouse4 = "RB"
            R = ""
            T = "B"
            H = "DpadUp"
            I = "RightUp"
            K = "RightDown"
            J = "RightLeft"
            L = "RightRight"
            "#,
        )
        .unwrap();
        let custom = Settings::parse(&rebound).unwrap().prompt_labels();
        assert!(!custom.right_stick_is_mouse);
        assert_eq!(custom.label(PadControl::RightShoulder), Some("M4"));
        assert_eq!(custom.label(PadControl::B), Some("T"));
        assert_eq!(custom.label(PadControl::Up), Some("H"));
        assert_eq!(custom.label(PadControl::RightStickUp), Some("I"));
        assert_eq!(custom.label(PadControl::RightStickDown), Some("K"));
        assert_eq!(custom.label(PadControl::RightStickLeft), Some("J"));
        assert_eq!(custom.label(PadControl::RightStickRight), Some("L"));
    }

    #[test]
    fn overlay_commands_rebind_tune_and_save_victory_toml() {
        let mut c = Controls::new(Settings::default());
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("victory.toml");
        std::fs::write(
            &cfg_path,
            "[paths]\ngame_dir = 'D:\\game'\ncache_dir = 'D:\\cache'\n",
        )
        .unwrap();

        c.apply_overlay_command(OverlayCommand::SetVehicle(true), false);
        assert!(c.in_vehicle());
        c.apply_overlay_command(OverlayCommand::StepSensitivity(4), false);
        assert!((c.s.sensitivity - 1.2).abs() < 1e-5);
        c.apply_overlay_command(OverlayCommand::StepDeadzone(-2), false);
        assert!((c.s.deadzone - 0.2).abs() < 1e-5);
        c.apply_overlay_command(OverlayCommand::StepCurve(2), false);
        assert!((c.s.curve - 1.2).abs() < 1e-5);
        c.apply_overlay_command(OverlayCommand::ToggleInvertY, false);
        assert!(c.s.invert_y);
        c.apply_overlay_command(
            OverlayCommand::Rebind {
                vehicle: false,
                output_id: "X",
                vk: b'J',
            },
            false,
        );
        assert_eq!(c.s.on_foot.outputs(b'J'), &[Output::Button(X)]);
        assert!(c.s.on_foot.outputs(0x20).is_empty());

        c.apply_overlay_command(
            OverlayCommand::Unbind {
                vehicle: false,
                output_id: "RB",
            },
            false,
        );
        assert!(c.s.on_foot.outputs(b'Q').is_empty());

        save_settings_to(&c.s, &cfg_path).unwrap();
        let text = std::fs::read_to_string(&cfg_path).unwrap();
        let parsed: toml::Table = toml::from_str(&text).unwrap();
        // Existing `[paths]` table is preserved.
        assert_eq!(
            parsed["paths"]["game_dir"].as_str(),
            Some("D:\\game")
        );
        // Saved `[controls]` round-trips cleanly through `Settings::parse`.
        let reloaded = Settings::parse(parsed["controls"].as_table().unwrap()).unwrap();
        assert!((reloaded.sensitivity - 1.2).abs() < 1e-5);
        assert!((reloaded.deadzone - 0.2).abs() < 1e-5);
        assert!((reloaded.curve - 1.2).abs() < 1e-5);
        assert!(reloaded.invert_y);
        assert_eq!(reloaded.on_foot.outputs(b'J'), &[Output::Button(X)]);
        assert!(reloaded.on_foot.outputs(0x20).is_empty());
        assert!(reloaded.on_foot.outputs(b'Q').is_empty());
    }
}
