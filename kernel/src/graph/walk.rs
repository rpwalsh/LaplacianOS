//! Public graph-walk state and normalized PowerWalk transition primitives.

use crate::graph::temporal;
use crate::graph::types::*;

pub const MAX_WALK_LEN: usize = 64;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WalkStep {
    pub node: NodeId,
    pub timestamp: Timestamp,
}

impl WalkStep {
    pub const EMPTY: Self = Self {
        node: 0,
        timestamp: 0,
    };
}

pub struct WalkState {
    pub steps: [WalkStep; MAX_WALK_LEN + 1],
    pub edge_kinds: [EdgeKind; MAX_WALK_LEN],
    pub edge_weights: [Weight; MAX_WALK_LEN],
    pub len: usize,
    pub origin_kind: NodeKind,
}

impl WalkState {
    pub fn new(origin: NodeId, origin_kind: NodeKind, t: Timestamp) -> Self {
        let mut state = Self {
            steps: [WalkStep::EMPTY; MAX_WALK_LEN + 1],
            edge_kinds: [EdgeKind::Owns; MAX_WALK_LEN],
            edge_weights: [0; MAX_WALK_LEN],
            len: 0,
            origin_kind,
        };
        state.steps[0] = WalkStep {
            node: origin,
            timestamp: t,
        };
        state
    }

    pub fn push(
        &mut self,
        node: NodeId,
        timestamp: Timestamp,
        edge_kind: EdgeKind,
        edge_weight: Weight,
    ) -> bool {
        if self.len >= MAX_WALK_LEN {
            return false;
        }
        if timestamp < self.steps[self.len].timestamp {
            return false;
        }
        self.edge_kinds[self.len] = edge_kind;
        self.edge_weights[self.len] = edge_weight;
        self.len += 1;
        self.steps[self.len] = WalkStep { node, timestamp };
        true
    }

    pub fn current_node(&self) -> NodeId {
        self.steps[self.len].node
    }

    pub fn previous_node(&self) -> Option<NodeId> {
        if self.len == 0 {
            None
        } else {
            Some(self.steps[self.len - 1].node)
        }
    }

    pub fn current_timestamp(&self) -> Timestamp {
        self.steps[self.len].timestamp
    }

    pub fn origin(&self) -> NodeId {
        self.steps[0].node
    }

    pub fn elapsed(&self) -> u64 {
        self.steps[self.len]
            .timestamp
            .saturating_sub(self.steps[0].timestamp)
    }

    pub fn has_visited(&self, node: NodeId) -> bool {
        for i in 0..=self.len {
            if self.steps[i].node == node {
                return true;
            }
        }
        false
    }

    pub fn count_kind(
        &self,
        target: NodeKind,
        kind_of: impl Fn(NodeId) -> Option<NodeKind>,
    ) -> usize {
        let mut count = 0;
        for i in 0..=self.len {
            if let Some(kind) = kind_of(self.steps[i].node)
                && kind == target
            {
                count += 1;
            }
        }
        count
    }
}

pub const fn second_order_bias(
    prev_is_candidate: bool,
    candidate_adjacent_to_prev: bool,
    return_p: Weight,
    inout_q: Weight,
) -> Weight {
    if prev_is_candidate {
        reciprocal_q16(return_p)
    } else if candidate_adjacent_to_prev {
        WEIGHT_ONE
    } else {
        reciprocal_q16(inout_q)
    }
}

/// `p < 1` encourages immediate return; `p > 1` discourages it.
/// `q > 1` favors local/BFS-like moves; `q < 1` favors outward/DFS-like moves.
const fn reciprocal_q16(value_q16: Weight) -> Weight {
    if value_q16 == 0 {
        return 0;
    }
    let quotient = (1u64 << 32) / value_q16 as u64;
    if quotient > Weight::MAX as u64 {
        Weight::MAX
    } else {
        quotient as Weight
    }
}

pub const fn transition_score(decayed_weight: Weight, bias: Weight) -> Weight {
    let product = decayed_weight as u64 * bias as u64;
    let result = product >> 16;
    if result > u32::MAX as u64 {
        u32::MAX
    } else {
        result as u32
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TransitionCandidate {
    pub node: NodeId,
    pub edge_weight_q16: Weight,
    pub lambda_per_tick_q16: Weight,
    pub elapsed_ticks: u64,
    pub is_previous_node: bool,
    pub is_adjacent_to_previous: bool,
    /// False for stale/deleted/non-neighbor candidates. Such entries receive
    /// exactly zero probability mass.
    pub valid_edge: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionProbability {
    pub node: NodeId,
    pub probability_q16: Weight,
}

/// Build the complete normalized second-order transition distribution.
///
/// The first step has no previous node and therefore uses unit bias. The
/// returned probabilities sum to exactly Q16 one whenever at least one valid
/// candidate has positive weight.
pub fn normalized_transition_distribution(
    candidates: &[TransitionCandidate],
    has_previous_node: bool,
    return_p_q16: Weight,
    inout_q_q16: Weight,
    output: &mut [TransitionProbability],
) -> usize {
    let count = candidates.len().min(output.len());
    let mut raw_scores = [0u64; MAX_WALK_LEN];
    let count = count.min(MAX_WALK_LEN);
    let mut total = 0u64;

    for index in 0..count {
        let candidate = candidates[index];
        output[index] = TransitionProbability {
            node: candidate.node,
            probability_q16: 0,
        };
        if !candidate.valid_edge {
            continue;
        }
        let decayed = temporal::decay_weight_q16(
            candidate.edge_weight_q16,
            candidate.lambda_per_tick_q16,
            candidate.elapsed_ticks,
        );
        let bias = if has_previous_node {
            second_order_bias(
                candidate.is_previous_node,
                candidate.is_adjacent_to_previous,
                return_p_q16,
                inout_q_q16,
            )
        } else {
            WEIGHT_ONE
        };
        let score = transition_score(decayed, bias) as u64;
        raw_scores[index] = score;
        total = total.saturating_add(score);
    }

    if total == 0 {
        return count;
    }

    let mut assigned = 0u64;
    let mut last_positive = None;
    for index in 0..count {
        if raw_scores[index] == 0 {
            continue;
        }
        let probability =
            (((raw_scores[index] as u128) << 16) / total as u128).min(WEIGHT_ONE as u128) as Weight;
        output[index].probability_q16 = probability;
        assigned = assigned.saturating_add(probability as u64);
        last_positive = Some(index);
    }
    if let Some(index) = last_positive {
        output[index].probability_q16 = output[index]
            .probability_q16
            .saturating_add(WEIGHT_ONE.saturating_sub(assigned.min(WEIGHT_ONE as u64) as Weight));
    }
    count
}

/// Small deterministic generator for reproducible kernel walk replay.
#[derive(Debug, Clone, Copy)]
pub struct WalkRng {
    state: u64,
}

impl WalkRng {
    pub const fn seeded(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9e37_79b9_7f4a_7c15
            } else {
                seed
            },
        }
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut value = self.state;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.state = value;
        (value >> 32) as u32
    }
}

pub fn sample_transition(
    probabilities: &[TransitionProbability],
    rng: &mut WalkRng,
) -> Option<NodeId> {
    let draw = rng.next_u32() & (WEIGHT_ONE - 1);
    let mut cumulative = 0u64;
    for candidate in probabilities {
        cumulative = cumulative.saturating_add(candidate.probability_q16 as u64);
        if (draw as u64) < cumulative {
            return Some(candidate.node);
        }
    }
    None
}

#[cfg(test)]
mod probability_tests {
    use super::*;

    fn candidate(node: NodeId, weight: Weight) -> TransitionCandidate {
        TransitionCandidate {
            node,
            edge_weight_q16: weight,
            lambda_per_tick_q16: 0,
            elapsed_ticks: 0,
            is_previous_node: false,
            is_adjacent_to_previous: true,
            valid_edge: true,
        }
    }

    #[test]
    fn unbiased_distribution_reduces_to_weighted_first_order_walk() {
        let candidates = [candidate(1, WEIGHT_ONE), candidate(2, WEIGHT_ONE * 3)];
        let mut output = [TransitionProbability {
            node: 0,
            probability_q16: 0,
        }; 2];
        normalized_transition_distribution(&candidates, false, WEIGHT_ONE, WEIGHT_ONE, &mut output);
        assert_eq!(output[0].probability_q16, WEIGHT_ONE / 4);
        assert_eq!(
            output
                .iter()
                .map(|entry| entry.probability_q16)
                .sum::<u32>(),
            WEIGHT_ONE
        );
    }

    #[test]
    fn invalid_edges_have_no_mass_and_seeded_replay_is_deterministic() {
        let mut candidates = [candidate(1, WEIGHT_ONE), candidate(2, WEIGHT_ONE)];
        candidates[1].valid_edge = false;
        let mut output = [TransitionProbability {
            node: 0,
            probability_q16: 0,
        }; 2];
        normalized_transition_distribution(&candidates, true, WEIGHT_ONE, WEIGHT_ONE, &mut output);
        assert_eq!(output[1].probability_q16, 0);
        let mut first = WalkRng::seeded(42);
        let mut second = WalkRng::seeded(42);
        for _ in 0..32 {
            assert_eq!(
                sample_transition(&output, &mut first),
                sample_transition(&output, &mut second)
            );
        }
    }
}
