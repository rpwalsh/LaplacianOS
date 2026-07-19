//! Host-side trainer harness.
//!
//! This package is deliberately not a production service. LaplacianOS runs the
//! protected ring-3 `trainerd` binary; the host package drives the exact same
//! scheduler and worker state machines against an in-process graph runtime.

pub use laplacianos_trainer_core::*;

use laplacianos_graph_runtime::{GraphRuntime, Mutation};

/// Execute a decoded INDEX or HYDRATE batch exactly as the protected adapter
/// does: one mutation at a time, with graph acknowledgements resolving IDs.
pub fn execute_batch(
    graph: &mut GraphRuntime,
    batch: RecordBatch,
    now: &mut u64,
) -> Result<i32, BatchError> {
    let mut execution = BatchExecution::new(batch);
    while !execution.is_complete() {
        let Some(work) = execution.next()? else {
            break;
        };
        *now = now.saturating_add(1);
        let (mutation, provenance) = match work {
            GraphWork::AddNode {
                kind,
                flags,
                weight,
                parent,
                provenance,
            } => (
                Mutation::AddNode {
                    kind,
                    flags,
                    weight,
                    parent,
                    now: *now,
                },
                provenance,
            ),
            GraphWork::AddEdge {
                kind,
                weight,
                from,
                to,
                provenance,
            } => (
                Mutation::AddEdge {
                    kind,
                    flags: 0,
                    weight,
                    from,
                    to,
                    now: *now,
                },
                provenance,
            ),
        };
        let object_id = graph
            .apply(mutation, provenance)
            .map_err(|_| BatchError::UnexpectedAck)?;
        execution.acknowledge(object_id, graph.generation())?;
    }
    Ok(execution.verifier())
}

/// Run relation candidate selection and promotion through the shared core.
pub fn execute_correlation(
    core: &mut TrainerCore,
    graph: &mut GraphRuntime,
    now: &mut u64,
) -> Result<i32, CoreError> {
    let job = core.running_job().ok_or(CoreError::MissingRunningJob)?;
    let neighborhood = graph.scored_neighborhood(job.target, *now, 8);
    let peer = neighborhood.nodes[..neighborhood.count]
        .iter()
        .map(|node| node.node.id)
        .find(|id| *id != job.target)
        .ok_or(CoreError::InvalidJob)?;
    let WorkerAction::CorrelatePromote { from, to, job_seq } = core.correlation_candidate(peer)?
    else {
        return Err(CoreError::InvalidJob);
    };
    *now = now.saturating_add(1);
    let edge = graph
        .apply(
            Mutation::AddEdge {
                kind: 30,
                flags: 0,
                weight: 1,
                from,
                to,
                now: *now,
            },
            job_seq as u64,
        )
        .map_err(|_| CoreError::InvalidJob)?;
    core.complete(true, *now, (edge & 0x7fff_ffff) as i32)
        .map(|job| job.verifier)
}

/// Refresh and verify graph structural/spectral state without entering the
/// interactive query path.
pub fn execute_spectral(
    core: &mut TrainerCore,
    graph: &GraphRuntime,
    now: u64,
) -> Result<i32, CoreError> {
    let snapshot = graph.spectral_snapshot();
    let verifier =
        ((snapshot.checksum ^ snapshot.spectral_energy ^ snapshot.dominant_eigen_q16 as u32 as u64)
            & 0x7fff_ffff) as i32;
    core.complete(true, now, verifier).map(|job| job.verifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded_batch(mode: JobType, owner: u64) -> Vec<u8> {
        let mut bytes = vec![0u8; BATCH_HEADER_SIZE + 2 * BATCH_RECORD_SIZE];
        bytes[..4].copy_from_slice(b"TRB1");
        bytes[4] = 1;
        bytes[5] = mode as u8;
        bytes[6..8].copy_from_slice(&2u16.to_le_bytes());
        bytes[8..16].copy_from_slice(&owner.to_le_bytes());
        let a = BATCH_HEADER_SIZE;
        bytes[a..a + 8].copy_from_slice(&101u64.to_le_bytes());
        bytes[a + 24..a + 26].copy_from_slice(&2u16.to_le_bytes());
        bytes[a + 32..a + 36].copy_from_slice(&8i32.to_le_bytes());
        let b = a + BATCH_RECORD_SIZE;
        bytes[b..b + 8].copy_from_slice(&102u64.to_le_bytes());
        bytes[b + 8..b + 16].copy_from_slice(&101u64.to_le_bytes());
        bytes[b + 16..b + 24].copy_from_slice(&101u64.to_le_bytes());
        bytes[b + 24..b + 26].copy_from_slice(&3u16.to_le_bytes());
        bytes[b + 26..b + 28].copy_from_slice(&7u16.to_le_bytes());
        bytes[b + 32..b + 36].copy_from_slice(&13i32.to_le_bytes());
        let checksum = hash_bytes(&bytes[BATCH_HEADER_SIZE..]);
        bytes[16..24].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    #[test]
    fn index_and_hydrate_share_acknowledged_batch_engine() {
        for mode in [JobType::Index, JobType::Hydrate] {
            let owner = 1;
            let bytes = encoded_batch(mode, owner);
            let batch = decode_batch(&bytes, owner, mode).unwrap();
            let mut graph = GraphRuntime::new();
            let mut now = 0;
            assert_eq!(
                graph
                    .apply(
                        Mutation::AddNode {
                            kind: 1,
                            flags: 0,
                            weight: 1,
                            parent: 0,
                            now
                        },
                        99
                    )
                    .unwrap(),
                owner
            );
            let verifier = execute_batch(&mut graph, batch, &mut now).unwrap();
            assert_eq!(graph.spectral_snapshot().active_nodes, 3);
            assert_eq!(graph.spectral_snapshot().active_edges, 1);
            assert_ne!(verifier, 0);
            execute_batch(&mut graph, batch, &mut now).unwrap();
            assert_eq!(graph.spectral_snapshot().active_nodes, 3);
            assert_eq!(graph.spectral_snapshot().active_edges, 1);
        }
    }

    #[test]
    fn correlation_and_spectral_complete_through_shared_scheduler() {
        let mut graph = GraphRuntime::new();
        let mut now = 1;
        let a = graph
            .apply(
                Mutation::AddNode {
                    kind: 1,
                    flags: 0,
                    weight: 10,
                    parent: 0,
                    now,
                },
                1,
            )
            .unwrap();
        now += 1;
        let b = graph
            .apply(
                Mutation::AddNode {
                    kind: 1,
                    flags: 0,
                    weight: 9,
                    parent: 0,
                    now,
                },
                2,
            )
            .unwrap();
        now += 1;
        graph
            .apply(
                Mutation::AddEdge {
                    kind: 1,
                    flags: 0,
                    weight: 5,
                    from: a,
                    to: b,
                    now,
                },
                3,
            )
            .unwrap();
        let mut core = TrainerCore::new();
        core.submit(JobType::Correlate, 1, a, now, 8).unwrap();
        assert!(matches!(
            core.claim_next(now + 1).unwrap(),
            Some(WorkerAction::CorrelateQuery { .. })
        ));
        assert_ne!(
            execute_correlation(&mut core, &mut graph, &mut now).unwrap(),
            0
        );
        core.submit(JobType::Spectral, 1, 0, now, 8).unwrap();
        assert!(matches!(
            core.claim_next(now + 1).unwrap(),
            Some(WorkerAction::SpectralRefresh { .. })
        ));
        assert_ne!(execute_spectral(&mut core, &graph, now + 2).unwrap(), 0);
    }
}
