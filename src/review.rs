//! Reading a diffnote review's event log (JSON Lines, one [`Event`] per
//! line -- see `model` for the event/anchor schema). The event log itself
//! now always lives inside a `.diffnote` bundle (see the `bundle` module),
//! which is why there's no writer here anymore: [`parse_jsonl`] is the
//! shared core both [`load`] (a bare `.jsonl` file, kept around as a small
//! standalone utility) and `bundle::load` (a `review.jsonl` zip entry) use.
//!
//! Also builds the flat [`Event`] stream into [`Thread`]s (a root comment
//! plus its ordered replies, with `resolve`/`reopen`/`reanchor` folded in),
//! which both the HTML exporter and round-trip annotation rendering need.

use crate::messages::mf;
use crate::model::{Anchor, Event, Reaction, Reactions, Settings, Viewed};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use time::OffsetDateTime;
use ulid::Ulid;

#[derive(Debug, Clone)]
pub struct Thread {
    pub root_id: Ulid,
    pub author: String,
    pub created_at: OffsetDateTime,
    pub body: String,
    /// The thread's current anchor: the root comment's own anchor, or the
    /// most recent `Reanchor` event's anchor if any were applied.
    pub anchor: Anchor,
    /// Whether the most recent `resolve`/`reopen` event left this resolved.
    pub resolved: bool,
    pub replies: Vec<Reply>,
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub author: String,
    pub created_at: OffsetDateTime,
    pub body: String,
}

/// The review's title, if it has one.
pub fn title(settings: &Settings) -> Option<&str> {
    settings
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// Makes `wanted` the title (an empty one takes the title away); whether that
/// changed anything.
pub fn set_title(settings: &mut Settings, wanted: &str) -> bool {
    let wanted = wanted.trim();
    if title(settings).unwrap_or("") == wanted {
        return false;
    }
    settings.title = (!wanted.is_empty()).then(|| wanted.to_string());
    true
}

/// The repositories a review of several is of now, each from where it is
/// reviewed: what the settings say, else the first revision's. `None` for a
/// review of one repository or of a directory.
pub fn repos_of(loaded: &crate::bundle::Loaded) -> Option<Vec<crate::model::RepoSource>> {
    if let Some(repos) = &loaded.settings.repos {
        return Some(repos.clone());
    }
    loaded.revisions().next().and_then(|r| match &r.source {
        crate::model::Source::Workspace(w) => Some(w.repos.clone()),
        _ => None,
    })
}

/// Makes `repos` the review's repositories from the next revision on.
/// Earlier revisions, and the threads on them, stay as they are.
pub fn set_repos(settings: &mut Settings, mut repos: Vec<crate::model::RepoSource>) {
    repos.sort_by(|a, b| a.path.cmp(&b.path));
    settings.repos = Some(repos);
}

/// Makes ignoring white space `wanted`; whether that changed anything.
pub fn set_ignore_whitespace(settings: &mut Settings, wanted: bool) -> bool {
    if settings.ignore_whitespace == wanted {
        return false;
    }
    settings.ignore_whitespace = wanted;
    true
}

/// The files the review leaves out of what it shows (`settings.ignore`),
/// as a matcher: `None` if it names none. A line that isn't a pattern is
/// passed over here (the settings screen refuses one: see [`ignore_error`]).
pub fn ignore_matcher(settings: &Settings) -> Option<ignore::gitignore::Gitignore> {
    if settings.ignore.trim().is_empty() {
        return None;
    }
    let mut builder = ignore::gitignore::GitignoreBuilder::new("");
    for line in settings.ignore.lines() {
        let _ = builder.add_line(None, line);
    }
    builder.build().ok().filter(|g| !g.is_empty())
}

/// Whether `path` is one the review leaves out.
pub fn is_ignored(matcher: &ignore::gitignore::Gitignore, path: &str) -> bool {
    matcher.matched_path_or_any_parents(path, false).is_ignore()
}

/// A list of files to leave out as it is kept: as written, but for the empty
/// lines at its end. Only those: the last line keeps what it ends with, as a
/// space written `\ ` (a name that ends in one) is part of the pattern, and
/// taken off it would leave a `\` with nothing after it -- no pattern at all.
pub fn tidy_ignore(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// The first line of `text` that is not a pattern `.gitignore` can take, and
/// why (for the settings screen to say).
pub fn ignore_error(text: &str) -> Option<(usize, String)> {
    let mut builder = ignore::gitignore::GitignoreBuilder::new("");
    for (i, line) in text.lines().enumerate() {
        if let Err(e) = builder.add_line(None, line) {
            return Some((i + 1, e.to_string()));
        }
    }
    None
}

/// `author` reacting to a comment with `emoji`: taken back if it was there,
/// added if not. Whether the reaction is there now.
pub fn toggle_reaction(
    reactions: &mut Reactions,
    comment: &str,
    emoji: &str,
    author: &str,
) -> bool {
    let list = reactions.entry(comment.to_string()).or_default();
    let now;
    match list.iter().position(|r| r.emoji == emoji) {
        Some(i) => match list[i].authors.iter().position(|a| a == author) {
            Some(at) => {
                list[i].authors.remove(at);
                if list[i].authors.is_empty() {
                    list.remove(i);
                }
                now = false;
            }
            None => {
                list[i].authors.push(author.to_string());
                now = true;
            }
        },
        None => {
            list.push(Reaction {
                emoji: emoji.to_string(),
                authors: vec![author.to_string()],
            });
            now = true;
        }
    }
    if list.is_empty() {
        reactions.remove(comment);
    }
    now
}

/// `author` marking a file of a revision as looked at (`sig`: what the file
/// was then), or taking the mark back (`None`). `order` is the revisions'
/// ids, oldest first. Taken back, the file is said not to be looked at (so
/// that a mark in an earlier revision doesn't reach it), unless no earlier
/// revision has a mark of it: then there is nothing to say. Nothing empty is
/// kept.
pub fn set_viewed(
    viewed: &mut Viewed,
    order: &[String],
    revision: &str,
    author: &str,
    path: &str,
    sig: Option<&str>,
) {
    let earlier = order.iter().take_while(|id| *id != revision).any(|id| {
        viewed
            .get(id)
            .and_then(|by| by.get(author))
            .is_some_and(|marks| marks.contains_key(path))
    });
    let by = viewed.entry(revision.to_string()).or_default();
    let marks = by.entry(author.to_string()).or_default();
    match sig {
        Some(sig) => {
            marks.insert(path.to_string(), Some(sig.to_string()));
        }
        None if earlier => {
            marks.insert(path.to_string(), None);
        }
        None => {
            marks.remove(path);
        }
    }
    if marks.is_empty() {
        by.remove(author);
    }
    if by.is_empty() {
        viewed.remove(revision);
    }
}

/// Groups a flat event stream into threads, in the order their root
/// comment first appeared. Replies/resolve/reopen/reanchor events for a
/// thread that was never actually created (a malformed file) are silently
/// dropped rather than erroring -- this is a read path used for display.
pub fn build_threads(events: &[Event]) -> Vec<Thread> {
    let mut order: Vec<Ulid> = Vec::new();
    let mut map: HashMap<Ulid, Thread> = HashMap::new();

    for event in events {
        match event {
            Event::Comment {
                id,
                parent: None,
                author,
                created_at,
                body,
                anchor: Some(a),
                ..
            } => {
                order.push(*id);
                map.insert(
                    *id,
                    Thread {
                        root_id: *id,
                        author: author.clone(),
                        created_at: *created_at,
                        body: body.clone(),
                        anchor: a.clone(),
                        resolved: false,
                        replies: Vec::new(),
                    },
                );
            }
            Event::Comment {
                parent: Some(p),
                author,
                created_at,
                body,
                ..
            } => {
                if let Some(t) = map.get_mut(p) {
                    t.replies.push(Reply {
                        author: author.clone(),
                        created_at: *created_at,
                        body: body.clone(),
                    });
                }
            }
            Event::Resolve { parent, .. } => {
                if let Some(t) = map.get_mut(parent) {
                    t.resolved = true;
                }
            }
            Event::Reopen { parent, .. } => {
                if let Some(t) = map.get_mut(parent) {
                    t.resolved = false;
                }
            }
            Event::Reanchor { parent, anchor, .. } => {
                if let Some(t) = map.get_mut(parent) {
                    t.anchor = anchor.clone();
                }
            }
            _ => {}
        }
    }

    order.into_iter().filter_map(|id| map.remove(&id)).collect()
}

/// Parses JSON-Lines event text (blank lines skipped). `source_label`
/// prefixes error messages (a file path, or a zip entry name).
pub fn parse_jsonl(text: &str, source_label: &str) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event: Event = serde_json::from_str(line).with_context(|| {
            mf(
                "review.bad_event",
                &[("source", source_label), ("line", &(i + 1).to_string())],
            )
        })?;
        events.push(event);
    }
    Ok(events)
}

pub fn load(path: &Path) -> Result<Vec<Event>> {
    let text = std::fs::read_to_string(path).with_context(|| {
        mf(
            "review.open_failed",
            &[("path", &path.display().to_string())],
        )
    })?;
    parse_jsonl(&text, &path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repositories_are_what_the_settings_say_else_the_first_revisions() {
        use crate::model::{GitSource, RepoSource, Revision, Source, WorkspaceSource};
        let repo = |path: &str| RepoSource {
            path: path.to_string(),
            range: GitSource {
                base: "b".into(),
                head: "h".into(),
                spec: "b..h".into(),
            },
            target: None,
        };
        let mut loaded = crate::bundle::empty();
        assert_eq!(repos_of(&loaded), None, "nothing yet");
        loaded.events.push(Event::Revision(Revision {
            id: ulid::Ulid::new(),
            created_at: time::OffsetDateTime::now_utc(),
            digest: "d".into(),
            source: Source::Workspace(WorkspaceSource {
                repos: vec![repo("a"), repo("m")],
            }),
            snapshot_mode: crate::model::SnapshotMode::Changed,
            files: Vec::new(),
            tree: Vec::new(),
            commits: Vec::new(),
        }));
        let paths = |l: &crate::bundle::Loaded| {
            repos_of(l).map(|r| r.iter().map(|r| r.path.clone()).collect::<Vec<_>>())
        };
        assert_eq!(paths(&loaded), Some(vec!["a".to_string(), "m".to_string()]));
        // Changed on the page: the settings say, sorted, whatever the first
        // revision was.
        set_repos(
            &mut loaded.settings,
            vec![repo("m"), repo("deep/z"), repo("a")],
        );
        assert_eq!(
            paths(&loaded),
            Some(vec!["a".to_string(), "deep/z".to_string(), "m".to_string()])
        );
        set_repos(&mut loaded.settings, Vec::new());
        assert_eq!(
            paths(&loaded),
            Some(Vec::new()),
            "none left is still an answer"
        );
    }

    #[test]
    fn a_list_of_files_to_leave_out_is_read_as_a_gitignore_is() {
        // Each: the list, then paths of the review and whether each is left out.
        let cases: &[(&str, &[(&str, bool)])] = &[
            // `/` in front: from the top of the review only.
            (
                "/foo",
                &[
                    ("foo", true),
                    ("foo/a.rs", true),
                    ("x/foo", false),
                    ("x/foo/a.rs", false),
                ],
            ),
            // No `/`: a file or a directory of that name, however deep.
            (
                "foo",
                &[
                    ("foo", true),
                    ("foo/a.rs", true),
                    ("x/foo", true),
                    ("x/foo/a.rs", true),
                ],
            ),
            // `/` at the end: a directory only, however deep.
            (
                "foo/",
                &[
                    ("foo", false),
                    ("foo/a.rs", true),
                    ("x/foo", false),
                    ("x/foo/a.rs", true),
                ],
            ),
            (
                "/foo/",
                &[("foo", false), ("foo/a.rs", true), ("x/foo/a.rs", false)],
            ),
            // A `/` in the middle: from the top, as if it began with one.
            (
                "src/gen",
                &[("src/gen/a.rs", true), ("x/src/gen/a.rs", false)],
            ),
            // `*`: anything but a `/`.
            (
                "*.lock",
                &[
                    ("Cargo.lock", true),
                    ("a/b/yarn.lock", true),
                    ("lock", false),
                ],
            ),
            ("/*.lock", &[("Cargo.lock", true), ("a/yarn.lock", false)]),
            ("src/*.rs", &[("src/a.rs", true), ("src/x/a.rs", false)]),
            // `**`: across directories.
            (
                "src/**/*.rs",
                &[("src/a.rs", true), ("src/x/y/a.rs", true), ("a.rs", false)],
            ),
            ("**/gen", &[("gen/a", true), ("x/y/gen/a", true)]),
            ("a/**", &[("a/b", true), ("a/b/c", true), ("a", false)]),
            (
                "a/**/b",
                &[("a/b", true), ("a/x/y/b", true), ("a/x/c", false)],
            ),
            // `?`: one character, not a `/`; `[...]`: one of a set (or not, with `!`).
            (
                "a?.rs",
                &[("ab.rs", true), ("a/.rs", false), ("abc.rs", false)],
            ),
            (
                "[ab].rs",
                &[("a.rs", true), ("b.rs", true), ("c.rs", false)],
            ),
            ("[!a]*.rs", &[("a.rs", false), ("b.rs", true)]),
            // `!`: shown again. The line about the file itself is the one that
            // counts, even in a directory left out (where git would not show it).
            (
                "*.ts\n!src/auth/login.ts",
                &[("src/a.ts", true), ("src/auth/login.ts", false)],
            ),
            (
                "src/\n!src/a.ts",
                &[("src/a.ts", false), ("src/b.ts", true)],
            ),
            // The last line about a path wins.
            ("!*.md\n*.md", &[("a.md", true)]),
            // Case counts.
            ("Foo.rs", &[("Foo.rs", true), ("foo.rs", false)]),
            // `#` begins a comment, `\#` and `\!` a name; spaces at the end of a
            // line don't count, but one written `\ ` does.
            (
                "\\#x\n#y\n\\!z",
                &[("#x", true), ("#y", false), ("y", false), ("!z", true)],
            ),
            ("foo   ", &[("foo", true)]),
            ("/ends\\ ", &[("ends ", true), ("ends", false)]),
        ];
        for (text, paths) in cases {
            let settings = Settings {
                ignore: text.to_string(),
                ..Settings::default()
            };
            let m = ignore_matcher(&settings).unwrap();
            for (path, out) in *paths {
                assert_eq!(is_ignored(&m, path), *out, "{text:?} and {path:?}");
            }
        }
    }

    #[test]
    fn a_list_is_kept_without_the_empty_lines_at_its_end_and_nothing_else() {
        assert_eq!(tidy_ignore("*.lock\n\n  \n"), "*.lock");
        assert_eq!(tidy_ignore("a\r\n\r\nb\r\n"), "a\n\nb");
        // A space written `\ ` at the very end is part of the pattern: taken
        // off, it would leave a `\` that is no pattern at all.
        assert_eq!(tidy_ignore("/ends\\ "), "/ends\\ ");
        assert!(ignore_error(&tidy_ignore("/ends\\ \n")).is_none());
        assert_eq!(tidy_ignore("  \n"), "");
    }

    #[test]
    fn a_line_the_page_writes_for_a_file_names_that_file_and_no_other() {
        let settings = |text: &str| Settings {
            ignore: text.to_string(),
            ..Settings::default()
        };
        let m = ignore_matcher(&settings("/src/a.rs")).unwrap();
        assert!(is_ignored(&m, "src/a.rs"));
        assert!(!is_ignored(&m, "lib/src/a.rs"), "from the top only");
        // What would be a pattern, written as itself (as `lib.ignoreLine` does).
        let m = ignore_matcher(&settings(r"/a\*b\?\[c].txt")).unwrap();
        assert!(is_ignored(&m, "a*b?[c].txt"));
        assert!(!is_ignored(&m, "aXbYc.txt"));
        let m = ignore_matcher(&settings("/#x\n/!y\n/ends\\ ")).unwrap();
        assert!(is_ignored(&m, "#x") && is_ignored(&m, "!y") && is_ignored(&m, "ends "));
        // A directory takes what is under it; comments and blank lines are none.
        let m = ignore_matcher(&settings("# 生成物\n\ndist/")).unwrap();
        assert!(is_ignored(&m, "dist/a.js") && is_ignored(&m, "web/dist/b.js"));
        assert!(ignore_matcher(&settings("# only a comment\n")).is_none());
    }

    use crate::model::Anchor;
    use time::OffsetDateTime;
    use ulid::Ulid;

    #[test]
    fn load_reads_events_written_as_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.jsonl");

        let meta = Event::Meta {
            version: 1,
            created_at: OffsetDateTime::now_utc(),
            description: None,
            context_lines: 3,
        };
        let comment = Event::Comment {
            id: Ulid::new(),
            parent: None,
            author: "you@example.com".to_string(),
            created_at: OffsetDateTime::now_utc(),
            anchor: Some(Anchor::Global {
                base: None,
                head: None,
            }),
            body: "hello".to_string(),
        };
        let content = format!(
            "{}\n{}\n",
            serde_json::to_string(&meta).unwrap(),
            serde_json::to_string(&comment).unwrap()
        );
        std::fs::write(&path, content).unwrap();

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(matches!(loaded[0], Event::Meta { .. }));
        assert!(matches!(loaded[1], Event::Comment { .. }));
    }

    #[test]
    fn load_rejects_malformed_lines_with_a_line_number() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.jsonl");
        let valid_meta = Event::Meta {
            version: 1,
            created_at: OffsetDateTime::now_utc(),
            description: None,
            context_lines: 3,
        };
        let content = format!(
            "{}\nnot json at all\n",
            serde_json::to_string(&valid_meta).unwrap()
        );
        std::fs::write(&path, content).unwrap();
        let err = load(&path).unwrap_err();
        assert!(err.to_string().contains(":2:"));
    }

    #[test]
    fn the_title_is_a_setting_that_changes_only_when_it_would_be_another() {
        let mut settings = Settings::default();
        assert_eq!(title(&settings), None);
        assert!(!set_title(&mut settings, ""), "nothing to clear");
        assert!(!set_title(&mut settings, "  "));
        assert!(set_title(&mut settings, " new "));
        assert_eq!(title(&settings), Some("new"), "trimmed");
        assert!(!set_title(&mut settings, "new"), "the same title");
        assert!(set_title(&mut settings, "other"));
        assert!(set_title(&mut settings, ""), "clearing changes it");
        assert_eq!(title(&settings), None);
        assert_eq!(settings.title, None, "no empty string is kept");
    }

    #[test]
    fn ignoring_white_space_is_a_setting_that_changes_only_when_it_would_be_another() {
        let mut settings = Settings::default();
        assert!(!settings.ignore_whitespace);
        assert!(!set_ignore_whitespace(&mut settings, false));
        assert!(set_ignore_whitespace(&mut settings, true));
        assert!(settings.ignore_whitespace);
        assert!(!set_ignore_whitespace(&mut settings, true));
        assert!(set_ignore_whitespace(&mut settings, false));
    }

    #[test]
    fn a_file_is_marked_looked_at_per_revision_and_per_person_and_nothing_empty_is_kept() {
        let order: Vec<String> = ["r1", "r2", "r3"].map(String::from).to_vec();
        let mut viewed = Viewed::new();
        let set = |viewed: &mut Viewed, rev: &str, who: &str, path: &str, sig: Option<&str>| {
            set_viewed(viewed, &order, rev, who, path, sig);
        };
        set(&mut viewed, "r1", "a", "x.rs", Some("o|n"));
        set(&mut viewed, "r1", "b", "x.rs", Some("o|n"));
        set(&mut viewed, "r2", "a", "y.rs", Some("|n"));
        // Marked again (the file as another one): what it is now.
        set(&mut viewed, "r1", "a", "x.rs", Some("o|m"));
        // Taken back where an earlier revision has a mark: said so, so that
        // the earlier one doesn't reach it.
        set(&mut viewed, "r2", "a", "x.rs", None);
        // Taken back with no earlier mark: nothing to say.
        set(&mut viewed, "r1", "b", "x.rs", None);
        set(&mut viewed, "r2", "a", "y.rs", None);
        assert_eq!(viewed["r1"]["a"]["x.rs"].as_deref(), Some("o|m"));
        assert!(
            !viewed["r1"].contains_key("b"),
            "a person with no mark is gone"
        );
        assert_eq!(viewed["r2"]["a"]["x.rs"], None);
        assert!(!viewed["r2"]["a"].contains_key("y.rs"));
        set(&mut viewed, "r3", "a", "z.rs", None);
        assert!(
            !viewed.contains_key("r3"),
            "taking back what isn't there leaves nothing"
        );
        set(&mut viewed, "r1", "a", "x.rs", None);
        set(&mut viewed, "r2", "a", "x.rs", None);
        assert!(viewed.is_empty(), "nor is anything empty kept");
    }

    #[test]
    fn a_reaction_is_added_and_taken_back_by_the_same_person_and_nothing_empty_is_kept() {
        let mut reactions = Reactions::new();
        assert!(toggle_reaction(&mut reactions, "c1", "👍", "a"));
        assert!(toggle_reaction(&mut reactions, "c1", "🎉", "a"));
        assert!(toggle_reaction(&mut reactions, "c1", "👍", "b"));
        let emoji: Vec<_> = reactions["c1"].iter().map(|r| r.emoji.as_str()).collect();
        assert_eq!(emoji, ["👍", "🎉"], "in the order first used");
        assert_eq!(reactions["c1"][0].authors, ["a", "b"]);
        // Again: taken back; one person's doesn't take another's.
        assert!(!toggle_reaction(&mut reactions, "c1", "👍", "a"));
        assert_eq!(reactions["c1"][0].authors, ["b"]);
        assert!(!toggle_reaction(&mut reactions, "c1", "👍", "b"));
        assert_eq!(reactions["c1"].len(), 1, "an emoji nobody has is gone");
        assert!(!toggle_reaction(&mut reactions, "c1", "🎉", "a"));
        assert!(reactions.is_empty(), "so is a comment that has none");
    }
}
