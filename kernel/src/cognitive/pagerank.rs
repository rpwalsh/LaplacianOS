//! Weighted PageRank and query-personalized PageRank in Q16.

use crate::graph::types::{WEIGHT_ONE, Weight};

const MAX_RANK_NODES: usize = 4096;
const MAX_RANK_EDGES: usize = 16384;
const DAMPING_Q16: u64 = 55_706;
const RESTART_Q16: u64 = 9_830;
const EPSILON_Q16: u64 = 66;
const MAX_ITERATIONS: usize = 64;

#[derive(Clone, Copy)]
struct RankEdge {
    from: u16,
    to: u16,
    weight_q16: Weight,
}

impl RankEdge {
    const EMPTY: Self = Self {
        from: 0,
        to: 0,
        weight_q16: 0,
    };
}

pub struct PageRankEngine {
    edges: [RankEdge; MAX_RANK_EDGES],
    edge_count: usize,
    out_weight_q16: [u64; MAX_RANK_NODES],
    node_ids: [u64; MAX_RANK_NODES],
    node_count: usize,
    scores: [Weight; MAX_RANK_NODES],
    next_scores: [u64; MAX_RANK_NODES],
    restart: [Weight; MAX_RANK_NODES],
    pub last_iterations: u32,
    pub last_delta: Weight,
}

impl PageRankEngine {
    pub const fn new() -> Self {
        Self {
            edges: [RankEdge::EMPTY; MAX_RANK_EDGES],
            edge_count: 0,
            out_weight_q16: [0; MAX_RANK_NODES],
            node_ids: [0; MAX_RANK_NODES],
            node_count: 0,
            scores: [0; MAX_RANK_NODES],
            next_scores: [0; MAX_RANK_NODES],
            restart: [0; MAX_RANK_NODES],
            last_iterations: 0,
            last_delta: 0,
        }
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }

    pub fn add_node(&mut self, external_id: u64) -> Option<u16> {
        if let Some(index) = (0..self.node_count).find(|index| self.node_ids[*index] == external_id)
        {
            return Some(index as u16);
        }
        if self.node_count >= MAX_RANK_NODES {
            return None;
        }
        let index = self.node_count;
        self.node_ids[index] = external_id;
        self.node_count += 1;
        Some(index as u16)
    }

    pub fn add_edge(&mut self, from: u16, to: u16, weight_q16: Weight) -> bool {
        if self.edge_count >= MAX_RANK_EDGES
            || from as usize >= self.node_count
            || to as usize >= self.node_count
            || weight_q16 == 0
        {
            return false;
        }
        self.edges[self.edge_count] = RankEdge {
            from,
            to,
            weight_q16,
        };
        self.edge_count += 1;
        self.out_weight_q16[from as usize] =
            self.out_weight_q16[from as usize].saturating_add(weight_q16 as u64);
        true
    }

    pub fn compute(&mut self) -> u32 {
        self.compute_personalized(&[])
    }

    /// Run personalized PageRank. Seed weights are keyed by external node ID.
    /// Dangling mass is redistributed into this same restart distribution.
    pub fn compute_personalized(&mut self, seeds: &[(u64, Weight)]) -> u32 {
        let n = self.node_count;
        if n == 0 {
            return 0;
        }
        self.restart[..n].fill(0);
        for (external_id, weight) in seeds {
            if let Some(index) = (0..n).find(|index| self.node_ids[*index] == *external_id) {
                self.restart[index] = self.restart[index].saturating_add(*weight);
            }
        }
        if self.restart[..n].iter().all(|weight| *weight == 0) {
            let uniform = (WEIGHT_ONE as u64 / n as u64) as Weight;
            self.restart[..n].fill(uniform);
            let assigned = uniform as u64 * n as u64;
            self.restart[n - 1] =
                self.restart[n - 1].saturating_add((WEIGHT_ONE as u64 - assigned) as Weight);
        } else {
            normalize_q16(&mut self.restart[..n]);
        }
        self.scores[..n].copy_from_slice(&self.restart[..n]);

        let mut iteration = 0u32;
        while (iteration as usize) < MAX_ITERATIONS {
            for index in 0..n {
                self.next_scores[index] = (RESTART_Q16 * self.restart[index] as u64) >> 16;
            }

            for edge in &self.edges[..self.edge_count] {
                let from = edge.from as usize;
                let transition_q16 = q16_div(edge.weight_q16 as u64, self.out_weight_q16[from]);
                let damped_source = q16_mul(DAMPING_Q16, self.scores[from] as u64);
                let contribution = q16_mul(damped_source, transition_q16);
                self.next_scores[edge.to as usize] =
                    self.next_scores[edge.to as usize].saturating_add(contribution);
            }

            let dangling_mass_q16 = (0..n)
                .filter(|index| self.out_weight_q16[*index] == 0)
                .fold(0u64, |sum, index| {
                    sum.saturating_add(self.scores[index] as u64)
                });
            let damped_dangling = q16_mul(DAMPING_Q16, dangling_mass_q16);
            for index in 0..n {
                self.next_scores[index] = self.next_scores[index]
                    .saturating_add(q16_mul(damped_dangling, self.restart[index] as u64));
            }
            normalize_u64_q16(&mut self.next_scores[..n]);

            let mut delta = 0u64;
            for index in 0..n {
                let next = self.next_scores[index].min(Weight::MAX as u64) as Weight;
                delta = delta.saturating_add(next.abs_diff(self.scores[index]) as u64);
                self.scores[index] = next;
            }
            iteration += 1;
            self.last_delta = delta.min(Weight::MAX as u64) as Weight;
            if delta < EPSILON_Q16 {
                break;
            }
        }
        self.last_iterations = iteration;
        iteration
    }

    pub fn score(&self, index: usize) -> Weight {
        self.scores.get(index).copied().unwrap_or(0)
    }

    pub fn external_id(&self, index: usize) -> u64 {
        self.node_ids.get(index).copied().unwrap_or(0)
    }

    pub fn top_k(&self, output: &mut [(u64, Weight)]) -> usize {
        let mut used = [false; MAX_RANK_NODES];
        let mut written = 0;
        while written < output.len() {
            let Some(best) = (0..self.node_count)
                .filter(|index| !used[*index] && self.scores[*index] > 0)
                .max_by_key(|index| self.scores[*index])
            else {
                break;
            };
            used[best] = true;
            output[written] = (self.node_ids[best], self.scores[best]);
            written += 1;
        }
        written
    }

    pub fn node_count(&self) -> usize {
        self.node_count
    }

    pub fn edge_count(&self) -> usize {
        self.edge_count
    }
}

fn q16_mul(left: u64, right: u64) -> u64 {
    ((left as u128 * right as u128) >> 16).min(u64::MAX as u128) as u64
}

fn q16_div(numerator_q16: u64, denominator_q16: u64) -> u64 {
    if denominator_q16 == 0 {
        0
    } else {
        (((numerator_q16 as u128) << 16) / denominator_q16 as u128).min(u64::MAX as u128) as u64
    }
}

fn normalize_q16(values: &mut [Weight]) {
    let sum = values
        .iter()
        .fold(0u64, |total, value| total + *value as u64);
    if sum == 0 {
        return;
    }
    let mut assigned = 0u64;
    for value in values.iter_mut() {
        *value = (((*value as u128) << 16) / sum as u128) as Weight;
        assigned += *value as u64;
    }
    if let Some(last) = values.iter_mut().rfind(|value| **value > 0) {
        *last =
            last.saturating_add((WEIGHT_ONE as u64 - assigned.min(WEIGHT_ONE as u64)) as Weight);
    }
}

fn normalize_u64_q16(values: &mut [u64]) {
    let sum = values
        .iter()
        .fold(0u128, |total, value| total + *value as u128);
    if sum == 0 {
        return;
    }
    let mut assigned = 0u64;
    for value in values.iter_mut() {
        *value = (((*value as u128) << 16) / sum).min(WEIGHT_ONE as u128) as u64;
        assigned += *value;
    }
    if let Some(last) = values.iter_mut().rfind(|value| **value > 0) {
        *last += WEIGHT_ONE as u64 - assigned.min(WEIGHT_ONE as u64);
    }
}
