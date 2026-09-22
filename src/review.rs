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
use crate::model::{Anchor, Event, Reaction, Reactions, Settings};
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

/// Makes ignoring white space `wanted`; whether that changed anything.
pub fn set_ignore_whitespace(settings: &mut Settings, wanted: bool) -> bool {
    if settings.ignore_whitespace == wanted {
        return false;
    }
    settings.ignore_whitespace = wanted;
    true
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
