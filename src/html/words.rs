//! Which words of a removed line and of the added line that replaces it are
//! what changed, to be emphasized.
//!
//! A line is cut into words (letters, digits and `_`), runs of white space, and
//! single other characters; the two lists are compared, and the parts that
//! aren't in both are the changes. Offsets are in UTF-16 code units, which is
//! how the page (JavaScript) counts.

use similar::{Algorithm, DiffOp, capture_diff_slices};

/// Lines longer than this (a minified file) aren't compared.
const MAX_LINE: usize = 2000;
/// How much of the text must be in both for the changes to be worth showing:
/// two lines that have almost nothing in common are just different lines.
const MIN_SIMILARITY: f64 = 0.35;

/// The changed ranges `[start, end)` of the old and of the new line, or `None`
/// if the lines are too different, too long, or the change is all of the line.
pub fn changed(old: &str, new: &str) -> Option<(Vec<[u32; 2]>, Vec<[u32; 2]>)> {
    if old.len() > MAX_LINE || new.len() > MAX_LINE {
        return None;
    }
    let a = words(old);
    let b = words(new);
    let a_text: Vec<&str> = a.iter().map(|&(s, e)| &old[s..e]).collect();
    let b_text: Vec<&str> = b.iter().map(|&(s, e)| &new[s..e]).collect();
    let ops = capture_diff_slices(Algorithm::Myers, &a_text, &b_text);

    let mut same = 0usize;
    let mut old_ranges: Vec<(usize, usize)> = Vec::new();
    let mut new_ranges: Vec<(usize, usize)> = Vec::new();
    let span = |tokens: &[(usize, usize)], index: usize, len: usize| -> Option<(usize, usize)> {
        (len > 0).then(|| (tokens[index].0, tokens[index + len - 1].1))
    };
    for op in ops {
        match op {
            DiffOp::Equal { old_index, len, .. } => {
                same += a_text[old_index..old_index + len]
                    .iter()
                    .map(|t| t.trim().len())
                    .sum::<usize>();
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => old_ranges.extend(span(&a, old_index, old_len)),
            DiffOp::Insert {
                new_index, new_len, ..
            } => new_ranges.extend(span(&b, new_index, new_len)),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                old_ranges.extend(span(&a, old_index, old_len));
                new_ranges.extend(span(&b, new_index, new_len));
            }
        }
    }
    let total = old.trim().len() + new.trim().len();
    if total == 0 || (same * 2) as f64 / (total as f64) < MIN_SIMILARITY {
        return None;
    }
    let old_ranges = merge(old, old_ranges);
    let new_ranges = merge(new, new_ranges);
    // Everything changed: nothing to tell apart.
    if covers_all(old, &old_ranges) && covers_all(new, &new_ranges) {
        return None;
    }
    Some((utf16(old, &old_ranges), utf16(new, &new_ranges)))
}

/// The byte ranges of the words of `s`.
fn words(s: &str) -> Vec<(usize, usize)> {
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut open: Option<(usize, Class)> = None;
    for (i, c) in s.char_indices() {
        let cl = class(c);
        match open {
            Some((_, prev)) if prev == cl && cl != Class::Other => {}
            Some((start, _)) => {
                out.push((start, i));
                open = Some((i, cl));
            }
            None => open = Some((i, cl)),
        }
    }
    if let Some((start, _)) = open {
        out.push((start, s.len()));
    }
    out
}

/// Joins ranges that touch or have only white space between them.
fn merge(s: &str, mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.sort();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        match out.last_mut() {
            Some(last) if s[last.1..start].trim().is_empty() => last.1 = end.max(last.1),
            _ => out.push((start, end)),
        }
    }
    out
}

fn covers_all(s: &str, ranges: &[(usize, usize)]) -> bool {
    let (lead, tail) = (s.len() - s.trim_start().len(), s.trim_end().len());
    match ranges {
        [] => s.trim().is_empty(),
        [(start, end)] => *start <= lead && *end >= tail,
        _ => false,
    }
}

fn utf16(s: &str, ranges: &[(usize, usize)]) -> Vec<[u32; 2]> {
    let at = |byte: usize| s[..byte].encode_utf16().count() as u32;
    ranges.iter().map(|&(a, b)| [at(a), at(b)]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The changed text of each side.
    fn shown(old: &str, new: &str) -> Option<(Vec<String>, Vec<String>)> {
        let (o, n) = changed(old, new)?;
        let cut = |s: &str, r: Vec<[u32; 2]>| {
            let u: Vec<u16> = s.encode_utf16().collect();
            r.iter()
                .map(|[a, b]| String::from_utf16(&u[*a as usize..*b as usize]).unwrap())
                .collect()
        };
        Some((cut(old, o), cut(new, n)))
    }

    #[test]
    fn a_changed_word_is_what_is_emphasized_on_each_side() {
        let (old, new) =
            shown("type=sha,format=long", "type=sha,prefix=sha-,format=short").unwrap();
        assert_eq!(old, vec!["long"]);
        assert_eq!(new, vec!["prefix=sha-,", "short"]);
    }

    #[test]
    fn an_added_part_is_only_on_the_new_side() {
        let (old, new) = shown("foo(a)", "foo(a, b)").unwrap();
        assert!(old.is_empty(), "{old:?}");
        assert_eq!(new, vec![", b"]);
    }

    #[test]
    fn a_word_is_letters_digits_and_underscore_so_a_renamed_identifier_is_one_change() {
        let (old, new) = shown("let value_a = compute(x);", "let value_b = compute(x);").unwrap();
        assert_eq!(
            (old, new),
            (vec!["value_a".to_string()], vec!["value_b".to_string()])
        );
    }

    #[test]
    fn lines_with_little_in_common_are_not_compared() {
        assert!(
            changed(
                "return res.status(401).end()",
                "  const ok = await compare(pass, hash)"
            )
            .is_none()
        );
        // Nor is a line rewritten in full.
        assert!(changed("abc", "xyz").is_none());
    }

    #[test]
    fn offsets_are_utf16_units_as_the_page_counts() {
        // 😀 is two units in UTF-16 (four bytes in UTF-8), あ is one.
        let (o, n) = changed("あ😀 old value here", "あ😀 new value here").unwrap();
        assert_eq!(o, vec![[4, 7]]);
        assert_eq!(n, vec![[4, 7]]);
    }

    #[test]
    fn white_space_between_changes_joins_them_and_long_lines_are_skipped() {
        let (o, _) = shown("call(a b c)", "call(x y c)").unwrap();
        assert_eq!(o, vec!["a b"]);
        let long = "x ".repeat(2000);
        assert!(changed(&long, &format!("{long}y")).is_none());
    }

    #[test]
    fn identical_lines_have_no_changes() {
        let (o, n) = changed("same line", "same line").unwrap();
        assert!(o.is_empty() && n.is_empty());
    }
}
