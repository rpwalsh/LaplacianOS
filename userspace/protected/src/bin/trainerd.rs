#![no_std]
#![no_main]

#[path = "../runtime.rs"]
mod runtime;

use core::panic::PanicInfo;
use laplacianos_trainer_core::{
    BatchExecution, EMPTY_JOB, EMPTY_SUBSYSTEM, GraphWork, Job, JobState, JobType, MAX_BATCH_SIZE,
    MAX_JOBS, ResourcePolicy, SUBSYSTEM_COUNT, TrainerCore, WorkerAction, decode_batch,
};

const TAG_PING: u8 = 0x01;
const TAG_PONG: u8 = 0x02;
const TAG_SHUTDOWN: u8 = 0x04;
const TAG_GRAPH_QUERY: u8 = 0x10;
const TAG_GRAPH_QUERY_RESULT: u8 = 0x11;
const TAG_GRAPH_MUTATE: u8 = 0x12;
const TAG_GRAPH_MUTATE_ACK: u8 = 0x13;
const TAG_JOB_SUBMIT: u8 = 0x40;
const TAG_JOB_REPORT: u8 = 0x41;
const TAG_CONTROL: u8 = 0x42;
const TAG_STATUS_QUERY: u8 = 0x43;
const TAG_STATUS_RESPONSE: u8 = 0x44;
const TAG_RUNTIME_EVENT: u8 = 0x60;
const JOB_RECORD: usize = 56;
const SNAPSHOT_CAP: usize = 32 + SUBSYSTEM_COUNT * 16 + MAX_JOBS * JOB_RECORD;
const STATE_PATH: &[u8] = b"/persist/trainerd.jobs";

/// The protected adapter owns IPC, persistence and VFS. Scheduling, resource
/// policy, claiming and batch worker mechanics live in laplacianos-trainer-core.
struct ProtectedTrainer {
    core: TrainerCore,
    batch: Option<BatchExecution>,
    clock: u64,
}

impl ProtectedTrainer {
    const fn new() -> Self {
        Self {
            core: TrainerCore::new(),
            batch: None,
            clock: 0,
        }
    }
    fn now(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }

    fn restore(&mut self) -> bool {
        let fd = runtime::vfs_open(STATE_PATH);
        if fd == u64::MAX {
            return true;
        }
        let mut data = [0u8; SNAPSHOT_CAP];
        let read = runtime::vfs_read(fd, &mut data);
        let _ = runtime::vfs_close(fd);
        if read != SNAPSHOT_CAP as u64 || &data[..4] != b"TRN2" {
            return false;
        }
        let next_seq = read_u32(&data[4..8]);
        let running_seq = read_u32(&data[8..12]);
        self.clock = read_u64(&data[16..24]);
        let mut subsystems = [EMPTY_SUBSYSTEM; SUBSYSTEM_COUNT];
        let mut offset = 32;
        for subsystem in &mut subsystems {
            subsystem.state = decode_state(data[offset]);
            subsystem.policy = ResourcePolicy {
                cpu_budget: data[offset + 1],
                io_budget: data[offset + 2],
                schedule: data[offset + 3],
            };
            subsystem.completed = read_u32(&data[offset + 4..offset + 8]);
            subsystem.failed = read_u32(&data[offset + 8..offset + 12]);
            offset += 16;
        }
        let mut jobs = [EMPTY_JOB; MAX_JOBS];
        for job in &mut jobs {
            job.occupied = data[offset] != 0;
            job.kind = match JobType::from_u8(data[offset + 1]) {
                Some(kind) => kind,
                None => return false,
            };
            job.priority = data[offset + 2];
            job.state = decode_state(data[offset + 3]);
            job.progress = data[offset + 4];
            job.seq = read_u32(&data[offset + 8..offset + 12]);
            job.requester = read_u32(&data[offset + 12..offset + 16]);
            job.target = read_u64(&data[offset + 16..offset + 24]);
            job.submitted_at = read_u64(&data[offset + 24..offset + 32]);
            job.started_at = read_u64(&data[offset + 32..offset + 40]);
            job.completed_at = read_u64(&data[offset + 40..offset + 48]);
            job.verifier = read_i32(&data[offset + 48..offset + 52]);
            offset += JOB_RECORD;
        }
        self.core.restore(jobs, subsystems, next_seq, running_seq);
        true
    }

    fn persist(&self) -> bool {
        let _ = runtime::vfs_mkdir(b"/persist");
        let mut data = [0u8; SNAPSHOT_CAP];
        data[..4].copy_from_slice(b"TRN2");
        data[4..8].copy_from_slice(&self.core.next_seq().to_le_bytes());
        data[8..12].copy_from_slice(&self.core.running_seq().to_le_bytes());
        data[16..24].copy_from_slice(&self.clock.to_le_bytes());
        let mut offset = 32;
        for subsystem in self.core.subsystems() {
            data[offset] = subsystem.state as u8;
            data[offset + 1] = subsystem.policy.cpu_budget;
            data[offset + 2] = subsystem.policy.io_budget;
            data[offset + 3] = subsystem.policy.schedule;
            data[offset + 4..offset + 8].copy_from_slice(&subsystem.completed.to_le_bytes());
            data[offset + 8..offset + 12].copy_from_slice(&subsystem.failed.to_le_bytes());
            offset += 16;
        }
        for job in self.core.jobs() {
            data[offset] = u8::from(job.occupied);
            data[offset + 1] = job.kind as u8;
            data[offset + 2] = job.priority;
            data[offset + 3] = job.state as u8;
            data[offset + 4] = job.progress;
            data[offset + 8..offset + 12].copy_from_slice(&job.seq.to_le_bytes());
            data[offset + 12..offset + 16].copy_from_slice(&job.requester.to_le_bytes());
            data[offset + 16..offset + 24].copy_from_slice(&job.target.to_le_bytes());
            data[offset + 24..offset + 32].copy_from_slice(&job.submitted_at.to_le_bytes());
            data[offset + 32..offset + 40].copy_from_slice(&job.started_at.to_le_bytes());
            data[offset + 40..offset + 48].copy_from_slice(&job.completed_at.to_le_bytes());
            data[offset + 48..offset + 52].copy_from_slice(&job.verifier.to_le_bytes());
            offset += JOB_RECORD;
        }
        let fd = runtime::vfs_create(STATE_PATH);
        if fd == u64::MAX {
            return false;
        }
        let written = runtime::vfs_write(fd, &data);
        let closed = runtime::vfs_close(fd);
        written == SNAPSHOT_CAP as u64 && closed
    }

    fn submit(&mut self, payload: &[u8], requester: u32) -> Result<u32, u8> {
        if payload.len() < 24 {
            return Err(1);
        }
        let kind = JobType::from_u8(payload[0]).ok_or(1u8)?;
        let seq = self
            .core
            .submit(
                kind,
                payload[1],
                read_u64(&payload[8..16]),
                read_u64(&payload[16..24]),
                requester,
            )
            .map_err(|_| 2u8)?;
        if !self.persist() {
            return Err(3);
        }
        self.emit_event(30, read_u64(&payload[8..16]), seq);
        Ok(seq)
    }

    fn control(&mut self, payload: &[u8]) -> bool {
        if payload.len() < 16 {
            return false;
        }
        let Some(kind) = JobType::from_u8(payload[0]) else {
            return false;
        };
        let policy = ResourcePolicy {
            cpu_budget: payload[2],
            io_budget: payload[3],
            schedule: payload[4],
        };
        let interrupted = payload[1] == 1
            && self
                .core
                .running_job()
                .map(|job| job.kind == kind)
                .unwrap_or(false);
        let controlled = self.core.control(kind, payload[1], policy).is_ok();
        if interrupted {
            self.batch = None;
        }
        controlled && self.persist()
    }

    fn dispatch(&mut self, idle: bool) {
        let now = self.now();
        let action = match self.core.claim_next_with_context(now, idle) {
            Ok(Some(action)) => action,
            Ok(None) | Err(_) => return,
        };
        let _ = self.persist();
        if let Some(job) = self.core.running_job() {
            self.send_report(job);
            self.emit_event(32, job.target, job.seq);
        }
        match action {
            WorkerAction::Index { document_node, .. } => {
                self.begin_batch(JobType::Index, document_node)
            }
            WorkerAction::Hydrate { corpus_node, .. } => {
                self.begin_batch(JobType::Hydrate, corpus_node)
            }
            WorkerAction::CorrelateQuery { entity_node, .. } => {
                let mut query = [0u8; 32];
                query[0] = 4;
                query[4] = 8;
                query[8..16].copy_from_slice(&entity_node.to_le_bytes());
                if !send_graph(&query, TAG_GRAPH_QUERY) {
                    self.finish(false, 0);
                }
            }
            WorkerAction::SpectralRefresh { .. } => {
                let mut query = [0u8; 32];
                query[0] = 6;
                if !send_graph(&query, TAG_GRAPH_QUERY) {
                    self.finish(false, 0);
                }
            }
            WorkerAction::CorrelatePromote { from, to, job_seq } => {
                if !send_graph_work(GraphWork::AddEdge {
                    kind: 30,
                    weight: 1,
                    from,
                    to,
                    provenance: job_seq as u64,
                }) {
                    self.finish(false, 0);
                }
            }
        }
    }

    fn begin_batch(&mut self, mode: JobType, owner: u64) {
        let (path, path_len) = batch_path(mode, owner);
        let fd = runtime::vfs_open(&path[..path_len]);
        if fd == u64::MAX {
            self.finish(false, 0);
            return;
        }
        let mut bytes = [0u8; MAX_BATCH_SIZE];
        let read = runtime::vfs_read(fd, &mut bytes) as usize;
        let _ = runtime::vfs_close(fd);
        let Ok(batch) = decode_batch(&bytes[..read.min(bytes.len())], owner, mode) else {
            self.finish(false, 0);
            return;
        };
        self.batch = Some(BatchExecution::new(batch));
        self.advance_batch();
    }

    fn advance_batch(&mut self) {
        let Some(batch) = self.batch.as_mut() else {
            self.finish(false, 0);
            return;
        };
        if batch.is_complete() {
            let verifier = batch.verifier();
            self.batch = None;
            self.finish(true, verifier);
            return;
        }
        match batch.next() {
            Ok(Some(work)) => {
                let progress = batch.progress();
                let _ = self.core.set_progress(progress.max(10));
                let _ = self.persist();
                if let Some(job) = self.core.running_job() {
                    self.send_report(job);
                }
                if !send_graph_work(work) {
                    self.batch = None;
                    self.finish(false, 0);
                }
            }
            _ => {
                self.batch = None;
                self.finish(false, 0);
            }
        }
    }

    fn handle_graph_result(&mut self, payload: &[u8]) {
        let Some(job) = self.core.running_job() else {
            return;
        };
        match job.kind {
            JobType::Correlate => {
                if payload.len() < 32 || payload[1] == 0 {
                    self.finish(false, 0);
                    return;
                }
                let mut peer = 0;
                for index in 0..payload[1].min(8) as usize {
                    let offset = 16 + index * 16;
                    if offset + 8 <= payload.len() {
                        let id = read_u64(&payload[offset..offset + 8]);
                        if id != 0 && id != job.target {
                            peer = id;
                            break;
                        }
                    }
                }
                let Ok(action) = self.core.correlation_candidate(peer) else {
                    self.finish(false, 0);
                    return;
                };
                if let WorkerAction::CorrelatePromote { from, to, job_seq } = action {
                    let _ = self.persist();
                    if let Some(job) = self.core.running_job() {
                        self.send_report(job);
                    }
                    if !send_graph_work(GraphWork::AddEdge {
                        kind: 30,
                        weight: 1,
                        from,
                        to,
                        provenance: job_seq as u64,
                    }) {
                        self.finish(false, 0);
                    }
                }
            }
            JobType::Spectral => {
                if payload.len() < 40 {
                    self.finish(false, 0);
                    return;
                }
                let checksum = read_u64(&payload[32..40]);
                let eigen = read_i32(&payload[40..44]);
                let energy = read_u64(&payload[48..56]);
                self.finish(
                    true,
                    ((checksum ^ energy ^ eigen as u32 as u64) & 0x7fff_ffff) as i32,
                );
            }
            _ => self.finish(false, 0),
        }
    }

    fn handle_graph_ack(&mut self, payload: &[u8]) {
        if payload.len() < 24 || payload[0] != 0 {
            self.batch = None;
            self.finish(false, 0);
            return;
        }
        if let Some(batch) = self.batch.as_mut() {
            if batch
                .acknowledge(read_u64(&payload[16..24]), read_u64(&payload[8..16]))
                .is_err()
            {
                self.batch = None;
                self.finish(false, 0);
                return;
            }
            self.advance_batch();
        } else {
            self.finish(true, (read_u64(&payload[8..16]) & 0x7fff_ffff) as i32);
        }
    }

    fn finish(&mut self, success: bool, verifier: i32) {
        let now = self.now();
        let Ok(job) = self.core.complete(success, now, verifier) else {
            return;
        };
        let _ = self.persist();
        self.send_report(job);
        self.emit_event(if success { 31 } else { 0xfe }, job.target, job.seq);
    }

    fn send_report(&self, job: Job) {
        let mut report = [0u8; 48];
        report[0] = job.kind as u8;
        report[1] = job.state as u8;
        report[2] = job.progress;
        report[4..8].copy_from_slice(&job.seq.to_le_bytes());
        report[8..16].copy_from_slice(&job.target.to_le_bytes());
        report[16..24].copy_from_slice(&job.started_at.to_le_bytes());
        report[24..32].copy_from_slice(&job.completed_at.to_le_bytes());
        report[36..40].copy_from_slice(&job.verifier.to_le_bytes());
        let _ = runtime::channel_send(job.requester, &report, TAG_JOB_REPORT);
        if let Some(sysd) = runtime::service_inbox(b"sysd") {
            let _ = runtime::channel_send(sysd, &report, TAG_JOB_REPORT);
        }
    }

    fn status(&self) -> [u8; 56] {
        let mut out = [0u8; 56];
        for (index, subsystem) in self.core.subsystems().iter().enumerate() {
            let offset = index * 12;
            out[offset] = subsystem.state as u8;
            out[offset + 1] = subsystem.policy.cpu_budget;
            out[offset + 2] = subsystem.policy.io_budget;
            out[offset + 3] = subsystem.policy.schedule;
            out[offset + 4..offset + 8].copy_from_slice(&subsystem.completed.to_le_bytes());
            out[offset + 8..offset + 12].copy_from_slice(&subsystem.failed.to_le_bytes());
        }
        out[48..52].copy_from_slice(&(self.core.pending_count() as u32).to_le_bytes());
        out[52..56].copy_from_slice(&self.core.next_seq().to_le_bytes());
        out
    }

    fn emit_event(&self, kind: u8, node: u64, value: u32) {
        let Some(sysd) = runtime::service_inbox(b"sysd") else {
            return;
        };
        let mut event = [0u8; 29];
        event[0] = kind;
        event[1..9].copy_from_slice(&node.to_le_bytes());
        event[17..21].copy_from_slice(&value.to_le_bytes());
        event[21..29].copy_from_slice(&self.clock.to_le_bytes());
        let _ = runtime::channel_send(sysd, &event, TAG_RUNTIME_EVENT);
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    runtime::panic(info)
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let inbox = runtime::service_inbox_or_die(b"trainerd");
    runtime::claim_inbox(inbox);
    let mut trainer = ProtectedTrainer::new();
    if !trainer.restore() {
        runtime::write_line(b"[trainerd] persisted job state corrupt; readiness withheld\n");
        runtime::exit(78);
    }
    runtime::write_line(b"[trainerd] protected bounded work engine online\n");
    let _ = runtime::bootstrap_named_status(b"service-ready:", b"trainerd");
    runtime::announce_service_ready(b"trainerd");
    let mut payload = [0u8; 256];
    loop {
        let raw = runtime::channel_recv(inbox, &mut payload);
        if raw == 0 || raw == u64::MAX {
            trainer.dispatch(true);
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
            TAG_JOB_SUBMIT => match trainer.submit(&payload[..len], reply) {
                Ok(seq) => {
                    let mut receipt = [0u8; 8];
                    receipt[4..8].copy_from_slice(&seq.to_le_bytes());
                    let _ = runtime::channel_send(reply, &receipt, TAG_JOB_REPORT);
                }
                Err(code) => {
                    let _ = runtime::channel_send(reply, &[code], TAG_JOB_REPORT);
                }
            },
            TAG_CONTROL => {
                let _ = trainer.control(&payload[..len]);
            }
            TAG_STATUS_QUERY | TAG_PING => {
                let _ = runtime::channel_send(
                    reply,
                    &trainer.status(),
                    if tag == TAG_PING {
                        TAG_PONG
                    } else {
                        TAG_STATUS_RESPONSE
                    },
                );
            }
            TAG_GRAPH_QUERY_RESULT => trainer.handle_graph_result(&payload[..len]),
            TAG_GRAPH_MUTATE_ACK => trainer.handle_graph_ack(&payload[..len]),
            TAG_SHUTDOWN => {
                let _ = trainer.persist();
                runtime::exit(0);
            }
            _ => {}
        }
        trainer.dispatch(false);
    }
}

fn send_graph(payload: &[u8], tag: u8) -> bool {
    runtime::service_inbox(b"graphd")
        .map(|graphd| runtime::channel_send(graphd, payload, tag) != u64::MAX)
        .unwrap_or(false)
}
fn send_graph_work(work: GraphWork) -> bool {
    let mut mutation = [0u8; 48];
    match work {
        GraphWork::AddNode {
            kind,
            flags,
            weight,
            parent,
            provenance,
        } => {
            mutation[0] = 0;
            mutation[1] = kind.min(u8::MAX as u16) as u8;
            mutation[4..8].copy_from_slice(&weight.to_le_bytes());
            mutation[8..16].copy_from_slice(&provenance.to_le_bytes());
            mutation[16..24].copy_from_slice(&parent.to_le_bytes());
            mutation[32..34].copy_from_slice(&flags.to_le_bytes());
        }
        GraphWork::AddEdge {
            kind,
            weight,
            from,
            to,
            provenance,
        } => {
            mutation[0] = 1;
            mutation[2] = kind.min(u8::MAX as u16) as u8;
            mutation[4..8].copy_from_slice(&weight.to_le_bytes());
            mutation[8..16].copy_from_slice(&from.to_le_bytes());
            mutation[16..24].copy_from_slice(&to.to_le_bytes());
            mutation[24..32].copy_from_slice(&provenance.to_le_bytes());
        }
    }
    send_graph(&mutation, TAG_GRAPH_MUTATE)
}

fn batch_path(mode: JobType, owner: u64) -> ([u8; 48], usize) {
    let prefix = if mode == JobType::Index {
        b"/persist/trainerd.index.".as_slice()
    } else {
        b"/persist/trainerd.hydrate.".as_slice()
    };
    let mut out = [0u8; 48];
    out[..prefix.len()].copy_from_slice(prefix);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for index in 0..16 {
        out[prefix.len() + index] = HEX[((owner >> ((15 - index) * 4)) & 0xf) as usize];
    }
    (out, prefix.len() + 16)
}
fn decode_state(value: u8) -> JobState {
    match value {
        1 => JobState::Running,
        2 => JobState::Paused,
        3 => JobState::Completed,
        4 => JobState::Failed,
        _ => JobState::Pending,
    }
}
fn read_u32(v: &[u8]) -> u32 {
    u32::from_le_bytes([v[0], v[1], v[2], v[3]])
}
fn read_i32(v: &[u8]) -> i32 {
    i32::from_le_bytes([v[0], v[1], v[2], v[3]])
}
fn read_u64(v: &[u8]) -> u64 {
    u64::from_le_bytes([v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]])
}
