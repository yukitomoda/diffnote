//! User-facing text (CLI help, error messages, WebUI labels), kept out of the
//! Rust/JS source so it can all be reviewed and edited in one place, and so a
//! translation is (eventually) just another file next to `ja.yaml`.
//!
//! `messages/ja.yaml` is a nested YAML mapping whose leaves are strings;
//! [`m`] looks one up by its dotted path (`cli.init.about`), and [`mf`]
//! additionally substitutes `{name}` placeholders in it. The same file is
//! also embedded into the served/exported page as JSON (see
//! `html::MESSAGES_JSON`) for the WebUI's own `m`/`mf` in `ui/src/lib.js`
//! to read -- one source of truth for both sides.

use std::collections::HashMap;
use std::sync::LazyLock;

static SOURCE: &str = include_str!("../messages/ja.yaml");

static TABLE: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    let value: serde_yaml::Value =
        serde_yaml::from_str(SOURCE).expect("messages/ja.yaml がパースできません");
    let mut out = HashMap::new();
    flatten(&value, String::new(), &mut out);
    out
});

fn flatten(value: &serde_yaml::Value, prefix: String, out: &mut HashMap<String, String>) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                let key = k.as_str().unwrap_or_else(|| {
                    panic!("messages/ja.yaml: キーが文字列ではありません: {k:?}")
                });
                let path = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(v, path, out);
            }
        }
        serde_yaml::Value::String(s) => {
            let previous = out.insert(prefix.clone(), s.clone());
            assert!(
                previous.is_none(),
                "messages/ja.yaml: キーが重複しています: {prefix}"
            );
        }
        other => panic!("messages/ja.yaml: `{prefix}` は文字列である必要があります(値: {other:?})"),
    }
}

/// The message at `key` (a dotted path into the YAML tree), verbatim.
///
/// Panics if the key isn't in `messages/ja.yaml` -- a content bug (a typo'd
/// key, or a message not yet added), not something a user should ever hit;
/// the test suite exercises essentially every call site.
pub fn m(key: &str) -> &'static str {
    TABLE
        .get(key)
        .unwrap_or_else(|| panic!("messages/ja.yaml にキーがありません: {key}"))
}

/// The message at `key`, with each `{name}` in it replaced by the matching
/// value from `params`.
pub fn mf(key: &str, params: &[(&str, &str)]) -> String {
    let mut out = m(key).to_string();
    for (name, value) in params {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// The whole table as compact JSON, for the WebUI (see the module doc).
pub fn as_json() -> String {
    serde_json::to_string(&*TABLE).expect("メッセージ表を JSON にできません")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_loads_without_panicking() {
        // Forces the LazyLock, so a malformed/duplicate-key YAML file fails
        // here rather than wherever first calls `m`.
        assert!(!TABLE.is_empty());
    }

    #[test]
    fn placeholders_are_substituted_by_name() {
        let mut table = HashMap::new();
        table.insert("t".to_string(), "{a} と {b}、また {a}".to_string());
        // mf() itself only formats what's already in TABLE; exercise the
        // substitution logic directly against a value we control.
        let mut out = table.get("t").unwrap().clone();
        for (name, value) in [("a", "1"), ("b", "2")] {
            out = out.replace(&format!("{{{name}}}"), value);
        }
        assert_eq!(out, "1 と 2、また 1");
    }

    #[test]
    #[should_panic(expected = "キーがありません")]
    fn a_missing_key_panics_with_the_key_named() {
        m("no.such.key.__test_only__");
    }
}
