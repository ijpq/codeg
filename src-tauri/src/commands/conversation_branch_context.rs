use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use crate::app_error::AppCommandError;
use crate::models::{ContentBlock, DbConversationDetail, ImageData, MessageTurn, TurnRole};
use crate::parsers::infer_context_window_max_tokens;

const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 128_000;
const CONTEXT_BUDGET_PERCENT: u64 = 65;
const MIN_CONTEXT_BUDGET_TOKENS: u64 = 16_000;
const RECENT_ENTRY_COUNT: usize = 12;
const OLDER_ASSISTANT_EXCERPT_CHARS: usize = 2_000;
const OVERSIZED_USER_EXCERPT_CHARS: usize = 6_000;
const MAX_INHERITED_IMAGES: usize = 8;
const MAX_INHERITED_IMAGE_BASE64_CHARS: usize = 24 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct BranchInheritanceSnapshot {
    pub context: String,
    pub images: Vec<ImageData>,
    pub inheritance_mode: String,
    pub inherited_message_count: i32,
    pub context_chars: i64,
    pub estimated_tokens: i64,
    pub source_context_chars: i64,
    pub source_estimated_tokens: i64,
    pub compressed: bool,
    pub truncated: bool,
    pub note: Option<String>,
    pub fork_message_id: Option<String>,
    pub forked_through_at: Option<DateTime<Utc>>,
    pub snapshot_version: i32,
}

#[derive(Debug, Clone)]
struct VisibleEntry {
    id: String,
    role: TurnRole,
    timestamp: DateTime<Utc>,
    text: String,
}

struct SnapshotResultMeta<'a> {
    mode: &'a str,
    entries: &'a [VisibleEntry],
    compressed: bool,
    truncated: bool,
    note: Option<String>,
    boundary: Option<&'a MessageTurn>,
    source_context_chars: i64,
    source_estimated_tokens: i64,
}

pub fn build_branch_inheritance_snapshot(
    detail: &DbConversationDetail,
    requested_fork_message_id: Option<&str>,
    token_budget_override: Option<u64>,
) -> Result<BranchInheritanceSnapshot, AppCommandError> {
    let mut turns = detail.turns.as_slice();
    let boundary_index = if let Some(message_id) = requested_fork_message_id {
        turns
            .iter()
            .position(|turn| turn.id == message_id)
            .ok_or_else(|| {
                AppCommandError::invalid_input(format!(
                    "The selected fork message is not present in the persisted source history: {message_id}"
                ))
            })?
    } else {
        // A parser may expose a partially-written final assistant turn while a
        // task is generating. The user prompt is persisted and safe to inherit;
        // the half reply is not. Never manufacture a boundary inside it.
        while turns.last().is_some_and(|turn| {
            matches!(turn.role, TurnRole::Assistant) && turn.completed_at.is_none()
        }) {
            turns = &turns[..turns.len() - 1];
        }
        turns.len().saturating_sub(1)
    };
    let bounded = if turns.is_empty() {
        &[][..]
    } else {
        &turns[..=boundary_index.min(turns.len() - 1)]
    };
    let boundary = bounded.last();
    let entries = visible_entries(bounded);
    let (images, omitted_images) = inherited_images(bounded);
    let deliverables = boundary
        .map(|turn| visible_deliverable_paths(detail, Some(turn.timestamp)))
        .unwrap_or_default();
    let max_tokens = detail
        .session_stats
        .as_ref()
        .and_then(|stats| stats.context_window_max_tokens)
        .or_else(|| infer_context_window_max_tokens(detail.summary.model.as_deref()))
        .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS);
    let budget = token_budget_override
        .unwrap_or_else(|| {
            (max_tokens.saturating_mul(CONTEXT_BUDGET_PERCENT) / 100).max(MIN_CONTEXT_BUDGET_TOKENS)
        })
        .saturating_sub((images.len() as u64).saturating_mul(1_500));

    let image_note = omitted_images.then_some(
        "Some inline images exceeded the safe branch attachment limit; their visible references remain in the text replay.",
    );
    let full = render_snapshot(
        detail,
        &entries,
        &deliverables,
        false,
        omitted_images,
        image_note,
    );
    let full_tokens = estimate_tokens(&full);
    let full_chars = full.chars().count() as i64;
    if full_tokens <= budget {
        return Ok(snapshot_result(
            full,
            images,
            SnapshotResultMeta {
                mode: "full_replay",
                entries: &entries,
                compressed: false,
                truncated: omitted_images,
                note: omitted_images.then(|| {
                    "Some inline images exceeded the safe branch attachment limit; their visible references remain in the text replay.".into()
                }),
                boundary,
                source_context_chars: full_chars,
                source_estimated_tokens: full_tokens as i64,
            },
        ));
    }

    let mut rendered_entries = vec![None; entries.len()];
    let mut used_tokens = estimate_tokens(&render_snapshot(
        detail,
        &[],
        &deliverables,
        true,
        false,
        None,
    ));
    let recent_start = entries.len().saturating_sub(RECENT_ENTRY_COUNT);

    // Requirements and corrections are the highest-value material. Preserve
    // every user turn verbatim when possible. If user text alone is too large,
    // give every user turn a fair chronological excerpt before spending space
    // anywhere else, so neither an early requirement nor a late correction is
    // silently starved by the other end of the conversation.
    let user_indices = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| matches!(entry.role, TurnRole::User).then_some(index))
        .collect::<Vec<_>>();
    let all_user_tokens = user_indices.iter().fold(0_u64, |total, index| {
        total.saturating_add(estimate_tokens(&entries[*index].text).saturating_add(16))
    });
    if used_tokens.saturating_add(all_user_tokens) <= budget {
        for index in &user_indices {
            let text = entries[*index].text.clone();
            used_tokens = used_tokens.saturating_add(estimate_tokens(&text).saturating_add(16));
            rendered_entries[*index] = Some(text);
        }
    } else if !user_indices.is_empty() {
        let fair_pool = budget.saturating_sub(used_tokens).saturating_mul(2) / 3;
        let fair_chars = (fair_pool / user_indices.len() as u64)
            .saturating_sub(16)
            .clamp(64, OVERSIZED_USER_EXCERPT_CHARS as u64) as usize;
        for index in &user_indices {
            let excerpt = priority_excerpt(&entries[*index].text, fair_chars);
            let excerpt_tokens = estimate_tokens(&excerpt).saturating_add(16);
            if used_tokens.saturating_add(excerpt_tokens) <= budget {
                rendered_entries[*index] = Some(excerpt);
                used_tokens = used_tokens.saturating_add(excerpt_tokens);
            }
        }
    }

    // Keep the latest exchange verbatim, then add bounded excerpts of older
    // user-visible conclusions. Thinking and tool blocks never entered entries.
    for index in (recent_start..entries.len()).rev() {
        let entry = &entries[index];
        let entry_tokens = estimate_tokens(&entry.text).saturating_add(16);
        let current_tokens = rendered_entries[index]
            .as_deref()
            .map(estimate_tokens)
            .unwrap_or(0)
            .saturating_add(u64::from(rendered_entries[index].is_some()) * 16);
        let extra_tokens = entry_tokens.saturating_sub(current_tokens);
        if used_tokens.saturating_add(extra_tokens) <= budget {
            rendered_entries[index] = Some(entry.text.clone());
            used_tokens = used_tokens.saturating_add(extra_tokens);
        }
    }
    for (index, entry) in entries.iter().enumerate() {
        if rendered_entries[index].is_some() || !matches!(entry.role, TurnRole::Assistant) {
            continue;
        }
        let excerpt = priority_excerpt(&entry.text, OLDER_ASSISTANT_EXCERPT_CHARS);
        let entry_tokens = estimate_tokens(&excerpt).saturating_add(16);
        if used_tokens.saturating_add(entry_tokens) <= budget {
            rendered_entries[index] = Some(excerpt);
            used_tokens = used_tokens.saturating_add(entry_tokens);
        }
    }

    let omitted = rendered_entries
        .iter()
        .filter(|entry| entry.is_none())
        .count();
    let truncated = omitted > 0
        || rendered_entries
            .iter()
            .enumerate()
            .any(|(index, rendered)| {
                rendered
                    .as_ref()
                    .is_some_and(|text| text != &entries[index].text)
            });
    let selected = entries
        .iter()
        .zip(rendered_entries)
        .filter_map(|(entry, text)| {
            text.map(|text| VisibleEntry {
                text,
                ..entry.clone()
            })
        })
        .collect::<Vec<_>>();
    let note = Some(format!(
        "Source history exceeded the reserved {budget}-token branch budget; preserved user requirements and corrections first, then the latest exchange and bounded older assistant conclusions. Omitted {omitted} lower-priority visible messages.{}",
        if omitted_images { " Some inline images exceeded the safe attachment limit." } else { "" }
    ));
    let context = render_snapshot(
        detail,
        &selected,
        &deliverables,
        true,
        truncated,
        note.as_deref(),
    );
    Ok(snapshot_result(
        context,
        images,
        SnapshotResultMeta {
            mode: "structured_snapshot",
            entries: &entries,
            compressed: true,
            truncated,
            note,
            boundary,
            source_context_chars: full_chars,
            source_estimated_tokens: full_tokens as i64,
        },
    ))
}

fn snapshot_result(
    context: String,
    images: Vec<ImageData>,
    meta: SnapshotResultMeta<'_>,
) -> BranchInheritanceSnapshot {
    BranchInheritanceSnapshot {
        estimated_tokens: estimate_tokens(&context) as i64,
        source_context_chars: meta.source_context_chars,
        source_estimated_tokens: meta.source_estimated_tokens,
        context_chars: context.chars().count() as i64,
        context,
        images,
        inheritance_mode: meta.mode.to_string(),
        inherited_message_count: meta.entries.len() as i32,
        compressed: meta.compressed,
        truncated: meta.truncated,
        note: meta.note,
        fork_message_id: meta.boundary.map(|turn| turn.id.clone()),
        forked_through_at: meta.boundary.map(|turn| turn.timestamp),
        snapshot_version: 2,
    }
}

fn inherited_images(turns: &[MessageTurn]) -> (Vec<ImageData>, bool) {
    let mut images = Vec::new();
    let mut total_chars = 0_usize;
    let mut omitted = false;
    for turn in turns
        .iter()
        .filter(|turn| matches!(turn.role, TurnRole::User))
    {
        for block in &turn.blocks {
            let ContentBlock::Image {
                data,
                mime_type,
                uri,
            } = block
            else {
                continue;
            };
            if data.is_empty()
                || images.len() >= MAX_INHERITED_IMAGES
                || total_chars.saturating_add(data.len()) > MAX_INHERITED_IMAGE_BASE64_CHARS
            {
                omitted = true;
                continue;
            }
            total_chars = total_chars.saturating_add(data.len());
            images.push(ImageData {
                data: data.clone(),
                mime_type: mime_type.clone(),
                uri: uri.clone(),
            });
        }
    }
    (images, omitted)
}

fn visible_entries(turns: &[MessageTurn]) -> Vec<VisibleEntry> {
    let mut entries = Vec::new();
    let mut citation_sources = Vec::new();
    for turn in turns
        .iter()
        .filter(|turn| matches!(turn.role, TurnRole::User | TurnRole::Assistant))
    {
        if matches!(turn.role, TurnRole::User) {
            citation_sources.clear();
        }
        let mut parts = Vec::new();
        for block in &turn.blocks {
            match block {
                ContentBlock::Text { text } if !text.trim().is_empty() => {
                    parts.push(crate::citations::render_plain_text_citations(
                        text.trim(),
                        &citation_sources,
                    ));
                }
                ContentBlock::ToolUse { meta, .. } => {
                    citation_sources.extend(crate::citations::sources_from_meta(meta.as_ref()));
                    citation_sources =
                        crate::citations::merge_sources(citation_sources.iter());
                }
                ContentBlock::Image { mime_type, uri, .. } => parts.push(format!(
                    "[Attached image: {}]",
                    uri.as_deref().unwrap_or(mime_type)
                )),
                ContentBlock::ImageGeneration {
                    image: Some(image), ..
                } => parts.push(format!(
                    "[Generated image: {}]",
                    image.uri.as_deref().unwrap_or(&image.mime_type)
                )),
                // Deliberately exclude hidden reasoning and raw tool traffic.
                ContentBlock::Thinking { .. }
                | ContentBlock::ToolResult { .. }
                | ContentBlock::ImageGeneration { image: None, .. } => {}
                ContentBlock::Text { .. } => {}
            }
        }
        if !parts.is_empty() {
            entries.push(VisibleEntry {
                id: turn.id.clone(),
                role: turn.role.clone(),
                timestamp: turn.timestamp,
                text: parts.join("\n"),
            });
        }
    }
    entries
}

fn visible_deliverable_paths(
    detail: &DbConversationDetail,
    boundary: Option<DateTime<Utc>>,
) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for run in &detail.deliverable_runs {
        if boundary.is_some_and(|boundary| run.started_at > boundary) {
            continue;
        }
        for item in &run.deliverables {
            if item.is_valid {
                paths.insert(format!("{} ({})", item.path, item.role));
            }
        }
    }
    paths.into_iter().collect()
}

fn render_snapshot(
    detail: &DbConversationDetail,
    entries: &[VisibleEntry],
    deliverables: &[String],
    compressed: bool,
    truncated: bool,
    note: Option<&str>,
) -> String {
    let mut output = String::new();
    output.push_str("Codeg conversation branch initialization context (private, read-only)\n");
    output.push_str(&format!(
        "Source conversation: {} (#{}); Agent: {}; Model: {}; Working directory: {}\n",
        detail.summary.title.as_deref().unwrap_or("Untitled"),
        detail.summary.id,
        detail.summary.agent_type,
        detail.summary.model.as_deref().unwrap_or("unknown"),
        detail
            .summary
            .origin_cwd
            .as_deref()
            .unwrap_or("project folder")
    ));
    output.push_str("Interpret later user corrections as superseding earlier requirements. Do not expose this initialization block as a user message.\n");
    output.push_str(&format!(
        "Inheritance: {}; truncated: {}.\n",
        if compressed {
            "structured snapshot"
        } else {
            "full user-visible replay"
        },
        truncated
    ));
    if let Some(note) = note {
        output.push_str("Compression note: ");
        output.push_str(note);
        output.push('\n');
    }
    for entry in entries {
        output.push_str("\n---\n");
        output.push_str(match entry.role {
            TurnRole::User => "User",
            TurnRole::Assistant => "Assistant",
            TurnRole::System => continue,
        });
        output.push_str(&format!(
            " [{} at {}]:\n",
            entry.id,
            entry.timestamp.to_rfc3339()
        ));
        output.push_str(&entry.text);
        output.push('\n');
    }
    if !deliverables.is_empty() {
        output.push_str("\nKey deliverables/files referenced before the fork:\n");
        for path in deliverables {
            output.push_str("- ");
            output.push_str(path);
            output.push('\n');
        }
    }
    output
}

fn priority_excerpt(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let half = max_chars / 2;
    let start = text.chars().take(half).collect::<String>();
    let end = text
        .chars()
        .rev()
        .take(half)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{start}\n[… earlier content compressed …]\n{end}")
}

pub fn estimate_tokens(text: &str) -> u64 {
    let mut ascii_chars = 0_u64;
    let mut non_ascii_chars = 0_u64;
    for ch in text.chars() {
        if ch.is_ascii() {
            ascii_chars += 1;
        } else {
            non_ascii_chars += 1;
        }
    }
    ascii_chars.div_ceil(4).saturating_add(non_ascii_chars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::conversation::ConversationKind;
    use crate::models::{AgentType, DbConversationSummary, SessionStats};

    fn turn(id: &str, role: TurnRole, text: &str, seconds: i64) -> MessageTurn {
        MessageTurn {
            id: id.into(),
            role,
            blocks: vec![ContentBlock::Text { text: text.into() }],
            timestamp: DateTime::from_timestamp(seconds, 0).unwrap(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: DateTime::from_timestamp(seconds + 1, 0),
        }
    }

    fn detail(turns: Vec<MessageTurn>) -> DbConversationDetail {
        DbConversationDetail {
            summary: DbConversationSummary {
                id: 7,
                folder_id: 1,
                title: Some("Long source".into()),
                title_locked: true,
                agent_type: AgentType::Codex,
                status: "completed".into(),
                kind: ConversationKind::Regular,
                model: Some("gpt-5.5".into()),
                git_branch: None,
                external_id: Some("source-session".into()),
                message_count: turns.len() as u32,
                child_count: 0,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                pinned_at: None,
                parent_id: None,
                parent_tool_use_id: None,
                delegation_call_id: None,
                origin_cwd: Some("/workspace/project".into()),
            },
            turns,
            session_stats: Some(SessionStats {
                total_usage: None,
                total_tokens: None,
                total_duration_ms: 0,
                context_window_used_tokens: None,
                context_window_max_tokens: Some(128_000),
                context_window_usage_percent: None,
            }),
            transcript_watermark: None,
            in_flight_user_turn_id: None,
            artifact_runs: vec![],
            deliverables: vec![],
            deliverable_runs: vec![],
            history_page: None,
            turns_offset: None,
            turns_total: None,
            assistant_turns_before_offset: None,
            prefix_hash: None,
            uncovered_prefix_max_ts: None,
            branch_history: None,
        }
    }

    #[test]
    fn long_history_keeps_early_unique_requirement_and_late_correction() {
        let mut turns = vec![turn(
            "early",
            TurnRole::User,
            "公众号长图唯一口径：EARLY-WECHAT-RULE-731",
            1,
        )];
        for index in 0..80 {
            turns.push(turn(
                &format!("assistant-{index}"),
                TurnRole::Assistant,
                &"verbose result ".repeat(800),
                (index * 2 + 2) as i64,
            ));
        }
        turns.push(turn(
            "correction",
            TurnRole::User,
            "最终纠正：颜色必须改为蓝色，不再使用红色。",
            200,
        ));
        let snapshot = build_branch_inheritance_snapshot(&detail(turns), None, Some(12_000))
            .expect("snapshot");
        assert_eq!(snapshot.inheritance_mode, "structured_snapshot");
        assert!(snapshot.context.contains("EARLY-WECHAT-RULE-731"));
        assert!(snapshot.context.contains("最终纠正：颜色必须改为蓝色"));
        assert!(snapshot.compressed);
    }

    #[test]
    fn exact_message_boundary_excludes_future_and_hidden_reasoning() {
        let mut hidden = turn("assistant", TurnRole::Assistant, "visible conclusion", 2);
        hidden.blocks.push(ContentBlock::Thinking {
            text: "SECRET_CHAIN_OF_THOUGHT".into(),
        });
        hidden.blocks.push(ContentBlock::ToolResult {
            tool_use_id: None,
            output_preview: Some("UNRELATED_DEBUG_LOG".into()),
            is_error: false,
            agent_stats: None,
            images: vec![],
        });
        let turns = vec![
            turn("before", TurnRole::User, "known before fork", 1),
            hidden,
            turn("after", TurnRole::User, "FUTURE_SECRET", 3),
        ];
        let snapshot =
            build_branch_inheritance_snapshot(&detail(turns), Some("assistant"), Some(100_000))
                .expect("snapshot");
        assert_eq!(snapshot.inheritance_mode, "full_replay");
        assert!(snapshot.context.contains("known before fork"));
        assert!(snapshot.context.contains("visible conclusion"));
        assert!(!snapshot.context.contains("FUTURE_SECRET"));
        assert!(!snapshot.context.contains("SECRET_CHAIN_OF_THOUGHT"));
        assert!(!snapshot.context.contains("UNRELATED_DEBUG_LOG"));
        assert_eq!(snapshot.fork_message_id.as_deref(), Some("assistant"));
    }

    #[test]
    fn missing_exact_boundary_fails_instead_of_leaking_latest_history() {
        let error = build_branch_inheritance_snapshot(
            &detail(vec![turn("one", TurnRole::User, "one", 1)]),
            Some("missing"),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("not present"));
    }

    #[test]
    fn near_window_limit_keeps_early_and_late_user_requirements() {
        let mut turns = Vec::new();
        for index in 0..40 {
            let marker = if index == 0 {
                "EARLIEST-REQUIREMENT"
            } else if index == 39 {
                "LATEST-CORRECTION-WINS"
            } else {
                "ordinary requirement"
            };
            turns.push(turn(
                &format!("user-{index}"),
                TurnRole::User,
                &format!("{marker} {}", "detail ".repeat(1_000)),
                index as i64 * 2 + 1,
            ));
            turns.push(turn(
                &format!("assistant-{index}"),
                TurnRole::Assistant,
                &"assistant explanation ".repeat(500),
                index as i64 * 2 + 2,
            ));
        }
        let snapshot =
            build_branch_inheritance_snapshot(&detail(turns), None, Some(8_000)).unwrap();
        assert!(snapshot.context.contains("EARLIEST-REQUIREMENT"));
        assert!(snapshot.context.contains("LATEST-CORRECTION-WINS"));
        assert!(snapshot.truncated);
    }

    #[test]
    fn carries_user_images_but_not_tool_or_assistant_internal_images() {
        let mut user = turn("user", TurnRole::User, "inspect this image", 1);
        user.blocks.push(ContentBlock::Image {
            data: "aW1hZ2U=".into(),
            mime_type: "image/png".into(),
            uri: Some("file:///workspace/reference.png".into()),
        });
        let snapshot =
            build_branch_inheritance_snapshot(&detail(vec![user]), None, Some(100_000)).unwrap();
        assert_eq!(snapshot.images.len(), 1);
        assert_eq!(snapshot.images[0].data, "aW1hZ2U=");
        assert!(snapshot.context.contains("reference.png"));
    }

    #[test]
    fn branch_snapshot_keeps_resolved_citation_urls_without_tool_noise() {
        let source = crate::citations::CitationSource {
            reference_id: "turn0search0".into(),
            url: "https://example.com/source".into(),
            title: "Example source".into(),
            site_name: "example.com".into(),
            source_type: "web_search".into(),
            call_id: Some("ws-1".into()),
            message_id: None,
            start_index: None,
            end_index: None,
            snippet: None,
        };
        let mut search = turn("search", TurnRole::Assistant, "", 2);
        search.blocks = vec![ContentBlock::ToolUse {
            tool_use_id: Some("ws-1".into()),
            tool_name: "web_search".into(),
            input_preview: None,
            status: Some("completed".into()),
            meta: Some(serde_json::json!({ "codeg.citations": [source] })),
        }];
        let answer = turn(
            "answer",
            TurnRole::Assistant,
            "Result \u{e200}cite\u{e202}turn0search0\u{e201}",
            3,
        );
        let snapshot = build_branch_inheritance_snapshot(
            &detail(vec![
                turn("user", TurnRole::User, "research", 1),
                search,
                answer,
            ]),
            None,
            Some(100_000),
        )
        .unwrap();
        assert!(snapshot.context.contains("https://example.com/source"));
        assert!(!snapshot.context.contains("turn0search0"));
    }
}
