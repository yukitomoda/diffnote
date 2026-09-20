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

use crate::model::{Anchor, Event};
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

/// The review's title: that of the last `Title` event, if it isn't empty.
pub fn title(events: &[Event]) -> Option<&str> {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::Title { title, .. } => Some(title.trim()),
            _ => None,
        })
        .filter(|t| !t.is_empty())
}

/// The `Title` event that makes `wanted` the title, if it isn't already.
/// An empty `wanted` takes the title away.
pub fn title_change(events: &[Event], wanted: &str, author: &str) -> Option<Event> {
    let wanted = wanted.trim();
    if title(events).unwrap_or("") == wanted {
        return None;
    }
    Some(Event::Title {
        title: wanted.to_string(),
        author: author.to_string(),
        created_at: time::OffsetDateTime::now_utc(),
    })
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
        let event: Event = serde_json::from_str(line)
            .with_context(|| format!("{source_label}:{}: イベントの形式が不正です", i + 1))?;
        events.push(event);
    }
    Ok(events)
}

pub fn load(path: &Path) -> Result<Vec<Event>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("{} を開けませんでした", path.display()))?;
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

    fn title_event(title: &str) -> Event {
        Event::Title {
            title: title.to_string(),
            author: "a@example.com".to_string(),
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn the_title_is_the_last_non_empty_one() {
        assert_eq!(title(&[]), None);
        assert_eq!(title(&[title_event("a")]), Some("a"));
        assert_eq!(title(&[title_event("a"), title_event(" b ")]), Some("b"));
        assert_eq!(title(&[title_event("a"), title_event("")]), None, "cleared");
        assert_eq!(title(&[title_event("a"), title_event(""), title_event("c")]), Some("c"));
    }

    #[test]
    fn a_title_event_is_only_made_when_the_title_would_change() {
        let none: Vec<Event> = Vec::new();
        assert!(title_change(&none, "", "me").is_none(), "nothing to clear");
        assert!(title_change(&none, "  ", "me").is_none());
        let Some(Event::Title { title: t, author, .. }) = title_change(&none, " new ", "me") else {
            panic!("a title should be set");
        };
        assert_eq!((t.as_str(), author.as_str()), ("new", "me"));
        let has = [title_event("new")];
        assert!(title_change(&has, "new", "me").is_none(), "same title");
        assert!(title_change(&has, "other", "me").is_some());
        let Some(Event::Title { title: t, .. }) = title_change(&has, "", "me") else {
            panic!("clearing is an event too");
        };
        assert_eq!(t, "");
    }

    #[test]
    fn a_title_event_round_trips_as_jsonl() {
        let line = serde_json::to_string(&title_event("ログイン改修")).unwrap();
        assert!(line.contains(r#""kind":"title""#), "{line}");
        let back: Event = serde_json::from_str(&line).unwrap();
        assert_eq!(title(&[back]), Some("ログイン改修"));
    }
}
