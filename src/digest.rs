//! Content digests (`sha256:<hex>`), the one form used everywhere a digest
//! is stored or compared.

use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub fn digest(bytes: impl AsRef<[u8]>) -> String {
    let hash = Sha256::digest(bytes.as_ref());
    format!("sha256:{hash:x}")
}

/// File contents looked up by their digest. Anchors record the digest of the
/// file version they were written against, so a viewer can find that text
/// (and the text of the version in front of it) wherever the bundle or the
/// current session happens to hold it, without knowing which revision it
/// came from.
#[derive(Default)]
pub struct Blobs<'a>(HashMap<String, &'a [u8]>);

impl<'a> Blobs<'a> {
    pub fn add(&mut self, bytes: &'a [u8]) {
        self.0.entry(digest(bytes)).or_insert(bytes);
    }

    /// The text with this digest, if held and valid UTF-8.
    pub fn text(&self, digest: &str) -> Option<&'a str> {
        std::str::from_utf8(self.0.get(digest)?).ok()
    }
}
