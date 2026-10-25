//! Files in and out of a call: the upload decoded from base64 straight into
//! the work folder, the media type of a result, and the answer built into one
//! buffer of exact size, with the result file encoded as base64 in chunks.
//! No decoded copy of an upload and no second copy of a result is held.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

use base64::engine::general_purpose::STANDARD;
use base64::read::DecoderReader;
use base64::write::EncoderWriter;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::error::ApiError;
use crate::tools::ToolOutput;

/// Bytes read or written in one step.
const CHUNK: usize = 1024 * 1024;

/// The media type of a file of the service by its extension.
pub fn media_type(extension: &str) -> &'static str {
    match extension {
        "xyz" | "pts" => "text/plain; charset=utf-8",
        "csv" => "text/csv",
        "dxf" => "image/vnd.dxf",
        "dwg" => "image/vnd.dwg",
        "obj" => "model/obj",
        _ => "application/octet-stream",
    }
}

/// The base64 text of `bytes`, with padding and without line breaks.
pub fn encode_base64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// The bytes of a base64 text, as the service reads an upload.
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    STANDARD.decode(text).ok()
}

/// The length of the base64 text of `bytes` bytes.
pub fn base64_len(bytes: u64) -> u64 {
    bytes.div_ceil(3) * 4
}

/// Decode the base64 text `data` into a new file at `destination`, a chunk
/// at a time, asking `keep_going` between chunks. Text that is not base64
/// is an argument error; a file that cannot be written is the service's.
pub fn decode_into(
    data: &str,
    destination: &Path,
    keep_going: pointcloud_core::ops::KeepGoing<'_>,
) -> Result<u64, ApiError> {
    let write_failed =
        |_: io::Error| ApiError::internal("The upload could not be written to the work folder.");
    let mut file = File::create(destination).map_err(write_failed)?;
    let mut decoder = DecoderReader::new(data.as_bytes(), &STANDARD);
    let mut buffer = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let read = decoder.read(&mut buffer).map_err(|error| {
            ApiError::argument(format!(
                "input.data is not base64 (RFC 4648 section 4, with padding, without line breaks): {error}"
            ))
        })?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(write_failed)?;
        total += read as u64;
        keep_going().map_err(|_| ApiError::internal("The work was cancelled."))?;
    }
    file.flush().map_err(write_failed)?;
    Ok(total)
}

/// The length and the SHA-256 of a file, read a chunk at a time.
pub fn digest(path: &Path) -> io::Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        total += read as u64;
    }
    let hex = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok((total, hex))
}

/// Append the base64 text of the file at `path` to `buffer`, a chunk at a
/// time.
fn append_base64(buffer: &mut Vec<u8>, path: &Path) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut encoder = EncoderWriter::new(buffer, &STANDARD);
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        encoder.write_all(&chunk[..read])?;
    }
    encoder.finish()?;
    Ok(())
}

/// The description of a result file without its data, and its length.
struct OutputMeta {
    json: Value,
    bytes: u64,
}

fn output_meta(output: &ToolOutput) -> Result<Option<OutputMeta>, ApiError> {
    let Some(file) = &output.file else {
        return Ok(None);
    };
    let (bytes, sha256) =
        digest(&file.path).map_err(|_| ApiError::internal("The result could not be read back."))?;
    Ok(Some(OutputMeta {
        json: json!({
            "format": file.format,
            "media_type": file.media_type,
            "file_name": file.file_name,
            "bytes": bytes,
            "sha256": sha256,
        }),
        bytes,
    }))
}

/// `{"output": {...}, "report": {...}}` without `output.data`, as MCP
/// gives it as `structuredContent`; `{"report": {...}}` without a file.
pub fn structured(output: &ToolOutput) -> Result<Value, ApiError> {
    Ok(match output_meta(output)? {
        Some(meta) => json!({"output": meta.json, "report": output.report}),
        None => json!({"report": output.report}),
    })
}

/// The JSON text of `value` without its closing brace, so that more keys can
/// follow.
fn open_object(value: &Value) -> Vec<u8> {
    let mut text = serde_json::to_vec(value).unwrap_or_default();
    text.pop();
    text
}

/// Build `prefix`, the base64 of `path` and `suffix` into one buffer of
/// exactly their size.
fn around_file(prefix: &[u8], path: &Path, bytes: u64, suffix: &[u8]) -> Result<Vec<u8>, ApiError> {
    let length = prefix.len() as u64 + base64_len(bytes) + suffix.len() as u64;
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(length as usize)
        .map_err(|_| ApiError::internal("The answer does not fit in memory."))?;
    buffer.extend_from_slice(prefix);
    append_base64(&mut buffer, path)
        .map_err(|_| ApiError::internal("The result could not be read back."))?;
    buffer.extend_from_slice(suffix);
    Ok(buffer)
}

/// The answer of `POST /v1/tools/{name}`: `{"output": {format, media_type,
/// file_name, bytes, sha256, data}, "report": {...}}`, or `{"report": ...}`
/// for a tool without a file.
pub fn rest_answer(output: &ToolOutput) -> Result<Vec<u8>, ApiError> {
    let (Some(meta), Some(file)) = (output_meta(output)?, &output.file) else {
        return serde_json::to_vec(&json!({"report": output.report}))
            .map_err(|_| ApiError::internal("The answer could not be written."));
    };
    let mut prefix = b"{\"output\":".to_vec();
    prefix.extend(open_object(&meta.json));
    prefix.extend_from_slice(b",\"data\":\"");
    let mut suffix = b"\"},\"report\":".to_vec();
    suffix.extend(serde_json::to_vec(&output.report).unwrap_or_default());
    suffix.push(b'}');
    around_file(&prefix, &file.path, meta.bytes, &suffix)
}

/// The JSON-RPC answer of an MCP `tools/call`: the report and the output
/// description as text, and a file as an embedded resource whose blob is
/// its base64; `structuredContent` is the REST answer without
/// `output.data`, when the protocol has it.
pub fn mcp_answer(
    id: &Value,
    output: &ToolOutput,
    structured_content: bool,
) -> Result<Vec<u8>, ApiError> {
    let meta = output_meta(output)?;
    let structured_value = match &meta {
        Some(meta) => json!({"output": meta.json, "report": output.report}),
        None => json!({"report": output.report}),
    };
    let text = json!({"type": "text", "text": structured_value.to_string()});
    let json = |value: &Value| serde_json::to_vec(value).unwrap_or_default();
    let mut prefix = b"{\"jsonrpc\":\"2.0\",\"id\":".to_vec();
    prefix.extend(json(id));
    prefix.extend_from_slice(b",\"result\":{\"content\":[");
    prefix.extend(json(&text));
    let mut suffix = b"],\"isError\":false".to_vec();
    if structured_content {
        suffix.extend_from_slice(b",\"structuredContent\":");
        suffix.extend(json(&structured_value));
    }
    suffix.extend_from_slice(b"}}");
    let (Some(meta), Some(file)) = (meta, &output.file) else {
        prefix.extend(suffix);
        return Ok(prefix);
    };
    // The resource is the last item of the content, its blob its last key.
    prefix.extend_from_slice(b",{\"type\":\"resource\",\"resource\":{\"uri\":");
    prefix.extend(json(&Value::from(format!("result:{}", file.file_name))));
    prefix.extend_from_slice(b",\"mimeType\":");
    prefix.extend(json(&Value::from(file.media_type)));
    prefix.extend_from_slice(b",\"blob\":\"");
    let mut closing = b"\"}}".to_vec();
    closing.extend(suffix);
    around_file(&prefix, &file.path, meta.bytes, &closing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::OutputFile;

    fn output(directory: &Path, bytes: &[u8]) -> ToolOutput {
        let path = directory.join("result.obj");
        std::fs::write(&path, bytes).unwrap();
        ToolOutput {
            report: json!({"vertices": 3}),
            file: Some(OutputFile {
                format: "obj".into(),
                media_type: media_type("obj"),
                file_name: "scan.obj".into(),
                path,
            }),
        }
    }

    #[test]
    fn an_upload_is_decoded_into_its_file_and_bad_base64_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("input.xyz");
        let data: Vec<u8> = (0..=255u8).cycle().take(3 * CHUNK + 7).collect();
        let written = decode_into(&encode_base64(&data), &file, &|| Ok(())).unwrap();
        assert_eq!(written, data.len() as u64);
        assert_eq!(std::fs::read(&file).unwrap(), data);
        for wrong in ["not base64!", "QUJD\nREVG", "QUJDRA"] {
            let error = decode_into(wrong, &file, &|| Ok(())).unwrap_err();
            assert_eq!(error.code, "ARGUMENT_ONGELDIG", "{wrong}");
        }
    }

    #[test]
    fn the_answers_carry_the_file_once_with_its_digest() {
        let directory = tempfile::tempdir().unwrap();
        let bytes: Vec<u8> = (0..100_000u32).map(|value| (value % 251) as u8).collect();
        let output = output(directory.path(), &bytes);
        let rest = rest_answer(&output).unwrap();
        assert_eq!(rest.capacity(), rest.len(), "one buffer of exact size");
        let rest: Value = serde_json::from_slice(&rest).unwrap();
        assert_eq!(
            decode_base64(rest["output"]["data"].as_str().unwrap()).unwrap(),
            bytes
        );
        assert_eq!(rest["output"]["bytes"], 100_000);
        assert_eq!(
            rest["output"]["sha256"],
            digest(&directory.path().join("result.obj")).unwrap().1
        );
        assert_eq!(rest["output"]["file_name"], "scan.obj");
        assert_eq!(rest["output"]["media_type"], "model/obj");
        assert_eq!(rest["report"]["vertices"], 3);

        let id = json!(7);
        let answer = mcp_answer(&id, &output, true).unwrap();
        assert_eq!(answer.capacity(), answer.len());
        let answer: Value = serde_json::from_slice(&answer).unwrap();
        assert_eq!(answer["id"], 7);
        let content = answer["result"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["resource"]["uri"], "result:scan.obj");
        assert_eq!(content[1]["resource"]["mimeType"], "model/obj");
        assert_eq!(content[1]["resource"]["blob"], rest["output"]["data"]);
        let mut without_data = rest.clone();
        without_data["output"]
            .as_object_mut()
            .unwrap()
            .remove("data");
        assert_eq!(answer["result"]["structuredContent"], without_data);
        let text: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, without_data);
        assert_eq!(answer["result"]["isError"], false);

        let older = mcp_answer(&json!("a"), &output, false).unwrap();
        let older: Value = serde_json::from_slice(&older).unwrap();
        assert!(older["result"].get("structuredContent").is_none());
        assert_eq!(
            older["result"]["content"][1]["resource"]["blob"],
            rest["output"]["data"]
        );
    }

    #[test]
    fn an_answer_without_a_file_is_its_report() {
        let output = ToolOutput {
            report: json!({"points": 4}),
            file: None,
        };
        let rest: Value = serde_json::from_slice(&rest_answer(&output).unwrap()).unwrap();
        assert_eq!(rest, json!({"report": {"points": 4}}));
        let answer: Value =
            serde_json::from_slice(&mcp_answer(&json!(1), &output, true).unwrap()).unwrap();
        assert_eq!(answer["result"]["structuredContent"], rest);
        assert_eq!(answer["result"]["content"].as_array().unwrap().len(), 1);
    }
}
