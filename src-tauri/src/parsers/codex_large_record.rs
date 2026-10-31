//! A single native record may exceed a normal page. Validate it without
//! allocating its strings, then build an explicitly partial bounded preview.
//! The original file and byte range remain the authority, never this preview.
use super::ParseError;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const CHUNK_BYTES: usize = 32 * 1024;
const STRING_BYTES: usize = 32 * 1024;
const PREVIEW_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct LargeRecord {
    pub sha256: String,
    pub preview: Option<Vec<u8>>,
    pub bytes: u64,
    pub max_read_bytes: usize,
}

pub(crate) struct CheckedReader<'a> {
    source: std::io::Take<File>,
    hash: Sha256,
    cancelled: Option<&'a AtomicBool>,
    max_read_bytes: usize,
    utf8_tail: Vec<u8>,
}

fn pending_utf8(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    match std::str::from_utf8(bytes) {
        Ok(_) => Ok(Vec::new()),
        Err(error) if error.error_len().is_none() => Ok(bytes[error.valid_up_to()..].to_vec()),
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Native history contains invalid UTF-8",
        )),
    }
}

impl Read for CheckedReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self
            .cancelled
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            return Err(std::io::Error::other("History read cancelled"));
        }
        let cap = buf.len().min(CHUNK_BYTES);
        let read = self.source.read(&mut buf[..cap])?;
        // IgnoredAny validates JSON syntax but skips UTF-8 validation inside
        // ignored strings. Check every byte, including beyond the preview,
        // retaining only an unfinished codepoint (at most three bytes).
        if !self.utf8_tail.is_empty() {
            if read == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Native history ends with incomplete UTF-8",
                ));
            }
            let mut joined = std::mem::take(&mut self.utf8_tail);
            joined.extend_from_slice(&buf[..read]);
            self.utf8_tail = pending_utf8(&joined)?;
        } else {
            self.utf8_tail = pending_utf8(&buf[..read])?;
        }
        self.max_read_bytes = self.max_read_bytes.max(read);
        self.hash.update(&buf[..read]);
        Ok(read)
    }
}

/// One-pass reader for ordinary pages. Both allocation and physical reads
/// remain bounded by the selected range, with cancellation checked per chunk.
pub(crate) fn page_reader<'a>(
    path: &Path,
    start: u64,
    end: u64,
    cancelled: Option<&'a AtomicBool>,
) -> Result<CheckedReader<'a>, ParseError> {
    source(path, start, end, cancelled)
}

fn source<'a>(
    path: &Path,
    start: u64,
    end: u64,
    cancelled: Option<&'a AtomicBool>,
) -> Result<CheckedReader<'a>, ParseError> {
    let mut file = File::open(path)?;
    if end < start || end > file.metadata()?.len() {
        return Err(ParseError::InvalidData(
            "HISTORY_SOURCE_CHANGED: native record range is unavailable; retry the history read"
                .into(),
        ));
    }
    file.seek(SeekFrom::Start(start))?;
    Ok(CheckedReader {
        source: file.take(end - start),
        hash: Sha256::new(),
        cancelled,
        max_read_bytes: 0,
        utf8_tail: Vec::new(),
    })
}

fn emit_string(out: &mut Vec<u8>, raw: &[u8], clipped: bool) -> Result<(), ParseError> {
    let mut literal = Vec::with_capacity(raw.len() + 2);
    literal.push(b'"');
    literal.extend_from_slice(raw);
    literal.push(b'"');
    if !clipped {
        out.extend_from_slice(&literal);
        return Ok(());
    }
    // A cut may bisect UTF-8, an escape, or a surrogate pair. Only retain a
    // prefix that serde proves is a complete JSON string.
    for _ in 0..32 {
        if let Ok(mut text) = serde_json::from_slice::<String>(&literal) {
            if text.starts_with("data:") {
                // An incomplete base64 image is not a valid preview.
                text.clear();
            } else {
                text.push_str("\n[Bounded preview; the complete native record is retained at the source reference.]");
            }
            out.extend_from_slice(&serde_json::to_vec(&text)?);
            return Ok(());
        }
        if literal.len() <= 2 {
            break;
        }
        literal.remove(literal.len() - 2);
    }
    Err(ParseError::InvalidData(
        "HISTORY_PARSE_FAILED: invalid string in large native record".into(),
    ))
}

pub(crate) fn read(
    path: &Path,
    start: u64,
    end: u64,
    cancelled: Option<&AtomicBool>,
) -> Result<LargeRecord, ParseError> {
    // IgnoredAny consumes strings/arrays without retaining them. A malformed
    // record must fail import, rather than becoming an apparently valid gap.
    let mut validated = BufReader::with_capacity(CHUNK_BYTES, source(path, start, end, cancelled)?);
    let mut json = serde_json::Deserializer::from_reader(&mut validated);
    serde::de::IgnoredAny::deserialize(&mut json)
        .and_then(|_| json.end())
        .map_err(|e| {
            ParseError::InvalidData(format!(
                "HISTORY_PARSE_FAILED: native record at bytes {start}..{end}: {e}"
            ))
        })?;
    let validated = validated.into_inner();
    let digest = format!("{:x}", validated.hash.finalize());

    let mut reader = BufReader::with_capacity(CHUNK_BYTES, source(path, start, end, cancelled)?);
    let mut out = Vec::new();
    let mut raw = Vec::new();
    let (mut string, mut escaped, mut clipped, mut limit) = (false, false, false, false);
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            break;
        }
        let consumed = chunk.len();
        if !limit {
            for &byte in chunk {
                if string {
                    if byte == b'"' && !escaped {
                        emit_string(&mut out, &raw, clipped)?;
                        raw.clear();
                        string = false;
                    } else {
                        if raw.len() < STRING_BYTES {
                            raw.push(byte);
                        } else {
                            clipped = true;
                        }
                        escaped = byte == b'\\' && !escaped;
                        if byte != b'\\' {
                            escaped = false;
                        }
                    }
                } else if byte == b'"' {
                    string = true;
                    escaped = false;
                    clipped = false;
                } else {
                    out.push(byte);
                }
                if out.len() > PREVIEW_BYTES {
                    limit = true;
                    out.clear();
                    raw.clear();
                    break;
                }
            }
        }
        reader.consume(consumed);
    }
    let reader = reader.into_inner();
    if format!("{:x}", reader.hash.finalize()) != digest {
        return Err(ParseError::InvalidData("HISTORY_SOURCE_CHANGED: native record changed while reading; retry without changing the original file".into()));
    }
    if !limit {
        serde_json::from_slice::<serde::de::IgnoredAny>(&out)?;
    }
    Ok(LargeRecord {
        sha256: digest,
        preview: (!limit).then_some(out),
        bytes: end - start,
        max_read_bytes: reader.max_read_bytes.max(validated.max_read_bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_preview_respects_utf8_escapes_images_and_original_hash() {
        for body in [
            "中文😀\\\"".repeat(10_000),
            "data:image/png;base64,".to_owned() + &"a".repeat(80_000),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("native.jsonl");
            let source = serde_json::to_vec(
                &serde_json::json!({"type":"response_item","payload":{"text":body}}),
            )
            .unwrap();
            std::fs::write(&path, &source).unwrap();
            let record = read(&path, 0, source.len() as u64, None).unwrap();
            let preview: serde_json::Value =
                serde_json::from_slice(record.preview.as_ref().unwrap()).unwrap();
            assert!(record.preview.as_ref().unwrap().len() < STRING_BYTES + 1024);
            assert_eq!(record.sha256, format!("{:x}", Sha256::digest(&source)));
            if body.starts_with("data:") {
                assert_eq!(preview["payload"]["text"], "");
            } else {
                assert!(preview["payload"]["text"]
                    .as_str()
                    .unwrap()
                    .contains("Bounded preview"));
            }
            assert_eq!(std::fs::read(&path).unwrap(), source);
        }
    }

    #[test]
    fn malformed_or_cancelled_stream_cannot_look_like_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("native.jsonl");
        std::fs::write(&path, b"{\"text\":\"invalid\\q\"}\n").unwrap();
        let end = std::fs::metadata(&path).unwrap().len();
        assert!(read(&path, 0, end, None)
            .err()
            .unwrap()
            .to_string()
            .contains("HISTORY_PARSE_FAILED"));
        let cancelled = AtomicBool::new(true);
        assert!(read(&path, 0, end, Some(&cancelled))
            .err()
            .unwrap()
            .to_string()
            .contains("cancelled"));
        assert!(read(&path, 0, end + 1, None)
            .err()
            .unwrap()
            .to_string()
            .contains("HISTORY_SOURCE_CHANGED"));
    }

    #[test]
    fn invalid_utf8_beyond_the_preview_cannot_pass_stream_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("native.jsonl");
        let mut bytes = b"{\"text\":\"".to_vec();
        bytes.extend(vec![b'x'; STRING_BYTES + 100]);
        bytes.push(255);
        bytes.extend(b"\"}\n");
        std::fs::write(&path, &bytes).unwrap();
        let error = read(&path, 0, bytes.len() as u64, None).err().unwrap();
        assert!(error.to_string().contains("HISTORY_PARSE_FAILED"));
        assert!(error.to_string().contains("UTF-8"));
        // A real codepoint may cross a physical read boundary.
        bytes = b"{\"text\":\"".to_vec();
        bytes.extend(vec![b'x'; CHUNK_BYTES - bytes.len() - 1]);
        bytes.extend("😀".as_bytes());
        bytes.extend(b"\"}\n");
        std::fs::write(&path, &bytes).unwrap();
        assert!(read(&path, 0, bytes.len() as u64, None).is_ok());
    }

    #[test]
    fn oversized_structural_preview_is_not_fabricated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("native.jsonl");
        let source = serde_json::to_vec(&vec!["x"; PREVIEW_BYTES / 3]).unwrap();
        std::fs::write(&path, &source).unwrap();
        let record = read(&path, 0, source.len() as u64, None).unwrap();
        assert!(
            record.preview.is_none(),
            "retain the whole original reference instead of a malformed JSON excerpt"
        );
    }
}
