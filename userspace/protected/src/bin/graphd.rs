#![no_std]
#![no_main]

#[path = "../runtime.rs"]
mod runtime;

use core::panic::PanicInfo;
use laplacianos_graph_runtime::{GraphRuntime, Mutation, QueryResult, SNAPSHOT_CAP};

const TAG_PING: u8 = 0x01;
const TAG_PONG: u8 = 0x02;
const TAG_ERROR: u8 = 0x03;
const TAG_SHUTDOWN: u8 = 0x04;
const TAG_QUERY: u8 = 0x10;
const TAG_QUERY_RESULT: u8 = 0x11;
const TAG_MUTATE: u8 = 0x12;
const TAG_MUTATE_ACK: u8 = 0x13;
const TAG_SUBSCRIBE: u8 = 0x14;
const TAG_GRAPH_EVENT: u8 = 0x15;
const STATE_PATH: &[u8] = b"/persist/graphd.snapshot";
const MAX_SUBSCRIBERS: usize = 16;

struct ProtectedGraphd {
    graph: GraphRuntime,
    subscribers: [u32; MAX_SUBSCRIBERS],
    logical_clock: u64,
    baseline_generation: u64,
    baseline_nodes: u32,
    baseline_edges: u32,
}

impl ProtectedGraphd {
    const fn new() -> Self {
        Self {
            graph: GraphRuntime::new(),
            subscribers: [0; MAX_SUBSCRIBERS],
            logical_clock: 0,
            baseline_generation: 0,
            baseline_nodes: 0,
            baseline_edges: 0,
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
        if self.graph.decode_snapshot(&snapshot).is_err() {
            return false;
        }
        let spectral = self.graph.spectral_snapshot();
        self.logical_clock = self.graph.generation();
        self.baseline_generation = spectral.generation;
        self.baseline_nodes = spectral.active_nodes;
        self.baseline_edges = spectral.active_edges;
        true
    }

    fn persist(&self) -> bool {
        let _ = runtime::vfs_mkdir(b"/persist");
        let mut snapshot = [0u8; SNAPSHOT_CAP];
        let Ok(len) = self.graph.encode_snapshot(&mut snapshot) else {
            return false;
        };
        let fd = runtime::vfs_create(STATE_PATH);
        if fd == u64::MAX {
            return false;
        }
        let written = runtime::vfs_write(fd, &snapshot[..len]);
        let closed = runtime::vfs_close(fd);
        written == len as u64 && closed
    }

    fn now(&mut self) -> u64 {
        self.logical_clock = self.logical_clock.saturating_add(1);
        self.logical_clock
    }

    fn mutate(&mut self, payload: &[u8]) -> Result<u64, u8> {
        if payload.len() < 48 {
            return Err(1);
        }
        let now = self.now();
        let weight = read_i32(&payload[4..8]);
        let from = read_u64(&payload[8..16]);
        let to = read_u64(&payload[16..24]);
        let target = read_u64(&payload[24..32]);
        let flags = read_u16(&payload[32..34]);
        let mutation = match payload[0] {
            0 => Mutation::AddNode {
                kind: payload[1] as u16,
                flags,
                weight,
                parent: to,
                now,
            },
            1 => Mutation::AddEdge {
                kind: payload[2] as u16,
                flags,
                weight,
                from,
                to,
                now,
            },
            2 => Mutation::UpdateNode {
                id: target,
                flags,
                weight,
                now,
            },
            3 => Mutation::UpdateEdge {
                id: target,
                weight,
                now,
            },
            4 => Mutation::DeactivateNode { id: target, now },
            5 => Mutation::DeactivateEdge { id: target, now },
            _ => return Err(2),
        };
        let object_id = self.graph.apply(mutation, from).map_err(|_| 3)?;
        if !self.persist() {
            return Err(4);
        }
        self.publish_event(payload[0], object_id);
        Ok(object_id)
    }

    fn query(&mut self, payload: &[u8], out: &mut [u8; 160]) -> usize {
        if payload.len() < 32 {
            out[0] = 1;
            return 16;
        }
        let kind = payload[0];
        let max_results = payload[4].max(1) as usize;
        let target = read_u64(&payload[8..16]);
        let since = read_u64(&payload[16..24]);
        let now = self.now();
        let result = match kind {
            0 => {
                let mut result = empty_result(self.graph.generation());
                if let Some(node) = self.graph.node(target) {
                    result.nodes[0] = laplacianos_graph_runtime::ScoredNode {
                        node,
                        score: node.weight,
                    };
                    result.count = 1;
                }
                result
            }
            3 => self.graph.nodes_by_kind(payload[1] as u16, max_results),
            4 => self.graph.scored_neighborhood(target, now, max_results),
            8 => self.graph.provenance(target),
            9 | 10 => empty_result(self.graph.generation()),
            _ => self.graph.scored_neighborhood(target, now, max_results),
        };
        if kind == 6 {
            return encode_structural(&self.graph, out);
        }
        if kind == 7 {
            return encode_drift(
                &self.graph,
                if since == 0 {
                    self.baseline_generation
                } else {
                    since
                },
                self.baseline_nodes,
                self.baseline_edges,
                out,
            );
        }
        if kind == 10 {
            return encode_stats(&self.graph, out);
        }
        encode_query_result(&result, out)
    }

    fn subscribe(&mut self, endpoint: u32) -> bool {
        if endpoint == 0 {
            return false;
        }
        if self.subscribers.contains(&endpoint) {
            return true;
        }
        let Some(slot) = self.subscribers.iter_mut().find(|slot| **slot == 0) else {
            return false;
        };
        *slot = endpoint;
        true
    }

    fn publish_event(&self, mutation_kind: u8, object_id: u64) {
        let mut event = [0u8; 24];
        event[0] = mutation_kind;
        event[8..16].copy_from_slice(&self.graph.generation().to_le_bytes());
        event[16..24].copy_from_slice(&object_id.to_le_bytes());
        for endpoint in self
            .subscribers
            .iter()
            .copied()
            .filter(|endpoint| *endpoint != 0)
        {
            let _ = runtime::channel_send(endpoint, &event, TAG_GRAPH_EVENT);
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    runtime::panic(info)
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let inbox = runtime::service_inbox_or_die(b"graphd");
    runtime::claim_inbox(inbox);
    let mut service = ProtectedGraphd::new();
    if !service.restore() {
        runtime::write_line(b"[graphd] snapshot invalid; readiness withheld\n");
        runtime::exit(78);
    }
    runtime::write_line(b"[graphd] durable analytical graph service online\n");
    let _ = runtime::bootstrap_named_status(b"service-ready:", b"graphd");
    runtime::announce_service_ready(b"graphd");

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
            TAG_QUERY => {
                let mut response = [0u8; 160];
                let response_len = service.query(&payload[..len], &mut response);
                let _ = runtime::channel_send(reply, &response[..response_len], TAG_QUERY_RESULT);
            }
            TAG_MUTATE => {
                let result = service.mutate(&payload[..len]);
                let mut ack = [0u8; 24];
                ack[0] = result.as_ref().err().copied().unwrap_or(0);
                ack[8..16].copy_from_slice(&service.graph.generation().to_le_bytes());
                ack[16..24].copy_from_slice(&result.unwrap_or(0).to_le_bytes());
                let _ = runtime::channel_send(reply, &ack, TAG_MUTATE_ACK);
            }
            TAG_SUBSCRIBE => {
                let ok = service.subscribe(reply);
                let _ = runtime::channel_send(reply, &[u8::from(!ok)], TAG_PONG);
            }
            TAG_PING => {
                let _ = runtime::channel_send(
                    reply,
                    &service.graph.generation().to_le_bytes(),
                    TAG_PONG,
                );
            }
            TAG_SHUTDOWN => {
                let _ = service.persist();
                runtime::exit(0);
            }
            _ => {
                let _ = runtime::channel_send(reply, b"unsupported graphd request", TAG_ERROR);
            }
        }
    }
}

fn empty_result(generation: u64) -> QueryResult {
    QueryResult {
        generation,
        count: 0,
        nodes: [laplacianos_graph_runtime::EMPTY_SCORED_NODE; 8],
    }
}

fn encode_query_result(result: &QueryResult, out: &mut [u8]) -> usize {
    out[..144].fill(0);
    out[1] = result.count as u8;
    out[8..16].copy_from_slice(&result.generation.to_le_bytes());
    for (index, scored) in result.nodes.iter().take(result.count).enumerate() {
        let offset = 16 + index * 16;
        out[offset..offset + 8].copy_from_slice(&scored.node.id.to_le_bytes());
        out[offset + 8..offset + 10].copy_from_slice(&scored.node.kind.to_le_bytes());
        out[offset + 10..offset + 12].copy_from_slice(&scored.node.flags.to_le_bytes());
        out[offset + 12..offset + 16].copy_from_slice(&scored.score.to_le_bytes());
    }
    144
}

fn encode_structural(graph: &GraphRuntime, out: &mut [u8]) -> usize {
    out[..56].fill(0);
    let snapshot = graph.spectral_snapshot();
    out[0..4].copy_from_slice(&snapshot.active_nodes.to_le_bytes());
    out[4..8].copy_from_slice(&snapshot.active_edges.to_le_bytes());
    out[8] = graph
        .nodes()
        .map(|node| node.kind)
        .fold(0u64, |bits, kind| bits | (1u64 << (kind % 64)))
        .count_ones() as u8;
    out[9] = u8::from(snapshot.active_nodes > 0);
    out[10] = snapshot.max_degree.min(u8::MAX as u32) as u8;
    let density = if snapshot.active_nodes > 1 {
        ((snapshot.active_edges as u64 * 65_536)
            / (snapshot.active_nodes as u64 * (snapshot.active_nodes as u64 - 1))) as i32
    } else {
        0
    };
    out[16..20].copy_from_slice(&density.to_le_bytes());
    out[24..32].copy_from_slice(&snapshot.generation.to_le_bytes());
    out[32..40].copy_from_slice(&snapshot.checksum.to_le_bytes());
    out[40..44].copy_from_slice(&snapshot.dominant_eigen_q16.to_le_bytes());
    out[48..56].copy_from_slice(&snapshot.spectral_energy.to_le_bytes());
    56
}

fn encode_drift(graph: &GraphRuntime, from: u64, nodes: u32, edges: u32, out: &mut [u8]) -> usize {
    out[..112].fill(0);
    let drift = graph.drift_report(from, nodes, edges);
    out[0] = 3;
    out[1] = u8::from(drift.journal_truncated || drift.mutation_count > 64);
    out[8..16].copy_from_slice(&drift.to_generation.to_le_bytes());
    write_drift_entry(
        out,
        16,
        2,
        drift.mutation_count as i32,
        drift.mutation_count as i32,
    );
    write_drift_entry(out, 40, 3, drift.node_delta, drift.node_delta);
    write_drift_entry(out, 64, 4, drift.edge_delta, drift.edge_delta);
    112
}

fn write_drift_entry(out: &mut [u8], offset: usize, metric: u8, current: i32, delta: i32) {
    out[offset + 8] = metric;
    out[offset + 9] = if delta > 0 {
        1
    } else if delta < 0 {
        2
    } else {
        0
    };
    out[offset + 12..offset + 16].copy_from_slice(&current.to_le_bytes());
    out[offset + 16..offset + 20].copy_from_slice(&delta.to_le_bytes());
}

fn encode_stats(graph: &GraphRuntime, out: &mut [u8]) -> usize {
    out[..32].fill(0);
    let snapshot = graph.spectral_snapshot();
    out[0..8].copy_from_slice(&snapshot.generation.to_le_bytes());
    out[8..12].copy_from_slice(&snapshot.active_nodes.to_le_bytes());
    out[12..16].copy_from_slice(&snapshot.active_edges.to_le_bytes());
    out[16..24].copy_from_slice(&snapshot.checksum.to_le_bytes());
    32
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
