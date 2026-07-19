#![no_std]

pub const MAX_NODES: usize = 128;
pub const MAX_EDGES: usize = 256;
pub const MAX_JOURNAL: usize = 256;
pub const MAX_RESULTS: usize = 8;
pub const SNAPSHOT_VERSION: u32 = 2;
pub const SNAPSHOT_CAP: usize = 32 + MAX_NODES * 56 + MAX_EDGES * 56 + MAX_JOURNAL * 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: u64,
    pub kind: u16,
    pub flags: u16,
    pub weight: i32,
    pub created_at: u64,
    pub updated_at: u64,
    pub provenance: u64,
    pub parent: u64,
    pub active: bool,
}

pub const EMPTY_NODE: Node = Node {
    id: 0,
    kind: 0,
    flags: 0,
    weight: 0,
    created_at: 0,
    updated_at: 0,
    provenance: 0,
    parent: 0,
    active: false,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub id: u64,
    pub from: u64,
    pub to: u64,
    pub kind: u16,
    pub flags: u16,
    pub weight: i32,
    pub created_at: u64,
    pub provenance: u64,
    pub active: bool,
}

pub const EMPTY_EDGE: Edge = Edge {
    id: 0,
    from: 0,
    to: 0,
    kind: 0,
    flags: 0,
    weight: 0,
    created_at: 0,
    provenance: 0,
    active: false,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutation {
    AddNode {
        kind: u16,
        flags: u16,
        weight: i32,
        parent: u64,
        now: u64,
    },
    AddEdge {
        kind: u16,
        flags: u16,
        weight: i32,
        from: u64,
        to: u64,
        now: u64,
    },
    UpdateNode {
        id: u64,
        flags: u16,
        weight: i32,
        now: u64,
    },
    UpdateEdge {
        id: u64,
        weight: i32,
        now: u64,
    },
    DeactivateNode {
        id: u64,
        now: u64,
    },
    DeactivateEdge {
        id: u64,
        now: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphError {
    Capacity,
    MissingNode,
    MissingEdge,
    InvalidEdge,
    InvalidSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JournalEntry {
    pub generation: u64,
    pub timestamp: u64,
    pub mutation_kind: u8,
    pub object_id: u64,
    pub provenance: u64,
}

const EMPTY_JOURNAL: JournalEntry = JournalEntry {
    generation: 0,
    timestamp: 0,
    mutation_kind: 0,
    object_id: 0,
    provenance: 0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScoredNode {
    pub node: Node,
    pub score: i32,
}

pub const EMPTY_SCORED_NODE: ScoredNode = ScoredNode {
    node: EMPTY_NODE,
    score: i32::MIN,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryResult {
    pub generation: u64,
    pub count: usize,
    pub nodes: [ScoredNode; MAX_RESULTS],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpectralSnapshot {
    pub generation: u64,
    pub active_nodes: u32,
    pub active_edges: u32,
    pub laplacian_trace: u64,
    pub max_degree: u32,
    /// Dominant graph-Laplacian eigenvalue in signed Q16 fixed point.
    pub dominant_eigen_q16: i32,
    /// Squared residual energy from the final bounded power iteration.
    pub spectral_energy: u64,
    pub checksum: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriftReport {
    pub from_generation: u64,
    pub to_generation: u64,
    pub mutation_count: u32,
    pub node_delta: i32,
    pub edge_delta: i32,
    pub journal_truncated: bool,
}

pub struct GraphRuntime {
    nodes: [Node; MAX_NODES],
    edges: [Edge; MAX_EDGES],
    journal: [JournalEntry; MAX_JOURNAL],
    journal_head: usize,
    journal_len: usize,
    generation: u64,
    next_id: u64,
}

impl Default for GraphRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphRuntime {
    pub const fn new() -> Self {
        Self {
            nodes: [EMPTY_NODE; MAX_NODES],
            edges: [EMPTY_EDGE; MAX_EDGES],
            journal: [EMPTY_JOURNAL; MAX_JOURNAL],
            journal_head: 0,
            journal_len: 0,
            generation: 0,
            next_id: 1,
        }
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter(|node| node.active)
    }
    pub fn edges(&self) -> impl Iterator<Item = &Edge> {
        self.edges.iter().filter(|edge| edge.active)
    }
    pub fn node(&self, id: u64) -> Option<Node> {
        self.nodes().find(|node| node.id == id).copied()
    }
    pub fn edge(&self, id: u64) -> Option<Edge> {
        self.edges().find(|edge| edge.id == id).copied()
    }

    pub fn apply(&mut self, mutation: Mutation, provenance: u64) -> Result<u64, GraphError> {
        let (kind, object_id, timestamp) = match mutation {
            Mutation::AddNode {
                kind,
                flags,
                weight,
                parent,
                now,
            } => {
                if parent != 0 && self.node(parent).is_none() {
                    return Err(GraphError::MissingNode);
                }
                if provenance != 0 {
                    if let Some(existing) = self
                        .nodes()
                        .find(|node| node.provenance == provenance && node.kind == kind)
                    {
                        return Ok(existing.id);
                    }
                }
                let id = self.allocate_id();
                let slot = self
                    .nodes
                    .iter_mut()
                    .find(|node| !node.active)
                    .ok_or(GraphError::Capacity)?;
                *slot = Node {
                    id,
                    kind,
                    flags,
                    weight,
                    created_at: now,
                    updated_at: now,
                    provenance,
                    parent,
                    active: true,
                };
                (0, id, now)
            }
            Mutation::AddEdge {
                kind,
                flags,
                weight,
                from,
                to,
                now,
            } => {
                if self.node(from).is_none() || self.node(to).is_none() {
                    return Err(GraphError::InvalidEdge);
                }
                if provenance != 0 {
                    if let Some(existing) = self.edges().find(|edge| {
                        edge.provenance == provenance
                            && edge.kind == kind
                            && edge.from == from
                            && edge.to == to
                    }) {
                        return Ok(existing.id);
                    }
                }
                let id = self.allocate_id();
                let slot = self
                    .edges
                    .iter_mut()
                    .find(|edge| !edge.active)
                    .ok_or(GraphError::Capacity)?;
                *slot = Edge {
                    id,
                    from,
                    to,
                    kind,
                    flags,
                    weight,
                    created_at: now,
                    provenance,
                    active: true,
                };
                (1, id, now)
            }
            Mutation::UpdateNode {
                id,
                flags,
                weight,
                now,
            } => {
                let node = self
                    .nodes
                    .iter_mut()
                    .find(|node| node.active && node.id == id)
                    .ok_or(GraphError::MissingNode)?;
                node.flags = flags;
                node.weight = weight;
                node.updated_at = now;
                (2, id, now)
            }
            Mutation::UpdateEdge { id, weight, now } => {
                let edge = self
                    .edges
                    .iter_mut()
                    .find(|edge| edge.active && edge.id == id)
                    .ok_or(GraphError::MissingEdge)?;
                edge.weight = weight;
                (3, id, now)
            }
            Mutation::DeactivateNode { id, now } => {
                let node = self
                    .nodes
                    .iter_mut()
                    .find(|node| node.active && node.id == id)
                    .ok_or(GraphError::MissingNode)?;
                node.active = false;
                node.updated_at = now;
                for edge in &mut self.edges {
                    if edge.active && (edge.from == id || edge.to == id) {
                        edge.active = false;
                    }
                }
                (4, id, now)
            }
            Mutation::DeactivateEdge { id, now } => {
                let edge = self
                    .edges
                    .iter_mut()
                    .find(|edge| edge.active && edge.id == id)
                    .ok_or(GraphError::MissingEdge)?;
                edge.active = false;
                (5, id, now)
            }
        };
        self.generation = self.generation.saturating_add(1);
        self.append_journal(JournalEntry {
            generation: self.generation,
            timestamp,
            mutation_kind: kind,
            object_id,
            provenance,
        });
        Ok(object_id)
    }

    pub fn scored_neighborhood(&self, target: u64, now: u64, max_results: usize) -> QueryResult {
        let mut result = QueryResult {
            generation: self.generation,
            count: 0,
            nodes: [EMPTY_SCORED_NODE; MAX_RESULTS],
        };
        let limit = max_results.min(MAX_RESULTS);
        for edge in self
            .edges()
            .filter(|edge| edge.from == target || edge.to == target)
        {
            let peer = if edge.from == target {
                edge.to
            } else {
                edge.from
            };
            let Some(node) = self.node(peer) else {
                continue;
            };
            let age = now.saturating_sub(node.updated_at).min(86_400);
            let recency = 65_536i64.saturating_sub((age as i64 * 65_536) / 86_401);
            let score = (node.weight as i64 + edge.weight as i64 + recency)
                .clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            insert_ranked(&mut result, ScoredNode { node, score }, limit);
        }
        result
    }

    pub fn nodes_by_kind(&self, kind: u16, max_results: usize) -> QueryResult {
        let mut result = QueryResult {
            generation: self.generation,
            count: 0,
            nodes: [EMPTY_SCORED_NODE; MAX_RESULTS],
        };
        let limit = max_results.min(MAX_RESULTS);
        for node in self.nodes().filter(|node| node.kind == kind) {
            insert_ranked(
                &mut result,
                ScoredNode {
                    node: *node,
                    score: node.weight,
                },
                limit,
            );
        }
        result
    }

    pub fn deterministic_walk(&self, start: u64, seed: u64, steps: usize) -> [u64; MAX_RESULTS] {
        let mut out = [0u64; MAX_RESULTS];
        let mut current = start;
        let mut rng = seed ^ start ^ self.generation;
        let count = steps.min(MAX_RESULTS);
        for slot in out.iter_mut().take(count) {
            *slot = current;
            let degree = self.edges().filter(|edge| edge.from == current).count();
            if degree == 0 {
                break;
            }
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let pick = (rng as usize) % degree;
            if let Some(edge) = self.edges().filter(|edge| edge.from == current).nth(pick) {
                current = edge.to;
            }
        }
        out
    }

    pub fn spectral_snapshot(&self) -> SpectralSnapshot {
        let active_nodes = self.nodes().count() as u32;
        let active_edges = self.edges().count() as u32;
        let mut max_degree = 0u32;
        let mut checksum = 0xcbf29ce484222325u64;
        for node in self.nodes() {
            let degree = self
                .edges()
                .filter(|edge| edge.from == node.id || edge.to == node.id)
                .count() as u32;
            max_degree = max_degree.max(degree);
            checksum ^= node.id ^ ((degree as u64) << 32) ^ node.weight as u32 as u64;
            checksum = checksum.wrapping_mul(0x100000001b3);
        }
        let (dominant_eigen_q16, spectral_energy) = self.laplacian_power_iteration();
        SpectralSnapshot {
            generation: self.generation,
            active_nodes,
            active_edges,
            laplacian_trace: active_edges as u64 * 2,
            max_degree,
            dominant_eigen_q16,
            spectral_energy,
            checksum,
        }
    }

    /// Twelve deterministic fixed-point power iterations over the unweighted
    /// graph Laplacian. This remains bounded for ring-3 execution while doing
    /// an actual spectral refresh rather than returning structural counters.
    fn laplacian_power_iteration(&self) -> (i32, u64) {
        let mut vector = [0i64; MAX_NODES];
        let mut image = [0i64; MAX_NODES];
        for (index, node) in self.nodes.iter().enumerate() {
            if node.active {
                vector[index] = if index & 1 == 0 { 65_536 } else { -65_536 };
            }
        }
        for _ in 0..12 {
            image.fill(0);
            for (index, node) in self.nodes.iter().enumerate() {
                if !node.active {
                    continue;
                }
                let mut degree = 0i64;
                let mut neighbor_sum = 0i64;
                for edge in self.edges() {
                    let peer = if edge.from == node.id {
                        edge.to
                    } else if edge.to == node.id {
                        edge.from
                    } else {
                        continue;
                    };
                    if let Some(peer_index) = self
                        .nodes
                        .iter()
                        .position(|candidate| candidate.active && candidate.id == peer)
                    {
                        degree += 1;
                        neighbor_sum = neighbor_sum.saturating_add(vector[peer_index]);
                    }
                }
                image[index] = degree
                    .saturating_mul(vector[index])
                    .saturating_sub(neighbor_sum);
            }
            let scale = image
                .iter()
                .map(|value| value.unsigned_abs())
                .max()
                .unwrap_or(0)
                .max(1);
            for index in 0..MAX_NODES {
                vector[index] = image[index].saturating_mul(65_536) / scale as i64;
            }
        }
        image.fill(0);
        let mut numerator = 0i128;
        let mut denominator = 0i128;
        let mut energy = 0u64;
        for (index, node) in self.nodes.iter().enumerate() {
            if !node.active {
                continue;
            }
            let mut degree = 0i64;
            let mut neighbor_sum = 0i64;
            for edge in self.edges() {
                let peer = if edge.from == node.id {
                    edge.to
                } else if edge.to == node.id {
                    edge.from
                } else {
                    continue;
                };
                if let Some(peer_index) = self
                    .nodes
                    .iter()
                    .position(|candidate| candidate.active && candidate.id == peer)
                {
                    degree += 1;
                    neighbor_sum = neighbor_sum.saturating_add(vector[peer_index]);
                }
            }
            image[index] = degree
                .saturating_mul(vector[index])
                .saturating_sub(neighbor_sum);
            numerator = numerator.saturating_add(vector[index] as i128 * image[index] as i128);
            denominator = denominator.saturating_add(vector[index] as i128 * vector[index] as i128);
            energy = energy.saturating_add(
                image[index]
                    .unsigned_abs()
                    .saturating_mul(image[index].unsigned_abs()),
            );
        }
        let eigen = if denominator == 0 {
            0
        } else {
            (numerator.saturating_mul(65_536) / denominator)
                .clamp(i32::MIN as i128, i32::MAX as i128) as i32
        };
        (eigen, energy)
    }

    pub fn drift_report(
        &self,
        from_generation: u64,
        baseline_nodes: u32,
        baseline_edges: u32,
    ) -> DriftReport {
        let current_nodes = self.nodes().count() as i32;
        let current_edges = self.edges().count() as i32;
        let mutation_count = self
            .journal_entries()
            .filter(|entry| entry.generation > from_generation)
            .count() as u32;
        DriftReport {
            from_generation,
            to_generation: self.generation,
            mutation_count,
            node_delta: current_nodes - baseline_nodes as i32,
            edge_delta: current_edges - baseline_edges as i32,
            journal_truncated: self.journal_len == MAX_JOURNAL
                && from_generation
                    < self
                        .journal_entries()
                        .next()
                        .map(|e| e.generation)
                        .unwrap_or(0),
        }
    }

    pub fn provenance(&self, provenance: u64) -> QueryResult {
        let mut result = QueryResult {
            generation: self.generation,
            count: 0,
            nodes: [EMPTY_SCORED_NODE; MAX_RESULTS],
        };
        for node in self
            .nodes()
            .filter(|node| node.provenance == provenance || node.id == provenance)
        {
            insert_ranked(
                &mut result,
                ScoredNode {
                    node: *node,
                    score: node.weight,
                },
                MAX_RESULTS,
            );
        }
        result
    }

    pub fn journal_entries(&self) -> impl Iterator<Item = &JournalEntry> {
        (0..self.journal_len)
            .map(move |offset| &self.journal[(self.journal_head + offset) % MAX_JOURNAL])
    }

    pub fn encode_snapshot(&self, out: &mut [u8]) -> Result<usize, GraphError> {
        if out.len() < SNAPSHOT_CAP {
            return Err(GraphError::Capacity);
        }
        out[..SNAPSHOT_CAP].fill(0);
        out[0..4].copy_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
        out[8..16].copy_from_slice(&self.generation.to_le_bytes());
        out[16..24].copy_from_slice(&self.next_id.to_le_bytes());
        out[24..28].copy_from_slice(&(self.journal_len as u32).to_le_bytes());
        let mut offset = 32;
        for node in &self.nodes {
            out[offset] = u8::from(node.active);
            out[offset + 2..offset + 4].copy_from_slice(&node.kind.to_le_bytes());
            out[offset + 4..offset + 6].copy_from_slice(&node.flags.to_le_bytes());
            out[offset + 8..offset + 16].copy_from_slice(&node.id.to_le_bytes());
            out[offset + 16..offset + 20].copy_from_slice(&node.weight.to_le_bytes());
            out[offset + 24..offset + 32].copy_from_slice(&node.created_at.to_le_bytes());
            out[offset + 32..offset + 40].copy_from_slice(&node.updated_at.to_le_bytes());
            out[offset + 40..offset + 48].copy_from_slice(&node.provenance.to_le_bytes());
            out[offset + 48..offset + 56].copy_from_slice(&node.parent.to_le_bytes());
            offset += 56;
        }
        for edge in &self.edges {
            out[offset] = u8::from(edge.active);
            out[offset + 2..offset + 4].copy_from_slice(&edge.kind.to_le_bytes());
            out[offset + 4..offset + 6].copy_from_slice(&edge.flags.to_le_bytes());
            out[offset + 8..offset + 16].copy_from_slice(&edge.id.to_le_bytes());
            out[offset + 16..offset + 24].copy_from_slice(&edge.from.to_le_bytes());
            out[offset + 24..offset + 32].copy_from_slice(&edge.to.to_le_bytes());
            out[offset + 32..offset + 36].copy_from_slice(&edge.weight.to_le_bytes());
            out[offset + 40..offset + 48].copy_from_slice(&edge.created_at.to_le_bytes());
            out[offset + 48..offset + 56].copy_from_slice(&edge.provenance.to_le_bytes());
            offset += 56;
        }
        for entry in self.journal_entries() {
            out[offset..offset + 8].copy_from_slice(&entry.generation.to_le_bytes());
            out[offset + 8..offset + 16].copy_from_slice(&entry.timestamp.to_le_bytes());
            out[offset + 16] = entry.mutation_kind;
            out[offset + 24..offset + 32].copy_from_slice(&entry.object_id.to_le_bytes());
            out[offset + 32..offset + 40].copy_from_slice(&entry.provenance.to_le_bytes());
            offset += 40;
        }
        Ok(SNAPSHOT_CAP)
    }

    pub fn decode_snapshot(&mut self, input: &[u8]) -> Result<(), GraphError> {
        if input.len() < SNAPSHOT_CAP || read_u32(&input[0..4]) != SNAPSHOT_VERSION {
            return Err(GraphError::InvalidSnapshot);
        }
        self.nodes = [EMPTY_NODE; MAX_NODES];
        self.edges = [EMPTY_EDGE; MAX_EDGES];
        self.generation = read_u64(&input[8..16]);
        self.next_id = read_u64(&input[16..24]).max(1);
        let persisted_journal_len = (read_u32(&input[24..28]) as usize).min(MAX_JOURNAL);
        let mut offset = 32;
        for node in &mut self.nodes {
            *node = Node {
                active: input[offset] != 0,
                kind: read_u16(&input[offset + 2..offset + 4]),
                flags: read_u16(&input[offset + 4..offset + 6]),
                id: read_u64(&input[offset + 8..offset + 16]),
                weight: read_i32(&input[offset + 16..offset + 20]),
                created_at: read_u64(&input[offset + 24..offset + 32]),
                updated_at: read_u64(&input[offset + 32..offset + 40]),
                provenance: read_u64(&input[offset + 40..offset + 48]),
                parent: read_u64(&input[offset + 48..offset + 56]),
            };
            offset += 56;
        }
        for edge in &mut self.edges {
            *edge = Edge {
                active: input[offset] != 0,
                kind: read_u16(&input[offset + 2..offset + 4]),
                flags: read_u16(&input[offset + 4..offset + 6]),
                id: read_u64(&input[offset + 8..offset + 16]),
                from: read_u64(&input[offset + 16..offset + 24]),
                to: read_u64(&input[offset + 24..offset + 32]),
                weight: read_i32(&input[offset + 32..offset + 36]),
                created_at: read_u64(&input[offset + 40..offset + 48]),
                provenance: read_u64(&input[offset + 48..offset + 56]),
            };
            offset += 56;
        }
        self.journal = [EMPTY_JOURNAL; MAX_JOURNAL];
        self.journal_head = 0;
        self.journal_len = persisted_journal_len;
        for entry in self.journal.iter_mut().take(persisted_journal_len) {
            *entry = JournalEntry {
                generation: read_u64(&input[offset..offset + 8]),
                timestamp: read_u64(&input[offset + 8..offset + 16]),
                mutation_kind: input[offset + 16],
                object_id: read_u64(&input[offset + 24..offset + 32]),
                provenance: read_u64(&input[offset + 32..offset + 40]),
            };
            offset += 40;
        }
        Ok(())
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }
    fn append_journal(&mut self, entry: JournalEntry) {
        let index = (self.journal_head + self.journal_len) % MAX_JOURNAL;
        self.journal[index] = entry;
        if self.journal_len < MAX_JOURNAL {
            self.journal_len += 1;
        } else {
            self.journal_head = (self.journal_head + 1) % MAX_JOURNAL;
        }
    }
}

fn insert_ranked(result: &mut QueryResult, candidate: ScoredNode, limit: usize) {
    if limit == 0 {
        return;
    }
    let mut pos = result.count.min(limit);
    for index in 0..result.count.min(limit) {
        if candidate.score > result.nodes[index].score {
            pos = index;
            break;
        }
    }
    if pos >= limit {
        return;
    }
    let upper = result.count.min(limit - 1);
    for index in (pos..upper).rev() {
        result.nodes[index + 1] = result.nodes[index];
    }
    result.nodes[pos] = candidate;
    result.count = (result.count + 1).min(limit);
}

fn read_u16(v: &[u8]) -> u16 {
    u16::from_le_bytes([v[0], v[1]])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_query_walk_spectral_and_snapshot_roundtrip() {
        let mut graph = GraphRuntime::new();
        let a = graph
            .apply(
                Mutation::AddNode {
                    kind: 1,
                    flags: 0,
                    weight: 30,
                    parent: 0,
                    now: 10,
                },
                100,
            )
            .unwrap();
        let b = graph
            .apply(
                Mutation::AddNode {
                    kind: 2,
                    flags: 0,
                    weight: 50,
                    parent: a,
                    now: 20,
                },
                101,
            )
            .unwrap();
        graph
            .apply(
                Mutation::AddEdge {
                    kind: 7,
                    flags: 0,
                    weight: 40,
                    from: a,
                    to: b,
                    now: 30,
                },
                102,
            )
            .unwrap();
        let ranked = graph.scored_neighborhood(a, 30, 8);
        assert_eq!(ranked.count, 1);
        assert_eq!(ranked.nodes[0].node.id, b);
        assert_eq!(graph.deterministic_walk(a, 42, 2)[1], b);
        assert_eq!(graph.spectral_snapshot().laplacian_trace, 2);
        assert_eq!(graph.provenance(101).nodes[0].node.id, b);
        let mut bytes = [0u8; SNAPSHOT_CAP];
        graph.encode_snapshot(&mut bytes).unwrap();
        let mut restored = GraphRuntime::new();
        restored.decode_snapshot(&bytes).unwrap();
        assert_eq!(restored.node(a).unwrap().kind, 1);
        assert_eq!(restored.spectral_snapshot(), graph.spectral_snapshot());
    }

    #[test]
    fn rejects_dangling_edges_and_reports_drift() {
        let mut graph = GraphRuntime::new();
        assert_eq!(
            graph.apply(
                Mutation::AddEdge {
                    kind: 1,
                    flags: 0,
                    weight: 1,
                    from: 1,
                    to: 2,
                    now: 1
                },
                0
            ),
            Err(GraphError::InvalidEdge)
        );
        graph
            .apply(
                Mutation::AddNode {
                    kind: 1,
                    flags: 0,
                    weight: 1,
                    parent: 0,
                    now: 1,
                },
                0,
            )
            .unwrap();
        let drift = graph.drift_report(0, 0, 0);
        assert_eq!(drift.mutation_count, 1);
        assert_eq!(drift.node_delta, 1);
    }
}
