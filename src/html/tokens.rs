//! Code as tokens: a line is a list of pieces of text, each with the kind of
//! thing it is (`keyword`, `string`, ...) or none. The colors are the page's
//! (`.tok-*` in `ui/style.css`), not the data's.
//!
//! In JSON a piece with no kind is just its text, and one with a kind is
//! `[kind, text]`.

use super::*;
use serde::Serialize;
use syntect::easy::ScopeRegionIterator;
use syntect::parsing::{ParseState, ScopeStack};

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(untagged)]
pub enum Token {
    Plain(String),
    Kind(&'static str, String),
}

impl Token {
    fn new(kind: Option<&'static str>, text: &str) -> Token {
        match kind {
            Some(kind) => Token::Kind(kind, text.to_string()),
            None => Token::Plain(text.to_string()),
        }
    }

    fn kind(&self) -> Option<&'static str> {
        match self {
            Token::Plain(_) => None,
            Token::Kind(kind, _) => Some(kind),
        }
    }

    fn text_mut(&mut self) -> &mut String {
        match self {
            Token::Plain(t) | Token::Kind(_, t) => t,
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Token::Plain(t) | Token::Kind(_, t) => t,
        }
    }
}

/// Reads the lines of one hunk (or file) in order: what a line means can
/// depend on the lines before it (a comment that goes on).
pub struct Tokenizer<'a> {
    syntax_set: &'a SyntaxSet,
    state: ParseState,
    stack: ScopeStack,
}

impl<'a> Tokenizer<'a> {
    pub fn new(syntax: &SyntaxReference, syntax_set: &'a SyntaxSet) -> Self {
        Tokenizer {
            syntax_set,
            state: ParseState::new(syntax),
            stack: ScopeStack::new(),
        }
    }

    pub fn line(&mut self, content: &str) -> Vec<Token> {
        // The parser wants the newline to read line comments right.
        let mut line = content.to_string();
        line.push('\n');
        let Ok(ops) = self.state.parse_line(&line, self.syntax_set) else {
            return vec![Token::Plain(content.to_string())];
        };
        let mut out: Vec<Token> = Vec::new();
        for (text, op) in ScopeRegionIterator::new(&ops, &line) {
            if self.stack.apply(op).is_err() {
                return vec![Token::Plain(content.to_string())];
            }
            let text = text.trim_end_matches('\n');
            if text.is_empty() {
                continue;
            }
            let kind = self.kind();
            match out.last_mut() {
                Some(last) if last.kind() == kind => last.text_mut().push_str(text),
                _ => out.push(Token::new(kind, text)),
            }
        }
        if out.is_empty() && !content.is_empty() {
            out.push(Token::Plain(content.to_string()));
        }
        out
    }

    fn kind(&self) -> Option<&'static str> {
        let scopes: Vec<String> = self
            .stack
            .as_slice()
            .iter()
            .map(|s| s.build_string())
            .collect();
        kind_of(&scopes)
    }
}

/// Whether `scope` is `prefix` or something under it (`string` covers
/// `string.quoted`, not `stringy`).
fn under(scope: &str, prefix: &str) -> bool {
    scope
        .strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// The kind of what the scopes (outermost first) say something is: that of the
/// innermost scope that has one (a key inside a string-like scalar is a key),
/// except that whatever is inside a comment is part of it.
fn kind_of(scopes: &[String]) -> Option<&'static str> {
    if scopes.iter().any(|s| under(s, "comment")) {
        return Some("comment");
    }
    scopes.iter().rev().find_map(|s| kind_of_one(s))
}

fn kind_of_one(s: &str) -> Option<&'static str> {
    let is = |prefix: &str| under(s, prefix);
    Some(if is("string") {
        "string"
    } else if is("constant.numeric") {
        "number"
    } else if is("constant") {
        "constant"
    } else if is("keyword.operator") {
        "operator"
    } else if is("keyword") || is("storage") {
        "keyword"
    } else if is("entity.name.function") || is("support.function") {
        "function"
    } else if is("entity.name.tag") {
        "tag"
    } else if is("entity.other.attribute-name") {
        "attribute"
    } else if is("entity.name.section") || is("markup.heading") {
        "heading"
    } else if is("entity.name")
        || is("support.type")
        || is("support.class")
        || is("entity.other.inherited-class")
    {
        "type"
    } else if is("variable.parameter") || is("variable.language") {
        "variable"
    } else if is("markup.bold") {
        "strong"
    } else if is("markup.italic") {
        "emphasis"
    } else if is("markup.raw") || is("markup.inline.raw") {
        "raw"
    } else if is("markup.underline.link") {
        "link"
    } else {
        return None;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(file: &str, text: &str) -> Vec<Vec<(Option<&'static str>, String)>> {
        let syntax_set = &*SYNTAXES;
        let mut t = Tokenizer::new(guess_syntax(file, syntax_set), syntax_set);
        text.lines()
            .map(|l| {
                t.line(l)
                    .into_iter()
                    .map(|k| (k.kind(), k.text().to_string()))
                    .collect()
            })
            .collect()
    }

    fn has(line: &[(Option<&'static str>, String)], kind: &str, text: &str) -> bool {
        line.iter().any(|(k, t)| *k == Some(kind) && t == text)
    }

    #[test]
    fn code_is_pieces_of_text_with_the_kind_of_each() {
        let out = lines("a.rs", "let n = 42; // note");
        let line = &out[0];
        assert!(has(line, "keyword", "let"), "{line:?}");
        assert!(has(line, "number", "42"), "{line:?}");
        assert!(
            line.iter()
                .any(|(k, t)| *k == Some("comment") && t.contains("note"))
        );
        // Nothing is lost or added: the pieces are the line.
        let whole: String = line.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(whole, "let n = 42; // note");
    }

    #[test]
    fn a_string_with_its_quotes_is_one_piece_and_neighbours_of_one_kind_join() {
        let out = lines("a.js", "const s = 'a b'");
        assert!(has(&out[0], "string", "'a b'"), "{:?}", out[0]);
        let plain: Vec<_> = out[0].iter().filter(|(k, _)| k.is_none()).collect();
        assert!(!plain.is_empty());
    }

    #[test]
    fn a_comment_that_goes_on_is_a_comment_on_the_next_line_too() {
        let out = lines("a.rs", "/* one\n   two */\nlet x = 1;");
        assert!(
            out[1].iter().all(|(k, _)| *k == Some("comment")),
            "{:?}",
            out[1]
        );
        assert!(has(&out[2], "keyword", "let"));
    }

    #[test]
    fn text_with_no_grammar_is_plain_and_an_empty_line_is_no_pieces() {
        let out = lines("notes.unknownext", "just words\n\nmore");
        assert_eq!(out[0], vec![(None, "just words".to_string())]);
        assert!(out[1].is_empty());
    }

    #[test]
    fn what_is_written_in_json_is_text_for_plain_and_a_pair_for_a_kind() {
        let json = serde_json::to_string(&vec![
            Token::Plain("x ".into()),
            Token::Kind("keyword", "if".into()),
        ])
        .unwrap();
        assert_eq!(json, r#"["x ",["keyword","if"]]"#);
    }

    #[test]
    fn markdown_headings_and_code_are_told_apart() {
        let out = lines("a.md", "# Title\n\nuse `code` here");
        assert!(
            out[0].iter().any(|(k, _)| *k == Some("heading")),
            "{:?}",
            out[0]
        );
        assert!(
            out[2]
                .iter()
                .any(|(k, t)| *k == Some("raw") && t.contains("code")),
            "{:?}",
            out[2]
        );
    }

    #[test]
    fn typescript_dockerfile_and_yaml_have_grammars_and_a_key_is_not_a_string() {
        let out = lines("a.ts", "export const x: number = 1");
        assert!(has(&out[0], "keyword", "export"), "{:?}", out[0]);
        let out = lines("Dockerfile", "FROM node:20 AS base");
        assert!(has(&out[0], "keyword", "FROM"), "{:?}", out[0]);
        let out = lines("ci.yml", "name: 'x'\nwith:\n  push: true");
        assert!(
            has(&out[0], "tag", "name") && has(&out[0], "string", "'x'"),
            "{:?}",
            out[0]
        );
        assert!(
            has(&out[2], "tag", "push") && has(&out[2], "constant", "true"),
            "{:?}",
            out[2]
        );
    }
}
