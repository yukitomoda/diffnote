//! Content digests (`sha256:<hex>`), the one form used everywhere a digest
//! is stored or compared.

use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub fn digest(bytes: impl AsRef<[u8]>) -> String {
    let hash = Sha256::digest(bytes.as_ref());
    format!("sha256:{hash:x}")
}

/// Two file versions, by digest: where a step or path starts and ends.
type VersionPair = (String, String);

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
    /// The steps revisions took (old -> new).
    forward: HashMap<String, Vec<String>>,
    // What has been worked out already, so a review with hundreds of threads
    // doesn't redo it for each: whether a version is valid UTF-8, the path of
    // steps between two versions, and the line diff of a step.
    texts: RefCell<HashMap<String, Option<&'a str>>>,
    paths: RefCell<HashMap<VersionPair, Option<Rc<Vec<String>>>>>,
    steps: RefCell<HashMap<VersionPair, Rc<Vec<similar::DiffOp>>>>,
}

impl<'a> Blobs<'a> {
    fn forget(&mut self) {
        self.texts.get_mut().clear();
        self.paths.get_mut().clear();
        self.steps.get_mut().clear();
    }

    pub fn add(&mut self, bytes: &'a [u8]) {
        self.forget();
        self.data.entry(digest(bytes)).or_insert(bytes);
    }

    /// Adds bytes whose digest is already known (the bundle names its blobs
    /// by digest), without hashing them again.
    pub fn add_known(&mut self, digest: String, bytes: &'a [u8]) {
        self.forget();
        self.data.entry(digest).or_insert(bytes);
    }

    /// Records that a revision turned the file version `old` into `new`.
    pub fn link(&mut self, old: &str, new: &str) {
        self.forget();
        self.forward
            .entry(old.to_string())
            .or_default()
            .push(new.to_string());
    }

    /// Whether recorded revisions took the version `older` on to `newer`
    /// (through any number of steps), i.e. `older` came first.
    pub fn precedes(&self, older: &str, newer: &str) -> bool {
        let mut seen: std::collections::HashSet<&str> = [older].into();
        let mut queue = std::collections::VecDeque::from([older]);
        while let Some(at) = queue.pop_front() {
            if at == newer && at != older {
                return true;
            }
            for next in self.forward.get(at).into_iter().flatten() {
                if next == newer {
                    return next != older;
                }
                if seen.insert(next) {
                    queue.push_back(next);
                }
            }
        }
        false
    }

    /// The text with this digest, if held and valid UTF-8.
    pub fn text(&self, digest: &str) -> Option<&'a str> {
        if let Some(known) = self.texts.borrow().get(digest) {
            return *known;
        }
        // Validating a big file's UTF-8 is not free, and is asked for often.
        let text = self
            .data
            .get(digest)
            .and_then(|bytes| std::str::from_utf8(bytes).ok());
        self.texts.borrow_mut().insert(digest.to_string(), text);
        text
    }

    /// The digests along the shortest run of recorded steps between `from`
    /// and `to` (all in one direction), both ends included, going only
    /// through versions whose text is held. With no such run but both ends
    /// held, just those two (a direct comparison). `None` if either end isn't
    /// held.
    pub fn path(&self, from: &str, to: &str) -> Option<Rc<Vec<String>>> {
        let key = (from.to_string(), to.to_string());
        if let Some(known) = self.paths.borrow().get(&key) {
            return known.clone();
        }
        let found = self.find_path(from, to).map(Rc::new);
        self.paths.borrow_mut().insert(key, found.clone());
        found
    }

    fn find_path(&self, from: &str, to: &str) -> Option<Vec<String>> {
        self.text(from)?;
        self.text(to)?;
        // Only along steps some revision really took, all one way: forward if
        // `from` came first, backward if `to` did. Going back to a common
        // base and forward again is not history (what one side added would
        // look deleted on the way back), so two versions that only share a
        // base are compared directly.
        if let Some(path) = self.forward_path(from, to) {
            return Some(path);
        }
        if let Some(mut path) = self.forward_path(to, from) {
            path.reverse();
            return Some(path);
        }
        Some(vec![from.to_string(), to.to_string()])
    }

    /// The shortest run of recorded steps from `from` to `to`, through
    /// versions whose text is held.
    fn forward_path(&self, from: &str, to: &str) -> Option<Vec<String>> {
        if from == to {
            return Some(vec![from.to_string()]);
        }
        let mut previous: HashMap<&str, &str> = HashMap::new();
        let mut queue = std::collections::VecDeque::from([from]);
        let mut seen: std::collections::HashSet<&str> = [from].into();
        while let Some(at) = queue.pop_front() {
            if at == to {
                let mut path = vec![at.to_string()];
                let mut node = at;
                while let Some(&before) = previous.get(node) {
                    path.push(before.to_string());
                    node = before;
                }
                path.reverse();
                return Some(path);
            }
            for next in self.forward.get(at).into_iter().flatten() {
                if self.text(next).is_some() && seen.insert(next) {
                    previous.insert(next, at);
                    queue.push_back(next);
                }
            }
        }
        None
    }

    /// The texts along [`Blobs::path`].
    pub fn chain(&self, from: &str, to: &str) -> Option<Vec<&'a str>> {
        self.path(from, to)?.iter().map(|d| self.text(d)).collect()
    }

    /// The line diff turning the version `from` into the version `to`, worked
    /// out once however often it is asked for.
    pub fn steps(&self, from: &str, to: &str) -> Option<Rc<Vec<similar::DiffOp>>> {
        let key = (from.to_string(), to.to_string());
        if let Some(known) = self.steps.borrow().get(&key) {
            return Some(known.clone());
        }
        let (old, new) = (self.text(from)?, self.text(to)?);
        let ops = Rc::new(crate::linediff::line_diff(old, new));
        self.steps.borrow_mut().insert(key, ops.clone());
        Some(ops)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blobs() -> Blobs<'static> {
        // v1 -> v2 -> v3, and an unrelated w1 -> w2; v2 -> x (a branch).
        let mut b = Blobs::default();
        for (a, c) in [("v1", "v2"), ("v2", "v3"), ("w1", "w2"), ("v2", "x")] {
            b.link(a, c);
        }
        b
    }

    #[test]
    fn precedes_follows_recorded_steps_forward_only() {
        let b = blobs();
        assert!(b.precedes("v1", "v2"));
        assert!(b.precedes("v1", "v3"), "through v2");
        assert!(b.precedes("v1", "x"), "through a branch");
        assert!(!b.precedes("v3", "v1"), "not backwards");
        assert!(!b.precedes("v3", "x"), "siblings don't precede each other");
        assert!(!b.precedes("v1", "w2"), "unrelated");
        assert!(!b.precedes("v1", "v1"), "not itself");
        assert!(!b.precedes("nope", "v1"));
    }

    #[test]
    fn precedes_terminates_on_a_cycle() {
        let mut b = Blobs::default();
        b.link("a", "b");
        b.link("b", "a");
        assert!(b.precedes("a", "b"));
        assert!(!b.precedes("a", "c"));
    }

    fn held<'a>(links: &[(&str, &str)], names: &[&'a str]) -> Blobs<'a> {
        let mut b = Blobs::default();
        for n in names {
            b.add_known(n.to_string(), n.as_bytes());
        }
        for (a, c) in links {
            b.link(a, c);
        }
        b
    }

    fn path(b: &Blobs, from: &str, to: &str) -> Vec<String> {
        b.path(from, to).unwrap().to_vec()
    }

    #[test]
    fn a_path_follows_recorded_steps_forward_and_backward() {
        let b = held(&[("v1", "v2"), ("v2", "v3")], &["v1", "v2", "v3"]);
        assert_eq!(path(&b, "v1", "v3"), ["v1", "v2", "v3"]);
        assert_eq!(path(&b, "v3", "v1"), ["v3", "v2", "v1"], "backward, one way");
        assert_eq!(path(&b, "v2", "v2"), ["v2"]);
    }

    #[test]
    fn versions_that_only_share_a_base_are_compared_directly() {
        // v1 -> v2 and v1 -> v3: neither came from the other. Going v2 -> v1
        // -> v3 would make what v2 added look deleted on the way back.
        let b = held(&[("v1", "v2"), ("v1", "v3")], &["v1", "v2", "v3"]);
        assert_eq!(path(&b, "v2", "v3"), ["v2", "v3"]);
        assert_eq!(path(&b, "v3", "v2"), ["v3", "v2"]);
        // ...and a version that did come from the other still goes by steps.
        assert_eq!(path(&b, "v1", "v3"), ["v1", "v3"]);
    }

    #[test]
    fn a_step_through_a_version_that_is_not_held_is_skipped() {
        let b = held(&[("v1", "v2"), ("v2", "v3")], &["v1", "v3"]);
        assert_eq!(path(&b, "v1", "v3"), ["v1", "v3"], "v2's text is gone");
    }

    #[test]
    fn a_path_needs_both_ends_held() {
        let b = held(&[("v1", "v2")], &["v1"]);
        assert!(b.path("v1", "v2").is_none());
        assert!(b.path("v9", "v1").is_none());
    }
}
