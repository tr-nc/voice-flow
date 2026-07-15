use std::io::{Read, Write};

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde_json::Value;

const VERSION: u8 = 0b0001;
const HEADER_WORDS: u8 = 0b0001;
const CLIENT_FULL_REQUEST: u8 = 0b0001;
const CLIENT_AUDIO_REQUEST: u8 = 0b0010;
const SERVER_FULL_RESPONSE: u8 = 0b1001;
const SERVER_ERROR_RESPONSE: u8 = 0b1111;
const FLAG_POSITIVE_SEQUENCE: u8 = 0b0001;
const FLAG_NEGATIVE_SEQUENCE: u8 = 0b0011;
const SERIALIZATION_NONE: u8 = 0b0000;
const SERIALIZATION_JSON: u8 = 0b0001;
const COMPRESSION_GZIP: u8 = 0b0001;

#[derive(Debug)]
pub struct ServerFrame {
    pub is_last: bool,
    pub payload: Option<Value>,
}

pub fn full_request(sequence: i32, payload: &Value) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(payload).context("failed to serialize the ASR request")?;
    let body = gzip(&json)?;
    let mut frame = header(
        CLIENT_FULL_REQUEST,
        FLAG_POSITIVE_SEQUENCE,
        SERIALIZATION_JSON,
        COMPRESSION_GZIP,
    );
    frame.extend_from_slice(&sequence.to_be_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

pub fn audio_request(sequence: i32, pcm: &[u8], is_last: bool) -> Result<Vec<u8>> {
    let body = gzip(pcm)?;
    let flags = if is_last {
        FLAG_NEGATIVE_SEQUENCE
    } else {
        FLAG_POSITIVE_SEQUENCE
    };
    let signed_sequence = if is_last {
        -sequence.abs()
    } else {
        sequence.abs()
    };

    let mut frame = header(
        CLIENT_AUDIO_REQUEST,
        flags,
        SERIALIZATION_NONE,
        COMPRESSION_GZIP,
    );
    frame.extend_from_slice(&signed_sequence.to_be_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

pub fn parse_server_frame(data: &[u8]) -> Result<ServerFrame> {
    if data.len() < 4 {
        bail!("invalid ASR frame: header is shorter than four bytes");
    }

    let header_bytes = usize::from(data[0] & 0x0f) * 4;
    if header_bytes < 4 || data.len() < header_bytes {
        bail!("invalid ASR frame: declared header size is out of bounds");
    }

    let message_type = data[1] >> 4;
    let flags = data[1] & 0x0f;
    let serialization = data[2] >> 4;
    let compression = data[2] & 0x0f;
    let mut offset = header_bytes;

    if flags & 0b0001 != 0 {
        read_exact(data, offset, 4, "sequence number")?;
        offset += 4;
    }

    match message_type {
        SERVER_FULL_RESPONSE => {
            let payload_size = read_u32(data, &mut offset, "response payload size")? as usize;
            let body = read_exact(data, offset, payload_size, "response payload")?;
            let payload = decode_payload(serialization, compression, body)?;
            Ok(ServerFrame {
                is_last: flags & 0b0010 != 0,
                payload,
            })
        }
        SERVER_ERROR_RESPONSE => {
            let code = read_i32(data, &mut offset, "error code")?;
            let payload_size = read_u32(data, &mut offset, "error payload size")? as usize;
            let body = read_exact(data, offset, payload_size, "error payload")?;
            let detail = decode_bytes(compression, body)
                .map(|decoded| String::from_utf8_lossy(&decoded).into_owned())
                .unwrap_or_else(|_| String::from_utf8_lossy(body).into_owned());
            bail!("VolcEngine ASR protocol error {code}: {detail}");
        }
        _ => Ok(ServerFrame {
            is_last: flags & 0b0010 != 0,
            payload: None,
        }),
    }
}

pub fn extract_text(payload: Option<&Value>) -> Option<String> {
    let result = payload?.get("result")?;
    if let Some(text) = result.get("text").and_then(Value::as_str)
        && !text.is_empty()
    {
        return Some(text.to_owned());
    }

    let utterances = result.get("utterances")?.as_array()?;
    let joined = utterances
        .iter()
        .filter_map(|utterance| utterance.get("text").and_then(Value::as_str))
        .collect::<String>();
    (!joined.is_empty()).then_some(joined)
}

fn header(message_type: u8, flags: u8, serialization: u8, compression: u8) -> Vec<u8> {
    vec![
        (VERSION << 4) | HEADER_WORDS,
        (message_type << 4) | flags,
        (serialization << 4) | compression,
        0,
    ]
}

fn gzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .context("failed to gzip an ASR payload")?;
    encoder
        .finish()
        .context("failed to finish ASR gzip payload")
}

fn decode_payload(serialization: u8, compression: u8, data: &[u8]) -> Result<Option<Value>> {
    let decoded = decode_bytes(compression, data)?;
    if decoded.is_empty() || serialization != SERIALIZATION_JSON {
        return Ok(None);
    }
    let value =
        serde_json::from_slice(&decoded).context("failed to decode the ASR JSON response")?;
    Ok(Some(value))
}

fn decode_bytes(compression: u8, data: &[u8]) -> Result<Vec<u8>> {
    if compression != COMPRESSION_GZIP || data.is_empty() {
        return Ok(data.to_vec());
    }

    let mut decoder = GzDecoder::new(data);
    let mut decoded = Vec::new();
    decoder
        .read_to_end(&mut decoded)
        .context("failed to gunzip an ASR response")?;
    Ok(decoded)
}

fn read_exact<'a>(data: &'a [u8], offset: usize, size: usize, label: &str) -> Result<&'a [u8]> {
    let end = offset
        .checked_add(size)
        .with_context(|| format!("invalid ASR frame: {label} overflow"))?;
    data.get(offset..end)
        .with_context(|| format!("invalid ASR frame: {label} is out of bounds"))
}

fn read_u32(data: &[u8], offset: &mut usize, label: &str) -> Result<u32> {
    let bytes: [u8; 4] = read_exact(data, *offset, 4, label)?
        .try_into()
        .expect("four-byte slice");
    *offset += 4;
    Ok(u32::from_be_bytes(bytes))
}

fn read_i32(data: &[u8], offset: &mut usize, label: &str) -> Result<i32> {
    let bytes: [u8; 4] = read_exact(data, *offset, 4, label)?
        .try_into()
        .expect("four-byte slice");
    *offset += 4;
    Ok(i32::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_final_audio_with_a_negative_sequence() {
        let frame = audio_request(7, &[1, 2, 3], true).unwrap();
        assert_eq!(frame[1] & 0x0f, FLAG_NEGATIVE_SEQUENCE);
        assert_eq!(i32::from_be_bytes(frame[4..8].try_into().unwrap()), -7);
    }

    #[test]
    fn extracts_full_text() {
        let payload = serde_json::json!({ "result": { "text": "实时文字" } });
        assert_eq!(extract_text(Some(&payload)).as_deref(), Some("实时文字"));
    }
}
