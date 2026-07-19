#![no_std]
#![no_main]

#[path = "../runtime.rs"]
mod runtime;

use core::panic::PanicInfo;

const TAG_PING: u8 = 0x01;
const TAG_PONG: u8 = 0x02;
const TAG_SHUTDOWN: u8 = 0x04;
const TAG_EVENT: u8 = 0x60;
const TAG_AUDIT: u8 = 0x61;
const TAG_DIAGNOSTIC_QUERY: u8 = 0x62;
const TAG_DIAGNOSTIC_RESPONSE: u8 = 0x63;
const TAG_COGNITIVE_STATUS: u8 = 0x64;
const CAP: usize = 64;
const HEADER_SIZE: usize = 72;
const EVENT_SIZE: usize = 48;
const AUDIT_SIZE: usize = 48;
const SNAPSHOT_CAP: usize = HEADER_SIZE + CAP * EVENT_SIZE + CAP * AUDIT_SIZE;
const STATE_PATH: &[u8] = b"/persist/sysd.telemetry";

#[derive(Clone, Copy)]
struct Event {
    seq: u64,
    kind: u8,
    node: u64,
    node2: u64,
    value: u32,
    timestamp: u64,
    valid: bool,
}
const EMPTY_EVENT: Event = Event {
    seq: 0,
    kind: 0,
    node: 0,
    node2: 0,
    value: 0,
    timestamp: 0,
    valid: false,
};

#[derive(Clone, Copy)]
struct Audit {
    seq: u64,
    timestamp: u64,
    actor: u64,
    action: u8,
    outcome: u8,
    target: u64,
    valid: bool,
}
const EMPTY_AUDIT: Audit = Audit {
    seq: 0,
    timestamp: 0,
    actor: 0,
    action: 0,
    outcome: 0,
    target: 0,
    valid: false,
};

struct Observatory {
    events: [Event; CAP],
    audits: [Audit; CAP],
    event_seq: u64,
    audit_seq: u64,
    doc_count: u32,
    entity_count: u32,
    relation_count: u32,
    spectral_refresh: u64,
    cognitive: [u8; 18],
    cognitive_active: bool,
}

impl Observatory {
    const fn new() -> Self {
        Self {
            events: [EMPTY_EVENT; CAP],
            audits: [EMPTY_AUDIT; CAP],
            event_seq: 0,
            audit_seq: 0,
            doc_count: 0,
            entity_count: 0,
            relation_count: 0,
            spectral_refresh: 0,
            cognitive: [0; 18],
            cognitive_active: false,
        }
    }

    fn push_event(&mut self, payload: &[u8]) -> bool {
        if payload.len() < 29 {
            return false;
        }
        self.event_seq = self.event_seq.saturating_add(1);
        let event = Event {
            seq: self.event_seq,
            kind: payload[0],
            node: read_u64(&payload[1..9]),
            node2: read_u64(&payload[9..17]),
            value: read_u32(&payload[17..21]),
            timestamp: read_u64(&payload[21..29]),
            valid: true,
        };
        self.events[(self.event_seq as usize - 1) % CAP] = event;
        match event.kind {
            0 => self.doc_count = self.doc_count.saturating_add(1),
            11 => self.entity_count = self.entity_count.saturating_add(1),
            12 => self.relation_count = self.relation_count.saturating_add(1),
            20 => self.spectral_refresh = event.timestamp,
            _ => {}
        }
        self.persist()
    }

    fn push_audit(&mut self, payload: &[u8]) -> bool {
        if payload.len() < 26 {
            return false;
        }
        self.audit_seq = self.audit_seq.saturating_add(1);
        self.audits[(self.audit_seq as usize - 1) % CAP] = Audit {
            seq: self.audit_seq,
            timestamp: read_u64(&payload[0..8]),
            actor: read_u64(&payload[8..16]),
            action: payload[16],
            outcome: payload[17],
            target: read_u64(&payload[18..26]),
            valid: true,
        };
        self.persist()
    }

    fn set_cognitive(&mut self, payload: &[u8]) -> bool {
        if payload.len() < 18 {
            return false;
        }
        self.cognitive.copy_from_slice(&payload[..18]);
        self.cognitive_active = payload[0] < 10;
        self.persist()
    }

    fn restore(&mut self) -> bool {
        let fd = runtime::vfs_open(STATE_PATH);
        if fd == u64::MAX {
            return true;
        }
        let mut data = [0u8; SNAPSHOT_CAP];
        let read = runtime::vfs_read(fd, &mut data);
        let _ = runtime::vfs_close(fd);
        if read != SNAPSHOT_CAP as u64 || &data[0..4] != b"SYD1" {
            return false;
        }
        self.event_seq = read_u64(&data[8..16]);
        self.audit_seq = read_u64(&data[16..24]);
        self.doc_count = read_u32(&data[24..28]);
        self.entity_count = read_u32(&data[28..32]);
        self.relation_count = read_u32(&data[32..36]);
        self.spectral_refresh = read_u64(&data[40..48]);
        self.cognitive.copy_from_slice(&data[48..66]);
        self.cognitive_active = data[66] != 0;
        let mut offset = HEADER_SIZE;
        for event in &mut self.events {
            event.valid = data[offset] != 0;
            event.kind = data[offset + 1];
            event.seq = read_u64(&data[offset + 8..offset + 16]);
            event.node = read_u64(&data[offset + 16..offset + 24]);
            event.node2 = read_u64(&data[offset + 24..offset + 32]);
            event.value = read_u32(&data[offset + 32..offset + 36]);
            event.timestamp = read_u64(&data[offset + 40..offset + 48]);
            offset += EVENT_SIZE;
        }
        for audit in &mut self.audits {
            audit.valid = data[offset] != 0;
            audit.action = data[offset + 1];
            audit.outcome = data[offset + 2];
            audit.seq = read_u64(&data[offset + 8..offset + 16]);
            audit.timestamp = read_u64(&data[offset + 16..offset + 24]);
            audit.actor = read_u64(&data[offset + 24..offset + 32]);
            audit.target = read_u64(&data[offset + 32..offset + 40]);
            offset += AUDIT_SIZE;
        }
        true
    }

    fn persist(&self) -> bool {
        let _ = runtime::vfs_mkdir(b"/persist");
        let mut data = [0u8; SNAPSHOT_CAP];
        data[0..4].copy_from_slice(b"SYD1");
        data[8..16].copy_from_slice(&self.event_seq.to_le_bytes());
        data[16..24].copy_from_slice(&self.audit_seq.to_le_bytes());
        data[24..28].copy_from_slice(&self.doc_count.to_le_bytes());
        data[28..32].copy_from_slice(&self.entity_count.to_le_bytes());
        data[32..36].copy_from_slice(&self.relation_count.to_le_bytes());
        data[40..48].copy_from_slice(&self.spectral_refresh.to_le_bytes());
        data[48..66].copy_from_slice(&self.cognitive);
        data[66] = u8::from(self.cognitive_active);
        let mut offset = HEADER_SIZE;
        for event in &self.events {
            data[offset] = u8::from(event.valid);
            data[offset + 1] = event.kind;
            data[offset + 8..offset + 16].copy_from_slice(&event.seq.to_le_bytes());
            data[offset + 16..offset + 24].copy_from_slice(&event.node.to_le_bytes());
            data[offset + 24..offset + 32].copy_from_slice(&event.node2.to_le_bytes());
            data[offset + 32..offset + 36].copy_from_slice(&event.value.to_le_bytes());
            data[offset + 40..offset + 48].copy_from_slice(&event.timestamp.to_le_bytes());
            offset += EVENT_SIZE;
        }
        for audit in &self.audits {
            data[offset] = u8::from(audit.valid);
            data[offset + 1] = audit.action;
            data[offset + 2] = audit.outcome;
            data[offset + 8..offset + 16].copy_from_slice(&audit.seq.to_le_bytes());
            data[offset + 16..offset + 24].copy_from_slice(&audit.timestamp.to_le_bytes());
            data[offset + 24..offset + 32].copy_from_slice(&audit.actor.to_le_bytes());
            data[offset + 32..offset + 40].copy_from_slice(&audit.target.to_le_bytes());
            offset += AUDIT_SIZE;
        }
        let fd = runtime::vfs_create(STATE_PATH);
        if fd == u64::MAX {
            return false;
        }
        let written = runtime::vfs_write(fd, &data);
        let closed = runtime::vfs_close(fd);
        written == SNAPSHOT_CAP as u64 && closed
    }

    fn diagnostics(&self) -> [u8; 49] {
        let mut out = [0u8; 49];
        out[0..4].copy_from_slice(&self.doc_count.to_le_bytes());
        out[4..8].copy_from_slice(&self.entity_count.to_le_bytes());
        out[8..12].copy_from_slice(&self.relation_count.to_le_bytes());
        out[19..27].copy_from_slice(&self.spectral_refresh.to_le_bytes());
        out[31..39].copy_from_slice(&self.event_seq.to_le_bytes());
        out[39..47].copy_from_slice(&self.audit_seq.to_le_bytes());
        out[47] = u8::from(self.cognitive_active);
        out[48] = self.cognitive[0];
        out
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    runtime::panic(info)
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let inbox = runtime::service_inbox_or_die(b"sysd");
    runtime::claim_inbox(inbox);
    let mut state = Observatory::new();
    if !state.restore() {
        runtime::write_line(b"[sysd] telemetry corrupt; readiness withheld\n");
        runtime::exit(78);
    }
    runtime::write_line(b"[sysd] durable observability service online\n");
    let _ = runtime::bootstrap_named_status(b"service-ready:", b"sysd");
    runtime::announce_service_ready(b"sysd");
    let mut payload = [0u8; 256];
    loop {
        let raw = runtime::channel_recv(inbox, &mut payload);
        if raw == 0 || raw == u64::MAX {
            runtime::yield_now();
            continue;
        }
        let len = (raw & 0xffff) as usize;
        let tag = ((raw >> 16) & 0xff) as u8;
        let reply = (raw >> 24) as u32;
        if len > payload.len() {
            continue;
        }
        match tag {
            TAG_EVENT => {
                let _ = state.push_event(&payload[..len]);
            }
            TAG_AUDIT => {
                let _ = state.push_audit(&payload[..len]);
            }
            TAG_COGNITIVE_STATUS => {
                let _ = state.set_cognitive(&payload[..len]);
            }
            TAG_DIAGNOSTIC_QUERY => {
                let _ = runtime::channel_send(reply, &state.diagnostics(), TAG_DIAGNOSTIC_RESPONSE);
            }
            TAG_PING => {
                let _ = runtime::channel_send(reply, &state.diagnostics(), TAG_PONG);
            }
            TAG_SHUTDOWN => {
                let _ = state.persist();
                runtime::exit(0);
            }
            _ => {}
        }
    }
}

fn read_u32(v: &[u8]) -> u32 {
    u32::from_le_bytes([v[0], v[1], v[2], v[3]])
}
fn read_u64(v: &[u8]) -> u64 {
    u64::from_le_bytes([v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]])
}
