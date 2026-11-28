//! Shared state bridge between the input layer (`victory-hle::modules::controls`)
//! and the in-game `F2` controls overlay + mode badge (`victory-gpu::host::controls_menu`).
//!
//! Pure data and thread-safe queues so `victory-gpu` stays decoupled from
//! `victory-hle` and `toml`/`serde`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// One row in the on-screen controls layout table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingEntry {
    /// Internal output identifier (matches `[controls]` binding names, e.g. `"A"`, `"LeftUp"`, `"RT"`).
    pub id: &'static str,
    /// Uppercase ASCII description shown on the left side of the row.
    pub label: &'static str,
    /// Uppercase ASCII summary of currently bound keys (e.g. `"LSHIFT, ENTER"` or `"UNBOUND"`).
    pub keys: String,
}

/// Snapshot published by `victory-hle::modules::controls` for the GPU overlay.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub enabled: bool,
    pub vehicle: bool,
    pub show_badge: bool,
    pub sensitivity: f32,
    pub y_scale: f32,
    pub deadzone: f32,
    pub curve: f32,
    pub invert_y: bool,
    pub on_foot: Vec<BindingEntry>,
    pub vehicle_binds: Vec<BindingEntry>,
    pub note: String,
    pub generation: u64,
}

/// An edit or toggle requested from the `F2` controls overlay, applied live by
/// `victory-hle::modules::controls` on its next poll tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    SetVehicle(bool),
    SetShowBadge(bool),
    StepSensitivity(i8),
    StepYScale(i8),
    StepDeadzone(i8),
    StepCurve(i8),
    ToggleInvertY,
    Rebind {
        vehicle: bool,
        output_id: &'static str,
        vk: u8,
    },
    Unbind {
        vehicle: bool,
        output_id: &'static str,
    },
    ResetDefaults,
    SaveNow,
}

static SNAPSHOT: OnceLock<Mutex<Option<Snapshot>>> = OnceLock::new();
static COMMANDS: OnceLock<Mutex<Vec<Command>>> = OnceLock::new();
static HAS_COMMANDS: AtomicBool = AtomicBool::new(false);

fn snapshot_lock() -> &'static Mutex<Option<Snapshot>> {
    SNAPSHOT.get_or_init(|| Mutex::new(None))
}

fn command_lock() -> &'static Mutex<Vec<Command>> {
    COMMANDS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Publishes the latest controls snapshot for the GPU overlay to display.
pub fn publish(snap: Snapshot) {
    *snapshot_lock().lock().unwrap_or_else(|e| e.into_inner()) = Some(snap);
}

/// Returns the latest controls snapshot, if `[controls]` has been initialised.
pub fn snapshot() -> Option<Snapshot> {
    snapshot_lock().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Queues a command from the `F2` controls overlay for `victory-hle` to apply live.
pub fn push(cmd: Command) {
    command_lock().lock().unwrap_or_else(|e| e.into_inner()).push(cmd);
    HAS_COMMANDS.store(true, Ordering::Release);
}

/// Fast-path check before locking the command queue on every input poll.
pub fn has_commands() -> bool {
    HAS_COMMANDS.load(Ordering::Acquire)
}

/// Drains all queued overlay commands (called by `victory-hle`).
pub fn take_commands() -> Vec<Command> {
    if !HAS_COMMANDS.swap(false, Ordering::AcqRel) {
        return Vec::new();
    }
    std::mem::take(&mut *command_lock().lock().unwrap_or_else(|e| e.into_inner()))
}
