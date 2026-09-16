//! MIME detection and base64 encoding for `read_media`.
//!
//! `read_file` refuses a binary file on purpose — decoding bytes as UTF-8 and
//! showing the lossy result is worse than saying "this is binary". An image or
//! a PDF is different: the bytes *are* the content, and a model that can see
//! an image should receive it as one. This module is the small amount of work
//! between the file on disk and the MCP image/PDF content block: decide what
//! it is, cap it, encode it.
//!
//! Base64 is hand-rolled rather than pulled in as a dependency. The encoder is
//! twenty lines with no policy of its own, and a new crate is a larger surface
//! than the function it would replace.

use std::path::Path;

/// The largest media file a single call will return.
///
/// 8 MiB is the point past which base64 (~4/3 expansion) plus the JSON
/// envelope starts to dominate the context window the tool exists to protect.
/// A screenshot or a design mock is well under it; a scanned PDF is not, and
/// gets refused rather than silently truncated to an unopenable prefix.
pub const MAX_MEDIA_BYTES: u64 = 8 * 1024 * 1024;

/// What kind of content block a file should become.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Pdf,
    /// A type we know but cannot hand over as media (or an unknown one). The
    /// caller falls back to `read_file` for these.
    Text,
}

/// The MIME type for a path extension, when it is one we can send as media.
///
/// Deliberately a closed list. "Guess from the extension and hope" is how a
/// `.dat` becomes `application/octet-stream` and then an unrenderable block;
/// an extension that is not here returns `None` and the caller is told the
/// file is not media.
pub fn mime_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        _ => return None,
    })
}

/// Which block a MIME type maps to.
pub fn kind_for(mime: &str) -> MediaKind {
    if mime.starts_with("image/") {
        MediaKind::Image
    } else if mime == "application/pdf" {
        MediaKind::Pdf
    } else {
        MediaKind::Text
    }
}

/// The standard base64 alphabet with `=` padding, RFC 4648 §4.
pub fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(n >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(n >> 6) as usize & 0x3f] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[n as usize & 0x3f] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        // RFC 4648 §10.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_round_trips_through_a_decoder() {
        // A tiny decoder, so the property is checked rather than asserted.
        fn decode(s: &str) -> Vec<u8> {
            const REV: &[u8; 128] = &{
                let mut t = [255u8; 128];
                let alpha = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
                let mut i = 0;
                while i < 64 {
                    t[alpha[i] as usize] = i as u8;
                    i += 1;
                }
                t
            };
            let mut out = Vec::new();
            let mut buf = 0u32;
            let mut bits = 0u32;
            for c in s.bytes().filter(|c| *c != b'=') {
                buf = (buf << 6) | REV[c as usize] as u32;
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    out.push((buf >> bits) as u8);
                }
            }
            out
        }
        for n in 0..=64usize {
            let bytes: Vec<u8> = (0..n).map(|i| (i * 7 + 13) as u8).collect();
            assert_eq!(decode(&base64_encode(&bytes)), bytes, "n={n}");
        }
    }

    #[test]
    fn media_extensions_map_and_others_do_not() {
        assert_eq!(mime_for(&PathBuf::from("a/b/shot.PNG")), Some("image/png"));
        assert_eq!(mime_for(PathBuf::from("doc.pdf").as_path()), Some("application/pdf"));
        assert_eq!(mime_for(PathBuf::from("x.rs").as_path()), None);
        assert_eq!(mime_for(PathBuf::from("noext").as_path()), None);
    }
}
