//! Allocation-free BM25 lexical retrieval with collision-safe term identity.
//!
//! Postings are incremental linked lists. Interleaving A/B/A insertions can
//! therefore never make one term read another term's postings. Scores use Q16
//! arithmetic without discarding fractional precision before division.

use crate::graph::temporal::ln_q16;
use crate::graph::types::Weight;

const MAX_TERMS: usize = 4096;
const MAX_TERM_BYTES: usize = 64;
const MAX_DOCS: usize = 2048;
const MAX_POSTINGS: usize = 32768;
const POSTING_NONE: u32 = u32::MAX;

const K1_Q16: u64 = 78_643; // 1.2
const B_Q16: u64 = 49_152; // 0.75
const Q16_ONE: u64 = 1 << 16;

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x00000100000001b3;

fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn q16_mul(left: u64, right: u64) -> u64 {
    ((left as u128).saturating_mul(right as u128) >> 16).min(u64::MAX as u128) as u64
}

fn q16_div(numerator_q16: u64, denominator_q16: u64) -> u64 {
    if denominator_q16 == 0 {
        return u64::MAX;
    }
    (((numerator_q16 as u128) << 16) / denominator_q16 as u128).min(u64::MAX as u128) as u64
}

#[derive(Clone, Copy)]
struct TermEntry {
    hash: u64,
    length: u8,
    bytes: [u8; MAX_TERM_BYTES],
    document_frequency: u32,
    posting_head: u32,
}

impl TermEntry {
    const EMPTY: Self = Self {
        hash: 0,
        length: 0,
        bytes: [0; MAX_TERM_BYTES],
        document_frequency: 0,
        posting_head: POSTING_NONE,
    };

    fn matches(&self, hash: u64, term: &[u8]) -> bool {
        self.hash == hash && self.length as usize == term.len() && self.bytes[..term.len()] == *term
    }

    fn from_term(hash: u64, term: &[u8]) -> Self {
        let mut bytes = [0; MAX_TERM_BYTES];
        bytes[..term.len()].copy_from_slice(term);
        Self {
            hash,
            length: term.len() as u8,
            bytes,
            document_frequency: 0,
            posting_head: POSTING_NONE,
        }
    }
}

#[derive(Clone, Copy)]
struct Posting {
    document_id: u16,
    term_frequency: u16,
    next: u32,
}

impl Posting {
    const EMPTY: Self = Self {
        document_id: 0,
        term_frequency: 0,
        next: POSTING_NONE,
    };
}

#[derive(Clone, Copy)]
struct DocumentMetadata {
    length: u32,
    external_id: u64,
}

impl DocumentMetadata {
    const EMPTY: Self = Self {
        length: 0,
        external_id: 0,
    };
}

pub struct Bm25Index {
    terms: [TermEntry; MAX_TERMS],
    term_count: usize,
    postings: [Posting; MAX_POSTINGS],
    posting_count: usize,
    documents: [DocumentMetadata; MAX_DOCS],
    document_count: usize,
    total_document_length: u64,
}

impl Bm25Index {
    pub const fn new() -> Self {
        Self {
            terms: [TermEntry::EMPTY; MAX_TERMS],
            term_count: 0,
            postings: [Posting::EMPTY; MAX_POSTINGS],
            posting_count: 0,
            documents: [DocumentMetadata::EMPTY; MAX_DOCS],
            document_count: 0,
            total_document_length: 0,
        }
    }

    pub fn add_document(&mut self, external_id: u64) -> Option<u16> {
        if self.document_count >= MAX_DOCS {
            return None;
        }
        let internal_id = self.document_count as u16;
        self.documents[self.document_count] = DocumentMetadata {
            length: 0,
            external_id,
        };
        self.document_count += 1;
        Some(internal_id)
    }

    pub fn index_term(&mut self, term: &[u8], document_id: u16) -> bool {
        self.index_term_with_hash(term, document_id, fnv1a64(term))
    }

    fn index_term_with_hash(&mut self, term: &[u8], document_id: u16, hash: u64) -> bool {
        if term.is_empty()
            || term.len() > MAX_TERM_BYTES
            || document_id as usize >= self.document_count
        {
            return false;
        }
        let Some(term_index) = self.find_or_create_term(hash, term) else {
            return false;
        };

        let mut cursor = self.terms[term_index].posting_head;
        while cursor != POSTING_NONE {
            let posting = &mut self.postings[cursor as usize];
            if posting.document_id == document_id {
                posting.term_frequency = posting.term_frequency.saturating_add(1);
                self.increment_document_length(document_id);
                return true;
            }
            cursor = posting.next;
        }

        if self.posting_count >= MAX_POSTINGS {
            return false;
        }
        let posting_index = self.posting_count;
        self.posting_count += 1;
        self.postings[posting_index] = Posting {
            document_id,
            term_frequency: 1,
            next: self.terms[term_index].posting_head,
        };
        self.terms[term_index].posting_head = posting_index as u32;
        self.terms[term_index].document_frequency =
            self.terms[term_index].document_frequency.saturating_add(1);
        self.increment_document_length(document_id);
        true
    }

    fn increment_document_length(&mut self, document_id: u16) {
        self.documents[document_id as usize].length = self.documents[document_id as usize]
            .length
            .saturating_add(1);
        self.total_document_length = self.total_document_length.saturating_add(1);
    }

    pub fn index_text(&mut self, text: &[u8], document_id: u16) -> u32 {
        let mut indexed = 0u32;
        let mut cursor = 0usize;
        while cursor < text.len() {
            while cursor < text.len() && is_whitespace(text[cursor]) {
                cursor += 1;
            }
            let start = cursor;
            while cursor < text.len() && !is_whitespace(text[cursor]) {
                cursor += 1;
            }
            if cursor > start && self.index_term(&text[start..cursor], document_id) {
                indexed = indexed.saturating_add(1);
            }
        }
        indexed
    }

    pub fn query(&self, query_terms: &[&[u8]], out: &mut [(u64, Weight)]) -> usize {
        if self.document_count == 0 || out.is_empty() || self.total_document_length == 0 {
            return 0;
        }

        const SCORE_CAPACITY: usize = MAX_DOCS;
        let scoring_documents = self.document_count.min(SCORE_CAPACITY);
        let document_count_q16 = (self.document_count as u64) << 16;
        let average_length_q16 = q16_div(self.total_document_length << 16, document_count_q16);
        let mut scores_q16 = [0u64; SCORE_CAPACITY];

        for query_term in query_terms {
            if query_term.is_empty() || query_term.len() > MAX_TERM_BYTES {
                continue;
            }
            let hash = fnv1a64(query_term);
            let Some(term_index) = self.find_term(hash, query_term) else {
                continue;
            };
            let term = &self.terms[term_index];
            let idf_q16 = idf_q16(term.document_frequency, self.document_count as u32);
            let mut cursor = term.posting_head;
            while cursor != POSTING_NONE {
                let posting = self.postings[cursor as usize];
                let document_index = posting.document_id as usize;
                if document_index < scoring_documents {
                    let term_frequency_q16 = (posting.term_frequency as u64) << 16;
                    let document_length_q16 = (self.documents[document_index].length as u64) << 16;
                    let contribution = bm25_term_score_q16(
                        term_frequency_q16,
                        document_length_q16,
                        average_length_q16,
                        idf_q16,
                    );
                    scores_q16[document_index] =
                        scores_q16[document_index].saturating_add(contribution);
                }
                cursor = posting.next;
            }
        }

        let mut used = [false; SCORE_CAPACITY];
        let mut written = 0usize;
        while written < out.len() {
            let mut best_document = None;
            let mut best_score = 0u64;
            for document in 0..scoring_documents {
                if !used[document] && scores_q16[document] > best_score {
                    best_score = scores_q16[document];
                    best_document = Some(document);
                }
            }
            let Some(document) = best_document else {
                break;
            };
            used[document] = true;
            out[written] = (
                self.documents[document].external_id,
                best_score.min(Weight::MAX as u64) as Weight,
            );
            written += 1;
        }
        written
    }

    fn find_term(&self, hash: u64, term: &[u8]) -> Option<usize> {
        (0..self.term_count).find(|index| self.terms[*index].matches(hash, term))
    }

    fn find_or_create_term(&mut self, hash: u64, term: &[u8]) -> Option<usize> {
        if let Some(index) = self.find_term(hash, term) {
            return Some(index);
        }
        if self.term_count >= MAX_TERMS {
            return None;
        }
        let index = self.term_count;
        self.terms[index] = TermEntry::from_term(hash, term);
        self.term_count += 1;
        Some(index)
    }

    pub fn doc_count(&self) -> usize {
        self.document_count
    }

    pub fn term_count(&self) -> usize {
        self.term_count
    }

    pub fn posting_count(&self) -> usize {
        self.posting_count
    }
}

fn idf_q16(document_frequency: u32, document_count: u32) -> u64 {
    // 1 + (N - df + 0.5)/(df + 0.5), with halves cleared exactly.
    let numerator_q16 = (document_count.saturating_sub(document_frequency) as u64 * 2 + 1) << 16;
    let denominator_q16 = (document_frequency as u64 * 2 + 1) << 16;
    let argument_q16 = Q16_ONE.saturating_add(q16_div(numerator_q16, denominator_q16));
    ln_q16(argument_q16)
}

fn bm25_term_score_q16(
    term_frequency_q16: u64,
    document_length_q16: u64,
    average_length_q16: u64,
    idf_q16: u64,
) -> u64 {
    if term_frequency_q16 == 0 || average_length_q16 == 0 {
        return 0;
    }
    let numerator_q16 = q16_mul(term_frequency_q16, K1_Q16 + Q16_ONE);
    let length_ratio_q16 = q16_div(document_length_q16, average_length_q16);
    let normalization_q16 = (Q16_ONE - B_Q16).saturating_add(q16_mul(B_Q16, length_ratio_q16));
    let denominator_q16 = term_frequency_q16.saturating_add(q16_mul(K1_Q16, normalization_q16));
    q16_mul(idf_q16, q16_div(numerator_q16, denominator_q16))
}

fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaved_postings_are_isolated() {
        let mut index = Bm25Index::new();
        let first = index.add_document(10).unwrap();
        let second = index.add_document(20).unwrap();
        assert!(index.index_term(b"alpha", first));
        assert!(index.index_term(b"beta", first));
        assert!(index.index_term(b"alpha", second));
        let mut output = [(0, 0); 2];
        assert_eq!(index.query(&[b"alpha"], &mut output), 2);
        assert!(output.iter().all(|entry| entry.1 > 0));
    }

    #[test]
    fn forced_hash_collision_does_not_merge_terms() {
        let mut index = Bm25Index::new();
        let first = index.add_document(10).unwrap();
        let second = index.add_document(20).unwrap();
        assert!(index.index_term_with_hash(b"alpha", first, 7));
        assert!(index.index_term_with_hash(b"omega", second, 7));
        assert_eq!(index.term_count(), 2);
        assert!(index.find_term(7, b"alpha").is_some());
        assert!(index.find_term(7, b"omega").is_some());
    }

    #[test]
    fn q16_reference_scores_are_within_one_thousandth_relative_error() {
        // N=2, df=1 => idf=ln(2); tf=1, dl=avgdl=1 => term factor=1.
        let idf = idf_q16(1, 2);
        let score = bm25_term_score_q16(Q16_ONE, Q16_ONE, Q16_ONE, idf);
        let reference_q16 = 45_426u64;
        let error = score.abs_diff(reference_q16);
        assert!(error * 1_000 < reference_q16);
    }

    #[test]
    fn empty_and_repeated_query_terms_are_defined() {
        let mut index = Bm25Index::new();
        index.add_document(1).unwrap();
        let mut output = [(0, 0); 1];
        assert_eq!(index.query(&[b"anything"], &mut output), 0);
        assert!(index.index_text(b"alpha alpha", 0) == 2);
        assert_eq!(index.query(&[b"alpha", b"alpha"], &mut output), 1);
        assert!(output[0].1 > 0);
    }
}
