//! Public retrieval pipeline with lexical/graph reciprocal-rank fusion.
//!
//! This module does not label score multiplication as causal or compositional
//! inference. It returns ranked evidence and an explicitly uncalibrated
//! `support_score`.

use crate::cognitive::bm25::Bm25Index;
use crate::cognitive::lsh::LshIndex;
use crate::cognitive::memory::Session;
use crate::cognitive::pagerank::PageRankEngine;
use crate::graph::arena;
use crate::graph::types::{NodeId, WEIGHT_ONE, Weight};

const MAX_EVIDENCE: usize = 8;
const CHANNEL_CANDIDATES: usize = 16;
const MAX_QUERY_TERMS: usize = 32;
const RRF_K: u32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RetrievalStrategy {
    ReciprocalRankFusion = 0,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecoveryStrategy {
    SynonymExpansion = 0,
    BroadenScope = 1,
    LowerThreshold = 2,
    AskClarifying = 3,
}

#[derive(Debug, Clone, Copy)]
pub struct Evidence {
    pub node_id: NodeId,
    pub score: Weight,
    pub primary_channel: RetrievalChannel,
}

impl Evidence {
    const EMPTY: Self = Self {
        node_id: 0,
        score: 0,
        primary_channel: RetrievalChannel::Lexical,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RetrievalChannel {
    Lexical = 0,
    Graph = 1,
}

pub struct PipelineResult {
    pub evidence: [Evidence; MAX_EVIDENCE],
    pub evidence_count: usize,
    pub strategy: RetrievalStrategy,
    pub recovery_used: bool,
    pub recovery_strategy: RecoveryStrategy,
    /// Rank-fusion support, not a probability of correctness.
    pub support_score: Weight,
    /// Count of results whose graph node still exists. This is provenance
    /// integrity only; it is not entailment or answer correctness.
    pub provenance_live_nodes: u32,
    /// Bit 0 = lexical channel, bit 1 = personalized graph channel.
    pub channels_used: u8,
}

impl PipelineResult {
    pub const fn empty() -> Self {
        Self {
            evidence: [Evidence::EMPTY; MAX_EVIDENCE],
            evidence_count: 0,
            strategy: RetrievalStrategy::ReciprocalRankFusion,
            recovery_used: false,
            recovery_strategy: RecoveryStrategy::AskClarifying,
            support_score: 0,
            provenance_live_nodes: 0,
            channels_used: 0,
        }
    }
}

pub struct Engines<'a> {
    pub bm25: &'a Bm25Index,
    pub pagerank: &'a mut PageRankEngine,
    pub lsh: &'a LshIndex,
    pub session: Option<&'a mut Session>,
}

pub fn execute(query: &[u8], engines: &mut Engines<'_>, query_fingerprint: u64) -> PipelineResult {
    let _review_context = (&engines.lsh, &engines.session, query_fingerprint);
    let mut terms: [&[u8]; MAX_QUERY_TERMS] = [&[]; MAX_QUERY_TERMS];
    let term_count = split_query(query, &mut terms);
    if term_count == 0 {
        return PipelineResult::empty();
    }

    let mut lexical = [(0u64, 0u32); CHANNEL_CANDIDATES];
    let lexical_count = engines.bm25.query(&terms[..term_count], &mut lexical);
    if lexical_count == 0 {
        return PipelineResult {
            recovery_used: true,
            ..PipelineResult::empty()
        };
    }

    engines
        .pagerank
        .compute_personalized(&lexical[..lexical_count]);
    let mut graph = [(0u64, 0u32); CHANNEL_CANDIDATES];
    let graph_count = engines.pagerank.top_k(&mut graph);

    let mut result = PipelineResult {
        channels_used: 1 | if graph_count > 0 { 2 } else { 0 },
        ..PipelineResult::empty()
    };
    for (rank, &(node_id, _)) in lexical.iter().take(lexical_count).enumerate() {
        add_rrf(&mut result, node_id, rank, RetrievalChannel::Lexical);
    }
    for (rank, &(node_id, _)) in graph.iter().take(graph_count).enumerate() {
        add_rrf(&mut result, node_id, rank, RetrievalChannel::Graph);
    }
    result.evidence[..result.evidence_count]
        .sort_unstable_by_key(|item| core::cmp::Reverse(item.score));
    result.support_score = result.evidence.first().map(|item| item.score).unwrap_or(0);
    result.provenance_live_nodes = result.evidence[..result.evidence_count]
        .iter()
        .filter(|item| arena::node_exists(item.node_id))
        .count() as u32;
    result
}

fn add_rrf(
    result: &mut PipelineResult,
    node_id: NodeId,
    zero_based_rank: usize,
    channel: RetrievalChannel,
) {
    let contribution = (WEIGHT_ONE as u64 / (RRF_K as u64 + zero_based_rank as u64 + 1))
        .min(Weight::MAX as u64) as Weight;
    if let Some(evidence) = result.evidence[..result.evidence_count]
        .iter_mut()
        .find(|item| item.node_id == node_id)
    {
        evidence.score = evidence.score.saturating_add(contribution);
        return;
    }
    if result.evidence_count < MAX_EVIDENCE {
        result.evidence[result.evidence_count] = Evidence {
            node_id,
            score: contribution,
            primary_channel: channel,
        };
        result.evidence_count += 1;
    }
}

fn split_query<'a>(query: &'a [u8], output: &mut [&'a [u8]]) -> usize {
    let mut count = 0;
    let mut cursor = 0;
    while cursor < query.len() && count < output.len() {
        while cursor < query.len() && query[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let start = cursor;
        while cursor < query.len() && !query[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor > start {
            output[count] = &query[start..cursor];
            count += 1;
        }
    }
    count
}
