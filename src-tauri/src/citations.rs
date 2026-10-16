use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

pub const CITATION_META_KEY: &str = "codeg.citations";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationSource {
    /// Stable Codex citation token (for example `turn0search1`). New payloads
    /// use the product-neutral `citation_id`; the alias keeps citation metadata
    /// written by CodeG 0.28.1-fix1..fix5 readable.
    #[serde(rename = "citation_id", alias = "reference_id")]
    pub reference_id: String,
    pub url: String,
    pub title: String,
    #[serde(rename = "domain", alias = "site_name")]
    pub site_name: String,
    #[serde(default = "default_source_type")]
    pub source_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

fn default_source_type() -> String {
    "web_search".to_string()
}

fn safe_http_url(raw: &str) -> Option<reqwest::Url> {
    let parsed = reqwest::Url::parse(raw).ok()?;
    matches!(parsed.scheme(), "http" | "https").then_some(parsed)
}

fn citation_id(value: &serde_json::Value) -> Option<String> {
    let object = value.as_object()?;
    for key in ["citation_id", "reference_id", "ref_id"] {
        if let Some(id) = object
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            return Some(id.to_string());
        }
    }
    let id = object
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)?;
    is_codex_citation_id(id).then(|| id.to_string())
}

fn is_codex_citation_id(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("turn") else {
        return false;
    };
    let digit_count = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count == 0 {
        return false;
    }
    let suffix = &rest[digit_count..];
    ["search", "view", "open", "fetch"]
        .iter()
        .any(|kind| suffix.strip_prefix(kind).is_some_and(|tail| {
            !tail.is_empty() && tail.bytes().all(|byte| byte.is_ascii_digit())
        }))
}

fn object_text<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

fn nested_text<'a>(value: &'a serde_json::Value, pointer: &str) -> Option<&'a str> {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn source_from_value(
    value: &serde_json::Value,
    inherited_call_id: Option<&str>,
) -> Option<CitationSource> {
    let object = value.as_object()?;
    let reference_id = citation_id(value)?;
    let raw_url = object_text(object, &["url", "uri"])
        .or_else(|| nested_text(value, "/source/url"))
        .or_else(|| nested_text(value, "/action/url"))?;
    let url = safe_http_url(raw_url)?;
    let url_text = url.as_str().to_string();
    let site_name = object_text(object, &["domain", "site_name"])
        .or_else(|| nested_text(value, "/source/domain"))
        .unwrap_or_else(|| url.host_str().unwrap_or_default())
        .to_string();
    let title = object_text(object, &["title", "name"])
        .or_else(|| nested_text(value, "/source/title"))
        .unwrap_or({
            if site_name.is_empty() {
                url_text.as_str()
            } else {
                site_name.as_str()
            }
        })
        .to_string();
    let source_type = object_text(object, &["source_type", "type"])
        .unwrap_or("web_search")
        .to_string();
    let own_call_id = object_text(object, &["call_id", "tool_call_id"])
        .or(inherited_call_id)
        .map(str::to_string);
    let message_id = object_text(object, &["message_id", "content_block_id"])
        .map(str::to_string);
    let start_index = object
        .get("start_index")
        .or_else(|| object.get("startIndex"))
        .and_then(serde_json::Value::as_u64);
    let end_index = object
        .get("end_index")
        .or_else(|| object.get("endIndex"))
        .and_then(serde_json::Value::as_u64);
    let snippet = object_text(object, &["snippet", "text"])
        .map(|value| value.chars().take(2_000).collect::<String>());
    Some(CitationSource {
        reference_id,
        url: url_text,
        title,
        site_name,
        source_type,
        call_id: own_call_id,
        message_id,
        start_index,
        end_index,
        snippet,
    })
}

fn collect_sources(
    value: &serde_json::Value,
    inherited_call_id: Option<&str>,
    depth: usize,
    output: &mut Vec<CitationSource>,
) {
    if depth > 10 {
        return;
    }
    match value {
        serde_json::Value::Array(items) => {
            for item in items.iter().take(1_024) {
                collect_sources(item, inherited_call_id, depth + 1, output);
            }
        }
        serde_json::Value::Object(object) => {
            let local_call_id = object_text(object, &["call_id", "tool_call_id"])
                .or_else(|| {
                    object_text(object, &["id"])
                        .filter(|id| !is_codex_citation_id(id))
                })
                .or(inherited_call_id);
            if let Some(source) = source_from_value(value, local_call_id) {
                output.push(source);
            }
            for (key, child) in object {
                // The app-server currently uses `results`; Responses-style
                // annotations and future adapters commonly use the remaining
                // keys. We recurse only through citation-bearing containers,
                // never arbitrary tool arguments, so an unrelated id + URL
                // cannot be guessed into a source.
                if matches!(
                    key.as_str(),
                    "results"
                        | "sources"
                        | "citations"
                        | "annotations"
                        | "content"
                        | "output"
                        | "raw_output"
                        | "rawOutput"
                        | "item"
                        | "items"
                        | "metadata"
                        | "_meta"
                        | "action"
                ) {
                    collect_sources(child, local_call_id, depth + 1, output);
                }
            }
        }
        _ => {}
    }
}

pub fn extract_sources_from_web_search_input(raw_input: &str) -> Vec<CitationSource> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw_input) else {
        return Vec::new();
    };
    let root_call_id = value
        .as_object()
        .and_then(|object| object_text(object, &["call_id", "tool_call_id", "id"]))
        .filter(|id| !is_codex_citation_id(id));
    let mut sources = Vec::new();
    collect_sources(&value, root_call_id, 0, &mut sources);
    merge_sources(sources.iter())
}

pub fn attach_sources_to_meta(
    meta: Option<serde_json::Value>,
    raw_input: Option<&str>,
) -> Option<serde_json::Value> {
    let sources = raw_input
        .map(extract_sources_from_web_search_input)
        .unwrap_or_default();
    attach_sources_to_meta_value(meta, &sources)
}

/// Add structured sources to an ACP metadata object without discarding
/// citations delivered by an earlier partial tool update. codex-acp emits a
/// web-search start, completion, and (on some providers) a raw Responses item;
/// any of those may carry only part of the final metadata.
pub fn attach_sources_to_meta_value(
    meta: Option<serde_json::Value>,
    sources: &[CitationSource],
) -> Option<serde_json::Value> {
    if sources.is_empty() {
        return meta;
    }
    let mut object = match meta {
        Some(serde_json::Value::Object(object)) => object,
        Some(other) => {
            let mut object = serde_json::Map::new();
            object.insert("upstream".to_string(), other);
            object
        }
        None => serde_json::Map::new(),
    };
    let existing = object
        .get(CITATION_META_KEY)
        .cloned()
        .and_then(|value| serde_json::from_value::<Vec<CitationSource>>(value).ok())
        .unwrap_or_default();
    let merged = merge_sources(existing.iter().chain(sources.iter()));
    object.insert(
        CITATION_META_KEY.to_string(),
        serde_json::to_value(merged).unwrap_or_default(),
    );
    Some(serde_json::Value::Object(object))
}

/// Merge only CodeG citation metadata while retaining the normal ACP
/// last-update-wins behavior for every other metadata field.
pub fn merge_citations_in_meta(
    previous: Option<&serde_json::Value>,
    incoming: &serde_json::Value,
) -> serde_json::Value {
    let previous_sources = sources_from_meta(previous);
    let incoming_sources = sources_from_meta(Some(incoming));
    if previous_sources.is_empty() {
        return incoming.clone();
    }
    let merged = if incoming_sources.is_empty() {
        previous_sources
    } else {
        merge_sources(previous_sources.iter().chain(incoming_sources.iter()))
    };
    attach_sources_to_meta_value(
        Some(incoming.clone()),
        &merged,
    )
    .unwrap_or_else(|| incoming.clone())
}

pub fn sources_from_meta(meta: Option<&serde_json::Value>) -> Vec<CitationSource> {
    meta.and_then(serde_json::Value::as_object)
        .and_then(|object| object.get(CITATION_META_KEY))
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub fn merge_sources<'a>(
    sources: impl IntoIterator<Item = &'a CitationSource>,
) -> Vec<CitationSource> {
    let mut merged = BTreeMap::new();
    for source in sources {
        merged
            .entry(source.reference_id.clone())
            .and_modify(|existing: &mut CitationSource| {
                if existing.title == existing.site_name && source.title != source.site_name {
                    existing.title.clone_from(&source.title);
                }
                if existing.site_name.is_empty() && !source.site_name.is_empty() {
                    existing.site_name.clone_from(&source.site_name);
                }
                if existing.snippet.is_none() {
                    existing.snippet.clone_from(&source.snippet);
                }
                if existing.call_id.is_none() {
                    existing.call_id.clone_from(&source.call_id);
                }
                if existing.message_id.is_none() {
                    existing.message_id.clone_from(&source.message_id);
                }
                if existing.start_index.is_none() {
                    existing.start_index = source.start_index;
                }
                if existing.end_index.is_none() {
                    existing.end_index = source.end_index;
                }
            })
            .or_insert_with(|| source.clone());
    }
    merged.into_values().collect()
}

pub fn reference_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("\u{e200}cite\u{e202}") {
        let marker = &rest[start + "\u{e200}cite\u{e202}".len()..];
        let Some(end) = marker.find('\u{e201}') else {
            break;
        };
        for id in marker[..end]
            .split('\u{e202}')
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            if !ids.iter().any(|existing| existing == id) {
                ids.push(id.to_string());
            }
        }
        rest = &marker[end + '\u{e201}'.len_utf8()..];
    }
    ids
}

/// Convert Codex private-use citation markers into readable plain text.
/// Resolved sources are numbered by URL and appended once; unresolved ids are
/// kept visible as an honest diagnostic instead of leaking the internal token.
pub fn render_plain_text_citations(text: &str, sources: &[CitationSource]) -> String {
    let by_reference: HashMap<&str, &CitationSource> = sources
        .iter()
        .map(|source| (source.reference_id.as_str(), source))
        .collect();
    let mut number_by_url: HashMap<&str, usize> = HashMap::new();
    let mut numbered_sources: Vec<&CitationSource> = Vec::new();
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("\u{e200}cite\u{e202}") {
        output.push_str(&rest[..start]);
        let marker = &rest[start + "\u{e200}cite\u{e202}".len()..];
        let Some(end) = marker.find('\u{e201}') else {
            output.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let ids = marker[..end]
            .split('\u{e202}')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .collect::<Vec<_>>();
        let mut labels = Vec::new();
        let mut unresolved = false;
        for id in ids {
            if let Some(source) = by_reference.get(id).copied() {
                let number = *number_by_url.entry(&source.url).or_insert_with(|| {
                    numbered_sources.push(source);
                    numbered_sources.len()
                });
                if !labels.contains(&number) {
                    labels.push(number);
                }
            } else {
                unresolved = true;
            }
        }
        if unresolved || labels.is_empty() {
            output.push_str("〔来源缺失〕");
        }
        for number in labels {
            output.push_str(&format!("[{number}]"));
        }
        rest = &marker[end + '\u{e201}'.len_utf8()..];
    }
    output.push_str(rest);
    if !numbered_sources.is_empty() {
        output.push_str("\n\n来源：");
        for (index, source) in numbered_sources.iter().enumerate() {
            output.push_str(&format!(
                "\n[{}] {}：{}",
                index + 1,
                source.title,
                source.url
            ));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_safe_structured_web_sources() {
        let sources = extract_sources_from_web_search_input(
            r#"{"results":[{"type":"text_result","ref_id":"turn0search0","url":"https://example.com/a?q=%E4%B8%AD%E6%96%87","title":"Example"},{"ref_id":"bad","url":"javascript:alert(1)","title":"bad"}]}"#,
        );
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].reference_id, "turn0search0");
        assert_eq!(sources[0].site_name, "example.com");
        assert_eq!(sources[0].source_type, "text_result");
    }

    #[test]
    fn extracts_responses_annotations_and_exact_open_page_references() {
        let sources = extract_sources_from_web_search_input(
            r#"{"call_id":"ws-7","annotations":[{"type":"url_citation","citation_id":"turn7view0","url":"https://example.com/%E4%B8%AD%E6%96%87?q=1","title":"中文标题","start_index":8,"end_index":19}],"action":{"type":"openPage","id":"turn7view1","url":"https://docs.example.org/open"}}"#,
        );
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].call_id.as_deref(), Some("ws-7"));
        assert_eq!(sources[0].start_index, Some(8));
        assert_eq!(sources[1].reference_id, "turn7view1");
        assert_eq!(sources[1].url, "https://docs.example.org/open");
    }

    #[test]
    fn never_guesses_a_generic_call_id_as_a_citation() {
        assert!(extract_sources_from_web_search_input(
            r#"{"id":"exec-123","action":{"type":"openPage","url":"https://example.com/private"}}"#
        )
        .is_empty());
    }

    #[test]
    fn structured_schema_serializes_new_names_and_reads_legacy_names() {
        let legacy = serde_json::json!({
            "reference_id": "turn2search0",
            "url": "https://example.com/source",
            "title": "Source",
            "site_name": "example.com"
        });
        let source: CitationSource = serde_json::from_value(legacy).expect("legacy source");
        let encoded = serde_json::to_value(source).expect("serialize source");
        assert_eq!(encoded["citation_id"], "turn2search0");
        assert_eq!(encoded["domain"], "example.com");
        assert!(encoded.get("reference_id").is_none());
        assert!(encoded.get("site_name").is_none());
    }

    #[test]
    fn partial_metadata_updates_keep_all_exact_sources() {
        let first = attach_sources_to_meta(
            None,
            Some(
                r#"{"results":[{"ref_id":"turn0search0","url":"https://a.test","title":"A"}]}"#,
            ),
        )
        .expect("first metadata");
        let second = attach_sources_to_meta(
            None,
            Some(
                r#"{"annotations":[{"citation_id":"turn0view0","url":"https://b.test","title":"B"}]}"#,
            ),
        )
        .expect("second metadata");
        let merged = merge_citations_in_meta(Some(&first), &second);
        let sources = sources_from_meta(Some(&merged));
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].reference_id, "turn0search0");
        assert_eq!(sources[1].reference_id, "turn0view0");
        let later_unrelated = merge_citations_in_meta(
            Some(&merged),
            &serde_json::json!({"provider_status": "completed"}),
        );
        assert_eq!(
            later_unrelated["provider_status"],
            serde_json::json!("completed")
        );
        assert_eq!(sources_from_meta(Some(&later_unrelated)).len(), 2);
    }

    #[test]
    fn plain_text_resolves_repeated_and_multi_source_markers() {
        let sources = vec![
            CitationSource {
                reference_id: "turn0search0".into(),
                url: "https://a.test/one".into(),
                title: "A".into(),
                site_name: "a.test".into(),
                source_type: "web_search".into(),
                call_id: None,
                message_id: None,
                start_index: None,
                end_index: None,
                snippet: None,
            },
            CitationSource {
                reference_id: "turn0search1".into(),
                url: "https://b.test/two".into(),
                title: "B".into(),
                site_name: "b.test".into(),
                source_type: "web_search".into(),
                call_id: None,
                message_id: None,
                start_index: None,
                end_index: None,
                snippet: None,
            },
        ];
        let rendered = render_plain_text_citations(
            "事实\u{e200}cite\u{e202}turn0search0\u{e202}turn0search1\u{e201}，再次\u{e200}cite\u{e202}turn0search0\u{e201}",
            &sources,
        );
        assert_eq!(rendered.matches("https://a.test/one").count(), 1);
        assert!(rendered.contains("事实[1][2]，再次[1]"));
    }

    #[test]
    fn unresolved_marker_is_not_silently_hidden() {
        assert_eq!(
            render_plain_text_citations("old \u{e200}cite\u{e202}turn99view0\u{e201}", &[]),
            "old 〔来源缺失〕"
        );
    }
}
