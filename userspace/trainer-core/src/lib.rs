#![no_std]

pub const MAX_JOBS: usize = 32;
pub const SUBSYSTEM_COUNT: usize = 4;
pub const MAX_BATCH_RECORDS: usize = 16;
pub const BATCH_HEADER_SIZE: usize = 32;
pub const BATCH_RECORD_SIZE: usize = 40;
pub const MAX_BATCH_SIZE: usize = BATCH_HEADER_SIZE + MAX_BATCH_RECORDS * BATCH_RECORD_SIZE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum JobType {
    Index = 0,
    Correlate = 1,
    Spectral = 2,
    Hydrate = 3,
}

impl JobType {
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Index),
            1 => Some(Self::Correlate),
            2 => Some(Self::Spectral),
            3 => Some(Self::Hydrate),
            _ => None,
        }
    }
    pub const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum JobState {
    Pending = 0,
    Running = 1,
    Paused = 2,
    Completed = 3,
    Failed = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourcePolicy {
    pub cpu_budget: u8,
    pub io_budget: u8,
    pub schedule: u8,
}
/// Protocol values: budgets Low=0/Medium=1/High=2; schedule Idle=0/Always=1.
pub const DEFAULT_POLICY: ResourcePolicy = ResourcePolicy {
    cpu_budget: 2,
    io_budget: 2,
    schedule: 0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Job {
    pub seq: u32,
    pub kind: JobType,
    pub priority: u8,
    pub state: JobState,
    pub progress: u8,
    pub target: u64,
    pub submitted_at: u64,
    pub started_at: u64,
    pub completed_at: u64,
    pub requester: u32,
    pub verifier: i32,
    pub occupied: bool,
}

pub const EMPTY_JOB: Job = Job {
    seq: 0,
    kind: JobType::Index,
    priority: 0,
    state: JobState::Pending,
    progress: 0,
    target: 0,
    submitted_at: 0,
    started_at: 0,
    completed_at: 0,
    requester: 0,
    verifier: 0,
    occupied: false,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subsystem {
    pub state: JobState,
    pub completed: u32,
    pub failed: u32,
    pub policy: ResourcePolicy,
}
pub const EMPTY_SUBSYSTEM: Subsystem = Subsystem {
    state: JobState::Pending,
    completed: 0,
    failed: 0,
    policy: DEFAULT_POLICY,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerAction {
    Index { job_seq: u32, document_node: u64 },
    CorrelateQuery { job_seq: u32, entity_node: u64 },
    CorrelatePromote { job_seq: u32, from: u64, to: u64 },
    SpectralRefresh { job_seq: u32 },
    Hydrate { job_seq: u32, corpus_node: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreError {
    InvalidJob,
    QueueFull,
    Busy,
    MissingRunningJob,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceRecord {
    pub external_id: u64,
    pub parent_external_id: u64,
    pub relation_external_id: u64,
    pub kind: u16,
    pub relation_kind: u16,
    pub flags: u16,
    pub weight: i32,
}

pub const EMPTY_SOURCE_RECORD: SourceRecord = SourceRecord {
    external_id: 0,
    parent_external_id: 0,
    relation_external_id: 0,
    kind: 0,
    relation_kind: 0,
    flags: 0,
    weight: 0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordBatch {
    pub owner: u64,
    pub mode: JobType,
    pub count: usize,
    pub records: [SourceRecord; MAX_BATCH_RECORDS],
    pub checksum: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphWork {
    AddNode {
        kind: u16,
        flags: u16,
        weight: i32,
        parent: u64,
        provenance: u64,
    },
    AddEdge {
        kind: u16,
        weight: i32,
        from: u64,
        to: u64,
        provenance: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchError {
    InvalidHeader,
    WrongOwner,
    WrongMode,
    InvalidCount,
    InvalidRecord,
    Checksum,
    MissingReference,
    UnexpectedAck,
}

/// A bounded, acknowledgement-driven import/index execution. Node identifiers are
/// resolved only from graphd acknowledgements; relations cannot race ahead of nodes.
pub struct BatchExecution {
    batch: RecordBatch,
    resolved: [u64; MAX_BATCH_RECORDS],
    node_cursor: usize,
    edge_cursor: usize,
    awaiting: bool,
    awaiting_edge: bool,
    verifier: u64,
}

impl BatchExecution {
    pub const fn new(batch: RecordBatch) -> Self {
        Self {
            batch,
            resolved: [0; MAX_BATCH_RECORDS],
            node_cursor: 0,
            edge_cursor: 0,
            awaiting: false,
            awaiting_edge: false,
            verifier: 0xcbf2_9ce4_8422_2325,
        }
    }

    pub fn progress(&self) -> u8 {
        let edges = self.batch.records[..self.batch.count]
            .iter()
            .filter(|record| record.relation_external_id != 0)
            .count();
        let total = self.batch.count + edges;
        if total == 0 {
            100
        } else {
            (((self.node_cursor + self.edge_cursor) * 100) / total).min(99) as u8
        }
    }

    pub const fn verifier(&self) -> i32 {
        (self.verifier & 0x7fff_ffff) as i32
    }
    pub fn is_complete(&self) -> bool {
        !self.awaiting
            && self.node_cursor == self.batch.count
            && self.next_edge_index(self.edge_cursor).is_none()
    }

    pub fn next(&mut self) -> Result<Option<GraphWork>, BatchError> {
        if self.awaiting {
            return Err(BatchError::UnexpectedAck);
        }
        if self.node_cursor < self.batch.count {
            let record = self.batch.records[self.node_cursor];
            let parent = if record.parent_external_id == 0 {
                self.batch.owner
            } else {
                self.resolve(record.parent_external_id)?
            };
            self.awaiting = true;
            self.awaiting_edge = false;
            return Ok(Some(GraphWork::AddNode {
                kind: record.kind,
                flags: record.flags,
                weight: record.weight,
                parent,
                provenance: record.external_id,
            }));
        }
        if let Some(index) = self.next_edge_index(self.edge_cursor) {
            let record = self.batch.records[index];
            let from = self.resolved[index];
            let to = self.resolve(record.relation_external_id)?;
            self.edge_cursor = index;
            self.awaiting = true;
            self.awaiting_edge = true;
            return Ok(Some(GraphWork::AddEdge {
                kind: record.relation_kind,
                weight: record.weight,
                from,
                to,
                provenance: record.external_id,
            }));
        }
        Ok(None)
    }

    pub fn acknowledge(&mut self, object_id: u64, generation: u64) -> Result<(), BatchError> {
        if !self.awaiting || object_id == 0 {
            return Err(BatchError::UnexpectedAck);
        }
        self.verifier = hash_u64(hash_u64(self.verifier, object_id), generation);
        if self.awaiting_edge {
            self.edge_cursor += 1;
        } else {
            self.resolved[self.node_cursor] = object_id;
            self.node_cursor += 1;
        }
        self.awaiting = false;
        Ok(())
    }

    fn resolve(&self, external_id: u64) -> Result<u64, BatchError> {
        self.batch.records[..self.node_cursor]
            .iter()
            .position(|record| record.external_id == external_id)
            .map(|index| self.resolved[index])
            .filter(|id| *id != 0)
            .ok_or(BatchError::MissingReference)
    }

    fn next_edge_index(&self, start: usize) -> Option<usize> {
        (start..self.batch.count).find(|index| self.batch.records[*index].relation_external_id != 0)
    }
}

/// Decode the durable `TRB1` interchange used by INDEX and HYDRATE workers.
/// The checksum covers the complete record region and rejects partial VFS writes.
pub fn decode_batch(
    bytes: &[u8],
    expected_owner: u64,
    expected_mode: JobType,
) -> Result<RecordBatch, BatchError> {
    if bytes.len() < BATCH_HEADER_SIZE || &bytes[..4] != b"TRB1" || bytes[4] != 1 {
        return Err(BatchError::InvalidHeader);
    }
    let mode = JobType::from_u8(bytes[5]).ok_or(BatchError::WrongMode)?;
    if mode != expected_mode || !matches!(mode, JobType::Index | JobType::Hydrate) {
        return Err(BatchError::WrongMode);
    }
    let count = read_u16(&bytes[6..8]) as usize;
    if count == 0
        || count > MAX_BATCH_RECORDS
        || bytes.len() != BATCH_HEADER_SIZE + count * BATCH_RECORD_SIZE
    {
        return Err(BatchError::InvalidCount);
    }
    let owner = read_u64(&bytes[8..16]);
    if owner != expected_owner {
        return Err(BatchError::WrongOwner);
    }
    let expected_checksum = read_u64(&bytes[16..24]);
    if hash_bytes(&bytes[BATCH_HEADER_SIZE..]) != expected_checksum {
        return Err(BatchError::Checksum);
    }
    let mut records = [EMPTY_SOURCE_RECORD; MAX_BATCH_RECORDS];
    for index in 0..count {
        let offset = BATCH_HEADER_SIZE + index * BATCH_RECORD_SIZE;
        let record = SourceRecord {
            external_id: read_u64(&bytes[offset..offset + 8]),
            parent_external_id: read_u64(&bytes[offset + 8..offset + 16]),
            relation_external_id: read_u64(&bytes[offset + 16..offset + 24]),
            kind: read_u16(&bytes[offset + 24..offset + 26]),
            relation_kind: read_u16(&bytes[offset + 26..offset + 28]),
            flags: read_u16(&bytes[offset + 28..offset + 30]),
            weight: read_i32(&bytes[offset + 32..offset + 36]),
        };
        if record.external_id == 0
            || record.kind == 0
            || records[..index]
                .iter()
                .any(|prior| prior.external_id == record.external_id)
        {
            return Err(BatchError::InvalidRecord);
        }
        if record.parent_external_id != 0
            && !records[..index]
                .iter()
                .any(|prior| prior.external_id == record.parent_external_id)
        {
            return Err(BatchError::MissingReference);
        }
        records[index] = record;
    }
    for record in &records[..count] {
        if record.relation_external_id != 0
            && !records[..count]
                .iter()
                .any(|candidate| candidate.external_id == record.relation_external_id)
        {
            return Err(BatchError::MissingReference);
        }
    }
    Ok(RecordBatch {
        owner,
        mode,
        count,
        records,
        checksum: expected_checksum,
    })
}

pub fn hash_bytes(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ *byte as u64).wrapping_mul(0x100_0000_01b3)
    })
}
fn hash_u64(mut hash: u64, value: u64) -> u64 {
    for byte in value.to_le_bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(0x100_0000_01b3);
    }
    hash
}
fn read_u16(v: &[u8]) -> u16 {
    u16::from_le_bytes([v[0], v[1]])
}
fn read_i32(v: &[u8]) -> i32 {
    i32::from_le_bytes([v[0], v[1], v[2], v[3]])
}
fn read_u64(v: &[u8]) -> u64 {
    u64::from_le_bytes([v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]])
}

pub struct TrainerCore {
    jobs: [Job; MAX_JOBS],
    subsystems: [Subsystem; SUBSYSTEM_COUNT],
    next_seq: u32,
    running_seq: u32,
}

impl Default for TrainerCore {
    fn default() -> Self {
        Self::new()
    }
}

impl TrainerCore {
    pub const fn new() -> Self {
        Self {
            jobs: [EMPTY_JOB; MAX_JOBS],
            subsystems: [EMPTY_SUBSYSTEM; SUBSYSTEM_COUNT],
            next_seq: 1,
            running_seq: 0,
        }
    }
    pub const fn next_seq(&self) -> u32 {
        self.next_seq
    }
    pub const fn running_seq(&self) -> u32 {
        self.running_seq
    }
    pub fn jobs(&self) -> &[Job; MAX_JOBS] {
        &self.jobs
    }
    pub fn subsystems(&self) -> &[Subsystem; SUBSYSTEM_COUNT] {
        &self.subsystems
    }
    pub fn pending_count(&self) -> usize {
        self.jobs
            .iter()
            .filter(|job| job.occupied && job.state == JobState::Pending)
            .count()
    }
    pub fn running_job(&self) -> Option<Job> {
        self.jobs
            .iter()
            .find(|job| job.occupied && job.seq == self.running_seq)
            .copied()
    }

    pub fn submit(
        &mut self,
        kind: JobType,
        priority: u8,
        target: u64,
        submitted_at: u64,
        requester: u32,
    ) -> Result<u32, CoreError> {
        let Some(index) = self.jobs.iter().position(|job| {
            !job.occupied || matches!(job.state, JobState::Completed | JobState::Failed)
        }) else {
            return Err(CoreError::QueueFull);
        };
        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        self.jobs[index] = Job {
            seq,
            kind,
            priority,
            state: JobState::Pending,
            progress: 0,
            target,
            submitted_at,
            started_at: 0,
            completed_at: 0,
            requester,
            verifier: 0,
            occupied: true,
        };
        Ok(seq)
    }

    pub fn control(
        &mut self,
        kind: JobType,
        action: u8,
        policy: ResourcePolicy,
    ) -> Result<(), CoreError> {
        let subsystem = &mut self.subsystems[kind.index()];
        subsystem.policy = policy;
        subsystem.state = match action {
            0 | 2 => JobState::Running,
            1 => JobState::Paused,
            _ => return Err(CoreError::InvalidJob),
        };
        if action == 1 {
            if let Some(index) = self
                .running_index()
                .filter(|index| self.jobs[*index].kind == kind)
            {
                self.jobs[index].state = JobState::Paused;
                self.running_seq = 0;
            }
        }
        for job in self
            .jobs
            .iter_mut()
            .filter(|job| job.occupied && job.kind == kind && job.state == JobState::Paused)
        {
            if action == 2 {
                job.state = JobState::Pending;
            }
        }
        Ok(())
    }

    pub fn claim_next(&mut self, now: u64) -> Result<Option<WorkerAction>, CoreError> {
        self.claim_next_with_context(now, true)
    }

    /// Claim with execution context. `schedule == 1` is idle-only; zero CPU
    /// budget disables every worker and zero I/O budget disables INDEX/HYDRATE.
    pub fn claim_next_with_context(
        &mut self,
        now: u64,
        idle: bool,
    ) -> Result<Option<WorkerAction>, CoreError> {
        if self.running_seq != 0 {
            return Err(CoreError::Busy);
        }
        let mut best: Option<usize> = None;
        for (index, job) in self
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, job)| job.occupied && job.state == JobState::Pending)
        {
            let subsystem = self.subsystems[job.kind.index()];
            let cpu_period = 1u64 << (2 - subsystem.policy.cpu_budget.min(2));
            let io_period = 1u64 << (2 - subsystem.policy.io_budget.min(2));
            if subsystem.state == JobState::Paused
                || (subsystem.policy.schedule == 0 && !idle)
                || now % cpu_period != 0
                || (matches!(job.kind, JobType::Index | JobType::Hydrate) && now % io_period != 0)
            {
                continue;
            }
            if best
                .map(|current| {
                    priority_rank(job.priority) > priority_rank(self.jobs[current].priority)
                        || (priority_rank(job.priority)
                            == priority_rank(self.jobs[current].priority)
                            && job.submitted_at < self.jobs[current].submitted_at)
                })
                .unwrap_or(true)
            {
                best = Some(index);
            }
        }
        let Some(index) = best else {
            return Ok(None);
        };
        self.jobs[index].state = JobState::Running;
        self.jobs[index].progress = 10;
        self.jobs[index].started_at = now;
        self.running_seq = self.jobs[index].seq;
        self.subsystems[self.jobs[index].kind.index()].state = JobState::Running;
        Ok(Some(action_for(self.jobs[index])))
    }

    pub fn correlation_candidate(&mut self, peer: u64) -> Result<WorkerAction, CoreError> {
        let job = self.running_job().ok_or(CoreError::MissingRunningJob)?;
        if job.kind != JobType::Correlate || peer == 0 || peer == job.target {
            return Err(CoreError::InvalidJob);
        }
        if let Some(index) = self.running_index() {
            self.jobs[index].progress = 60;
        }
        Ok(WorkerAction::CorrelatePromote {
            job_seq: job.seq,
            from: job.target,
            to: peer,
        })
    }

    pub fn set_progress(&mut self, progress: u8) -> Result<(), CoreError> {
        let index = self.running_index().ok_or(CoreError::MissingRunningJob)?;
        self.jobs[index].progress = progress.min(99);
        Ok(())
    }

    pub fn complete(
        &mut self,
        success: bool,
        completed_at: u64,
        verifier: i32,
    ) -> Result<Job, CoreError> {
        let index = self.running_index().ok_or(CoreError::MissingRunningJob)?;
        let kind = self.jobs[index].kind.index();
        self.jobs[index].state = if success {
            JobState::Completed
        } else {
            JobState::Failed
        };
        if success {
            self.jobs[index].progress = 100;
            self.subsystems[kind].completed = self.subsystems[kind].completed.saturating_add(1);
        } else {
            self.subsystems[kind].failed = self.subsystems[kind].failed.saturating_add(1);
        }
        self.jobs[index].completed_at = completed_at;
        self.jobs[index].verifier = verifier;
        self.subsystems[kind].state = JobState::Pending;
        self.running_seq = 0;
        Ok(self.jobs[index])
    }

    pub fn recover_interrupted(&mut self) {
        if let Some(index) = self.running_index() {
            self.jobs[index].state = JobState::Pending;
            self.jobs[index].progress = 0;
        }
        self.running_seq = 0;
    }

    pub fn restore(
        &mut self,
        jobs: [Job; MAX_JOBS],
        subsystems: [Subsystem; SUBSYSTEM_COUNT],
        next_seq: u32,
        running_seq: u32,
    ) {
        self.jobs = jobs;
        self.subsystems = subsystems;
        self.next_seq = next_seq.max(1);
        self.running_seq = running_seq;
        self.recover_interrupted();
    }

    fn running_index(&self) -> Option<usize> {
        self.jobs
            .iter()
            .position(|job| job.occupied && job.seq == self.running_seq)
    }
}

fn action_for(job: Job) -> WorkerAction {
    match job.kind {
        JobType::Index => WorkerAction::Index {
            job_seq: job.seq,
            document_node: job.target,
        },
        JobType::Correlate => WorkerAction::CorrelateQuery {
            job_seq: job.seq,
            entity_node: job.target,
        },
        JobType::Spectral => WorkerAction::SpectralRefresh { job_seq: job.seq },
        JobType::Hydrate => WorkerAction::Hydrate {
            job_seq: job.seq,
            corpus_node: job.target,
        },
    }
}
fn priority_rank(priority: u8) -> u8 {
    match priority {
        1 => 2,
        0 => 1,
        2 => 0,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_highest_priority_and_waits_for_receipt() {
        let mut core = TrainerCore::new();
        core.submit(JobType::Index, 0, 10, 1, 4).unwrap();
        core.submit(JobType::Spectral, 1, 0, 2, 5).unwrap();
        assert_eq!(
            core.claim_next(3).unwrap(),
            Some(WorkerAction::SpectralRefresh { job_seq: 2 })
        );
        assert_eq!(core.claim_next(4), Err(CoreError::Busy));
        assert_eq!(
            core.complete(true, 5, 99).unwrap().state,
            JobState::Completed
        );
    }

    #[test]
    fn pause_and_recovery_preserve_work() {
        let mut core = TrainerCore::new();
        core.submit(JobType::Hydrate, 1, 42, 1, 7).unwrap();
        core.control(JobType::Hydrate, 1, DEFAULT_POLICY).unwrap();
        assert_eq!(core.claim_next(2).unwrap(), None);
        core.control(JobType::Hydrate, 2, DEFAULT_POLICY).unwrap();
        assert!(matches!(
            core.claim_next(3).unwrap(),
            Some(WorkerAction::Hydrate { .. })
        ));
        core.recover_interrupted();
        assert_eq!(core.pending_count(), 1);
    }

    #[test]
    fn budgets_and_idle_schedule_gate_claiming() {
        let mut core = TrainerCore::new();
        core.submit(JobType::Index, 1, 42, 1, 7).unwrap();
        core.control(
            JobType::Index,
            0,
            ResourcePolicy {
                cpu_budget: 1,
                io_budget: 1,
                schedule: 0,
            },
        )
        .unwrap();
        assert_eq!(core.claim_next_with_context(2, false).unwrap(), None);
        assert!(core.claim_next_with_context(4, true).unwrap().is_some());
        core.complete(true, 5, 1).unwrap();
        core.submit(JobType::Hydrate, 1, 43, 5, 7).unwrap();
        core.control(
            JobType::Hydrate,
            0,
            ResourcePolicy {
                cpu_budget: 0,
                io_budget: 0,
                schedule: 1,
            },
        )
        .unwrap();
        assert_eq!(core.claim_next_with_context(6, true).unwrap(), None);
        assert!(core.claim_next_with_context(8, false).unwrap().is_some());
    }

    #[test]
    fn correlation_requires_a_distinct_candidate() {
        let mut core = TrainerCore::new();
        core.submit(JobType::Correlate, 1, 10, 1, 7).unwrap();
        core.claim_next(2).unwrap();
        assert_eq!(core.correlation_candidate(10), Err(CoreError::InvalidJob));
        assert_eq!(
            core.correlation_candidate(11).unwrap(),
            WorkerAction::CorrelatePromote {
                job_seq: 1,
                from: 10,
                to: 11
            }
        );
    }

    #[test]
    fn batch_waits_for_graph_ack_and_resolves_relations() {
        let mut bytes = [0u8; BATCH_HEADER_SIZE + 2 * BATCH_RECORD_SIZE];
        bytes[..4].copy_from_slice(b"TRB1");
        bytes[4] = 1;
        bytes[5] = JobType::Index as u8;
        bytes[6..8].copy_from_slice(&2u16.to_le_bytes());
        bytes[8..16].copy_from_slice(&77u64.to_le_bytes());
        let first = BATCH_HEADER_SIZE;
        bytes[first..first + 8].copy_from_slice(&100u64.to_le_bytes());
        bytes[first + 24..first + 26].copy_from_slice(&2u16.to_le_bytes());
        let second = first + BATCH_RECORD_SIZE;
        bytes[second..second + 8].copy_from_slice(&101u64.to_le_bytes());
        bytes[second + 8..second + 16].copy_from_slice(&100u64.to_le_bytes());
        bytes[second + 16..second + 24].copy_from_slice(&100u64.to_le_bytes());
        bytes[second + 24..second + 26].copy_from_slice(&3u16.to_le_bytes());
        bytes[second + 26..second + 28].copy_from_slice(&9u16.to_le_bytes());
        let checksum = hash_bytes(&bytes[BATCH_HEADER_SIZE..]);
        bytes[16..24].copy_from_slice(&checksum.to_le_bytes());
        let batch = decode_batch(&bytes, 77, JobType::Index).unwrap();
        let mut execution = BatchExecution::new(batch);
        assert!(matches!(
            execution.next().unwrap(),
            Some(GraphWork::AddNode { parent: 77, .. })
        ));
        assert_eq!(execution.next(), Err(BatchError::UnexpectedAck));
        execution.acknowledge(500, 1).unwrap();
        assert!(matches!(
            execution.next().unwrap(),
            Some(GraphWork::AddNode { parent: 500, .. })
        ));
        execution.acknowledge(501, 2).unwrap();
        assert!(matches!(
            execution.next().unwrap(),
            Some(GraphWork::AddEdge {
                from: 501,
                to: 500,
                ..
            })
        ));
        execution.acknowledge(900, 3).unwrap();
        assert!(execution.is_complete());
        assert_ne!(execution.verifier(), 0);
    }
}
