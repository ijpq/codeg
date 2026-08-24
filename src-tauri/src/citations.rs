use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

pub const CITATION_META_KEY: &str = "codeg.citations";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationSource {
    pub reference_id: String,
    pub url: String,
    pub title: String,
    pub site_name: String,
}

fn safe_http_url(raw: &str) -> Option<reqwest::Url> {
    let parsed = reqwest::Url::parse(raw).ok()?;
    matches!(parsed.scheme(), "http" | "https").then_some(parsed)
}

pub fn extract_sources_from_web_search_input(raw_input: &str) -> Vec<CitationSource> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw_input) else {
        return Vec::new();
    };
    let Some(results) = value.get("results").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut by_reference = BTreeMap::new();
    for result in results {
        let Some(reference_id) = result
            .get("ref_id")
            .or_else(|| result.get("reference_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(url) = result
            .get("url")
            .and_then(serde_json::Value::as_str)
            .and_then(safe_http_url)
        else {
            continue;
        };
        let url_text = url.as_str().to_string();
        let site_name = url.host_str().unwrap_or_default().to_string();
        let title = result
            .get("title")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                if site_name.is_empty() {
                    url_text.as_str()
                } else {
                    &site_name
                }
            })
            .to_string();
        by_reference.insert(
            reference_id.to_string(),
            CitationSource {
                reference_id: reference_id.to_string(),
                url: url_text,
                title,
                site_name,
            },
        );
    }
    by_reference.into_values().collect()
}

pub fn attach_sources_to_meta(
    meta: Option<serde_json::Value>,
    raw_input: Option<&str>,
) -> Option<serde_json::Value> {
    let sources = raw_input
        .map(extract_sources_from_web_search_input)
        .unwrap_or_default();
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
    object.insert(
        CITATION_META_KEY.to_string(),
        serde_json::to_value(sources).unwrap_or_default(),
    );
    Some(serde_json::Value::Object(object))
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
            output.push_str("［引用来源暂不可解析］");
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
    }

    #[test]
    fn plain_text_resolves_repeated_and_multi_source_markers() {
        let sources = vec![
            CitationSource {
                reference_id: "turn0search0".into(),
                url: "https://a.test/one".into(),
                title: "A".into(),
                site_name: "a.test".into(),
            },
            CitationSource {
                reference_id: "turn0search1".into(),
                url: "https://b.test/two".into(),
                title: "B".into(),
                site_name: "b.test".into(),
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
            "old ［引用来源暂不可解析］"
        );
    }
}
