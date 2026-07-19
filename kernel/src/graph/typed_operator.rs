//! Typed temporal graph operators.
//!
//! This module intentionally makes no Walsh, Fourier, or diagonalization
//! claim.  It exposes only the public display/review contract: a dense Q16
//! type-channel operator and a typed message coefficient.  Temporal decay is
//! supplied independently by [`crate::graph::temporal`].

use crate::graph::temporal;
use crate::graph::types::{EdgeKind, NODE_KIND_COUNT, NodeKind, WEIGHT_ONE, Weight};

/// One Q16 channel per public node kind.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeSignal {
    pub data: [Weight; NODE_KIND_COUNT],
}

impl TypeSignal {
    pub const ZERO: Self = Self {
        data: [0; NODE_KIND_COUNT],
    };

    pub const ONE: Self = Self {
        data: [WEIGHT_ONE; NODE_KIND_COUNT],
    };

    pub fn impulse(kind: NodeKind, weight: Weight) -> Self {
        let mut signal = Self::ZERO;
        signal.set(kind, weight);
        signal
    }

    pub fn get(&self, kind: NodeKind) -> Weight {
        self.data[kind.index()]
    }

    pub fn set(&mut self, kind: NodeKind, value: Weight) {
        self.data[kind.index()] = value;
    }

    pub fn accumulate(&mut self, kind: NodeKind, delta: Weight) {
        let index = kind.index();
        self.data[index] = self.data[index].saturating_add(delta);
    }
}

/// Dense Q16 type-channel matrix implementing `out[dst] = sum M[src,dst] in[src]`.
#[derive(Debug, Clone, Copy)]
pub struct TypeChannelOperator {
    coefficients: [[Weight; NODE_KIND_COUNT]; NODE_KIND_COUNT],
}

impl TypeChannelOperator {
    pub const ZERO: Self = Self {
        coefficients: [[0; NODE_KIND_COUNT]; NODE_KIND_COUNT],
    };

    pub fn identity() -> Self {
        let mut result = Self::ZERO;
        let mut index = 0;
        while index < NODE_KIND_COUNT {
            result.coefficients[index][index] = WEIGHT_ONE;
            index += 1;
        }
        result
    }

    pub fn coefficient(&self, source: NodeKind, destination: NodeKind) -> Weight {
        self.coefficients[source.index()][destination.index()]
    }

    pub fn set_coefficient(
        &mut self,
        source: NodeKind,
        destination: NodeKind,
        coefficient_q16: Weight,
    ) {
        self.coefficients[source.index()][destination.index()] = coefficient_q16;
    }

    pub fn apply(&self, input: &TypeSignal) -> TypeSignal {
        let mut output = TypeSignal::ZERO;
        let mut destination = 0;
        while destination < NODE_KIND_COUNT {
            let mut sum_q32 = 0u128;
            let mut source = 0;
            while source < NODE_KIND_COUNT {
                sum_q32 = sum_q32.saturating_add(
                    (input.data[source] as u128)
                        .saturating_mul(self.coefficients[source][destination] as u128),
                );
                source += 1;
            }
            output.data[destination] = ((sum_q32 >> 16).min(Weight::MAX as u128)) as Weight;
            destination += 1;
        }
        output
    }
}

/// One incoming graph message. `coefficient_q16` is structural and does not
/// encode a decay rate.
#[derive(Debug, Clone, Copy)]
pub struct TypedMessage {
    pub source_kind: NodeKind,
    pub destination_kind: NodeKind,
    pub edge_kind: EdgeKind,
    pub edge_weight_q16: Weight,
    pub source_value_q16: Weight,
    pub coefficient_q16: Weight,
    pub lambda_per_tick_q16: Weight,
    pub elapsed_ticks: u64,
}

/// Evaluate one term of typed temporal message passing in Q16.
pub fn evaluate_message(message: &TypedMessage) -> Weight {
    let _type_identity = (
        message.source_kind,
        message.destination_kind,
        message.edge_kind,
    );
    let decayed_edge = temporal::decay_weight_q16(
        message.edge_weight_q16,
        message.lambda_per_tick_q16,
        message.elapsed_ticks,
    );
    let structural = q16_mul(decayed_edge, message.coefficient_q16);
    q16_mul(structural, message.source_value_q16)
}

/// Saturating sum of independently evaluated incoming messages.
pub fn aggregate_messages(messages: &[TypedMessage]) -> Weight {
    messages.iter().fold(0, |sum, message| {
        sum.saturating_add(evaluate_message(message))
    })
}

pub const fn q16_mul(left: Weight, right: Weight) -> Weight {
    let value = (left as u64).saturating_mul(right as u64) >> 16;
    if value > Weight::MAX as u64 {
        Weight::MAX
    } else {
        value as Weight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_operator_preserves_all_38_channels() {
        let mut signal = TypeSignal::ZERO;
        for (index, value) in signal.data.iter_mut().enumerate() {
            *value = ((index + 1) as u32) << 12;
        }
        assert_eq!(TypeChannelOperator::identity().apply(&signal), signal);
    }

    #[test]
    fn message_coefficient_and_decay_are_independent() {
        let message = TypedMessage {
            source_kind: NodeKind::Task,
            destination_kind: NodeKind::Service,
            edge_kind: EdgeKind::DependsOn,
            edge_weight_q16: WEIGHT_ONE,
            source_value_q16: WEIGHT_ONE,
            coefficient_q16: WEIGHT_ONE / 2,
            lambda_per_tick_q16: 0,
            elapsed_ticks: 10,
        };
        assert_eq!(evaluate_message(&message), WEIGHT_ONE / 2);
    }
}
