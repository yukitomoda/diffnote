//! The file provider: reviews a plain directory (no version control), by
//! comparing its current content with the snapshot the bundle last recorded.
//!
//! A directory's content is read as a [`Tree`] (sorted `path -> bytes`,
//! `/`-separated, relative to the directory). Two trees are turned into the
//! same git-style unified diff the git provider yields, so everything
//! downstream (annotation, anchors, HTML) is shared between the two.

use crate::digest::digest;
use crate::messages::{m, mf};
use crate::model::FileDigest;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub type Tree = BTreeMap<String, Vec<u8>>;

/// Ignore file that, when present at the directory's root, replaces
/// `.gitignore` (same syntax).
pub const IGNORE_FILE: &str = ".diffnoteignore";

/// Reads every file under `root` that the ignore rules leave in.
///
/// If `root/.diffnoteignore` exists, only `.diffnoteignore` files (at any
/// depth) apply; otherwise `.gitignore` files apply, whether or not `root`
/// is a git repository. Global/`.git/info` excludes, parent directories'
/// ignore files and `.git` itself are never consulted, so the same
/// directory yields the same tree on every machine. `exclude` paths (the
/// bundle and its draft) are skipped.
pub fn read_tree(root: &Path, exclude: &[PathBuf]) -> Result<Tree> {
    let exclude: Vec<PathBuf> = exclude
        .iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect();
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(false)
        .require_git(false)
        .ignore(false)
        .parents(false)
        .git_global(false)
        .git_exclude(false)
        .filter_entry(|e| e.file_name() != ".git");
    if root.join(IGNORE_FILE).is_file() {
        builder
            .git_ignore(false)
            .add_custom_ignore_filename(IGNORE_FILE);
    } else {
        builder.git_ignore(true);
    }

    let mut tree = Tree::new();
    for entry in builder.build() {
        let entry = entry.context(m("files.walk_failed"))?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if std::fs::canonicalize(path).is_ok_and(|c| exclude.contains(&c)) {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let bytes = std::fs::read(path).with_context(|| {
            mf(
                "files.read_failed",
                &[("path", &path.display().to_string())],
            )
        })?;
        tree.insert(rel.to_string_lossy().replace('\\', "/"), bytes);
    }
    Ok(tree)
}

/// A digest identifying a whole tree (its paths and every file's content).
pub fn tree_digest(tree: &Tree) -> String {
    let mut manifest = String::new();
    for (path, bytes) in tree {
        manifest.push_str(&format!("{path}\0{}\n", digest(bytes)));
    }
    digest(manifest)
}

/// Git's own heuristic: a NUL byte in the first 8000 bytes, or text that
/// isn't valid UTF-8 (which the diff/annotation parsers couldn't carry).
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8000)].contains(&0) || std::str::from_utf8(bytes).is_err()
}

/// The unified diff turning `old` into `new`, plus the digests of every file
/// it touches. Files with identical content on both sides are left out.
/// Binary files appear as a `Binary files ... differ` entry only, so they
/// can carry file-level comments.
pub fn diff_trees(old: &Tree, new: &Tree) -> (String, Vec<FileDigest>) {
    let paths: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    let mut text = String::new();
    let mut files = Vec::new();

    for path in paths {
        let (before, after) = (old.get(path), new.get(path));
        if before == after {
            continue;
        }
        let a = format!("a/{path}");
        let b = format!("b/{path}");
        text.push_str(&format!("diff --git {a} {b}\n"));
        match (before, after) {
            (None, Some(_)) => text.push_str("new file mode 100644\n"),
            (Some(_), None) => text.push_str("deleted file mode 100644\n"),
            _ => {}
        }

        let binary = before.is_some_and(|x| is_binary(x)) || after.is_some_and(|x| is_binary(x));
        let old_name = before.map_or("/dev/null", |_| a.as_str());
        let new_name = after.map_or("/dev/null", |_| b.as_str());
        if binary {
            text.push_str(&format!("Binary files {old_name} and {new_name} differ\n"));
        } else {
            let old_text = before.map_or("", |x| std::str::from_utf8(x).unwrap_or(""));
            let new_text = after.map_or("", |x| std::str::from_utf8(x).unwrap_or(""));
            if !old_text.is_empty() || !new_text.is_empty() {
                text.push_str(
                    &similar::TextDiff::from_lines(old_text, new_text)
                        .unified_diff()
                        .context_radius(3)
                        .header(old_name, new_name)
                        .to_string(),
                );
            }
        }

        files.push(FileDigest {
            old_path: before.map(|_| path.clone()),
            new_path: after.map(|_| path.clone()),
            old: before.map(digest),
            new: after.map(digest),
        });
    }
    (text, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(entries: &[(&str, &[u8])]) -> Tree {
        entries
            .iter()
            .map(|(p, c)| (p.to_string(), c.to_vec()))
            .collect()
    }

    #[test]
    fn identical_trees_have_no_diff() {
        let t = tree(&[("a.txt", b"x\n")]);
        let (text, files) = diff_trees(&t, &t);
        assert!(text.is_empty());
        assert!(files.is_empty());
    }

    #[test]
    fn diff_covers_modified_added_and_deleted_files_and_parses_back() {
        let old = tree(&[
            ("keep.txt", b"k\n"),
            ("mod.txt", b"one\ntwo\nthree\n"),
            ("gone.txt", b"g\n"),
        ]);
        let new = tree(&[
            ("keep.txt", b"k\n"),
            ("mod.txt", b"one\nTWO\nthree\n"),
            ("new.txt", b"n\n"),
        ]);
        let (text, files) = diff_trees(&old, &new);

        let parsed = crate::diff::parse(&text).unwrap();
        let paths: Vec<_> = parsed
            .files
            .iter()
            .map(|f| (f.old_path.as_deref(), f.new_path.as_deref()))
            .collect();
        assert_eq!(
            paths,
            [
                (Some("gone.txt"), None),
                (Some("mod.txt"), Some("mod.txt")),
                (None, Some("new.txt")),
            ]
        );
        let mod_hunk = &parsed.files[1].hunks[0];
        assert!(mod_hunk.lines.iter().any(|l| l.content == "TWO"));

        assert_eq!(files.len(), 3);
        assert_eq!(files[1].old, Some(digest(b"one\ntwo\nthree\n")));
        assert_eq!(files[1].new, Some(digest(b"one\nTWO\nthree\n")));
        assert_eq!(files[0].new, None);
        assert_eq!(files[2].old, None);
    }

    #[test]
    fn binary_files_become_a_headers_only_entry() {
        let old = tree(&[("img.png", &[0, 1, 2])]);
        let new = tree(&[("img.png", &[0, 9, 9]), ("bad.txt", &[0xff, 0xfe])]);
        let (text, files) = diff_trees(&old, &new);
        let parsed = crate::diff::parse(&text).unwrap();
        assert_eq!(parsed.files.len(), 2);
        assert!(
            parsed
                .files
                .iter()
                .all(|f| f.is_binary && f.hunks.is_empty())
        );
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn missing_trailing_newline_is_marked() {
        let (text, _) = diff_trees(&tree(&[("a", b"x\n")]), &tree(&[("a", b"x")]));
        assert!(text.contains("\\ No newline at end of file"));
        crate::diff::parse(&text).unwrap();
    }

    #[test]
    fn empty_new_file_is_listed_without_hunks() {
        let (text, files) = diff_trees(&Tree::new(), &tree(&[("empty", b"")]));
        let parsed = crate::diff::parse(&text).unwrap();
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn tree_digest_depends_on_paths_and_content() {
        let a = tree(&[("a", b"1")]);
        assert_eq!(tree_digest(&a), tree_digest(&a.clone()));
        assert_ne!(tree_digest(&a), tree_digest(&tree(&[("a", b"2")])));
        assert_ne!(tree_digest(&a), tree_digest(&tree(&[("b", b"1")])));
    }

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn keys(t: &Tree) -> Vec<&str> {
        t.keys().map(String::as_str).collect()
    }

    #[test]
    fn gitignore_is_honored_without_a_git_repository() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), ".gitignore", "*.log\nbuild/\n");
        write(dir.path(), "a.txt", "a");
        write(dir.path(), "x.log", "log");
        write(dir.path(), "build/out", "o");
        write(dir.path(), ".git/config", "c");
        let t = read_tree(dir.path(), &[]).unwrap();
        assert_eq!(keys(&t), [".gitignore", "a.txt"]);
    }

    #[test]
    fn diffnoteignore_replaces_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), ".gitignore", "a.txt\n");
        write(dir.path(), ".diffnoteignore", "*.log\n");
        write(dir.path(), "a.txt", "a");
        write(dir.path(), "x.log", "log");
        write(dir.path(), "sub/y.log", "log");
        let t = read_tree(dir.path(), &[]).unwrap();
        // a.txt is kept (the .gitignore no longer applies); *.log is ignored.
        assert_eq!(keys(&t), [".diffnoteignore", ".gitignore", "a.txt"]);
    }

    #[test]
    fn excluded_paths_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", "a");
        write(dir.path(), ".diffnote", "bundle");
        let t = read_tree(dir.path(), &[dir.path().join(".diffnote")]).unwrap();
        assert_eq!(keys(&t), ["a.txt"]);
    }
}
