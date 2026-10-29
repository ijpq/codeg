//! Protocol-neutral business prose. Native tools and private reasoning are not
//! replayable messages; their wire payloads never cross an engine boundary.
use super::{ContentBlock, MessageTurn, TurnRole};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub fn history_prose(turn: &MessageTurn) -> String {
    turn.blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } if !matches!(turn.role, TurnRole::System) => {
                Some(text.clone())
            }
            ContentBlock::TurnError { message } => Some(format!("[execution failed: {message}]")),
            ContentBlock::Image { uri, .. } => Some(format!(
                "[image reference: {}]",
                uri.as_deref()
                    .unwrap_or("available in original CodeG history")
            )),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn prose_digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub fn projection_digest(turn: &MessageTurn) -> String {
    prose_digest(&format!("{:?}\n{}", turn.role, history_prose(turn)))
}

/// UTF-8 byte ranges actually included in an accepted context bridge. A
/// partial record is never represented as acknowledgement of the whole turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistoryDelivery {
    pub sequence: i64,
    pub digest: String,
    pub total_bytes: usize,
    pub ranges: Vec<[usize; 2]>,
}

impl HistoryDelivery {
    pub fn normalize(&mut self) {
        self.ranges
            .retain(|r| r[0] < r[1] && r[1] <= self.total_bytes);
        self.ranges.sort_unstable();
        let mut merged: Vec<[usize; 2]> = Vec::new();
        for range in self.ranges.drain(..) {
            if let Some(last) = merged.last_mut().filter(|last| last[1] >= range[0]) {
                last[1] = last[1].max(range[1]);
            } else {
                merged.push(range);
            }
        }
        self.ranges = merged;
    }

    pub fn complete(&self) -> bool {
        self.total_bytes == 0 || self.ranges.as_slice() == [[0, self.total_bytes]]
    }

    pub fn gaps(&self) -> Vec<[usize; 2]> {
        let mut start = 0;
        let mut gaps = Vec::new();
        for [a, b] in &self.ranges {
            if start < *a {
                gaps.push([start, *a]);
            }
            start = start.max(*b);
        }
        if start < self.total_bytes {
            gaps.push([start, self.total_bytes]);
        }
        gaps
    }
}

pub struct HistoryRecord {
    pub turn: MessageTurn,
    pub delivery: HistoryDelivery,
}
