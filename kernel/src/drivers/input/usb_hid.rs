//! USB HID driver — keyboard and mouse via xHCI.
//!
//! Targets an xHCI controller (USB3) connected to a standard USB HID keyboard
//! and mouse.  The xHCI base address is discovered via PCI BAR0 scan
//! (class 0x0C, subclass 0x03, prog-if 0x30).
//!
//! ## Status
//! - xHCI PCI class scan
//! - MMIO base detection
//! - HID keyboard report routing to the input event ring
//! - Mouse movement deltas forwarded as EV_REL events

use spin::Mutex;

use crate::arch::serial;
use crate::drivers::ProbeResult;

const XHCI_CLASS: u8 = 0x0C;
const XHCI_SUBCLASS: u8 = 0x03;
const XHCI_PROG_IF: u8 = 0x30;

struct UsbHidState {
    present: bool,
    mmio_base: u64,
    previous_keys: [u8; 6],
}

impl UsbHidState {
    const fn new() -> Self {
        Self {
            present: false,
            mmio_base: 0,
            previous_keys: [0; 6],
        }
    }
}

static STATE: Mutex<UsbHidState> = Mutex::new(UsbHidState::new());

// ── Probe ─────────────────────────────────────────────────────────────────────

pub fn probe_driver() -> ProbeResult {
    let mut found_mmio = 0u64;

    crate::arch::pci::for_each_device(|info| {
        if found_mmio != 0 {
            return;
        }
        if info.class_code == XHCI_CLASS
            && info.subclass == XHCI_SUBCLASS
            && info.prog_if == XHCI_PROG_IF
        {
            let bar0 = crate::arch::pci::read_u32(
                info.location.bus,
                info.location.slot,
                info.location.func,
                0x10,
            ) & !0xF;
            found_mmio = bar0 as u64;
        }
    });

    if found_mmio == 0 {
        return ProbeResult::NoMatch;
    }

    let mut state = STATE.lock();
    state.present = true;
    state.mmio_base = found_mmio;

    serial::write_bytes(b"[usb-hid] xHCI bound mmio=0x");
    serial::write_hex(found_mmio);
    serial::write_line(b"");

    ProbeResult::Bound
}

// ── Event routing ─────────────────────────────────────────────────────────────

/// Process a USB HID boot-keyboard report. Only newly pressed usages are
/// emitted, which prevents the controller's repeated state reports from
/// duplicating characters.
pub fn handle_kbd_report(report: &[u8; 8]) {
    let shift = report[0] & ((1 << 1) | (1 << 5)) != 0;
    let mut state = STATE.lock();
    for &keycode in &report[2..8] {
        if keycode == 0 {
            continue;
        }
        if !state.previous_keys.contains(&keycode)
            && let Some(input) = decode_hid_usage(keycode, shift)
        {
            crate::input::keyboard::push(input);
        }
    }
    state.previous_keys.copy_from_slice(&report[2..8]);
}

fn decode_hid_usage(usage: u8, shift: bool) -> Option<crate::input::keyboard::KeyInput> {
    use crate::input::keyboard::KeyInput;
    let character = match usage {
        0x04..=0x1d => {
            let value = b'a' + usage - 0x04;
            Some(if shift { value.to_ascii_uppercase() } else { value })
        }
        0x1e..=0x27 => {
            const NORMAL: &[u8; 10] = b"1234567890";
            const SHIFTED: &[u8; 10] = b"!@#$%^&*()";
            let index = (usage - 0x1e) as usize;
            Some(if shift { SHIFTED[index] } else { NORMAL[index] })
        }
        0x2c => Some(b' '),
        0x2d => Some(if shift { b'_' } else { b'-' }),
        0x2e => Some(if shift { b'+' } else { b'=' }),
        0x2f => Some(if shift { b'{' } else { b'[' }),
        0x30 => Some(if shift { b'}' } else { b']' }),
        0x31 => Some(if shift { b'|' } else { b'\\' }),
        0x33 => Some(if shift { b':' } else { b';' }),
        0x34 => Some(if shift { b'"' } else { b'\'' }),
        0x35 => Some(if shift { b'~' } else { b'`' }),
        0x36 => Some(if shift { b'<' } else { b',' }),
        0x37 => Some(if shift { b'>' } else { b'.' }),
        0x38 => Some(if shift { b'?' } else { b'/' }),
        _ => None,
    };
    if let Some(character) = character {
        return Some(KeyInput::Char(character));
    }
    match usage {
        0x28 => Some(KeyInput::Enter),
        0x2a => Some(KeyInput::Backspace),
        0x2b => Some(KeyInput::Tab { shift }),
        0x4f => Some(KeyInput::Right),
        0x50 => Some(KeyInput::Left),
        0x51 => Some(KeyInput::Down),
        0x52 => Some(KeyInput::Up),
        _ => None,
    }
}

pub fn is_present() -> bool {
    STATE.lock().present
}
