//! Zero-Memory Streaming Local Media Protocol Handler
//!
//! Governed by PRD.md, TRD.md Section 4.2, and GEMINI.md.
//! Implements a secure custom URI scheme (`media://...`) that streams local
//! audio, image, and video files directly from `%LOCALAPPDATA%\InstagramChatHistoryViewer\media`
//! to WebView2 using standard HTTP 206 Partial Content range requests.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Resolves standard MIME types from file extensions.
pub fn get_mime_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("mp4") | Some("m4a") | Some("aac") => "audio/mp4",
        Some("mp3") => "audio/mpeg",
        Some("ogg") | Some("oga") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("webm") => "video/webm",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// Validates and resolves a requested relative media path against the media sandbox directory.
/// Strictly rejects path traversals (`..`), colons, drive letters, and UNC paths.
pub fn resolve_sandboxed_media_path(
    media_root: &Path,
    raw_path: &str,
) -> Result<PathBuf, &'static str> {
    // Reject Windows drive letters, colons, and UNC paths immediately
    if raw_path.starts_with(r"\\") || raw_path.starts_with("//") || raw_path.contains(':') {
        return Err("Forbidden character in media path");
    }

    // Strip leading forward/backward slashes
    let trimmed = raw_path.trim_start_matches('/').trim_start_matches('\\');
    if trimmed.is_empty() {
        return Err("Empty media path");
    }

    // Reject Windows drive letters, colons, and UNC paths in trimmed path
    if trimmed.contains(':') || trimmed.starts_with(r"\\") || trimmed.starts_with("//") {
        return Err("Forbidden character in media path");
    }

    // Percent-decode path segments safely
    let decoded = match percent_decode_str(trimmed) {
        Some(d) => d,
        None => return Err("Invalid percent-encoded path"),
    };

    let rel_path = Path::new(&decoded);

    // Verify each component is a normal component (no ParentDir, CurDir, or Prefix)
    for component in rel_path.components() {
        match component {
            std::path::Component::Normal(_) => {}
            _ => return Err("Path traversal attempt detected"),
        }
    }

    let candidate = media_root.join(rel_path);

    // Ensure candidate starts with media_root
    // Note: Canonicalization only works if file exists; for non-existing files, verify prefix
    let normalized_media_root = media_root
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('\\', "/");
    let normalized_candidate = candidate
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('\\', "/");

    if !normalized_candidate.starts_with(&normalized_media_root) {
        return Err("Target path outside media sandbox");
    }

    Ok(candidate)
}

/// Simple percent-decoding helper without external dependencies
fn percent_decode_str(input: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut chars = input.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let h1 = chars.next()?;
            let h2 = chars.next()?;
            let hex_bytes = [h1, h2];
            let hex_str = std::str::from_utf8(&hex_bytes).ok()?;
            let byte = u8::from_str_radix(hex_str, 16).ok()?;
            bytes.push(byte);
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8(bytes).ok()
}

/// Handles incoming `media://` scheme requests.
/// Implements HTTP 206 Partial Content range requests for zero-memory audio and video playback.
pub fn handle_media_request(
    req: &tauri::http::Request<Vec<u8>>,
    media_root: &Path,
) -> tauri::http::Response<Vec<u8>> {
    let raw_path = req.uri().path();
    let resolved_path = match resolve_sandboxed_media_path(media_root, raw_path) {
        Ok(p) => p,
        Err(_) => {
            return tauri::http::Response::builder()
                .status(403)
                .header("Content-Type", "text/plain")
                .header("Access-Control-Allow-Origin", "*")
                .body(b"403 Forbidden: Path outside media sandbox".to_vec())
                .unwrap();
        }
    };

    if !resolved_path.exists() || !resolved_path.is_file() {
        return tauri::http::Response::builder()
            .status(404)
            .header("Content-Type", "text/plain")
            .header("Access-Control-Allow-Origin", "*")
            .body(b"404 Not Found: Media file unavailable".to_vec())
            .unwrap();
    }

    let mut file = match File::open(&resolved_path) {
        Ok(f) => f,
        Err(e) => {
            return tauri::http::Response::builder()
                .status(500)
                .header("Content-Type", "text/plain")
                .body(format!("500 Internal Server Error: {}", e).into_bytes())
                .unwrap();
        }
    };

    let file_len = match file.metadata().map(|m| m.len()) {
        Ok(l) => l,
        Err(e) => {
            return tauri::http::Response::builder()
                .status(500)
                .body(format!("Failed to read metadata: {}", e).into_bytes())
                .unwrap();
        }
    };

    let mime_type = get_mime_type(&resolved_path);

    // Inspect HTTP Range header
    let range_header = req.headers().get("range").and_then(|v| v.to_str().ok());

    if let Some(range_str) = range_header {
        if let Some((start, end)) = parse_range_header(range_str, file_len) {
            if start >= file_len {
                return tauri::http::Response::builder()
                    .status(416)
                    .header("Content-Range", format!("bytes */{}", file_len))
                    .body(Vec::new())
                    .unwrap();
            }

            let end = end.min(file_len - 1);
            // Cap single range chunk at 2 MB for zero-memory streaming
            let max_chunk_size: u64 = 2 * 1024 * 1024;
            let end = if (end - start + 1) > max_chunk_size {
                start + max_chunk_size - 1
            } else {
                end
            };

            let chunk_len = (end - start + 1) as usize;
            let mut buffer = vec![0u8; chunk_len];

            if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut buffer).is_err() {
                return tauri::http::Response::builder()
                    .status(500)
                    .body(b"Failed to seek or read range".to_vec())
                    .unwrap();
            }

            return tauri::http::Response::builder()
                .status(206)
                .header("Content-Type", mime_type)
                .header(
                    "Content-Range",
                    format!("bytes {}-{}/{}", start, end, file_len),
                )
                .header("Content-Length", chunk_len.to_string())
                .header("Accept-Ranges", "bytes")
                .header("Access-Control-Allow-Origin", "*")
                .body(buffer)
                .unwrap();
        }
    }

    // Default: full file content for images or small files (up to 10 MB)
    let max_full_size: u64 = 10 * 1024 * 1024;
    if file_len <= max_full_size {
        let mut buffer = Vec::with_capacity(file_len as usize);
        if file.read_to_end(&mut buffer).is_err() {
            return tauri::http::Response::builder()
                .status(500)
                .body(b"Failed to read file".to_vec())
                .unwrap();
        }

        tauri::http::Response::builder()
            .status(200)
            .header("Content-Type", mime_type)
            .header("Content-Length", file_len.to_string())
            .header("Accept-Ranges", "bytes")
            .header("Access-Control-Allow-Origin", "*")
            .body(buffer)
            .unwrap()
    } else {
        // For large files requested without a range header, stream the first 2 MB chunk as 206
        let chunk_len = 2 * 1024 * 1024;
        let mut buffer = vec![0u8; chunk_len];
        let _ = file.read_exact(&mut buffer);

        tauri::http::Response::builder()
            .status(206)
            .header("Content-Type", mime_type)
            .header(
                "Content-Range",
                format!("bytes 0-{}/{}", chunk_len - 1, file_len),
            )
            .header("Content-Length", chunk_len.to_string())
            .header("Accept-Ranges", "bytes")
            .header("Access-Control-Allow-Origin", "*")
            .body(buffer)
            .unwrap()
    }
}

/// Parses standard HTTP `bytes=start-end` range headers.
fn parse_range_header(header: &str, file_len: u64) -> Option<(u64, u64)> {
    let trimmed = header.trim();
    if !trimmed.starts_with("bytes=") {
        return None;
    }
    let range_part = &trimmed["bytes=".len()..];
    let mut parts = range_part.split('-');
    let start_str = parts.next()?.trim();
    let end_str = parts.next()?.trim();

    if start_str.is_empty() {
        // Suffix range: bytes=-500 (last 500 bytes)
        let suffix_len: u64 = end_str.parse().ok()?;
        let start = file_len.saturating_sub(suffix_len);
        let end = file_len.saturating_sub(1);
        Some((start, end))
    } else {
        let start: u64 = start_str.parse().ok()?;
        let end = if end_str.is_empty() {
            file_len.saturating_sub(1)
        } else {
            end_str.parse().ok()?
        };
        Some((start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_mime_type_resolution() {
        assert_eq!(get_mime_type(Path::new("note.mp4")), "audio/mp4");
        assert_eq!(get_mime_type(Path::new("song.mp3")), "audio/mpeg");
        assert_eq!(get_mime_type(Path::new("photo.jpg")), "image/jpeg");
        assert_eq!(get_mime_type(Path::new("photo.png")), "image/png");
        assert_eq!(get_mime_type(Path::new("clip.webm")), "video/webm");
        assert_eq!(
            get_mime_type(Path::new("unknown.xyz")),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_path_sanitization_rejects_traversal() {
        let root = Path::new("C:/test_media");
        assert!(resolve_sandboxed_media_path(root, "../secret.txt").is_err());
        assert!(resolve_sandboxed_media_path(root, "conv1/../../secret.txt").is_err());
        assert!(resolve_sandboxed_media_path(root, "C:/Windows/system32").is_err());
        assert!(resolve_sandboxed_media_path(root, "file.mp4:stream").is_err());
        assert!(resolve_sandboxed_media_path(root, "\\\\server\\share\\x.mp4").is_err());
        assert!(resolve_sandboxed_media_path(root, "conv1%2F..%2F..%2Fsecret.txt").is_err());
    }

    #[test]
    fn test_valid_sandboxed_path_resolves() {
        let root = Path::new("C:/test_media");
        let resolved = resolve_sandboxed_media_path(root, "conv_123/audio/voice_1.mp4").unwrap();
        assert_eq!(
            resolved.to_string_lossy().replace('\\', "/"),
            "C:/test_media/conv_123/audio/voice_1.mp4"
        );
    }

    #[test]
    fn test_range_parser() {
        assert_eq!(parse_range_header("bytes=0-499", 1000), Some((0, 499)));
        assert_eq!(parse_range_header("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_range_header("bytes=-200", 1000), Some((800, 999)));
        assert_eq!(parse_range_header("invalid", 1000), None);
    }

    #[test]
    fn test_handle_media_request_range_and_missing() {
        let temp_dir = std::env::temp_dir().join(format!("test_media_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let file_path = temp_dir.join("test.mp4");
        let test_data = b"0123456789abcdefghijklmnopqrstuvwxyz";
        {
            let mut f = File::create(&file_path).unwrap();
            f.write_all(test_data).unwrap();
        }

        // Test 1: Full request (200 OK)
        let req_full = tauri::http::Request::builder()
            .uri("media://localhost/test.mp4")
            .body(Vec::new())
            .unwrap();
        let resp_full = handle_media_request(&req_full, &temp_dir);
        assert_eq!(resp_full.status(), 200);
        assert_eq!(resp_full.body(), test_data);

        // Test 2: Range request (206 Partial Content)
        let req_range = tauri::http::Request::builder()
            .uri("media://localhost/test.mp4")
            .header("Range", "bytes=5-14")
            .body(Vec::new())
            .unwrap();
        let resp_range = handle_media_request(&req_range, &temp_dir);
        assert_eq!(resp_range.status(), 206);
        assert_eq!(resp_range.body(), &test_data[5..=14]);
        assert_eq!(
            resp_range
                .headers()
                .get("Content-Range")
                .unwrap()
                .to_str()
                .unwrap(),
            format!("bytes 5-14/{}", test_data.len())
        );

        // Test 3: Missing file (404)
        let req_missing = tauri::http::Request::builder()
            .uri("media://localhost/non_existent.mp4")
            .body(Vec::new())
            .unwrap();
        let resp_missing = handle_media_request(&req_missing, &temp_dir);
        assert_eq!(resp_missing.status(), 404);

        // Test 4: Traversal (403)
        let req_traversal = tauri::http::Request::builder()
            .uri("media://localhost/../secret.txt")
            .body(Vec::new())
            .unwrap();
        let resp_traversal = handle_media_request(&req_traversal, &temp_dir);
        assert_eq!(resp_traversal.status(), 403);

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
