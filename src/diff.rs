//! Parsing for **unified diff** text only (`git diff` / `diff -u` output).
//! Legacy context-diff format (`>`/`<` change markers) is intentionally
//! unsupported, since it would collide with the annotation format's own use
//! of `>` (see the `annotation` module).
//!
//! This parses bare diff text with no interleaved comments. The annotation
//! format layers `>`-prefixed lines on top of this same line structure, but
//! has its own parser in the `annotation` module.

use crate::messages::{m, mf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// One content line of a hunk. `old_line`/`new_line` are the 1-based line
/// numbers on each side, when that side has this line at all (a pure
/// addition has no `old_line`, a pure removal has no `new_line`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// Set when this line is immediately followed by a
    /// `\ No newline at end of file` marker in the source diff.
    pub no_newline_at_eof: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// The trailing `@@ ... @@ <heading>` text some diffs include (e.g. the
    /// enclosing function signature).
    pub section_heading: Option<String>,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub is_rename: bool,
    pub is_binary: bool,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnifiedDiff {
    pub files: Vec<FileDiff>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    Malformed { line: usize, message: String },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ParseError::Malformed { line, message } = self;
        write!(
            f,
            "{}",
            mf(
                "parse.error_line",
                &[("line", &line.to_string()), ("message", message)]
            )
        )
    }
}

impl std::error::Error for ParseError {}

impl ParseError {
    pub fn line(&self) -> usize {
        match self {
            ParseError::Malformed { line, .. } => *line,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            ParseError::Malformed { message, .. } => message,
        }
    }
}

fn err(line: usize, message: impl Into<String>) -> ParseError {
    ParseError::Malformed {
        line,
        message: message.into(),
    }
}

pub(crate) fn finish_hunk(file: &mut Option<FileDiff>, hunk: &mut Option<Hunk>) {
    if let Some(h) = hunk.take()
        && let Some(f) = file.as_mut()
    {
        f.hunks.push(h);
    }
}

pub(crate) fn finish_file(
    files: &mut Vec<FileDiff>,
    file: &mut Option<FileDiff>,
    hunk: &mut Option<Hunk>,
) {
    finish_hunk(file, hunk);
    if let Some(f) = file.take() {
        files.push(f);
    }
}

pub fn parse(text: &str) -> Result<UnifiedDiff, ParseError> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut current_file: Option<FileDiff> = None;
    let mut current_hunk: Option<Hunk> = None;
    let mut old_no: u32 = 0;
    let mut new_no: u32 = 0;

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;

        if let Some(rest) = raw_line.strip_prefix("diff --git ") {
            let _ = rest; // paths here are redundant with the following --- / +++ lines
            finish_file(&mut files, &mut current_file, &mut current_hunk);
            current_file = Some(FileDiff::default());
            continue;
        }

        if raw_line.starts_with("index ")
            || raw_line.starts_with("old mode ")
            || raw_line.starts_with("new mode ")
            || raw_line.starts_with("deleted file mode ")
            || raw_line.starts_with("new file mode ")
            || raw_line.starts_with("similarity index ")
        {
            continue;
        }

        if let Some(rest) = raw_line.strip_prefix("rename from ") {
            let file = current_file.get_or_insert_with(FileDiff::default);
            file.is_rename = true;
            file.old_path = Some(written_path(rest));
            continue;
        }
        if let Some(rest) = raw_line.strip_prefix("rename to ") {
            let file = current_file.get_or_insert_with(FileDiff::default);
            file.is_rename = true;
            file.new_path = Some(written_path(rest));
            continue;
        }

        if let Some((old_path, new_path)) = parse_binary_line(raw_line) {
            // A bare (header-less) diff has no `diff --git` line to start a
            // new file, so a finished file can't be reused here.
            let file_already_has_content = current_hunk.is_some()
                || current_file
                    .as_ref()
                    .is_some_and(|f| !f.hunks.is_empty() || f.is_binary);
            if file_already_has_content {
                finish_file(&mut files, &mut current_file, &mut current_hunk);
            }
            let file = current_file.get_or_insert_with(FileDiff::default);
            file.is_binary = true;
            file.old_path = old_path;
            file.new_path = new_path;
            continue;
        }

        if let Some(rest) = raw_line.strip_prefix("--- ") {
            // A bare `diff -u` file has no preceding `diff --git`, so a new
            // file section can start right here. `current_hunk` must be
            // checked too, not just `current_file.hunks` -- a hunk that's
            // still being accumulated (not yet flushed by finish_hunk)
            // wouldn't show up in `.hunks` yet, and missing it here would
            // silently merge two files' hunks into one (the second file's
            // "---"/"+++" would just overwrite the first file's paths).
            let file_already_has_content = current_hunk.is_some()
                || current_file
                    .as_ref()
                    .is_some_and(|f| !f.hunks.is_empty() || f.is_binary);
            if current_file.is_none() || file_already_has_content {
                finish_file(&mut files, &mut current_file, &mut current_hunk);
                current_file = Some(FileDiff::default());
            }
            let file = current_file.as_mut().expect("just ensured");
            file.old_path = parse_path(rest);
            continue;
        }

        if let Some(rest) = raw_line.strip_prefix("+++ ") {
            let file = current_file
                .as_mut()
                .ok_or_else(|| err(line_no, m("diff.file_header_stray_plus")))?;
            file.new_path = parse_path(rest);
            continue;
        }

        if raw_line.starts_with("@@ ") || raw_line == "@@" {
            finish_hunk(&mut current_file, &mut current_hunk);
            if current_file.is_none() {
                return Err(err(line_no, m("diff.hunk_header_outside_file")));
            }
            let (old_start, old_lines, new_start, new_lines, section_heading) =
                parse_hunk_header(raw_line, line_no)?;
            old_no = old_start;
            new_no = new_start;
            current_hunk = Some(Hunk {
                old_start,
                old_lines,
                new_start,
                new_lines,
                section_heading,
                lines: Vec::new(),
            });
            continue;
        }

        if let Some(marker) = raw_line.strip_prefix('\\') {
            let _ = marker; // "\ No newline at end of file"
            let hunk = current_hunk
                .as_mut()
                .ok_or_else(|| err(line_no, m("diff.backslash_outside_hunk")))?;
            if let Some(last) = hunk.lines.last_mut() {
                last.no_newline_at_eof = true;
            }
            continue;
        }

        if raw_line.is_empty() {
            // A genuinely empty line never appears inside a real unified
            // diff hunk (every content line carries at least a 1-char
            // prefix), so treat it as the end of the diff content here.
            finish_hunk(&mut current_file, &mut current_hunk);
            continue;
        }

        // Split on the first *character*, not byte, so a content line that
        // happens to start with multi-byte UTF-8 (once we're past the
        // single-byte prefix check below) can never panic on a char
        // boundary.
        let mut chars = raw_line.chars();
        let prefix_char = chars.next().expect("checked not empty above");
        let content = chars.as_str();
        let hunk = current_hunk
            .as_mut()
            .ok_or_else(|| err(line_no, m("diff.content_outside_hunk")))?;
        let line = match prefix_char {
            ' ' => {
                let line = DiffLine {
                    kind: LineKind::Context,
                    content: content.to_string(),
                    old_line: Some(old_no),
                    new_line: Some(new_no),
                    no_newline_at_eof: false,
                };
                old_no += 1;
                new_no += 1;
                line
            }
            '+' => {
                let line = DiffLine {
                    kind: LineKind::Added,
                    content: content.to_string(),
                    old_line: None,
                    new_line: Some(new_no),
                    no_newline_at_eof: false,
                };
                new_no += 1;
                line
            }
            '-' => {
                let line = DiffLine {
                    kind: LineKind::Removed,
                    content: content.to_string(),
                    old_line: Some(old_no),
                    new_line: None,
                    no_newline_at_eof: false,
                };
                old_no += 1;
                line
            }
            other => {
                return Err(err(
                    line_no,
                    mf("diff.bad_line_prefix", &[("char", &format!("{other:?}"))]),
                ));
            }
        };
        hunk.lines.push(line);
    }

    finish_file(&mut files, &mut current_file, &mut current_hunk);
    Ok(UnifiedDiff { files })
}

/// The path of a `---`/`+++` line (or a side of a `Binary files` one), without
/// its `a/` or `b/`; `None` for `/dev/null`. Spaces are part of a name: git
/// ends one that has any with a tab (and may put a time after it), so what
/// comes before a tab is the name, as it is, spaces at either end and all.
pub(crate) fn parse_path(rest: &str) -> Option<String> {
    let path_part = if rest.starts_with('"') {
        written_path(rest)
    } else {
        rest.split('\t').next().unwrap_or(rest).to_string()
    };
    if path_part == "/dev/null" {
        None
    } else {
        let stripped = path_part
            .strip_prefix("a/")
            .or_else(|| path_part.strip_prefix("b/"))
            .unwrap_or(&path_part);
        Some(stripped.to_string())
    }
}

/// A path as git writes it in a diff: as it is, or, where it has a character
/// that would be read as something else (a `"`, a `\`, a tab, a line break,
/// another control character), in quotes with those written as C does.
/// (Other characters, Japanese ones too, are written as they are: diffnote
/// runs git with `core.quotepath` off.)
pub(crate) fn quote_path(path: &str) -> String {
    if !path
        .chars()
        .any(|c| c == '"' || c == '\\' || c.is_control())
    {
        return path.to_string();
    }
    let mut out = String::from("\"");
    for c in path.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => {
                let mut bytes = [0; 4];
                for b in c.encode_utf8(&mut bytes).bytes() {
                    out.push_str(&format!("\\{b:03o}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A path as git wrote it (see [`quote_path`]): in quotes, what they hold,
/// read as C writes it; else the text as it is.
fn written_path(text: &str) -> String {
    let Some(inner) = text.strip_prefix('"') else {
        return text.to_string();
    };
    let mut bytes = Vec::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('a') => bytes.push(0x07),
                Some('b') => bytes.push(0x08),
                Some('t') => bytes.push(b'\t'),
                Some('n') => bytes.push(b'\n'),
                Some('v') => bytes.push(0x0b),
                Some('f') => bytes.push(0x0c),
                Some('r') => bytes.push(b'\r'),
                Some(d @ '0'..='7') => {
                    // Up to three octal digits: one byte of the name.
                    let mut value = d.to_digit(8).unwrap_or(0);
                    let mut rest = chars.clone();
                    for _ in 0..2 {
                        match rest.next().and_then(|x| x.to_digit(8)) {
                            Some(v) => {
                                value = value * 8 + v;
                                chars.next();
                            }
                            None => break,
                        }
                    }
                    bytes.push(value as u8);
                }
                Some(other) => {
                    let mut buf = [0; 4];
                    bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
                }
                None => bytes.push(b'\\'),
            },
            c => {
                let mut buf = [0; 4];
                bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Parses `Binary files <old> and <new> differ` into the two paths
/// (`None` for `/dev/null`). Returns `None` if `line` isn't such a line.
pub(crate) fn parse_binary_line(line: &str) -> Option<(Option<String>, Option<String>)> {
    let rest = line
        .strip_prefix("Binary files ")?
        .strip_suffix(" differ")?;
    // The paths themselves may contain " and "; prefer the split where both
    // names agree (same path, or one side is /dev/null), else the first.
    let splits: Vec<usize> = rest.match_indices(" and ").map(|(i, _)| i).collect();
    let pick = splits
        .iter()
        .find(|&&i| {
            let (old, new) = (parse_path(&rest[..i]), parse_path(&rest[i + 5..]));
            old.is_none() || new.is_none() || old == new
        })
        .or(splits.first())?;
    Some((parse_path(&rest[..*pick]), parse_path(&rest[pick + 5..])))
}

pub(crate) fn parse_hunk_header(
    line: &str,
    line_no: usize,
) -> Result<(u32, u32, u32, u32, Option<String>), ParseError> {
    let rest = line
        .strip_prefix("@@ ")
        .ok_or_else(|| err(line_no, m("diff.bad_hunk_header")))?;
    let (ranges, heading) = rest
        .split_once(" @@")
        .ok_or_else(|| err(line_no, m("diff.bad_hunk_header_unclosed")))?;
    let heading = {
        let h = heading.trim_start();
        if h.is_empty() {
            None
        } else {
            Some(h.to_string())
        }
    };

    let mut parts = ranges.split_whitespace();
    let old_range = parts
        .next()
        .ok_or_else(|| err(line_no, m("diff.bad_hunk_header_no_old_range")))?;
    let new_range = parts
        .next()
        .ok_or_else(|| err(line_no, m("diff.bad_hunk_header_no_new_range")))?;

    let (old_start, old_lines) = parse_range(old_range, '-', line_no)?;
    let (new_start, new_lines) = parse_range(new_range, '+', line_no)?;
    Ok((old_start, old_lines, new_start, new_lines, heading))
}

fn parse_range(s: &str, prefix: char, line_no: usize) -> Result<(u32, u32), ParseError> {
    let s = s.strip_prefix(prefix).ok_or_else(|| {
        err(
            line_no,
            mf("diff.bad_hunk_range", &[("value", &format!("{s:?}"))]),
        )
    })?;
    if let Some((start, len)) = s.split_once(',') {
        let start = start.parse().map_err(|_| {
            err(
                line_no,
                mf(
                    "diff.bad_hunk_range_start",
                    &[("value", &format!("{start:?}"))],
                ),
            )
        })?;
        let len = len.parse().map_err(|_| {
            err(
                line_no,
                mf(
                    "diff.bad_hunk_range_length",
                    &[("value", &format!("{len:?}"))],
                ),
            )
        })?;
        Ok((start, len))
    } else {
        let start = s.parse().map_err(|_| {
            err(
                line_no,
                mf("diff.bad_hunk_range", &[("value", &format!("{s:?}"))]),
            )
        })?;
        Ok((start, 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 83db48f..bf269c9 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -10,4 +10,8 @@ impl Foo {
     fn bar(&self) -> i32 {
         self.value
     }
+
+    fn baz(&self) -> i32 {
+        self.value * 2
+    }
 }
";

    #[test]
    fn parses_a_single_file_single_hunk_diff() {
        let parsed = parse(SAMPLE).expect("valid diff");
        assert_eq!(parsed.files.len(), 1);
        let file = &parsed.files[0];
        assert_eq!(file.old_path.as_deref(), Some("src/lib.rs"));
        assert_eq!(file.new_path.as_deref(), Some("src/lib.rs"));
        assert_eq!(file.hunks.len(), 1);

        let hunk = &file.hunks[0];
        assert_eq!(hunk.old_start, 10);
        assert_eq!(hunk.old_lines, 4);
        assert_eq!(hunk.new_start, 10);
        assert_eq!(hunk.new_lines, 8);
        assert_eq!(hunk.section_heading.as_deref(), Some("impl Foo {"));
        assert_eq!(hunk.lines.len(), 8);

        // "self.value * 2" is a pure addition on the new side only.
        let added = &hunk.lines[5];
        assert_eq!(added.kind, LineKind::Added);
        assert_eq!(added.content, "        self.value * 2");
        assert_eq!(added.old_line, None);
        assert_eq!(added.new_line, Some(15));

        // trailing "}" is unchanged context present on both sides.
        let trailing = hunk.lines.last().unwrap();
        assert_eq!(trailing.kind, LineKind::Context);
        assert_eq!(trailing.old_line, Some(13));
        assert_eq!(trailing.new_line, Some(17));
    }

    #[test]
    fn parses_a_bare_diff_dash_u_file_with_no_git_headers() {
        let bare = "\
--- old.txt
+++ new.txt
@@ -1,2 +1,2 @@
-hello
+hello world
 bye
";
        let parsed = parse(bare).expect("valid diff");
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].old_path.as_deref(), Some("old.txt"));
        assert_eq!(parsed.files[0].new_path.as_deref(), Some("new.txt"));
        assert_eq!(parsed.files[0].hunks[0].lines.len(), 3);
    }

    #[test]
    fn rejects_context_diff_style_markers() {
        // `>`/`<` are not valid unified-diff content prefixes.
        let context_style = "\
--- old.txt
+++ new.txt
@@ -1,1 +1,1 @@
< hello
---
> hello world
";
        assert!(parse(context_style).is_err());
    }

    #[test]
    fn binary_entries_carry_their_paths() {
        let text = "\
diff --git a/img.png b/img.png
Binary files a/img.png and b/img.png differ
diff --git a/new.bin b/new.bin
new file mode 100644
Binary files /dev/null and b/new.bin differ
diff --git a/t.txt b/t.txt
--- a/t.txt
+++ b/t.txt
@@ -1 +1 @@
-a
+b
";
        let files = parse(text).unwrap().files;
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].new_path.as_deref(), Some("img.png"));
        assert!(files[0].is_binary && files[0].hunks.is_empty());
        assert_eq!(
            (files[1].old_path.as_deref(), files[1].new_path.as_deref()),
            (None, Some("new.bin"))
        );
        assert!(!files[2].is_binary);
        assert_eq!(files[2].hunks.len(), 1);
    }

    #[test]
    fn a_name_keeps_its_spaces_at_either_end_and_inside() {
        // git ends a name with spaces in it with a tab.
        let text = "diff --git a/ends  b/ends \n--- a/ends \t\n+++ b/ends \t\n@@ -1 +1 @@\n-a\n+b\n\
diff --git a/ lead.txt b/ lead.txt\n--- a/ lead.txt\t\n+++ b/ lead.txt\t\n@@ -1 +1 @@\n-a\n+b\n\
diff --git a/a b.txt b/a b.txt\n--- a/a b.txt\t2024-01-01 00:00\n+++ b/a b.txt\t\n@@ -1 +1 @@\n-a\n+b\n";
        let files = parse(text).unwrap().files;
        let names: Vec<_> = files
            .iter()
            .map(|f| f.new_path.as_deref().unwrap())
            .collect();
        assert_eq!(names, ["ends ", " lead.txt", "a b.txt"]);
        assert_eq!(files[0].old_path.as_deref(), Some("ends "));
    }

    #[test]
    fn a_name_git_writes_in_quotes_is_read_as_it_is() {
        let text = "diff --git \"a/we\\\"ird\" \"b/we\\\"ird\"\n--- \"a/we\\\"ird\"\n+++ \"b/we\\\"ird\"\n@@ -1 +1 @@\n-a\n+b\n\
diff --git \"a/tab\\there\" \"b/tab\\there\"\n--- \"a/tab\\there\"\n+++ \"b/tab\\there\"\n@@ -1 +1 @@\n-a\n+b\n\
diff --git \"a/back\\\\slash\" \"b/back\\\\slash\"\n--- \"a/back\\\\slash\"\n+++ \"b/back\\\\slash\"\n@@ -1 +1 @@\n-a\n+b\n\
diff --git \"a/\\346\\227\\245.txt\" \"b/\\346\\227\\245.txt\"\n--- \"a/\\346\\227\\245.txt\"\n+++ \"b/\\346\\227\\245.txt\"\n@@ -1 +1 @@\n-a\n+b\n";
        let names: Vec<String> = parse(text)
            .unwrap()
            .files
            .into_iter()
            .map(|f| f.new_path.unwrap())
            .collect();
        // (The last as git writes it with core.quotepath on: byte by byte.)
        assert_eq!(names, ["we\"ird", "tab\there", "back\\slash", "日.txt"]);
    }

    #[test]
    fn a_renamed_name_and_a_picture_keep_their_spaces_and_quotes() {
        let text = "diff --git a/old  b/new \nsimilarity index 100%\nrename from old \nrename to new \n\
diff --git \"a/q\\\"\" \"b/q2\\\"\"\nsimilarity index 100%\nrename from \"q\\\"\"\nrename to \"q2\\\"\"\n\
diff --git a/my pic .png b/my pic .png\nBinary files a/my pic .png and b/my pic .png differ\n";
        let files = parse(text).unwrap().files;
        assert_eq!(
            (files[0].old_path.as_deref(), files[0].new_path.as_deref()),
            (Some("old "), Some("new "))
        );
        assert_eq!(
            (files[1].old_path.as_deref(), files[1].new_path.as_deref()),
            (Some("q\""), Some("q2\""))
        );
        assert_eq!(files[2].new_path.as_deref(), Some("my pic .png"));
        assert_eq!(
            parse_binary_line("Binary files \"a/x\\ty\" and \"b/x\\ty\" differ"),
            Some((Some("x\ty".into()), Some("x\ty".into())))
        );
    }

    #[test]
    fn a_path_is_quoted_as_git_quotes_it_and_read_back_the_same() {
        for path in [
            "plain.txt",
            "a b.txt",
            "ends ",
            "日本語.md",
            "we\"ird",
            "back\\slash",
            "tab\there",
            "line\nbreak",
            "bell\u{7}",
        ] {
            let written = quote_path(&format!("a/{path}"));
            assert_eq!(parse_path(&written).as_deref(), Some(path), "{written}");
        }
        assert_eq!(
            quote_path("a b.txt"),
            "a b.txt",
            "spaces alone are not quoted"
        );
        assert_eq!(quote_path("日本語.md"), "日本語.md");
        assert_eq!(quote_path("we\"ird"), "\"we\\\"ird\"");
    }

    #[test]
    fn binary_line_paths_may_contain_and() {
        assert_eq!(
            parse_binary_line("Binary files a/rock and roll.bin and b/rock and roll.bin differ"),
            Some((
                Some("rock and roll.bin".into()),
                Some("rock and roll.bin".into())
            ))
        );
        assert_eq!(parse_binary_line("Binary files a/x differ"), None);
        assert_eq!(parse_binary_line("not binary"), None);
    }
}
