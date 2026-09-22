//! Images attached to comments: which may be, and how a comment names one.
//!
//! An image is kept in the bundle under the hex of its sha256 digest, and a
//! comment refers to it as `![alt](diffnote-image:<hex>)`. Nothing is ever
//! loaded from an address: a link to an image anywhere else is drawn as its
//! alt text. The page shows an image only in an `<img>` (where an SVG can't run
//! anything), and the server sends it with a policy that stops it from doing
//! anything if it is opened by itself.

use crate::messages::m;

/// What a comment's link to an image starts with.
pub const SCHEME: &str = "diffnote-image:";

/// What a comment's link to a file that is not an image starts with.
pub const FILE_SCHEME: &str = "diffnote-file:";

/// The most the server takes in one request, whatever the review's own limit
/// on an attached file says (it is all held in memory).
pub const CEILING: usize = 100 * 1024 * 1024;

/// The id an image has: the hex of its sha256 digest.
pub fn id_of(bytes: &[u8]) -> String {
    crate::digest::digest(bytes)
        .trim_start_matches("sha256:")
        .to_string()
}

/// Whether `s` is an image id (64 hex digits, lower case).
pub fn is_id(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The ids of the images a text (a comment's body) refers to.
pub fn ids_in(text: &str) -> Vec<String> {
    ids_after(SCHEME, text)
}

/// The ids of the other attached files a text refers to.
pub fn file_ids_in(text: &str) -> Vec<String> {
    ids_after(FILE_SCHEME, text)
}

fn ids_after(scheme: &str, text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(scheme) {
        let after = &rest[at + scheme.len()..];
        if after.len() >= 64 && is_id(&after[..64]) && !found.contains(&after[..64].to_string()) {
            found.push(after[..64].to_string());
        }
        rest = &rest[at + scheme.len()..];
    }
    found
}

/// A name to save an attached file under: what the page says it is called,
/// without anything that could make it a path or spoil a header.
pub fn file_name(asked: &str) -> String {
    let base = asked.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | ':' | '*' | '?' | '<' | '>' | '|'))
        .take(120)
        .collect();
    let clean = clean.trim().trim_start_matches('.').to_string();
    if clean.is_empty() {
        "file".to_string()
    } else {
        clean
    }
}

/// The media type of an image that may be attached (PNG, JPEG, GIF, WebP, or an
/// SVG with nothing in it that runs or loads), read from its bytes; or why it
/// may not.
pub fn kind(bytes: &[u8]) -> Result<&'static str, String> {
    if bytes.is_empty() {
        return Err(m("image.empty").into());
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Ok("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Ok("image/gif");
    }
    if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Ok("image/webp");
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        let lower = text.trim_start_matches('\u{feff}').to_ascii_lowercase();
        if lower.contains("<svg") {
            return safe_svg(&lower).map(|_| "image/svg+xml");
        }
    }
    Err(m("image.unsupported_format").into())
}

/// An SVG is refused (not changed) if anything in it could run, or load
/// something from elsewhere. `lower` is its text in lower case.
fn safe_svg(lower: &str) -> Result<(), String> {
    let runs = || m("image.svg_runs").to_string();
    for bad in [
        "<script",
        "<foreignobject",
        "<iframe",
        "<object",
        "<embed",
        "<!entity",
        "<!doctype",
        "javascript:",
        "vbscript:",
    ] {
        if lower.contains(bad) {
            return Err(runs());
        }
    }
    // Addresses: the namespaces are the ones it may name, and nothing else (a
    // relative one, `#id`, is fine).
    let without_namespaces = lower
        .replace("http://www.w3.org/2000/svg", "")
        .replace("http://www.w3.org/1999/xlink", "");
    if ["http://", "https://", "=\"//", "='//", "(//", "data:text"]
        .iter()
        .any(|a| without_namespaces.contains(a))
    {
        return Err(m("image.svg_external_ref").into());
    }
    // An event handler: ` onload=`, `"onclick =` ...
    let bytes = lower.as_bytes();
    for i in 1..bytes.len().saturating_sub(2) {
        let after_gap = matches!(
            bytes[i - 1],
            b' ' | b'\t' | b'\n' | b'\r' | b'"' | b'\'' | b'/'
        );
        if after_gap
            && bytes[i] == b'o'
            && bytes[i + 1] == b'n'
            && bytes[i + 2].is_ascii_alphabetic()
        {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j].is_ascii_alphabetic() {
                j += 1;
            }
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'=' {
                return Err(runs());
            }
        }
    }
    Ok(())
}

/// The image as a `data:` address, for a page that has no server to ask.
pub fn data_uri(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine;
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R',
    ];

    #[test]
    fn an_image_is_told_by_its_bytes_and_only_the_known_kinds_may_be_attached() {
        assert_eq!(kind(PNG), Ok("image/png"));
        assert_eq!(kind(&[0xff, 0xd8, 0xff, 0xe0, 0]), Ok("image/jpeg"));
        assert_eq!(kind(b"GIF89a....."), Ok("image/gif"));
        assert_eq!(kind(b"RIFF\x10\0\0\0WEBPVP8 "), Ok("image/webp"));
        assert!(kind(b"").is_err());
        assert!(kind(b"just text").is_err());
        assert!(kind(b"<html><body>x</body></html>").is_err());
    }

    #[test]
    fn an_svg_with_nothing_that_runs_or_loads_is_taken_and_any_other_is_refused() {
        let good = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4 4"><rect width="4" height="4" fill="red"/></svg>"#;
        assert_eq!(kind(good), Ok("image/svg+xml"));
        for bad in [
            r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" ONCLICK = "x"></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><foreignObject><div/></foreignObject></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="https://x.example/a.png"/></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><a href="javascript:alert(1)"><rect/></a></svg>"#,
            r#"<!DOCTYPE svg [<!ENTITY x "y">]><svg xmlns="http://www.w3.org/2000/svg"/>"#,
        ] {
            assert!(kind(bad.as_bytes()).is_err(), "{bad}");
        }
        // A word that only looks like a handler is fine.
        assert_eq!(
            kind(br#"<svg xmlns="http://www.w3.org/2000/svg"><text>only one action on it</text></svg>"#),
            Ok("image/svg+xml")
        );
    }

    #[test]
    fn a_name_to_save_a_file_under_is_only_a_name() {
        assert_eq!(file_name("report.pdf"), "report.pdf");
        assert_eq!(file_name("C:\\dir\\log file.txt"), "log file.txt");
        assert_eq!(file_name("../../etc/passwd"), "passwd");
        assert_eq!(file_name("a\"b\r\n.txt"), "ab.txt");
        assert_eq!(file_name(".hidden"), "hidden");
        assert_eq!(file_name(""), "file");
        assert_eq!(file_name("///"), "file");
        assert_eq!(file_name(&"x".repeat(500)).chars().count(), 120);
    }

    #[test]
    fn a_comment_names_its_images_by_their_digests() {
        let id = id_of(PNG);
        assert!(is_id(&id));
        assert!(!is_id("ABC"));
        let body =
            format!("look ![a]({SCHEME}{id}) and again ![b]({SCHEME}{id}) and ![c]({SCHEME}short)");
        assert_eq!(ids_in(&body), vec![id]);
        assert!(data_uri("image/png", PNG).starts_with("data:image/png;base64,iVBORw0KGgo"));
    }
}
