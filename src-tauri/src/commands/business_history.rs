//! Bounded, ordered direct context with exact accepted byte-range receipts.
use crate::models::business_context::{history_prose, HistoryDelivery, HistoryRecord};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct HistoryPlan {
    pub text: String,
    pub deliveries: Vec<HistoryDelivery>,
    pub through: Option<i64>,
}

fn floor_boundary(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn ceil_boundary(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

fn select(record: &mut HistoryRecord, budget: &mut usize, tail: bool) -> Vec<[usize; 2]> {
    let text = history_prose(&record.turn);
    let mut gaps = record.delivery.gaps();
    if tail {
        gaps.reverse();
    }
    let mut selected = Vec::new();
    for [start, end] in gaps {
        // Leave enough room for sequence/role/range labels. Every byte of actual
        // text counts, including multibyte UTF-8; no hidden per-block clipping.
        if *budget < 192 {
            break;
        }
        let available = budget.saturating_sub(192);
        let range = if tail {
            [
                ceil_boundary(&text, end.saturating_sub(available).max(start)),
                end,
            ]
        } else {
            [
                start,
                floor_boundary(&text, start.saturating_add(available).min(end)),
            ]
        };
        if range[0] == range[1] {
            break;
        }
        *budget -= range[1] - range[0] + 192;
        selected.push(range);
        record.delivery.ranges.push(range);
        record.delivery.normalize();
    }
    selected
}

pub fn plan_history(
    early: Vec<HistoryRecord>,
    recent: Vec<HistoryRecord>,
    requirements: Vec<HistoryRecord>,
) -> HistoryPlan {
    let early_sequences = early
        .iter()
        .map(|r| r.delivery.sequence)
        .collect::<Vec<_>>();
    let mut records: BTreeMap<i64, HistoryRecord> = early
        .into_iter()
        .map(|r| (r.delivery.sequence, r))
        .collect();
    let recent_sequences = recent
        .iter()
        .map(|r| r.delivery.sequence)
        .collect::<Vec<_>>();
    for r in recent {
        records.entry(r.delivery.sequence).or_insert(r);
    }
    let requirement_sequences = requirements
        .iter()
        .map(|r| r.delivery.sequence)
        .collect::<Vec<_>>();
    for r in requirements {
        records.entry(r.delivery.sequence).or_insert(r);
    }
    let mut selected: BTreeMap<i64, Vec<[usize; 2]>> = BTreeMap::new();
    // Recent business discussion is delivered directly, before older backlog
    // consumes its budget. On oversized records keep the latest correction;
    // the precise undispatched gap stays available to the history tool.
    let mut recent_budget = 24_000;
    let mut recent_priority = recent_sequences.iter().rev().copied().collect::<Vec<_>>();
    // User corrections and safety requirements precede a large assistant
    // narrative in the allocation, while rendering still follows sequence.
    recent_priority
        .sort_by_key(|seq| !matches!(records[seq].turn.role, crate::models::TurnRole::User));
    for seq in &recent_priority {
        let ranges = select(records.get_mut(seq).unwrap(), &mut recent_budget, true);
        selected.entry(*seq).or_default().extend(ranges);
    }
    let mut old_budget = 31_000;
    // Earlier user requirements must not be crowded out by long assistant/tool
    // narratives, even when they lie beyond the first chronological page.
    for seq in &requirement_sequences {
        let ranges = select(records.get_mut(seq).unwrap(), &mut old_budget, false);
        selected.entry(*seq).or_default().extend(ranges);
    }
    for seq in &early_sequences {
        let record = records.get_mut(seq).unwrap();
        let ranges = select(record, &mut old_budget, false);
        selected.entry(*seq).or_default().extend(ranges);
    }
    let mut result = HistoryPlan::default();
    let mut contiguous = true;
    for seq in early_sequences {
        if !records[&seq].delivery.complete() {
            contiguous = false;
        }
        if contiguous {
            result.through = Some(seq);
        }
    }
    let incomplete = records.values().any(|r| !r.delivery.complete());
    for (seq, mut ranges) in selected {
        let record = &records[&seq];
        if ranges.is_empty() && record.delivery.total_bytes != 0 {
            continue;
        }
        if record.delivery.total_bytes == 0 {
            result.deliveries.push(record.delivery.clone());
            continue;
        }
        ranges.sort_unstable();
        let prose = history_prose(&record.turn);
        result.text.push_str(&format!(
            "[sequence {seq}, {:?}, {}]\n",
            record.turn.role,
            if record.delivery.complete() {
                "complete prose included"
            } else {
                "partial prose; not fully acknowledged"
            }
        ));
        for [a, b] in &ranges {
            if *a != 0 || *b != prose.len() {
                result
                    .text
                    .push_str(&format!("[UTF-8 bytes {a}..{b}/{}]\n", prose.len()));
            }
            result.text.push_str(&prose[*a..*b]);
            result.text.push('\n');
        }
        // Store only this request's ranges; accept merges them atomically with
        // prior receipts. An unaccepted/failed request cannot advance coverage.
        result.deliveries.push(HistoryDelivery {
            ranges,
            ..record.delivery.clone()
        });
    }
    if incomplete || records.len() >= 129 {
        result.text.push_str("[Bridge budget reached; older undispatched history remains retrievable and is not acknowledged. These are excerpts, not the whole conversation. Use get_session_info with history_cursor=\"sequence:<number>\" for the complete relevant record, or history_cursor=\"start\" and follow next_cursor for the original ordered prose. Read missing relevant requirements/failed attempts before acting; do not assume their contents.]\n");
    }
    result
}
