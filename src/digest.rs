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
/// came from. Versions are also linked when some revision took one to the
/// other (its file went from one digest to the other), which lets a position
/// be followed through the intermediate versions instead of in one jump.
#[derive(Default)]
pub struct Blobs<'a> {
    data: HashMap<String, &'a [u8]>,
    links: HashMap<String, Vec<String>>,
}

impl<'a> Blobs<'a> {
    pub fn add(&mut self, bytes: &'a [u8]) {
        self.data.entry(digest(bytes)).or_insert(bytes);
    }

    /// Adds bytes whose digest is already known (the bundle names its blobs
    /// by digest), without hashing them again.
    pub fn add_known(&mut self, digest: String, bytes: &'a [u8]) {
        self.data.entry(digest).or_insert(bytes);
    }

    /// Records that a revision turned the file version `old` into `new`.
    pub fn link(&mut self, old: &str, new: &str) {
        self.links
            .entry(old.to_string())
            .or_default()
            .push(new.to_string());
        self.links
            .entry(new.to_string())
            .or_default()
            .push(old.to_string());
    }

    /// The text with this digest, if held and valid UTF-8.
    pub fn text(&self, digest: &str) -> Option<&'a str> {
        std::str::from_utf8(self.data.get(digest)?).ok()
    }

    /// The texts along the shortest path of linked versions from `from` to
    /// `to`, both ends included, going only through versions whose text is
    /// held. With no such path but both ends held, just those two (a direct
    /// comparison).
    pub fn chain(&self, from: &str, to: &str) -> Option<Vec<&'a str>> {
        let (start, goal) = (self.text(from)?, self.text(to)?);
        let mut previous: HashMap<&str, &str> = HashMap::new();
        let mut queue = std::collections::VecDeque::from([from]);
        let mut seen: std::collections::HashSet<&str> = [from].into();
        while let Some(at) = queue.pop_front() {
            if at == to {
                let mut path = vec![goal];
                let mut node = at;
                while let Some(&before) = previous.get(node) {
                    path.push(self.text(before)?);
                    node = before;
                }
                path.reverse();
                return Some(path);
            }
            for next in self.links.get(at).into_iter().flatten() {
                if self.text(next).is_some() && seen.insert(next) {
                    previous.insert(next, at);
                    queue.push_back(next);
                }
            }
        }
        Some(vec![start, goal])
    }
}
