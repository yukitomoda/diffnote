//! `edit --show PATH[:START[-END]]`: asking for a file, or some lines of it,
//! to be put in the edit buffer as context so comments can be written on
//! them, whether or not the diff touches them.
//!
//! What is shown is read from the head of the review (the version the diff
//! leads to), so a comment written there is about that version of the file.
//! Nothing is recorded for the request itself: only comments are.

use crate::anchor::ViewVersions;
use crate::digest::Blobs;
use crate::expand::Want;
use crate::model::Side;

/// One `--show` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Show {
    /// Repo- or directory-relative, with `/` separators.
    pub path: String,
    /// Inclusive lines; `None` is the whole file.
    pub lines: Option<(u32, u32)>,
}

/// Reads `PATH`, `PATH:N` or `PATH:N-M`. A `:` that isn't followed by a line
/// range belongs to the path.
pub fn parse(spec: &str) -> Result<Show, String> {
    let (path, range) = match spec.rsplit_once(':') {
        Some((p, r)) if !r.is_empty() && r.chars().all(|c| c.is_ascii_digit() || c == '-') => {
            (p, Some(r))
        }
        _ => (spec, None),
    };
    let path = path.strip_prefix("./").unwrap_or(path);
    if path.is_empty() {
        return Err("no file name".to_string());
    }
    if path.starts_with('/') || path.split('/').any(|c| c == "..") {
        return Err(format!(
            "{path:?} isn't a path inside the reviewed tree (no absolute paths or `..`)"
        ));
    }
    let lines = match range {
        None => None,
        Some(r) => {
            let number = |s: &str| {
                s.parse::<u32>()
                    .map_err(|_| format!("{r:?} isn't a line or a range of lines"))
            };
            let (start, end) = match r.split_once('-') {
                Some((a, b)) => (number(a)?, number(b)?),
                None => (number(r)?, number(r)?),
            };
            if start == 0 {
                return Err("lines are numbered from 1".to_string());
            }
            if end < start {
                return Err(format!("{r:?} runs backwards"));
            }
            Some((start, end))
        }
    };
    Ok(Show {
        path: path.to_string(),
        lines,
    })
}

/// What each request asks the buffer to show, checked against the head of the
/// review: the file must have a text version there, and the first line must
/// exist (an end past the last line just means "to the end").
pub fn wants(shows: &[Show], view: &ViewVersions, blobs: &Blobs) -> Result<Vec<Want>, String> {
    let mut out = Vec::new();
    for show in shows {
        let Some(digest) = view.head(&show.path) else {
            let deleted = view
                .files
                .iter()
                .any(|f| f.old_path.as_deref() == Some(show.path.as_str()) && f.new.is_none());
            return Err(if deleted {
                format!("{}: this diff deletes it, so there is nothing at the head to show", show.path)
            } else {
                format!("{}: no such file at the head of this review", show.path)
            });
        };
        let Some(text) = blobs.text(digest) else {
            return Err(format!(
                "{}: not a text file (or its content isn't available)",
                show.path
            ));
        };
        let total = text.lines().count() as u32;
        if total == 0 {
            return Err(format!("{}: the file is empty, there is nothing to show", show.path));
        }
        let (start, end) = match show.lines {
            None => (1, total),
            Some((start, _)) if start > total => {
                return Err(format!(
                    "{}: the file has {total} line(s); line {start} is past the end",
                    show.path
                ));
            }
            Some((start, end)) => (start, end.min(total)),
        };
        out.push(Want {
            file: show.path.clone(),
            lines: Some((Side::New, start, end)),
            row: None,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::digest;
    use crate::model::{FileDigest, TreeFile};

    fn show(path: &str, lines: Option<(u32, u32)>) -> Show {
        Show {
            path: path.to_string(),
            lines,
        }
    }

    // ---- parsing -------------------------------------------------------------

    #[test]
    fn a_path_alone_is_the_whole_file() {
        assert_eq!(parse("README.md"), Ok(show("README.md", None)));
        assert_eq!(parse("src/lib.rs"), Ok(show("src/lib.rs", None)));
        assert_eq!(parse("./src/lib.rs"), Ok(show("src/lib.rs", None)), "a leading ./ goes");
    }

    #[test]
    fn a_line_or_a_range_follows_a_colon() {
        assert_eq!(parse("a.rs:7"), Ok(show("a.rs", Some((7, 7)))));
        assert_eq!(parse("a.rs:7-12"), Ok(show("a.rs", Some((7, 12)))));
        assert_eq!(parse("a.rs:3-3"), Ok(show("a.rs", Some((3, 3)))));
        assert_eq!(parse("dir/a.rs:1-999"), Ok(show("dir/a.rs", Some((1, 999)))));
    }

    #[test]
    fn a_colon_that_is_not_followed_by_lines_is_part_of_the_name() {
        assert_eq!(parse("notes: draft.md"), Ok(show("notes: draft.md", None)));
        assert_eq!(parse("a:b"), Ok(show("a:b", None)));
        assert_eq!(parse("weird:"), Ok(show("weird:", None)));
        // ...while a real range after such a name still counts.
        assert_eq!(parse("a:b.md:4"), Ok(show("a:b.md", Some((4, 4)))));
    }

    #[test]
    fn nonsense_is_refused_with_a_reason() {
        for (spec, why) in [
            ("", "no file name"),
            (":5", "no file name"),
            ("a.rs:0", "from 1"),
            ("a.rs:0-3", "from 1"),
            ("a.rs:9-3", "backwards"),
            ("a.rs:1-2-3", "isn't a line"),
            ("a.rs:-", "isn't a line"),
            ("a.rs:-5", "isn't a line"),
            ("/etc/passwd", "inside the reviewed tree"),
            ("../secret", "inside the reviewed tree"),
            ("a/../../b", "inside the reviewed tree"),
            ("a.rs:99999999999", "isn't a line"),
        ] {
            let err = parse(spec).unwrap_err();
            assert!(err.contains(why), "{spec:?}: {err}");
        }
    }

    // ---- against a review ---------------------------------------------------------

    struct Scene {
        files: Vec<FileDigest>,
        tree: Vec<TreeFile>,
        held: Vec<Vec<u8>>,
    }

    /// `calc.rs` changed (v1 -> v2), `README.md` untouched, `deleted.rs`
    /// removed, `logo.png` binary and untouched, `empty.txt` empty.
    fn scene() -> Scene {
        let calc = "a\nb\nc\nd\ne\n";
        let readme = "# t\nx\ny\n";
        let logo: Vec<u8> = vec![0, 159, 146, 150, 255];
        let tree_entry = |p: &str, bytes: &[u8]| TreeFile {
            path: p.to_string(),
            digest: digest(bytes),
        };
        Scene {
            files: vec![
                FileDigest {
                    old_path: Some("calc.rs".into()),
                    new_path: Some("calc.rs".into()),
                    old: Some(digest("old")),
                    new: Some(digest(calc)),
                },
                FileDigest {
                    old_path: Some("deleted.rs".into()),
                    new_path: None,
                    old: Some(digest("gone")),
                    new: None,
                },
            ],
            tree: vec![
                tree_entry("README.md", readme.as_bytes()),
                tree_entry("logo.png", &logo),
                tree_entry("empty.txt", b""),
                tree_entry("missing-blob.txt", b"never stored"),
            ],
            held: vec![calc.as_bytes().to_vec(), readme.as_bytes().to_vec(), logo, Vec::new()],
        }
    }

    fn check(specs: &[&str]) -> Result<Vec<Want>, String> {
        let sc = scene();
        let mut blobs = Blobs::default();
        for h in &sc.held {
            blobs.add(h);
        }
        let shows: Vec<Show> = specs.iter().map(|s| parse(s).unwrap()).collect();
        wants(
            &shows,
            &ViewVersions {
                files: &sc.files,
                tree: &sc.tree,
            },
            &blobs,
        )
    }

    fn lines_of(w: &Want) -> (u32, u32) {
        let (_, a, b) = w.lines.unwrap();
        (a, b)
    }

    #[test]
    fn a_whole_file_is_all_of_its_lines() {
        let w = check(&["README.md", "calc.rs"]).unwrap();
        assert_eq!((w[0].file.as_str(), lines_of(&w[0])), ("README.md", (1, 3)));
        assert_eq!((w[1].file.as_str(), lines_of(&w[1])), ("calc.rs", (1, 5)), "a touched file too");
        assert!(w.iter().all(|w| w.lines.unwrap().0 == Side::New && w.row.is_none()));
    }

    #[test]
    fn a_range_is_as_asked_and_an_end_past_the_file_means_to_the_end() {
        let w = check(&["README.md:2", "README.md:2-3", "README.md:2-99"]).unwrap();
        assert_eq!(lines_of(&w[0]), (2, 2));
        assert_eq!(lines_of(&w[1]), (2, 3));
        assert_eq!(lines_of(&w[2]), (2, 3));
    }

    #[test]
    fn a_start_past_the_end_is_an_error_that_says_how_long_the_file_is() {
        let err = check(&["README.md:4"]).unwrap_err();
        assert!(err.contains("README.md") && err.contains("3 line") && err.contains("line 4"), "{err}");
    }

    #[test]
    fn a_file_that_is_not_there_says_so_and_a_deleted_one_says_why() {
        let err = check(&["nope.txt"]).unwrap_err();
        assert!(err.contains("no such file"), "{err}");
        let err = check(&["deleted.rs"]).unwrap_err();
        assert!(err.contains("deletes it"), "{err}");
    }

    #[test]
    fn a_file_that_is_not_text_or_not_kept_or_empty_is_refused() {
        assert!(check(&["logo.png"]).unwrap_err().contains("not a text file"));
        assert!(check(&["missing-blob.txt"]).unwrap_err().contains("not a text file"));
        assert!(check(&["empty.txt"]).unwrap_err().contains("empty"));
    }

    #[test]
    fn the_first_bad_request_stops_the_rest() {
        assert!(check(&["README.md", "nope.txt", "calc.rs"]).is_err());
        assert!(check(&[]).unwrap().is_empty());
    }
}
