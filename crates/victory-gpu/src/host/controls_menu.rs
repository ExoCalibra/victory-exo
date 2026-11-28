//! The F2 controls layout overlay and live `ON FOOT` / `VEHICLE` HUD badge.
//!
//! - While closed (and when `show_badge` is on): draws a compact 1-line badge
//!   at the top-center of the screen showing the active layout (`ON FOOT` or
//!   `VEHICLE`), the quick-toggle key (`G`), and `F2:CONTROLS`.
//! - Press `F2` to open/close the centered interactive controls menu: view
//!   every On-Foot and Vehicle binding, switch layouts live, tune mouse
//!   sensitivity / Y-scale / deadzone / curve / invert-Y live, press `Enter`
//!   on any action to rebind it to any key or mouse button, or `Del` to clear
//!   it. Every edit applies immediately and saves to `victory.toml`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_BACK, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_F2, VK_LEFT, VK_RETURN, VK_RIGHT,
    VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::controls_ui::{self, BindingEntry, Command, Snapshot};

use super::font::{colours, glyph};
use super::{Host, HostList, transition};

const GLYPH: (u32, u32) = (5, 7);
const CELL: (u32, u32) = (6, 9);
const COLUMNS: u32 = 62;
const ROWS: usize = 20;
/// Title + help + blank + ROWS + blank + note + footer.
const LINES: u32 = ROWS as u32 + 6;
const RIM: u32 = 4;
const UPLOADS: usize = 4;

/// Top quick-setting rows before the binding list.
const SETTINGS_COUNT: usize = 8;
/// Bottom action rows after the binding list.
const ACTIONS: [&str; 3] = ["SAVE TO VICTORY.TOML", "RESET DEFAULTS (LIVE)", "CLOSE"];

/// All bindable keys/mouse buttons polled while capturing a new binding.
/// Excludes `VK_ESCAPE` (handled as cancel) and `F1`/`F2`/`F3` (overlays).
const BINDABLE_VKS: &[u8] = &[
    // Mouse buttons
    0x01, 0x02, 0x04, 0x05, 0x06, // Backspace, Tab, Enter, CapsLock, Space
    0x08, 0x09, 0x0D, 0x14, 0x20, // PgUp, PgDn, End, Home, Arrows, Ins, Del
    0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2D, 0x2E, // 0..9
    b'0', b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', // A..Z
    b'A', b'B', b'C', b'D', b'E', b'F', b'G', b'H', b'I', b'J', b'K', b'L', b'M', b'N', b'O',
    b'P', b'Q', b'R', b'S', b'T', b'U', b'V', b'W', b'X', b'Y', b'Z', // Num0..Num9
    0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, // F4..F12
    0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x7B,
    // LShift, RShift, LCtrl, RCtrl, LAlt, RAlt
    0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, // Punctuation ; = , - . / ` [ \ ] '
    0xBA, 0xBB, 0xBC, 0xBD, 0xBE, 0xBF, 0xC0, 0xDB, 0xDC, 0xDD, 0xDE,
];

/// True while the overlay is waiting for the user to press a key/mouse button
/// to rebind the selected row.
static CAPTURING_BIND: AtomicBool = AtomicBool::new(false);

/// Key/mouse events delivered from the 10 ms watcher thread.
#[derive(Clone, Copy, Debug)]
enum KeyEvent {
    Nav(u16),
    Bind(u8),
    CancelBind,
}

fn events() -> &'static Mutex<Vec<KeyEvent>> {
    static EVENTS: OnceLock<Mutex<Vec<KeyEvent>>> = OnceLock::new();
    EVENTS.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("controls menu keys".into())
            .spawn(|| {
                const NAV: [u16; 10] = [
                    VK_F2, VK_UP, VK_DOWN, VK_LEFT, VK_RIGHT, VK_RETURN, VK_ESCAPE, VK_DELETE,
                    VK_BACK, VK_TAB,
                ];
                let mut was_nav = [false; NAV.len()];
                let mut was_bind = vec![false; BINDABLE_VKS.len()];
                let mut was_esc_bind = false;
                let mut prev_capturing = false;
                loop {
                    std::thread::sleep(Duration::from_millis(10));
                    let mut process = 0;
                    // SAFETY: plain Win32 query with valid output pointer.
                    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut process) };
                    let ours = process == std::process::id();
                    let capturing = CAPTURING_BIND.load(Ordering::Acquire);
                    if capturing {
                        if !prev_capturing {
                            // Snapshot currently-held keys on entry so Enter (or whatever
                            // started capture) must be released before a new press registers.
                            for (i, &vk) in BINDABLE_VKS.iter().enumerate() {
                                // SAFETY: plain Win32 query.
                                was_bind[i] = ours && unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;
                            }
                            // SAFETY: plain Win32 query.
                            was_esc_bind =
                                ours && unsafe { GetAsyncKeyState(i32::from(VK_ESCAPE)) } < 0;
                            prev_capturing = true;
                            continue;
                        }
                        // SAFETY: plain Win32 query.
                        let esc_down = ours && unsafe { GetAsyncKeyState(i32::from(VK_ESCAPE)) } < 0;
                        if esc_down && !was_esc_bind {
                            events()
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push(KeyEvent::CancelBind);
                            was_esc_bind = true;
                            continue;
                        }
                        was_esc_bind = esc_down;
                        for (i, &vk) in BINDABLE_VKS.iter().enumerate() {
                            // SAFETY: plain Win32 query.
                            let down = ours && unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;
                            if down && !was_bind[i] {
                                events()
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .push(KeyEvent::Bind(vk));
                            }
                            was_bind[i] = down;
                        }
                    } else {
                        if prev_capturing {
                            // Snapshot nav keys on exit so Esc/Enter from capture does not
                            // also fire as a nav press.
                            for (i, &vk) in NAV.iter().enumerate() {
                                // SAFETY: plain Win32 query.
                                was_nav[i] = ours && unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;
                            }
                            prev_capturing = false;
                            continue;
                        }
                        for (i, &vk) in NAV.iter().enumerate() {
                            // SAFETY: plain Win32 query.
                            let down = ours && unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;
                            if down && !was_nav[i] {
                                events()
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .push(KeyEvent::Nav(vk));
                            }
                            was_nav[i] = down;
                        }
                    }
                }
            });
        Mutex::new(Vec::new())
    })
}

const fn size(scale: u32) -> (u32, u32) {
    (
        (COLUMNS * CELL.0 + 2 * RIM) * scale,
        (LINES * CELL.1 + 2 * RIM) * scale,
    )
}

fn sanitise(text: &str) -> String {
    text.chars()
        .map(|c| c.to_ascii_uppercase())
        .map(|c| {
            if c.is_ascii() && glyph(c as u8) != glyph(b'#') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn lit(lines: &[String], x: u32, y: u32) -> bool {
    let (Some(x), Some(y)) = (x.checked_sub(RIM), y.checked_sub(RIM)) else {
        return false;
    };
    if x % CELL.0 >= GLYPH.0 || y % CELL.1 >= GLYPH.1 || x / CELL.0 >= COLUMNS {
        return false;
    }
    let Some(&c) = lines
        .get((y / CELL.1) as usize)
        .and_then(|l| l.as_bytes().get((x / CELL.0) as usize))
    else {
        return false;
    };
    (glyph(c.to_ascii_uppercase())[(y % CELL.1) as usize] >> (GLYPH.0 - 1 - x % CELL.0)) & 1 != 0
}

fn painted(lines: &[String], scale: u32, colours: [&[u8]; 2], pitch: usize) -> Vec<u8> {
    let (width, height) = size(scale);
    let bytes = colours[0].len();
    let mut out = vec![0; pitch * height as usize];
    for y in 0..height / scale {
        let top = pitch * (y * scale) as usize;
        for x in 0..width / scale {
            let colour = colours[usize::from(lit(lines, x, y))];
            for i in 0..scale {
                let at = top + (x * scale + i) as usize * bytes;
                out[at..at + bytes].copy_from_slice(colour);
            }
        }
        for i in 1..scale as usize {
            out.copy_within(top..top + pitch, top + i * pitch);
        }
    }
    out
}

struct Buffers {
    format: DXGI_FORMAT,
    scale: u32,
    pitch: u32,
    uploads: Vec<(ID3D12Resource, *mut u8)>,
    card: ID3D12Resource,
    turn: usize,
    fresh: bool,
    used: bool,
}

// SAFETY: free-threaded D3D12 resources, written only under the presenter's lock.
unsafe impl Send for Buffers {}

impl Buffers {
    fn new(host: &Host, format: DXGI_FORMAT, scale: u32) -> Option<Buffers> {
        let (width, height) = size(scale);
        let pitch = (width * colours(format)?[0].len() as u32).next_multiple_of(256);
        let bytes = u64::from(pitch) * u64::from(height);
        let uploads = (0..UPLOADS)
            .map(|_| host.upload_buffer(bytes))
            .collect::<Option<Vec<_>>>()?;
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: bytes,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let mut card: Option<ID3D12Resource> = None;
        // SAFETY: plain creation of a D3D12 buffer.
        unsafe {
            host.device
                .CreateCommittedResource(
                    &heap,
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_COMMON,
                    None,
                    &mut card,
                )
                .ok()?;
        }
        Some(Buffers {
            format,
            scale,
            pitch,
            uploads,
            card: card?,
            turn: 0,
            fresh: false,
            used: false,
        })
    }

    fn write(&mut self, lines: &[String]) {
        let Some(colours) = colours(self.format) else {
            return;
        };
        let picture = painted(lines, self.scale, colours, self.pitch as usize);
        self.turn = (self.turn + 1) % UPLOADS;
        // SAFETY: mapped upload buffer sized for `picture`, unused by the GPU.
        unsafe {
            std::ptr::copy_nonoverlapping(
                picture.as_ptr(),
                self.uploads[self.turn].1,
                picture.len(),
            );
        }
        self.fresh = true;
    }
}

/// The F2 controls overlay and top-center mode badge of a presenter.
pub(super) struct ControlsMenu {
    snap: Option<Snapshot>,
    /// Which layout tab is being viewed/edited (`false` = On Foot, `true` = Vehicle).
    editing_vehicle: bool,
    /// Selected row index across settings + bindings + actions.
    at: usize,
    /// When set, waiting for the next key/mouse press to rebind `(vehicle, output_id)`.
    capturing: Option<(bool, &'static str)>,
    local_note: String,
    changed: bool,
    painted_badge: Option<String>,
    buffers: Option<Buffers>,
    failed: bool,
}

impl ControlsMenu {
    pub(super) fn new() -> ControlsMenu {
        let _ = events();
        ControlsMenu {
            snap: controls_ui::snapshot(),
            editing_vehicle: false,
            at: 0,
            capturing: None,
            local_note: String::new(),
            changed: true,
            painted_badge: None,
            buffers: None,
            failed: false,
        }
    }

    fn open() -> bool {
        crate::CONTROLS_MENU_OPEN.load(Ordering::Relaxed)
    }

    fn current_bindings<'a>(&self, snap: &'a Snapshot) -> &'a [BindingEntry] {
        if self.editing_vehicle {
            &snap.vehicle_binds
        } else {
            &snap.on_foot
        }
    }

    fn total_rows(&self, snap: &Snapshot) -> usize {
        SETTINGS_COUNT + self.current_bindings(snap).len() + ACTIONS.len()
    }

    fn badge_line(snap: &Snapshot) -> Option<String> {
        if !snap.enabled || !snap.show_badge {
            return None;
        }
        Some(if snap.vehicle {
            "VEHICLE  G:ON FOOT  F2:CONTROLS".to_owned()
        } else {
            "ON FOOT  G:VEHICLE  F2:CONTROLS".to_owned()
        })
    }

    fn set_capturing(&mut self, target: Option<(bool, &'static str)>) {
        self.capturing = target;
        CAPTURING_BIND.store(target.is_some(), Ordering::Release);
    }

    fn lines(&self, snap: &Snapshot) -> Vec<String> {
        let bindings = self.current_bindings(snap);
        let total = SETTINGS_COUNT + bindings.len() + ACTIONS.len();
        let at = self.at.min(total.saturating_sub(1));
        let first = at.saturating_sub(ROWS / 2).min(total.saturating_sub(ROWS));
        let tab_name = if self.editing_vehicle {
            "VEHICLE"
        } else {
            "ON FOOT"
        };
        let mut out = vec![
            format!("CONTROLS ({tab_name:<7})               F2 OR ESC CLOSES"),
            "UP/DOWN PICK  ENTER REBIND  DEL CLEAR  TAB LAYOUT".to_owned(),
            String::new(),
        ];
        for row in first..(first + ROWS).min(total) {
            let cursor = if row == at { '>' } else { ' ' };
            let raw = if row < SETTINGS_COUNT {
                match row {
                    0 => format!(
                        "{cursor} {:<26} < {} > LIVE",
                        "ACTIVE MODE",
                        if snap.vehicle { "VEHICLE" } else { "ON FOOT" }
                    ),
                    1 => format!("{cursor} {:<26} < {} >", "EDITING LAYOUT", tab_name),
                    2 => format!(
                        "{cursor} {:<26} < {} > LIVE",
                        "SHOW MODE BADGE",
                        if snap.show_badge { "ON" } else { "OFF" }
                    ),
                    3 => format!(
                        "{cursor} {:<26} < {:.2} > LIVE",
                        "MOUSE SENSITIVITY", snap.sensitivity
                    ),
                    4 => format!(
                        "{cursor} {:<26} < {:.2} > LIVE",
                        "MOUSE Y SCALE", snap.y_scale
                    ),
                    5 => format!(
                        "{cursor} {:<26} < {:.2} > LIVE",
                        "STICK DEADZONE", snap.deadzone
                    ),
                    6 => format!("{cursor} {:<26} < {:.2} > LIVE", "STICK CURVE", snap.curve),
                    _ => format!(
                        "{cursor} {:<26} < {} > LIVE",
                        "INVERT MOUSE Y",
                        if snap.invert_y { "ON" } else { "OFF" }
                    ),
                }
            } else if row < SETTINGS_COUNT + bindings.len() {
                let entry = &bindings[row - SETTINGS_COUNT];
                let waiting = self
                    .capturing
                    .is_some_and(|(v, id)| v == self.editing_vehicle && id == entry.id);
                if waiting {
                    format!("{cursor} {:<26} [PRESS KEY/MOUSE...]", entry.label)
                } else {
                    format!("{cursor} {:<26} {}", entry.label, entry.keys)
                }
            } else {
                format!("{cursor} {}", ACTIONS[row - SETTINGS_COUNT - bindings.len()])
            };
            out.push(sanitise(&raw).chars().take(COLUMNS as usize).collect());
        }
        while out.len() < 3 + ROWS {
            out.push(String::new());
        }
        out.push(String::new());
        let note = if self.capturing.is_some() {
            "PRESS ANY KEY OR MOUSE BUTTON TO BIND  (ESC CANCELS)".to_owned()
        } else if !self.local_note.is_empty() {
            sanitise(&self.local_note)
        } else if !snap.note.is_empty() {
            sanitise(&snap.note)
        } else {
            "ALL CHANGES APPLY LIVE AND AUTO-SAVE TO VICTORY.TOML".to_owned()
        };
        out.push(note.chars().take(COLUMNS as usize).collect());
        let footer = format!(
            "ROW {}/{total}  G TOGGLES MODE  LEFT/RIGHT ADJUSTS TUNING",
            at + 1
        );
        out.push(footer.chars().take(COLUMNS as usize).collect());
        out
    }

    /// Processes input and refreshes the latest snapshot from `victory-hle`.
    pub(super) fn frame(&mut self) {
        if let Some(latest) = controls_ui::snapshot()
            && self
                .snap
                .as_ref()
                .is_none_or(|cur| cur.generation != latest.generation)
        {
            self.snap = Some(latest);
            self.changed = true;
        }
        let queued = std::mem::take(&mut *events().lock().unwrap_or_else(|e| e.into_inner()));
        if queued.is_empty() {
            return;
        }
        for ev in queued {
            match ev {
                KeyEvent::CancelBind => {
                    self.set_capturing(None);
                    self.local_note = "REBIND CANCELLED".to_owned();
                    self.changed = true;
                }
                KeyEvent::Bind(vk) => {
                    if let Some((vehicle, output_id)) = self.capturing {
                        self.set_capturing(None);
                        self.local_note.clear();
                        controls_ui::push(Command::Rebind {
                            vehicle,
                            output_id,
                            vk,
                        });
                        self.changed = true;
                    }
                }
                KeyEvent::Nav(vk) => self.handle_nav(vk),
            }
        }
    }

    fn handle_nav(&mut self, vk: u16) {
        let open = Self::open();
        if vk == VK_F2 || (open && vk == VK_ESCAPE) {
            let next = !open && vk == VK_F2;
            if next {
                crate::SETTINGS_MENU_OPEN.store(false, Ordering::Relaxed);
                if let Some(snap) = &self.snap {
                    self.editing_vehicle = snap.vehicle;
                }
            }
            self.set_capturing(None);
            self.local_note.clear();
            crate::CONTROLS_MENU_OPEN.store(next, Ordering::Relaxed);
            self.changed = true;
            return;
        }
        if !open {
            return;
        }
        let Some(snap) = self.snap.clone() else {
            return;
        };
        let total = self.total_rows(&snap);
        if total == 0 {
            return;
        }
        self.at = self.at.min(total - 1);
        self.local_note.clear();

        if vk == VK_TAB {
            self.editing_vehicle = !self.editing_vehicle;
            self.at = self.at.min(self.total_rows(&snap).saturating_sub(1));
            self.changed = true;
            return;
        }

        let bindings_len = self.current_bindings(&snap).len();
        match vk {
            VK_UP => self.at = (self.at + total - 1) % total,
            VK_DOWN => self.at = (self.at + 1) % total,
            VK_LEFT | VK_RIGHT if self.at < SETTINGS_COUNT => {
                let dir: i8 = if vk == VK_RIGHT { 1 } else { -1 };
                self.adjust_setting(self.at, dir, &snap);
            }
            VK_RETURN if self.at < SETTINGS_COUNT => {
                self.adjust_setting(self.at, 1, &snap);
            }
            VK_RETURN if self.at < SETTINGS_COUNT + bindings_len => {
                let entry = &self.current_bindings(&snap)[self.at - SETTINGS_COUNT];
                self.set_capturing(Some((self.editing_vehicle, entry.id)));
            }
            VK_DELETE | VK_BACK
                if self.at >= SETTINGS_COUNT && self.at < SETTINGS_COUNT + bindings_len =>
            {
                let entry = &self.current_bindings(&snap)[self.at - SETTINGS_COUNT];
                controls_ui::push(Command::Unbind {
                    vehicle: self.editing_vehicle,
                    output_id: entry.id,
                });
            }
            VK_RETURN => match self.at - SETTINGS_COUNT - bindings_len {
                0 => controls_ui::push(Command::SaveNow),
                1 => controls_ui::push(Command::ResetDefaults),
                _ => {
                    self.set_capturing(None);
                    crate::CONTROLS_MENU_OPEN.store(false, Ordering::Relaxed);
                }
            },
            _ => {}
        }
        self.changed = true;
    }

    fn adjust_setting(&mut self, row: usize, dir: i8, snap: &Snapshot) {
        match row {
            0 => {
                let next = !snap.vehicle;
                controls_ui::push(Command::SetVehicle(next));
                self.editing_vehicle = next;
            }
            1 => {
                self.editing_vehicle = !self.editing_vehicle;
            }
            2 => controls_ui::push(Command::SetShowBadge(!snap.show_badge)),
            3 => controls_ui::push(Command::StepSensitivity(dir)),
            4 => controls_ui::push(Command::StepYScale(dir)),
            5 => controls_ui::push(Command::StepDeadzone(dir)),
            6 => controls_ui::push(Command::StepCurve(dir)),
            _ => controls_ui::push(Command::ToggleInvertY),
        }
    }

    /// Copies either the centered F2 controls menu or the top-center mode badge
    /// onto `back` (in `D3D12_RESOURCE_STATE_COPY_DEST`).
    pub(super) fn draw(
        &mut self,
        host: &Host,
        list: &HostList,
        back: &ID3D12Resource,
        (width, height): (u32, u32),
        format: DXGI_FORMAT,
    ) {
        if self.failed {
            return;
        }
        let Some(snap) = self.snap.as_ref() else {
            return;
        };
        let open = Self::open();
        let f1_open = crate::SETTINGS_MENU_OPEN.load(Ordering::Relaxed);
        let badge = if open || f1_open {
            None
        } else {
            Self::badge_line(snap)
        };
        if !open && badge.is_none() {
            return;
        }
        #[cfg(feature = "diagnostics")]
        let _ours = crate::replay::record::unrecorded();
        let scale = (height / 540).max(1);
        if self
            .buffers
            .as_ref()
            .is_none_or(|b| (b.format, b.scale) != (format, scale))
        {
            std::mem::forget(self.buffers.take());
            self.buffers = Buffers::new(host, format, scale);
            self.failed = self.buffers.is_none();
            self.changed = true;
        }
        if self.painted_badge != badge {
            self.painted_badge = badge.clone();
            self.changed = true;
        }
        let lines = if self.changed {
            Some(
                badge
                    .as_ref()
                    .map_or_else(|| self.lines(snap), |line| vec![line.clone()]),
            )
        } else {
            None
        };
        let Some(buffers) = &mut self.buffers else {
            return;
        };
        if let Some(lines) = lines {
            self.changed = false;
            buffers.write(&lines);
        }
        let (w, h) = match &badge {
            Some(line) => (
                (line.len() as u32 * CELL.0 + 2 * RIM) * scale,
                (CELL.1 + 2 * RIM) * scale,
            ),
            None => size(scale),
        };
        let (w, h) = (w.min(width), h.min(height));
        let (x, y) = if badge.is_some() {
            ((width - w) / 2, 0)
        } else {
            ((width - w) / 2, (height - h) / 2)
        };
        let list = &list.list;
        // SAFETY: open command list and live D3D12 resources; region fits both.
        unsafe {
            if std::mem::take(&mut buffers.fresh) {
                if std::mem::replace(&mut buffers.used, true) {
                    list.ResourceBarrier(&[transition(
                        &buffers.card,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                    )]);
                }
                list.CopyResource(&buffers.card, &buffers.uploads[buffers.turn].0);
                list.ResourceBarrier(&[transition(
                    &buffers.card,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                )]);
            }
            let from = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&buffers.card),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: 0,
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                            Format: format,
                            Width: size(scale).0,
                            Height: size(scale).1,
                            Depth: 1,
                            RowPitch: buffers.pitch,
                        },
                    },
                },
            };
            let to = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(back),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: 0,
                },
            };
            let part = D3D12_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: w,
                bottom: h,
                back: 1,
            };
            list.CopyTextureRegion(&to, x, y, 0, &from, Some(&part));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> Snapshot {
        Snapshot {
            enabled: true,
            vehicle: false,
            show_badge: true,
            sensitivity: 1.0,
            y_scale: 1.0,
            deadzone: 0.30,
            curve: 1.40,
            invert_y: false,
            on_foot: vec![
                BindingEntry {
                    id: "LeftUp",
                    label: "MOVE FORWARD (L-UP)",
                    keys: "W".to_owned(),
                },
                BindingEntry {
                    id: "A",
                    label: "SPRINT / CONFIRM (A)",
                    keys: "LSHIFT, ENTER".to_owned(),
                },
            ],
            vehicle_binds: vec![BindingEntry {
                id: "RT",
                label: "THROTTLE (RT)",
                keys: "W, MOUSELEFT".to_owned(),
            }],
            note: String::new(),
            generation: 1,
        }
    }

    #[test]
    fn every_controls_menu_line_fits_and_has_glyphs() {
        let snap = sample_snapshot();
        let mut menu = ControlsMenu {
            snap: Some(snap.clone()),
            editing_vehicle: false,
            at: 0,
            capturing: None,
            local_note: String::new(),
            changed: false,
            painted_badge: None,
            buffers: None,
            failed: false,
        };
        for at in 0..menu.total_rows(&snap) {
            menu.at = at;
            let lines = menu.lines(&snap);
            assert_eq!(lines.len(), LINES as usize);
            for line in &lines {
                assert!(line.len() <= COLUMNS as usize, "{line}");
                for c in line.bytes() {
                    assert_ne!(
                        glyph(c.to_ascii_uppercase()),
                        glyph(b'#'),
                        "{line}: {}",
                        c as char
                    );
                }
            }
        }
        let badge = ControlsMenu::badge_line(&snap).unwrap();
        assert!(badge.len() <= COLUMNS as usize);
        assert!(badge.bytes().all(|c| glyph(c) != glyph(b'#')));
    }
}
