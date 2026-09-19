//! Parsing for **unified diff** text only (`git diff` / `diff -u` output).
//! Legacy context-diff format (`>`/`<` change markers) is intentionally
//! unsupported, since it would collide with the annotation format's own use
//! of `>` (see the `annotation` module).
//!
//! This parses bare diff text with no interleaved comments. The annotation
//! format layers `>`-prefixed lines on top of this same line structure, but
//! has its own parser in the `annotation` module.

use thiserror::Error;

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

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("line {line}: {message}")]
    Malformed { line: usize, message: String },
}

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
            file.old_path = Some(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = raw_line.strip_prefix("rename to ") {
            let file = current_file.get_or_insert_with(FileDiff::default);
            file.is_rename = true;
            file.new_path = Some(rest.trim().to_string());
            continue;
        }

        if raw_line.starts_with("Binary files ") && raw_line.ends_with(" differ") {
            let file = current_file.get_or_insert_with(FileDiff::default);
            file.is_binary = true;
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
                || current_file.as_ref().is_some_and(|f| !f.hunks.is_empty());
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
                .ok_or_else(|| err(line_no, "'+++' line outside of a file header"))?;
            file.new_path = parse_path(rest);
            continue;
        }

        if raw_line.starts_with("@@ ") || raw_line == "@@" {
            finish_hunk(&mut current_file, &mut current_hunk);
            if current_file.is_none() {
                return Err(err(line_no, "hunk header outside of a file"));
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
                .ok_or_else(|| err(line_no, "'\\' marker outside of a hunk"))?;
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
            .ok_or_else(|| err(line_no, "diff content outside of a hunk"))?;
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
                    format!("unrecognized diff line prefix {other:?}"),
                ));
            }
        };
        hunk.lines.push(line);
    }

    finish_file(&mut files, &mut current_file, &mut current_hunk);
    Ok(UnifiedDiff { files })
}

pub(crate) fn parse_path(rest: &str) -> Option<String> {
    let path_part = rest.split('\t').next().unwrap_or(rest).trim();
    if path_part == "/dev/null" {
        None
    } else {
        let stripped = path_part
            .strip_prefix("a/")
            .or_else(|| path_part.strip_prefix("b/"))
            .unwrap_or(path_part);
        Some(stripped.to_string())
    }
}

pub(crate) fn parse_hunk_header(
    line: &str,
    line_no: usize,
) -> Result<(u32, u32, u32, u32, Option<String>), ParseError> {
    let rest = line
        .strip_prefix("@@ ")
        .ok_or_else(|| err(line_no, "malformed hunk header"))?;
    let (ranges, heading) = rest
        .split_once(" @@")
        .ok_or_else(|| err(line_no, "malformed hunk header: missing closing '@@'"))?;
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
        .ok_or_else(|| err(line_no, "malformed hunk header: missing old range"))?;
    let new_range = parts
        .next()
        .ok_or_else(|| err(line_no, "malformed hunk header: missing new range"))?;

    let (old_start, old_lines) = parse_range(old_range, '-', line_no)?;
    let (new_start, new_lines) = parse_range(new_range, '+', line_no)?;
    Ok((old_start, old_lines, new_start, new_lines, heading))
}

fn parse_range(s: &str, prefix: char, line_no: usize) -> Result<(u32, u32), ParseError> {
    let s = s
        .strip_prefix(prefix)
        .ok_or_else(|| err(line_no, format!("malformed hunk range {s:?}")))?;
    if let Some((start, len)) = s.split_once(',') {
        let start = start
            .parse()
            .map_err(|_| err(line_no, format!("malformed hunk range start {start:?}")))?;
        let len = len
            .parse()
            .map_err(|_| err(line_no, format!("malformed hunk range length {len:?}")))?;
        Ok((start, len))
    } else {
        let start = s
            .parse()
            .map_err(|_| err(line_no, format!("malformed hunk range {s:?}")))?;
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
}
