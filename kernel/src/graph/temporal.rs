//! Public temporal graph primitives using one explicit unit: integer ticks.
//!
//! Decay rates are Q16 values per tick.  Structural message coefficients are
//! deliberately not stored here; see [`crate::graph::typed_operator`].

use spin::Mutex;

use crate::graph::types::{NODE_KIND_COUNT, NodeKind, Timestamp, WEIGHT_ONE, Weight};

pub const LN_2_Q16: Weight = 45_426;
pub const SECONDS_PER_DAY: u64 = 86_400;
const PAIR_COUNT: usize = NODE_KIND_COUNT * NODE_KIND_COUNT;
const MIN_ADAPTATION_SAMPLES: u64 = 8;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypePairEntry {
    /// Exponential decay rate in Q16 per tick.
    pub lambda_per_tick_q16: Weight,
    /// Node2vec return parameter `p` in Q16.
    pub return_p: Weight,
    /// Node2vec in/out parameter `q` in Q16.
    pub inout_q: Weight,
}

impl TypePairEntry {
    pub const DEFAULT: Self = Self {
        lambda_per_tick_q16: 0,
        return_p: WEIGHT_ONE,
        inout_q: WEIGHT_ONE,
    };

    pub const STATIC: Self = Self::DEFAULT;
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypePairStats {
    pub transitions: u64,
    pub sum_elapsed_ticks: u64,
    pub return_count: u64,
    pub outward_count: u64,
}

impl TypePairStats {
    pub const ZERO: Self = Self {
        transitions: 0,
        sum_elapsed_ticks: 0,
        return_count: 0,
        outward_count: 0,
    };

    pub const fn is_ready(&self) -> bool {
        self.transitions >= MIN_ADAPTATION_SAMPLES && self.sum_elapsed_ticks > 0
    }
}

static PARAMS: Mutex<[TypePairEntry; PAIR_COUNT]> =
    Mutex::new([TypePairEntry::DEFAULT; PAIR_COUNT]);
static STATS: Mutex<[TypePairStats; PAIR_COUNT]> = Mutex::new([TypePairStats::ZERO; PAIR_COUNT]);

const fn pair_index(source: NodeKind, destination: NodeKind) -> usize {
    source.index() * NODE_KIND_COUNT + destination.index()
}

pub fn init() {
    *PARAMS.lock() = [TypePairEntry::DEFAULT; PAIR_COUNT];
    *STATS.lock() = [TypePairStats::ZERO; PAIR_COUNT];
}

pub fn get_decay(source: NodeKind, destination: NodeKind) -> Weight {
    get_params(source, destination).lambda_per_tick_q16
}

pub fn get_params(source: NodeKind, destination: NodeKind) -> TypePairEntry {
    PARAMS.lock()[pair_index(source, destination)]
}

pub fn set_params(source: NodeKind, destination: NodeKind, entry: TypePairEntry) {
    PARAMS.lock()[pair_index(source, destination)] = entry;
}

pub const fn delta_t(now_tick: Timestamp, edge_tick: Timestamp) -> u64 {
    now_tick.saturating_sub(edge_tick)
}

/// Convert a Q16 rate per day to a Q16 rate per integer tick.
pub fn rate_per_day_to_rate_per_tick_q16(
    rate_per_day_q16: Weight,
    seconds_per_tick: u64,
) -> Weight {
    let scaled = (rate_per_day_q16 as u128).saturating_mul(seconds_per_tick as u128);
    (scaled / SECONDS_PER_DAY as u128).min(Weight::MAX as u128) as Weight
}

/// Q16 decay rate corresponding to the requested half-life in ticks.
pub fn lambda_for_half_life_ticks(half_life_ticks: u64) -> Option<Weight> {
    if half_life_ticks == 0 {
        return None;
    }
    Some(
        ((LN_2_Q16 as u64 + half_life_ticks / 2) / half_life_ticks).min(Weight::MAX as u64)
            as Weight,
    )
}

/// Monotone Q16 approximation of `exp(-x)` for a nonnegative Q16 input.
///
/// Range reduction uses `x = n ln(2) + r`; a 16-segment LUT interpolates
/// `exp(-r)` on `[0, ln(2)]`, followed by the exact binary scale `2^-n`.
pub fn exp_neg_q16(x_q16: u64) -> Weight {
    const LUT: [u32; 17] = [
        65_536, 62_757, 60_097, 57_549, 55_109, 52_772, 50_535, 48_392, 46_341, 44_376, 42_495,
        40_693, 38_968, 37_316, 35_734, 34_219, 32_768,
    ];

    let binary_exponent = x_q16 / LN_2_Q16 as u64;
    if binary_exponent >= 32 {
        return 0;
    }
    let remainder = x_q16 % LN_2_Q16 as u64;
    let position = remainder.saturating_mul(16);
    let segment = (position / LN_2_Q16 as u64) as usize;
    let fraction = position % LN_2_Q16 as u64;
    let upper = (segment + 1).min(16);
    let span = LUT[segment].saturating_sub(LUT[upper]) as u64;
    let interpolated =
        (LUT[segment] as u64).saturating_sub(span.saturating_mul(fraction) / LN_2_Q16 as u64);
    (interpolated >> binary_exponent) as Weight
}

/// Apply `weight * exp(-lambda * elapsed_ticks)` in Q16.
pub fn decay_weight_q16(
    weight_q16: Weight,
    lambda_per_tick_q16: Weight,
    elapsed_ticks: u64,
) -> Weight {
    let x_q16 = (lambda_per_tick_q16 as u64).saturating_mul(elapsed_ticks);
    let factor_q16 = exp_neg_q16(x_q16);
    (((weight_q16 as u64).saturating_mul(factor_q16 as u64)) >> 16).min(Weight::MAX as u64)
        as Weight
}

/// A mixing-time upper bound only for a validated lazy, reversible,
/// irreducible chain. Returns `None` when any prerequisite or input is absent.
pub fn reversible_lazy_walk_bound(
    spectral_gap_q16: Weight,
    epsilon_q16: Weight,
    minimum_stationary_mass_q16: Weight,
    prerequisites_validated: bool,
) -> Option<u32> {
    if !prerequisites_validated
        || spectral_gap_q16 == 0
        || epsilon_q16 == 0
        || minimum_stationary_mass_q16 == 0
    {
        return None;
    }
    let product_q16 = ((epsilon_q16 as u64 * minimum_stationary_mass_q16 as u64) >> 16).max(1);
    let inverse_q16 = ((1u128 << 32) / product_q16 as u128).min(u64::MAX as u128) as u64;
    let logarithm_q16 = ln_q16(inverse_q16.max(WEIGHT_ONE as u64));
    let steps = ((logarithm_q16 as u128) << 16)
        .div_ceil(spectral_gap_q16 as u128)
        .min(u32::MAX as u128);
    Some(steps as u32)
}

/// Natural logarithm for a positive Q16 input, returned in Q16.
pub fn ln_q16(value_q16: u64) -> u64 {
    if value_q16 == 0 {
        return 0;
    }
    let most_significant = 63 - value_q16.leading_zeros() as i32;
    let exponent = most_significant - 16;
    let normalized_q31 = if most_significant <= 31 {
        value_q16 << (31 - most_significant)
    } else {
        value_q16 >> (most_significant - 31)
    };
    // atanh series: ln(m) = 2(z + z^3/3 + z^5/5 + ...), m in [1,2).
    let one_q31 = 1u64 << 31;
    let z_q31 =
        (((normalized_q31 - one_q31) as u128) << (31 / (normalized_q31 + one_q31) as u128)) as u64;
    let z2_q31 = ((z_q31 as u128 * z_q31 as u128) >> 31) as u64;
    let mut term_q31 = z_q31;
    let mut sum_q31 = term_q31;
    for denominator in [3u64, 5, 7, 9, 11] {
        term_q31 = ((term_q31 as u128 * z2_q31 as u128) >> 31) as u64;
        sum_q31 = sum_q31.saturating_add(term_q31 / denominator);
    }
    let mantissa_ln_q16 = sum_q31.saturating_mul(2) >> 15;
    if exponent >= 0 {
        mantissa_ln_q16.saturating_add(exponent as u64 * LN_2_Q16 as u64)
    } else {
        mantissa_ln_q16.saturating_sub((-exponent) as u64 * LN_2_Q16 as u64)
    }
}

pub fn accumulate_transition(
    previous_kind: Option<NodeKind>,
    source_kind: NodeKind,
    destination_kind: NodeKind,
    elapsed_ticks: u64,
) {
    let mut stats = STATS.lock();
    let pair = &mut stats[pair_index(source_kind, destination_kind)];
    pair.transitions = pair.transitions.saturating_add(1);
    pair.sum_elapsed_ticks = pair.sum_elapsed_ticks.saturating_add(elapsed_ticks);
    if previous_kind == Some(destination_kind) {
        pair.return_count = pair.return_count.saturating_add(1);
    } else if previous_kind.is_some() {
        pair.outward_count = pair.outward_count.saturating_add(1);
    }
}

pub fn snapshot_stats(out: &mut [TypePairStats]) -> u32 {
    let stats = STATS.lock();
    let count = out.len().min(PAIR_COUNT);
    out[..count].copy_from_slice(&stats[..count]);
    count as u32
}

pub fn snapshot_stats_pair(
    source_kind: u16,
    destination_kind: u16,
    out: &mut [TypePairStats],
) -> u32 {
    let (Some(source), Some(destination), Some(slot)) = (
        NodeKind::from_u16(source_kind),
        NodeKind::from_u16(destination_kind),
        out.first_mut(),
    ) else {
        return 0;
    };
    *slot = STATS.lock()[pair_index(source, destination)];
    1
}

/// Heuristic adaptation of type-pair decay rates from accumulated observations.
///
/// This is deliberately named a heuristic: it estimates only the exponential
/// waiting-time rate `n / sum(dt)`. It does not pretend to fit node2vec `p/q`
/// or perform an EM latent-variable step.
pub fn heuristic_type_pair_adaptation() -> u32 {
    let stats = STATS.lock();
    let mut params = PARAMS.lock();
    let mut updated = 0u32;
    for index in 0..PAIR_COUNT {
        if stats[index].is_ready() {
            params[index].lambda_per_tick_q16 = (((stats[index].transitions as u128) << 16)
                / stats[index].sum_elapsed_ticks as u128)
                .min(Weight::MAX as u128) as Weight;
            updated = updated.saturating_add(1);
        }
    }
    updated
}

pub fn adaptation_ready_count() -> usize {
    STATS.lock().iter().filter(|stats| stats.is_ready()).count()
}

pub fn dump() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn relative_error(actual: u32, expected: f64) -> f64 {
        ((actual as f64 / 65_536.0) - expected).abs() / expected.max(1e-12)
    }

    #[test]
    fn exponential_reference_points_and_monotonicity() {
        assert_eq!(exp_neg_q16(0), WEIGHT_ONE);
        assert!(relative_error(exp_neg_q16(LN_2_Q16 as u64), 0.5) < 1e-4);
        assert!(relative_error(exp_neg_q16(WEIGHT_ONE as u64), (-1.0f64).exp()) < 1e-3);
        let mut previous = WEIGHT_ONE;
        for x in 0..=(8 * WEIGHT_ONE) {
            let current = exp_neg_q16(x as u64);
            assert!(current <= previous);
            previous = current;
        }
    }

    #[test]
    fn half_life_and_tick_scaling_are_not_shifted_twice() {
        let lambda = lambda_for_half_life_ticks(100).unwrap();
        let decayed = decay_weight_q16(WEIGHT_ONE, lambda, 100);
        assert!(relative_error(decayed, 0.5) < 0.01);
        assert_eq!(
            rate_per_day_to_rate_per_tick_q16(WEIGHT_ONE, 86_400),
            WEIGHT_ONE
        );
    }

    #[test]
    fn adaptation_uses_q16_not_q32_scaling() {
        init();
        for _ in 0..8 {
            accumulate_transition(None, NodeKind::Task, NodeKind::Service, 4);
        }
        assert_eq!(heuristic_type_pair_adaptation(), 1);
        assert_eq!(get_decay(NodeKind::Task, NodeKind::Service), WEIGHT_ONE / 4);
    }
}
