#![no_std]
#![no_main]

#[path = "../runtime.rs"]
mod runtime;

use core::panic::PanicInfo;

const TAG_PING: u8 = 0x01;
const TAG_PONG: u8 = 0x02;
const TAG_ERROR: u8 = 0x03;
const TAG_SHUTDOWN: u8 = 0x04;
const TAG_CREATED: u8 = 0x50;
const TAG_QUERY: u8 = 0x51;
const TAG_RESPONSE: u8 = 0x52;
const TAG_RUNTIME_EVENT: u8 = 0x60;
const TAG_AUDIT: u8 = 0x61;
const MAX_ARTIFACTS: usize = 128;
const RECORD_SIZE: usize = 40;
const SNAPSHOT_CAP: usize = 8 + MAX_ARTIFACTS * RECORD_SIZE;
const STATE_PATH: &[u8] = b"/persist/artifactsd.catalog";

#[derive(Clone, Copy)]
struct Artifact {
    node_id: u64,
    hash: [u8; 16],
    kind: u8,
    size: u32,
    created_at: u64,
    occupied: bool,
}

const EMPTY: Artifact = Artifact {
    node_id: 0,
    hash: [0; 16],
    kind: 0,
    size: 0,
    created_at: 0,
    occupied: false,
};

struct Catalog {
    entries: [Artifact; MAX_ARTIFACTS],
    generation: u64,
}

impl Catalog {
    const fn new() -> Self {
        Self {
            entries: [EMPTY; MAX_ARTIFACTS],
            generation: 0,
        }
    }

    fn restore(&mut self) -> bool {
        let fd = runtime::vfs_open(STATE_PATH);
        if fd == u64::MAX {
            return true;
        }
        let mut snapshot = [0u8; SNAPSHOT_CAP];
        let read = runtime::vfs_read(fd, &mut snapshot);
        let _ = runtime::vfs_close(fd);
        if read != SNAPSHOT_CAP as u64 {
            return false;
        }
        self.generation = read_u64(&snapshot[0..8]);
        let mut offset = 8;
        for entry in &mut self.entries {
            entry.occupied = snapshot[offset] != 0;
            entry.kind = snapshot[offset + 1];
            entry.node_id = read_u64(&snapshot[offset + 4..offset + 12]);
            entry
                .hash
                .copy_from_slice(&snapshot[offset + 12..offset + 28]);
            entry.size = read_u32(&snapshot[offset + 28..offset + 32]);
            entry.created_at = read_u64(&snapshot[offset + 32..offset + 40]);
            offset += RECORD_SIZE;
        }
        true
    }

    fn persist(&self) -> bool {
        let _ = runtime::vfs_mkdir(b"/persist");
        let mut snapshot = [0u8; SNAPSHOT_CAP];
        snapshot[0..8].copy_from_slice(&self.generation.to_le_bytes());
        let mut offset = 8;
        for entry in &self.entries {
            snapshot[offset] = u8::from(entry.occupied);
            snapshot[offset + 1] = entry.kind;
            snapshot[offset + 4..offset + 12].copy_from_slice(&entry.node_id.to_le_bytes());
            snapshot[offset + 12..offset + 28].copy_from_slice(&entry.hash);
            snapshot[offset + 28..offset + 32].copy_from_slice(&entry.size.to_le_bytes());
            snapshot[offset + 32..offset + 40].copy_from_slice(&entry.created_at.to_le_bytes());
            offset += RECORD_SIZE;
        }
        let fd = runtime::vfs_create(STATE_PATH);
        if fd == u64::MAX {
            return false;
        }
        let written = runtime::vfs_write(fd, &snapshot);
        let closed = runtime::vfs_close(fd);
        written == SNAPSHOT_CAP as u64 && closed
    }

    fn register(&mut self, payload: &[u8]) -> Result<Artifact, u8> {
        if payload.len() < 37 {
            return Err(1);
        }
        let node_id = read_u64(&payload[0..8]);
        let mut hash = [0u8; 16];
        hash.copy_from_slice(&payload[8..24]);
        if self
            .entries
            .iter()
            .any(|entry| entry.occupied && (entry.node_id == node_id || entry.hash == hash))
        {
            return Err(2);
        }
        let Some(slot) = self.entries.iter_mut().find(|entry| !entry.occupied) else {
            return Err(3);
        };
        *slot = Artifact {
            node_id,
            hash,
            kind: payload[24],
            size: read_u32(&payload[25..29]),
            created_at: read_u64(&payload[29..37]),
            occupied: true,
        };
        let artifact = *slot;
        self.generation = self.generation.saturating_add(1);
        if !self.persist() {
            return Err(4);
        }
        Ok(artifact)
    }

    fn find(&self, node_id: u64) -> Option<&Artifact> {
        self.entries
            .iter()
            .find(|entry| entry.occupied && entry.node_id == node_id)
    }
    fn count(&self) -> u32 {
        self.entries.iter().filter(|entry| entry.occupied).count() as u32
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    runtime::panic(info)
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let inbox = runtime::service_inbox_or_die(b"artifactsd");
    runtime::claim_inbox(inbox);
    let mut catalog = Catalog::new();
    if !catalog.restore() {
        runtime::write_line(b"[artifactsd] catalog corrupt; readiness withheld\n");
        runtime::exit(78);
    }
    runtime::write_line(b"[artifactsd] durable artifact catalog online\n");
    let _ = runtime::bootstrap_named_status(b"service-ready:", b"artifactsd");
    runtime::announce_service_ready(b"artifactsd");

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
            TAG_CREATED => match catalog.register(&payload[..len]) {
                Ok(artifact) => {
                    send_descriptor(reply, &artifact);
                    emit_observability(&artifact, reply);
                }
                Err(code) => {
                    let _ = runtime::channel_send(reply, &[code], TAG_ERROR);
                }
            },
            TAG_QUERY => {
                if len < 8 {
                    let _ = runtime::channel_send(reply, &[1], TAG_ERROR);
                    continue;
                }
                match catalog.find(read_u64(&payload[..8])) {
                    Some(entry) => send_descriptor(reply, entry),
                    None => {
                        let _ = runtime::channel_send(reply, &[1], TAG_ERROR);
                    }
                }
            }
            TAG_PING => {
                let mut health = [0u8; 12];
                health[..8].copy_from_slice(&catalog.generation.to_le_bytes());
                health[8..12].copy_from_slice(&catalog.count().to_le_bytes());
                let _ = runtime::channel_send(reply, &health, TAG_PONG);
            }
            TAG_SHUTDOWN => {
                let _ = catalog.persist();
                runtime::exit(0);
            }
            _ => {
                let _ = runtime::channel_send(reply, &[0xff], TAG_ERROR);
            }
        }
    }
}

fn send_descriptor(endpoint: u32, entry: &Artifact) {
    let mut response = [0u8; 37];
    response[0..8].copy_from_slice(&entry.node_id.to_le_bytes());
    response[8..24].copy_from_slice(&entry.hash);
    response[24] = entry.kind;
    response[25..29].copy_from_slice(&entry.size.to_le_bytes());
    response[29..37].copy_from_slice(&entry.created_at.to_le_bytes());
    let _ = runtime::channel_send(endpoint, &response, TAG_RESPONSE);
}

fn emit_observability(entry: &Artifact, actor: u32) {
    let Some(sysd) = runtime::service_inbox(b"sysd") else {
        return;
    };
    let mut event = [0u8; 29];
    event[0] = 40;
    event[1..9].copy_from_slice(&entry.node_id.to_le_bytes());
    event[17..21].copy_from_slice(&entry.size.to_le_bytes());
    event[21..29].copy_from_slice(&entry.created_at.to_le_bytes());
    let _ = runtime::channel_send(sysd, &event, TAG_RUNTIME_EVENT);
    let mut audit = [0u8; 26];
    audit[0..8].copy_from_slice(&entry.created_at.to_le_bytes());
    audit[8..16].copy_from_slice(&(actor as u64).to_le_bytes());
    audit[16] = 1;
    audit[17] = 1;
    audit[18..26].copy_from_slice(&entry.node_id.to_le_bytes());
    let _ = runtime::channel_send(sysd, &audit, TAG_AUDIT);
}

fn read_u32(v: &[u8]) -> u32 {
    u32::from_le_bytes([v[0], v[1], v[2], v[3]])
}
fn read_u64(v: &[u8]) -> u64 {
    u64::from_le_bytes([v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]])
}
