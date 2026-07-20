//! HTTP Content-Encoding helpers for the local Codex proxy.
//!
//! ChatGPT Desktop (logged-in) may compress `/v1/responses` request bodies with
//! `Content-Encoding: zstd` (and sometimes gzip/deflate/br or stacked codings).
//! Parsing those bytes as JSON fails with "expected value at line 1 column 1".
//!
//! Behavior matches the well-known Desktop + local-proxy seam (gzip / deflate /
//! brotli / zstd, reverse decode for stacked encodings). Implementation is
//! independent; do not paste third-party sources.

use std::io::Read;

use axum::{body::Bytes, http::HeaderMap};

fn split_codings(content_encoding: &str) -> Vec<&str> {
    content_encoding
        .split(',')
        .map(str::trim)
        .filter(|coding| !coding.is_empty() && *coding != "identity")
        .collect()
}

fn is_single_supported(coding: &str) -> bool {
    matches!(
        coding,
        "gzip" | "x-gzip" | "deflate" | "br" | "zstd" | "zst"
    )
}

fn decompress_single(coding: &str, body: &[u8]) -> Result<Option<Vec<u8>>, std::io::Error> {
    match coding {
        "gzip" | "x-gzip" => {
            let mut decoder = flate2::read::GzDecoder::new(body);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out)?;
            Ok(Some(out))
        }
        "deflate" => {
            // RFC 9110: zlib-wrapped. Some clients send raw deflate — fall back.
            let mut out = Vec::new();
            let mut zlib = flate2::read::ZlibDecoder::new(body);
            match zlib.read_to_end(&mut out) {
                Ok(_) => Ok(Some(out)),
                Err(_) => {
                    let mut raw_out = Vec::new();
                    let mut raw = flate2::read::DeflateDecoder::new(body);
                    raw.read_to_end(&mut raw_out)?;
                    Ok(Some(raw_out))
                }
            }
        }
        "br" => {
            let mut out = Vec::new();
            brotli::BrotliDecompress(&mut std::io::Cursor::new(body), &mut out)?;
            Ok(Some(out))
        }
        "zstd" | "zst" => {
            // Desktop login path uses Compression::Zstd on the request body.
            let out = zstd::stream::decode_all(std::io::Cursor::new(body))?;
            Ok(Some(out))
        }
        _ => Ok(None),
    }
}

/// Decompress a body according to `Content-Encoding` (supports stacked codings).
///
/// Returns `Ok(None)` when there is nothing to decode, or when an unsupported
/// coding is present (caller should not strip the encoding header).
pub fn decompress_body(
    content_encoding: &str,
    body: &[u8],
) -> Result<Option<Vec<u8>>, std::io::Error> {
    let codings = split_codings(content_encoding);
    if codings.is_empty() {
        return Ok(None);
    }
    if !codings.iter().all(|coding| is_single_supported(coding)) {
        tracing::warn!(%content_encoding, "unsupported content-encoding; leaving body compressed");
        return Ok(None);
    }

    // Codings are listed in application order; reverse when decoding.
    let mut data: Option<Vec<u8>> = None;
    for coding in codings.iter().rev() {
        let input = data.as_deref().unwrap_or(body);
        match decompress_single(coding, input)? {
            Some(decoded) => data = Some(decoded),
            None => return Ok(None),
        }
    }
    Ok(data)
}

pub fn is_supported_content_encoding(content_encoding: &str) -> bool {
    let codings = split_codings(content_encoding);
    !codings.is_empty() && codings.iter().all(|coding| is_single_supported(coding))
}

/// Merge repeated `Content-Encoding` headers into a comma-separated list.
pub fn get_content_encoding(headers: &HeaderMap) -> Option<String> {
    let combined = headers
        .get_all("content-encoding")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
        .to_ascii_lowercase();
    if split_codings(&combined).is_empty() {
        None
    } else {
        Some(combined)
    }
}

/// Decode a Codex/Desktop request body if `Content-Encoding` is present.
///
/// On success the returned bytes are plain JSON and entity headers that no
/// longer apply are removed from `headers`.
pub fn decode_request_body(headers: &mut HeaderMap, body: Bytes) -> Result<Bytes, String> {
    let Some(encoding) = get_content_encoding(headers) else {
        return Ok(body);
    };
    if !is_supported_content_encoding(&encoding) {
        return Err(format!("Unsupported request content-encoding: {encoding}"));
    }
    tracing::debug!(%encoding, bytes = body.len(), "decompressing Codex request body");
    let decompressed = match decompress_body(&encoding, &body) {
        Ok(Some(decoded)) => decoded,
        Ok(None) => {
            return Err(format!("Unsupported request content-encoding: {encoding}"));
        }
        Err(error) => {
            return Err(format!(
                "Failed to decompress request body ({encoding}): {error}"
            ));
        }
    };
    headers.remove(axum::http::header::CONTENT_ENCODING);
    headers.remove(axum::http::header::CONTENT_LENGTH);
    headers.remove(axum::http::header::TRANSFER_ENCODING);
    Ok(Bytes::from(decompressed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn decompresses_zstd_like_codex_desktop() {
        let payload = br#"{"model":"spur-route-test","input":"hi"}"#;
        let compressed = zstd::stream::encode_all(std::io::Cursor::new(&payload[..]), 0).unwrap();
        let decoded = decompress_body("zstd", &compressed).unwrap().unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn decompresses_gzip() {
        use std::io::Write;
        let payload = br#"{"ok":true}"#;
        let mut encoder =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(payload).unwrap();
        let compressed = encoder.finish().unwrap();
        let decoded = decompress_body("gzip", &compressed).unwrap().unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn decode_request_body_strips_entity_headers() {
        let payload = br#"{"model":"x"}"#;
        let compressed = zstd::stream::encode_all(std::io::Cursor::new(&payload[..]), 0).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("content-encoding", HeaderValue::from_static("zstd"));
        headers.insert(
            "content-length",
            HeaderValue::from_str(&compressed.len().to_string()).unwrap(),
        );
        let decoded = decode_request_body(&mut headers, Bytes::from(compressed)).expect("decode");
        assert_eq!(decoded.as_ref(), payload);
        assert!(headers.get("content-encoding").is_none());
        assert!(headers.get("content-length").is_none());
    }

    #[test]
    fn get_content_encoding_merges_repeated_headers() {
        let mut headers = HeaderMap::new();
        headers.append("content-encoding", HeaderValue::from_static("gzip"));
        headers.append("content-encoding", HeaderValue::from_static("zstd"));
        assert_eq!(
            get_content_encoding(&headers).as_deref(),
            Some("gzip, zstd")
        );
    }
}
