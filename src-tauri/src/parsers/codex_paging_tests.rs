//! Structural regressions based on the reported byte spans, not copies of
//! private user transcripts. Every fixture keeps its original JSONL untouched.
use super::*;
use std::io::{BufWriter, Write};

fn line(writer: &mut impl Write, kind: &str, payload: serde_json::Value) {
    writeln!(
        writer,
        "{}",
        serde_json::json!({
            "timestamp":"2026-10-29T00:00:00Z", "type":kind, "payload":payload
        })
    )
    .unwrap();
}

fn round(writer: &mut impl Write, index: usize, padding: usize) {
    line(
        writer,
        "event_msg",
        serde_json::json!({"type":"task_started","turn_id":format!("task-{index}")}),
    );
    line(
        writer,
        "turn_context",
        serde_json::json!({"turn_id":format!("task-{index}"),"model":"gpt-6-astra"}),
    );
    line(
        writer,
        "event_msg",
        serde_json::json!({"type":"user_message","message":format!("requirement-{index}")}),
    );
    line(
        writer,
        "response_item",
        serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":format!("requirement-{index}")}]}),
    );
    if padding > 0 {
        line(
            writer,
            "response_item",
            serde_json::json!({"type":"function_call","name":"exec_command","call_id":format!("call-{index}"),"arguments":"{}"}),
        );
        write!(writer, "{{\"timestamp\":\"2026-10-29T00:00:00Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"function_call_output\",\"call_id\":\"call-{index}\",\"output\":\"").unwrap();
        let chunk = vec![b'x'; 32 * 1024];
        let mut remaining = padding;
        while remaining > 0 {
            let take = remaining.min(chunk.len());
            writer.write_all(&chunk[..take]).unwrap();
            remaining -= take;
        }
        writer.write_all(b"\"}}\n").unwrap();
    }
    line(
        writer,
        "event_msg",
        serde_json::json!({"type":"agent_message","message":format!("conclusion-{index}")}),
    );
    line(
        writer,
        "response_item",
        serde_json::json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":format!("conclusion-{index}")}]}),
    );
    line(
        writer,
        "event_msg",
        serde_json::json!({"type":"task_complete","turn_id":format!("task-{index}"),"duration_ms":168795+index}),
    );
}

fn index_all(path: &Path) {
    loop {
        let (complete, _, _, _) =
            build_history_index_slice(path, CODEX_HISTORY_FIRST_PAGE_INITIAL_BYTES).unwrap();
        if complete {
            break;
        }
    }
}

fn fixture(
    id: &str,
    rounds: usize,
    heavy_rounds: usize,
    total_heavy_mib: f64,
) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("rollout-{id}.jsonl"));
    let mut writer = BufWriter::new(fs::File::create(&path).unwrap());
    line(
        &mut writer,
        "session_meta",
        serde_json::json!({"id":id,"cwd":"/fixture/business"}),
    );
    let padding = (total_heavy_mib * 1024.0 * 1024.0 / heavy_rounds as f64) as usize;
    for i in 0..rounds {
        round(&mut writer, i, if i < heavy_rounds { padding } else { 0 });
    }
    writer.flush().unwrap();
    index_all(&path);
    (dir, path)
}

fn walk(
    parser: &CodexParser,
    id: &str,
    path: &Path,
    expected_rounds: usize,
) -> Vec<CodexConversationPage> {
    let mut pages = Vec::new();
    let mut end = fs::metadata(path).unwrap().len();
    loop {
        let page = parser
            .get_conversation_page(id, Some(end), 25, Some("/fixture/business".into()))
            .unwrap();
        assert!(
            page.page_complete,
            "indexed ready history must progress: {:?} {}..{}",
            page.read_issue, page.start_offset, page.end_offset
        );
        assert_eq!(page.end_offset, end);
        assert!(page.start_offset < end, "every nonempty page must advance");
        assert!(page.index_hit);
        assert_eq!(page.scan_bytes, 0);
        if page.read_mode != "streamed_record" {
            assert!(page.end_offset - page.start_offset <= CODEX_HISTORY_FIRST_PAGE_MAX_BYTES);
        }
        end = page.start_offset;
        pages.push(page);
        if end == 0 {
            break;
        }
        assert!(pages.len() < expected_rounds + 10);
    }
    let users: Vec<_> = pages
        .iter()
        .rev()
        .flat_map(|p| &p.detail.turns)
        .filter(|t| matches!(t.role, TurnRole::User))
        .flat_map(|t| &t.blocks)
        .filter_map(|b| {
            if let ContentBlock::Text { text } = b {
                Some(text.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        users,
        (0..expected_rounds)
            .map(|i| format!("requirement-{i}"))
            .collect::<Vec<_>>(),
        "no missing or duplicated native/canonical user twins across byte pages"
    );
    let conclusions: Vec<_> = pages
        .iter()
        .rev()
        .flat_map(|p| &p.detail.turns)
        .flat_map(|t| &t.blocks)
        .filter_map(|b| {
            if let ContentBlock::Text { text } = b {
                text.starts_with("conclusion-").then_some(text.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        conclusions,
        (0..expected_rounds)
            .map(|i| format!("conclusion-{i}"))
            .collect::<Vec<_>>()
    );
    pages
}

#[test]
fn long_history_ready_180_mib_first_page_is_byte_bounded_and_lossless() {
    let id = "2k-structural-regression";
    let (dir, path) = fixture(id, 25, 25, 180.22);
    let parser = CodexParser::with_base_dir(dir.path().to_path_buf());
    let pages = walk(&parser, id, &path, 25);
    assert!(pages.len() > 2, "25 rounds cannot fit one bounded page");
    assert!(pages.iter().any(|p| p.read_mode == "byte_budget"));
    let mut outputs = Vec::new();
    for page in pages {
        for block in page.detail.turns.iter().flat_map(|t| &t.blocks) {
            if let ContentBlock::ToolResult {
                tool_use_id: Some(id),
                ..
            } = block
            {
                outputs.push(id.clone());
            }
        }
        for turn in page
            .detail
            .turns
            .iter()
            .filter(|t| matches!(t.role, TurnRole::Assistant))
        {
            if let Some(id) = &turn.native_task_id {
                let index: u64 = id.strip_prefix("task-").unwrap().parse().unwrap();
                if turn.blocks.iter().any(
                    |b| matches!(b,ContentBlock::Text{text} if text.starts_with("conclusion-")),
                ) {
                    assert_eq!(turn.duration_ms, Some(168795 + index));
                }
            }
        }
    }
    outputs.sort();
    let mut expected = (0..25).map(|i| format!("call-{i}")).collect::<Vec<_>>();
    expected.sort();
    assert_eq!(
        outputs, expected,
        "native tool results are neither lost nor duplicated"
    );
    tests::remove_history_index_fixture(&path);
}

#[test]
fn long_history_ready_thirteenth_page_64_66_mib_keeps_advancing() {
    let id = "agent-debug-structural-regression";
    let (dir, path) = fixture(id, 325, 25, 64.66);
    let parser = CodexParser::with_base_dir(dir.path().to_path_buf());
    let pages = walk(&parser, id, &path, 325);
    assert!(pages.len() > 13);
    assert_eq!(pages[12].read_mode, "byte_budget");
    tests::remove_history_index_fixture(&path);
}

#[test]
fn long_history_single_tool_record_over_ceiling_has_an_explicit_source_reference() {
    let id = "single-record-structural-regression";
    let (dir, path) = fixture(id, 1, 1, 65.0);
    let parser = CodexParser::with_base_dir(dir.path().to_path_buf());
    let before_len = fs::metadata(&path).unwrap().len();
    let pages = walk(&parser, id, &path, 1);
    let page = pages
        .iter()
        .find(|p| p.read_mode == "streamed_record")
        .expect("dedicated single-record stream");
    let refs: Vec<_> = page
        .detail
        .turns
        .iter()
        .flat_map(|t| &t.blocks)
        .filter_map(|b| match b {
            ContentBlock::ToolUse {
                meta: Some(meta), ..
            } => meta.get("codeg.nativeRecord"),
            _ => None,
        })
        .collect();
    assert_eq!(refs.len(), 1);
    assert_eq!(
        refs[0]["byteCount"].as_u64(),
        Some(page.end_offset - page.start_offset)
    );
    assert_eq!(refs[0]["maxReadBytes"].as_u64(), Some(32 * 1024));
    assert_eq!(refs[0]["sha256"].as_str().unwrap().len(), 64);
    assert!(serde_json::to_vec(&page.detail).unwrap().len() < 128 * 1024);
    assert_eq!(fs::metadata(&path).unwrap().len(), before_len);
    let completed = pages
        .iter()
        .flat_map(|p| &p.detail.turns)
        .find(|t| t.duration_ms.is_some())
        .unwrap();
    assert_eq!(completed.native_task_id.as_deref(), Some("task-0"));
    assert_eq!(
        completed.duration_ms,
        Some(168795),
        "whole native duration belongs to the terminal fragment, not the single tool"
    );
    tests::remove_history_index_fixture(&path);
}

#[test]
fn long_history_committed_parse_errors_are_not_misreported_as_index_building() {
    let dir = tempfile::tempdir().unwrap();
    let id = "invalid-native-record";
    let path = dir.path().join(format!("rollout-{id}.jsonl"));
    fs::write(&path, b"{\"type\":\"event_msg\",INVALID}\n").unwrap();
    index_all(&path);
    let parser = CodexParser::with_base_dir(dir.path().to_path_buf());
    let error = parser
        .get_conversation_page(id, None, 25, None)
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("HISTORY_PARSE_FAILED"),
        "{error}"
    );
    tests::remove_history_index_fixture(&path);
}

#[test]
fn long_history_pending_last_record_and_invalid_cursor_are_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let id = "pending-native-record";
    let path = dir.path().join(format!("rollout-{id}.jsonl"));
    fs::write(
        &path,
        b"{\"type\":\"session_meta\",\"payload\":{}}\n{\"type\":",
    )
    .unwrap();
    let (ready, _, _, _) = build_history_index_slice(&path, 1024).unwrap();
    assert!(!ready);
    let parser = CodexParser::with_base_dir(dir.path().to_path_buf());
    let error = parser
        .get_conversation_page(id, None, 25, None)
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("HISTORY_RECORD_INCOMPLETE"),
        "{error}"
    );
    let error = parser
        .get_conversation_page(id, Some(12), 25, None)
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("HISTORY_CURSOR_INVALID"),
        "{error}"
    );
    tests::remove_history_index_fixture(&path);
}

#[test]
fn long_history_index_tracks_every_record_and_rejects_a_gap_after_restart() {
    let (dir, path) = fixture("index-continuity", 2, 1, 0.001);
    let metadata = fs::metadata(&path).unwrap();
    let indexes = HISTORY_INDEX_CACHE.lock().unwrap();
    let index = indexes.get(&path).unwrap();
    let mut end = 0;
    for entry in &index.entries {
        assert_eq!(entry.start_offset, end);
        end = entry.end_offset;
    }
    assert_eq!(end, metadata.len());
    assert!(index
        .entries
        .iter()
        .any(|e| e.kind == CodexHistoryEntryKind::Other));
    drop(indexes);
    let entries_path = history_index_entries_path(&path).unwrap();
    let mut bytes = fs::read(&entries_path).unwrap();
    bytes[CODEX_HISTORY_INDEX_ENTRY_BYTES..CODEX_HISTORY_INDEX_ENTRY_BYTES + 8]
        .copy_from_slice(&1u64.to_le_bytes());
    fs::write(entries_path, bytes).unwrap();
    HISTORY_INDEX_CACHE.lock().unwrap().remove(&path);
    let index = load_history_index(&path, &metadata);
    assert_eq!(index.load_status, "corrupt");
    assert!(index.entries.is_empty());
    let _ = dir;
    tests::remove_history_index_fixture(&path);
}

#[test]
fn long_history_task_identity_survives_background_index_cache_ownership() {
    let (_dir, path) = fixture("task-context-race", 2, 1, 0.001);
    let start = HISTORY_INDEX_CACHE
        .lock()
        .unwrap()
        .get(&path)
        .unwrap()
        .entries
        .iter()
        .rev()
        .find(|e| e.kind == CodexHistoryEntryKind::Message)
        .unwrap()
        .start_offset;
    let original = history_task_prefix(&path, start).unwrap();
    assert!(String::from_utf8_lossy(&original).contains("task-1"));
    assert!(!String::from_utf8_lossy(&original).contains("requirement-"));
    HISTORY_INDEX_CACHE.lock().unwrap().remove(&path);
    assert_eq!(history_task_prefix(&path, start).unwrap(), original);
    tests::remove_history_index_fixture(&path);
}

#[test]
fn long_history_task_prefix_rejects_a_corrupt_durable_index() {
    let (_dir, path) = fixture("task-context-corrupt", 2, 1, 0.001);
    let start = fs::metadata(&path).unwrap().len();
    fs::write(history_index_entries_path(&path).unwrap(), b"corrupt").unwrap();
    HISTORY_INDEX_CACHE.lock().unwrap().remove(&path);
    let error = history_task_prefix(&path, start).unwrap_err();
    assert!(error.to_string().contains("HISTORY_INDEX_REBUILDING"));
    tests::remove_history_index_fixture(&path);
}

#[test]
fn long_history_task_prefix_cannot_borrow_a_different_tasks_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-context-identity.jsonl");
    let mut writer = BufWriter::new(fs::File::create(&path).unwrap());
    line(
        &mut writer,
        "event_msg",
        serde_json::json!({"type":"task_started","turn_id":"old"}),
    );
    line(
        &mut writer,
        "turn_context",
        serde_json::json!({"turn_id":"new"}),
    );
    writer.flush().unwrap();
    index_all(&path);
    let prefix = history_task_prefix(&path, fs::metadata(&path).unwrap().len()).unwrap();
    let text = String::from_utf8(prefix).unwrap();
    assert!(text.contains("new"));
    assert!(!text.contains("old"));
    tests::remove_history_index_fixture(&path);
}

#[test]
fn long_history_single_giant_image_keeps_source_without_a_broken_data_uri() {
    let dir = tempfile::tempdir().unwrap();
    let id = "giant-tool-image";
    let path = dir.path().join(format!("rollout-{id}.jsonl"));
    let mut writer = BufWriter::new(fs::File::create(&path).unwrap());
    line(
        &mut writer,
        "session_meta",
        serde_json::json!({"id":id,"cwd":"/fixture/business"}),
    );
    line(
        &mut writer,
        "event_msg",
        serde_json::json!({"type":"task_started","turn_id":"image-task"}),
    );
    line(
        &mut writer,
        "event_msg",
        serde_json::json!({"type":"user_message","message":"inspect giant image"}),
    );
    write!(writer, "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"function_call_output\",\"call_id\":\"image-call\",\"output\":[{{\"type\":\"input_image\",\"image_url\":\"data:image/png;base64,").unwrap();
    let chunk = [b'A'; 32 * 1024];
    for _ in 0..(65 * 1024 * 1024 / chunk.len()) {
        writer.write_all(&chunk).unwrap();
    }
    writer.write_all(b"\"}]}}\n").unwrap();
    line(
        &mut writer,
        "event_msg",
        serde_json::json!({"type":"agent_message","message":"image reviewed"}),
    );
    line(
        &mut writer,
        "event_msg",
        serde_json::json!({"type":"task_complete","turn_id":"image-task","duration_ms":309047}),
    );
    writer.flush().unwrap();
    index_all(&path);
    let parser = CodexParser::with_base_dir(dir.path().to_path_buf());
    let mut end = fs::metadata(&path).unwrap().len();
    let mut streamed = false;
    while end > 0 {
        let page = parser
            .get_conversation_page(id, Some(end), 25, Some("/fixture/business".into()))
            .unwrap();
        assert!(page.page_complete);
        assert!(page.start_offset < end);
        let encoded = serde_json::to_string(&page.detail).unwrap();
        assert!(!encoded.contains("data:image/png;base64,A"));
        if page.read_mode == "streamed_record" {
            streamed = true;
            assert!(encoded.contains("codeg.nativeRecord"));
            assert!(encoded.len() < 128 * 1024);
        }
        end = page.start_offset;
    }
    assert!(streamed);
    tests::remove_history_index_fixture(&path);
}
