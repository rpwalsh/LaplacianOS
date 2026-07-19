//! Authoritative Walsh-2026 cognition primitives used by LaplacianOS SCCE.
//!
//! This fixed-point Rust module follows the implementations in
//! `scce/packages/core/src/{cfcb,subspaceDriftEntropy,causalMinCoverCoding}.ts`
//! and the equations in `papers/2026-04-causally-constrained-cognition.md`.
//! It is unrelated to the removed type-space Walsh-Hadamard interpretation.

use crate::graph::temporal::{LN_2_Q16, ln_q16};
use crate::graph::types::{NodeId, WEIGHT_ONE, Weight};

pub const MAX_CMC_CANDIDATES: usize = 32;
pub const MAX_CMC_ENTITIES_PER_CANDIDATE: usize = 16;
pub const MAX_CMC_ENTITIES: usize = 128;

#[inline]
fn q16_mul(left: Weight, right: Weight) -> Weight {
    (((left as u64 * right as u64) + (1 << 15)) >> 16).min(Weight::MAX as u64) as Weight
}

#[inline]
fn q16_div(left: Weight, right: Weight) -> Weight {
    if right == 0 {
        Weight::MAX
    } else {
        (((left as u128) << 16) / right as u128).min(Weight::MAX as u128) as Weight
    }
}

fn integer_sqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut estimate = 1u128 << (128 - value.leading_zeros() as usize).div_ceil(2);
    loop {
        let next = (estimate + value / estimate) / 2;
        if next >= estimate {
            return estimate;
        }
        estimate = next;
    }
}

#[inline]
pub fn sqrt_q16(value: Weight) -> Weight {
    integer_sqrt((value as u128) << 16).min(Weight::MAX as u128) as Weight
}

#[inline]
fn clamp_unit(value: Weight) -> Weight {
    value.min(WEIGHT_ONE)
}

/// One-sided Hoeffding radius `sqrt(ln(1/alpha) / (2M))` in Q16.
pub fn hoeffding_radius_q16(samples: usize, alpha: Weight) -> Weight {
    if samples == 0 {
        return Weight::MAX;
    }
    let alpha = alpha.clamp(1, WEIGHT_ONE - 1);
    let inverse_q16 = (((WEIGHT_ONE as u128) << 16) / alpha as u128).min(u64::MAX as u128) as u64;
    let logarithm = ln_q16(inverse_q16).min(Weight::MAX as u64) as Weight;
    sqrt_q16((logarithm as u64 / (2 * samples) as u64) as Weight)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CfcbRejectReason {
    NoFaithWindows = 0,
    InvalidCausalMass = 1,
    FaithLcbBelowFloor = 2,
    CausalLcbBelowFloor = 3,
}

pub struct CfcbInput<'a> {
    pub faithfulness: &'a [Weight],
    pub causal_mass: Weight,
    pub causal_mass_valid: bool,
    pub causal_entity_count: u32,
    pub causal_sin_theta: Weight,
    pub faith_floor: Weight,
    pub causal_floor: Weight,
    pub alpha: Weight,
}

#[derive(Debug, Clone, Copy)]
pub struct CfcbReport {
    pub faith_mean: Weight,
    pub sample_count: u32,
    pub faith_radius: Weight,
    pub faith_lcb: Weight,
    pub faith_slack: i64,
    pub faith_pass: bool,
    pub causal_mass: Weight,
    pub causal_entity_count: u32,
    pub causal_sin_theta: Weight,
    pub causal_radius: Weight,
    pub causal_lcb: Weight,
    pub causal_slack: i64,
    pub causal_pass: bool,
    pub joint_false_accept_bound: Weight,
    pub accept: bool,
    pub reject_reason: Option<CfcbRejectReason>,
}

/// Deterministic Davis-Kahan/Cauchy-Schwarz radius
/// `sqrt(|E_S|) * sin(theta)` in Q16.
pub fn davis_kahan_causal_radius_q16(entity_count: u32, sin_theta: Weight) -> Weight {
    if entity_count == 0 {
        return Weight::MAX;
    }
    let root_count_q16 =
        integer_sqrt((entity_count as u128) << 32).min(Weight::MAX as u128) as Weight;
    q16_mul(root_count_q16, clamp_unit(sin_theta))
}

/// Evaluate the revised CFCB gate without assuming independence between the
/// faithfulness and causal-mass signals.
pub fn cfcb(input: CfcbInput<'_>) -> CfcbReport {
    let count = input.faithfulness.len();
    let sum = input
        .faithfulness
        .iter()
        .fold(0u128, |total, value| total + clamp_unit(*value) as u128);
    let faith_mean = if count == 0 {
        0
    } else {
        (sum / count as u128).min(WEIGHT_ONE as u128) as Weight
    };
    let alpha = input.alpha.clamp(1, WEIGHT_ONE - 1);
    let faith_radius = hoeffding_radius_q16(count, alpha);
    let faith_lcb = faith_mean.saturating_sub(faith_radius).min(WEIGHT_ONE);
    let faith_floor = clamp_unit(input.faith_floor);
    let faith_slack = faith_lcb as i64 - faith_floor as i64;
    let faith_pass = count > 0 && faith_lcb >= faith_floor;

    let causal_mass = clamp_unit(input.causal_mass);
    let causal_entity_count = input.causal_entity_count.max(1);
    let causal_sin_theta = clamp_unit(input.causal_sin_theta);
    let causal_radius = davis_kahan_causal_radius_q16(causal_entity_count, causal_sin_theta);
    let causal_lcb = causal_mass.saturating_sub(causal_radius).min(WEIGHT_ONE);
    let causal_floor = clamp_unit(input.causal_floor);
    let causal_slack = causal_lcb as i64 - causal_floor as i64;
    let causal_pass = input.causal_mass_valid && causal_lcb >= causal_floor;

    let reject_reason = if count == 0 {
        Some(CfcbRejectReason::NoFaithWindows)
    } else if !input.causal_mass_valid {
        Some(CfcbRejectReason::InvalidCausalMass)
    } else if !faith_pass {
        Some(CfcbRejectReason::FaithLcbBelowFloor)
    } else if !causal_pass {
        Some(CfcbRejectReason::CausalLcbBelowFloor)
    } else {
        None
    };

    CfcbReport {
        faith_mean,
        sample_count: count.min(u32::MAX as usize) as u32,
        faith_radius,
        faith_lcb,
        faith_slack,
        faith_pass,
        causal_mass,
        causal_entity_count,
        causal_sin_theta,
        causal_radius,
        causal_lcb,
        causal_slack,
        causal_pass,
        joint_false_accept_bound: alpha,
        accept: reject_reason.is_none(),
        reject_reason,
    }
}

/// Compute squared per-coordinate principal cosines in the entity-axis basis.
pub fn principal_cosines_squared_q16(
    previous: &[Weight],
    current: &[Weight],
    output: &mut [Weight],
) -> usize {
    let k = previous.len().max(current.len()).min(output.len());
    if k == 0 {
        return 0;
    }
    let previous_norm_sq = (0..k).fold(0u128, |sum, index| {
        let value = previous.get(index).copied().unwrap_or(0) as u128;
        sum.saturating_add(value.saturating_mul(value))
    });
    let current_norm_sq = (0..k).fold(0u128, |sum, index| {
        let value = current.get(index).copied().unwrap_or(0) as u128;
        sum.saturating_add(value.saturating_mul(value))
    });
    let previous_norm = integer_sqrt(previous_norm_sq);
    let current_norm = integer_sqrt(current_norm_sq);
    for (index, coordinate) in output.iter_mut().take(k).enumerate() {
        if previous_norm == 0 || current_norm == 0 {
            *coordinate = 0;
            continue;
        }
        let previous_unit = (((previous.get(index).copied().unwrap_or(0) as u128) << 16)
            / previous_norm)
            .min(Weight::MAX as u128) as Weight;
        let current_unit = (((current.get(index).copied().unwrap_or(0) as u128) << 16)
            / current_norm)
            .min(Weight::MAX as u128) as Weight;
        let coordinate_cosine = q16_mul(previous_unit, current_unit);
        *coordinate = q16_mul(coordinate_cosine, coordinate_cosine);
    }
    k
}

#[derive(Debug, Clone, Copy)]
pub struct SdeStep {
    pub entropy: Weight,
    pub delta_entropy: i64,
    pub margin: Weight,
    pub stopped_dropping: bool,
    pub concentrated: bool,
    pub converged: bool,
}

/// Evaluate one Walsh Subspace Drift Entropy step.
pub fn subspace_drift_step_q16(
    cos_squared: &[Weight],
    previous_entropy: Option<Weight>,
    epsilon: Weight,
    gamma: Weight,
) -> SdeStep {
    let used = cos_squared.len();
    let total = cos_squared
        .iter()
        .fold(0u128, |sum, value| sum.saturating_add(*value as u128));
    let mut entropy = 0u128;
    if total > 0 {
        for coordinate in cos_squared.iter().copied() {
            if coordinate == 0 {
                continue;
            }
            let probability = ((coordinate as u128) << 16)
                .checked_div(total)
                .unwrap_or(0)
                .min(WEIGHT_ONE as u128) as Weight;
            if probability == 0 {
                continue;
            }
            let inverse = q16_div(WEIGHT_ONE, probability).max(WEIGHT_ONE);
            let negative_log = ln_q16(inverse as u64).min(Weight::MAX as u64) as Weight;
            entropy = entropy.saturating_add(q16_mul(probability, negative_log) as u128);
        }
    }
    let entropy = entropy.min(Weight::MAX as u128) as Weight;
    let log_k = if used > 1 {
        ln_q16((used as u64) << 16).min(Weight::MAX as u64) as Weight
    } else {
        WEIGHT_ONE
    };
    let margin = log_k.saturating_sub(entropy);
    let delta_entropy = previous_entropy
        .map(|prior| entropy as i64 - prior as i64)
        .unwrap_or(0);
    let stopped_dropping =
        previous_entropy.is_some() && delta_entropy.saturating_neg().max(0) < epsilon as i64;
    let concentrated = margin > gamma;
    SdeStep {
        entropy,
        delta_entropy,
        margin,
        stopped_dropping,
        concentrated,
        converged: stopped_dropping && concentrated,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CmcCandidate {
    pub id: NodeId,
    pub entities: [NodeId; MAX_CMC_ENTITIES_PER_CANDIDATE],
    pub entity_count: u8,
}

impl CmcCandidate {
    pub const EMPTY: Self = Self {
        id: 0,
        entities: [0; MAX_CMC_ENTITIES_PER_CANDIDATE],
        entity_count: 0,
    };
}

#[derive(Debug, Clone, Copy)]
pub struct CausalMass {
    pub entity: NodeId,
    pub mass: Weight,
}

#[derive(Debug, Clone, Copy)]
pub struct CmcConfig {
    pub alpha: Weight,
    pub lambda: Weight,
    pub mu: Weight,
    pub max_spans: u8,
    pub tolerance: Weight,
}

impl Default for CmcConfig {
    fn default() -> Self {
        Self {
            alpha: WEIGHT_ONE,
            lambda: 8 * WEIGHT_ONE,
            mu: WEIGHT_ONE / 2,
            max_spans: MAX_CMC_CANDIDATES as u8,
            tolerance: 1,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CmcStep {
    pub picked_id: NodeId,
    pub cost: u64,
    pub cover_mass: Weight,
    pub description_bits: u64,
    pub redundancy: u64,
}

impl CmcStep {
    pub const EMPTY: Self = Self {
        picked_id: 0,
        cost: 0,
        cover_mass: 0,
        description_bits: 0,
        redundancy: 0,
    };
}

#[derive(Debug, Clone, Copy)]
pub struct CmcReport {
    pub selected: [NodeId; MAX_CMC_CANDIDATES],
    pub selected_count: u8,
    pub final_cost: u64,
    pub trajectory: [CmcStep; MAX_CMC_CANDIDATES],
    pub greedy_slack_bound: u64,
}

fn mass_index(masses: &[CausalMass], entity: NodeId) -> Option<usize> {
    masses.iter().position(|entry| entry.entity == entity)
}

fn candidate_contains(candidate: &CmcCandidate, entity: NodeId) -> bool {
    let count = (candidate.entity_count as usize).min(MAX_CMC_ENTITIES_PER_CANDIDATE);
    candidate.entities[..count].contains(&entity)
}

fn cmc_components(
    selected_count: usize,
    degrees: &[u8],
    masses: &[CausalMass],
    config: CmcConfig,
    log2_n: Weight,
    total_mass: Weight,
) -> (u64, Weight, u64, u64) {
    let cover_mass = degrees
        .iter()
        .zip(masses.iter())
        .filter(|(degree, _)| **degree > 0)
        .fold(0u64, |sum, (_, entry)| {
            sum.saturating_add(clamp_unit(entry.mass) as u64)
        })
        .min(WEIGHT_ONE as u64) as Weight;
    let redundancy_raw = degrees
        .iter()
        .zip(masses.iter())
        .filter(|(degree, _)| **degree > 1)
        .fold(0u128, |sum, (degree, entry)| {
            sum.saturating_add((*degree as u128 - 1) * clamp_unit(entry.mass) as u128)
        });
    let description = q16_mul(config.alpha, log2_n) as u64 * selected_count as u64;
    let gap_cost = q16_mul(config.lambda, total_mass.saturating_sub(cover_mass)) as u64;
    let redundancy = ((redundancy_raw * config.mu as u128) >> 16).min(u64::MAX as u128) as u64;
    (
        description
            .saturating_add(gap_cost)
            .saturating_add(redundancy),
        cover_mass,
        description,
        redundancy,
    )
}

/// Deterministic greedy C-MC2 selector. Equal-cost ties retain candidate order.
pub fn causal_min_cover_q16(
    candidates: &[CmcCandidate],
    causal_masses: &[CausalMass],
    config: CmcConfig,
) -> CmcReport {
    let candidate_count = candidates.len().min(MAX_CMC_CANDIDATES);
    let masses = &causal_masses[..causal_masses.len().min(MAX_CMC_ENTITIES)];
    let ln_n = ln_q16((candidate_count.max(1) as u64) << 16);
    let log2_n = (((ln_n as u128) << 16) / LN_2_Q16 as u128).min(Weight::MAX as u128) as Weight;
    let total_mass = masses
        .iter()
        .enumerate()
        .filter(|(index, entry)| {
            !masses[..*index]
                .iter()
                .any(|prior| prior.entity == entry.entity)
                && candidates[..candidate_count]
                    .iter()
                    .any(|candidate| candidate_contains(candidate, entry.entity))
        })
        .fold(0u64, |sum, (_, entry)| {
            sum.saturating_add(clamp_unit(entry.mass) as u64)
        })
        .min(WEIGHT_ONE as u64) as Weight;

    let mut report = CmcReport {
        selected: [0; MAX_CMC_CANDIDATES],
        selected_count: 0,
        final_cost: q16_mul(config.lambda, total_mass) as u64,
        trajectory: [CmcStep::EMPTY; MAX_CMC_CANDIDATES],
        greedy_slack_bound: 0,
    };
    let mut degrees = [0u8; MAX_CMC_ENTITIES];
    let mut selected = [false; MAX_CMC_CANDIDATES];
    let limit = (config.max_spans as usize).min(candidate_count);

    while (report.selected_count as usize) < limit {
        let mut best_index = None;
        let mut best_cost = report.final_cost;
        let mut best_degrees = degrees;
        let mut best_components = (0, 0, 0);
        for index in 0..candidate_count {
            if selected[index] {
                continue;
            }
            let mut trial_degrees = degrees;
            let candidate = &candidates[index];
            let entity_count =
                (candidate.entity_count as usize).min(MAX_CMC_ENTITIES_PER_CANDIDATE);
            for offset in 0..entity_count {
                let entity = candidate.entities[offset];
                if candidate.entities[..offset].contains(&entity) {
                    continue;
                }
                if let Some(mass_index_value) = mass_index(masses, entity) {
                    trial_degrees[mass_index_value] =
                        trial_degrees[mass_index_value].saturating_add(1);
                }
            }
            let (cost, cover, description, redundancy) = cmc_components(
                report.selected_count as usize + 1,
                &trial_degrees[..masses.len()],
                masses,
                config,
                log2_n,
                total_mass,
            );
            if cost.saturating_add(config.tolerance as u64) < best_cost {
                best_index = Some(index);
                best_cost = cost;
                best_degrees = trial_degrees;
                best_components = (cover, description, redundancy);
            }
        }
        let Some(index) = best_index else {
            break;
        };
        selected[index] = true;
        degrees = best_degrees;
        let step_index = report.selected_count as usize;
        report.selected[step_index] = candidates[index].id;
        report.trajectory[step_index] = CmcStep {
            picked_id: candidates[index].id,
            cost: best_cost,
            cover_mass: best_components.0,
            description_bits: best_components.1,
            redundancy: best_components.2,
        };
        report.selected_count += 1;
        report.final_cost = best_cost;
    }

    let max_redundancy = report.trajectory[..report.selected_count as usize]
        .iter()
        .map(|step| step.redundancy)
        .max()
        .unwrap_or(0);
    report.greedy_slack_bound = max_redundancy.saturating_mul(report.selected_count as u64);
    report
}
