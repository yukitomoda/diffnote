//! The line diff between two versions of a file, fast and the same every time
//! on any machine.
//!
//! Small inputs go straight to Myers, which finds a shortest edit script. Its
//! cost grows with how much *differs*, though, so a big file that was mostly
//! rewritten takes tens of seconds -- and cutting it off by time would make the
//! answer depend on how fast the machine is. Big inputs are therefore done
//! another way, whose cost doesn't depend on the edit:
//!
//! 1. common lines at the start and end are matched off;
//! 2. lines that occur exactly once on each side are candidates for pairing,
//!    and the longest run of them that keeps their order (the same order on
//!    both sides) is taken as fixed points;
//! 3. between two fixed points only a small stretch is left, which is diffed
//!    with Myers -- unless it is too big, in which case it is one replacement.
//!
//! When every line is unique this finds as many matching lines as Myers does
//! (the longest common subsequence is then exactly that longest ordered run).

use similar::{Algorithm, DiffOp, TextDiff, capture_diff_slices};
use std::collections::HashMap;

/// Inputs with at most this many lines in all go to Myers as they are.
const SMALL: usize = 10_000;
/// A stretch between fixed points is diffed with Myers only up to this many
/// old lines times new lines.
const GAP_LIMIT: usize = 1_000_000;

/// The edit script turning `old` into `new`, line by line (each line with its
/// newline), in the form `similar` uses: equal runs, and replaced, deleted and
/// inserted blocks.
pub fn line_diff(old: &str, new: &str) -> Vec<DiffOp> {
    let a: Vec<&str> = old.split_inclusive('\n').collect();
    let b: Vec<&str> = new.split_inclusive('\n').collect();
    if a.len() + b.len() <= SMALL {
        return canonical(TextDiff::from_lines(old, new).ops().to_vec());
    }
    fixed_points(&a, &b)
}

/// The same script with every index recomputed from how many lines each side
/// has been through. `similar` can put a removal's `new_index` (or an
/// insertion's `old_index`) somewhere other than where the script actually is
/// at that point, and a position taken from it would be wrong.
fn canonical(ops: Vec<DiffOp>) -> Vec<DiffOp> {
    let (mut o, mut n) = (0usize, 0usize);
    ops.into_iter()
        .map(|op| {
            let fixed = match op {
                DiffOp::Equal { len, .. } => DiffOp::Equal {
                    old_index: o,
                    new_index: n,
                    len,
                },
                DiffOp::Delete { old_len, .. } => DiffOp::Delete {
                    old_index: o,
                    old_len,
                    new_index: n,
                },
                DiffOp::Insert { new_len, .. } => DiffOp::Insert {
                    old_index: o,
                    new_index: n,
                    new_len,
                },
                DiffOp::Replace {
                    old_len, new_len, ..
                } => DiffOp::Replace {
                    old_index: o,
                    old_len,
                    new_index: n,
                    new_len,
                },
            };
            o += fixed.old_range().len();
            n += fixed.new_range().len();
            fixed
        })
        .collect()
}

fn fixed_points(a: &[&str], b: &[&str]) -> Vec<DiffOp> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (ca, cb) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);

    let mut ops: Vec<DiffOp> = Vec::new();
    if prefix > 0 {
        ops.push(DiffOp::Equal {
            old_index: 0,
            new_index: 0,
            len: prefix,
        });
    }

    // Fixed points: lines unique on both sides, in an order both sides share.
    let mut seen: HashMap<&str, (u32, u32, usize)> = HashMap::new();
    for (j, line) in cb.iter().enumerate() {
        let e = seen.entry(line).or_insert((0, 0, j));
        e.1 += 1;
    }
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for line in ca {
        if let Some(e) = seen.get_mut(line) {
            e.0 += 1;
        }
    }
    for (i, line) in ca.iter().enumerate() {
        if let Some(&(1, 1, j)) = seen.get(line) {
            pairs.push((i, j));
        }
    }
    let anchors = longest_increasing(&pairs);

    let (mut oi, mut nj) = (0usize, 0usize);
    for (i, j) in anchors {
        gap(&mut ops, ca, cb, (oi, i), (nj, j), (prefix, prefix));
        ops.push(DiffOp::Equal {
            old_index: prefix + i,
            new_index: prefix + j,
            len: 1,
        });
        (oi, nj) = (i + 1, j + 1);
    }
    gap(
        &mut ops,
        ca,
        cb,
        (oi, ca.len()),
        (nj, cb.len()),
        (prefix, prefix),
    );

    if suffix > 0 {
        ops.push(DiffOp::Equal {
            old_index: a.len() - suffix,
            new_index: b.len() - suffix,
            len: suffix,
        });
    }
    canonical(normalize(ops))
}

/// The `(i, j)` pairs (already in increasing `i`) forming the longest run in
/// which `j` also increases.
fn longest_increasing(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    // tails[k]: the index into `pairs` ending the best run of length k + 1.
    let mut tails: Vec<usize> = Vec::new();
    let mut before: Vec<Option<usize>> = vec![None; pairs.len()];
    for (at, &(_, j)) in pairs.iter().enumerate() {
        let k = tails.partition_point(|&t| pairs[t].1 < j);
        if k > 0 {
            before[at] = Some(tails[k - 1]);
        }
        if k == tails.len() {
            tails.push(at);
        } else {
            tails[k] = at;
        }
    }
    let mut out = Vec::new();
    let mut at = tails.last().copied();
    while let Some(k) = at {
        out.push(pairs[k]);
        at = before[k];
    }
    out.reverse();
    out
}

/// The edits for `a[from.0..from.1]` -> `b[to.0..to.1]` (offsets by `shift`).
fn gap(
    ops: &mut Vec<DiffOp>,
    a: &[&str],
    b: &[&str],
    (a0, a1): (usize, usize),
    (b0, b1): (usize, usize),
    shift: (usize, usize),
) {
    let (mut a0, mut b0) = (a0, b0);
    let (mut a1, mut b1) = (a1, b1);
    // Lines that match at the edges of the stretch.
    let head = a[a0..a1]
        .iter()
        .zip(&b[b0..b1])
        .take_while(|(x, y)| x == y)
        .count();
    if head > 0 {
        ops.push(DiffOp::Equal {
            old_index: shift.0 + a0,
            new_index: shift.1 + b0,
            len: head,
        });
        (a0, b0) = (a0 + head, b0 + head);
    }
    let tail = a[a0..a1]
        .iter()
        .rev()
        .zip(b[b0..b1].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let tail_at = (a1 - tail, b1 - tail);
    (a1, b1) = tail_at;

    let (old_len, new_len) = (a1 - a0, b1 - b0);
    let (old_index, new_index) = (shift.0 + a0, shift.1 + b0);
    if old_len == 0 && new_len > 0 {
        ops.push(DiffOp::Insert {
            old_index,
            new_index,
            new_len,
        });
    } else if new_len == 0 && old_len > 0 {
        ops.push(DiffOp::Delete {
            old_index,
            old_len,
            new_index,
        });
    } else if old_len > 0 {
        if old_len.saturating_mul(new_len) <= GAP_LIMIT {
            for op in capture_diff_slices(Algorithm::Myers, &a[a0..a1], &b[b0..b1]) {
                ops.push(shifted(op, old_index, new_index));
            }
        } else {
            ops.push(DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            });
        }
    }
    if tail > 0 {
        ops.push(DiffOp::Equal {
            old_index: shift.0 + tail_at.0,
            new_index: shift.1 + tail_at.1,
            len: tail,
        });
    }
}

fn shifted(op: DiffOp, old: usize, new: usize) -> DiffOp {
    match op {
        DiffOp::Equal {
            old_index,
            new_index,
            len,
        } => DiffOp::Equal {
            old_index: old + old_index,
            new_index: new + new_index,
            len,
        },
        DiffOp::Delete {
            old_index,
            old_len,
            new_index,
        } => DiffOp::Delete {
            old_index: old + old_index,
            old_len,
            new_index: new + new_index,
        },
        DiffOp::Insert {
            old_index,
            new_index,
            new_len,
        } => DiffOp::Insert {
            old_index: old + old_index,
            new_index: new + new_index,
            new_len,
        },
        DiffOp::Replace {
            old_index,
            old_len,
            new_index,
            new_len,
        } => DiffOp::Replace {
            old_index: old + old_index,
            old_len,
            new_index: new + new_index,
            new_len,
        },
    }
}

/// Joins neighbouring runs, and a removal next to an insertion into one
/// replacement.
fn normalize(ops: Vec<DiffOp>) -> Vec<DiffOp> {
    let mut out: Vec<DiffOp> = Vec::new();
    for op in ops {
        let merged = match (out.last().copied(), op) {
            (
                Some(DiffOp::Equal {
                    old_index,
                    new_index,
                    len,
                }),
                DiffOp::Equal { len: more, .. },
            ) => Some(DiffOp::Equal {
                old_index,
                new_index,
                len: len + more,
            }),
            (
                Some(DiffOp::Delete {
                    old_index,
                    old_len,
                    new_index,
                }),
                DiffOp::Insert { new_len, .. },
            ) => Some(DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            }),
            (
                Some(DiffOp::Insert {
                    old_index,
                    new_index,
                    new_len,
                }),
                DiffOp::Delete { old_len, .. },
            ) => Some(DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            }),
            (
                Some(DiffOp::Delete {
                    old_index,
                    old_len,
                    new_index,
                }),
                DiffOp::Delete { old_len: more, .. },
            ) => Some(DiffOp::Delete {
                old_index,
                old_len: old_len + more,
                new_index,
            }),
            (
                Some(DiffOp::Insert {
                    old_index,
                    new_index,
                    new_len,
                }),
                DiffOp::Insert { new_len: more, .. },
            ) => Some(DiffOp::Insert {
                old_index,
                new_index,
                new_len: new_len + more,
            }),
            (
                Some(DiffOp::Replace {
                    old_index,
                    old_len,
                    new_index,
                    new_len,
                }),
                DiffOp::Replace {
                    old_len: o,
                    new_len: n,
                    ..
                },
            ) => Some(DiffOp::Replace {
                old_index,
                old_len: old_len + o,
                new_index,
                new_len: new_len + n,
            }),
            _ => None,
        };
        match merged {
            Some(m) => {
                out.pop();
                out.push(m);
            }
            None => out.push(op),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[String]) -> String {
        lines.iter().map(|l| format!("{l}\n")).collect()
    }

    /// The script is a valid one for these inputs: it covers every old and
    /// every new line, in order, each once, and equal runs really are equal.
    fn check(old: &str, new: &str, ops: &[DiffOp]) {
        let a: Vec<&str> = old.split_inclusive('\n').collect();
        let b: Vec<&str> = new.split_inclusive('\n').collect();
        let (mut oi, mut nj) = (0usize, 0usize);
        for op in ops {
            let (o, n) = (op.old_range(), op.new_range());
            assert_eq!(
                (o.start, n.start),
                (oi, nj),
                "ops must be contiguous: {ops:?}"
            );
            if let DiffOp::Equal { len, .. } = op {
                assert!(*len > 0);
                assert_eq!(a[o.clone()], b[n.clone()], "equal run isn't equal: {op:?}");
            } else {
                assert!(!o.is_empty() || !n.is_empty(), "empty change {op:?}");
            }
            oi = o.end;
            nj = n.end;
        }
        assert_eq!(
            (oi, nj),
            (a.len(), b.len()),
            "the script must reach the ends"
        );
        // Normalized: no two neighbours of the same kind, no lone delete+insert.
        for pair in ops.windows(2) {
            let same = std::mem::discriminant(&pair[0]) == std::mem::discriminant(&pair[1]);
            assert!(!same, "unmerged neighbours {pair:?}");
            assert!(
                !matches!(
                    (pair[0], pair[1]),
                    (DiffOp::Delete { .. }, DiffOp::Insert { .. })
                        | (DiffOp::Insert { .. }, DiffOp::Delete { .. })
                ),
                "delete and insert should be one replace: {pair:?}"
            );
        }
    }

    fn matched(ops: &[DiffOp]) -> usize {
        ops.iter()
            .map(|op| {
                if let DiffOp::Equal { len, .. } = op {
                    *len
                } else {
                    0
                }
            })
            .sum()
    }

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, below: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % below.max(1)
        }
    }

    /// Runs the big-input path whatever the size.
    fn big(old: &str, new: &str) -> Vec<DiffOp> {
        let a: Vec<&str> = old.split_inclusive('\n').collect();
        let b: Vec<&str> = new.split_inclusive('\n').collect();
        fixed_points(&a, &b)
    }

    #[test]
    fn small_inputs_are_diffed_exactly_as_myers_does() {
        let (old, new) = ("a\nb\nc\nd\n", "a\nB\nc\nd\ne\n");
        assert_eq!(
            line_diff(old, new),
            TextDiff::from_lines(old, new).ops().to_vec()
        );
    }

    #[test]
    fn a_valid_script_for_the_simple_shapes() {
        for (old, new) in [
            ("", ""),
            ("", "a\nb\n"),
            ("a\nb\n", ""),
            ("a\nb\n", "a\nb\n"),
            ("a\nb\nc\n", "a\nX\nc\n"),
            ("a\nb\nc\n", "c\nb\na\n"),
            ("a\nb", "a\nb\n"),
            ("x\nx\nx\n", "x\nx\n"),
            ("a\nb\nc\nd\ne\n", "a\nc\nX\nY\ne\n"),
        ] {
            let ops = big(old, new);
            check(old, new, &ops);
        }
    }

    #[test]
    fn unrelated_files_are_one_replacement() {
        let old = "a\nb\nc\n";
        let new = "x\ny\n";
        assert_eq!(
            big(old, new),
            vec![DiffOp::Replace {
                old_index: 0,
                old_len: 3,
                new_index: 0,
                new_len: 2
            }]
        );
    }

    #[test]
    fn edits_lines_added_and_removed_are_found_like_myers_finds_them() {
        let old: Vec<String> = (0..50).map(|i| format!("line {i}")).collect();
        let mut new = old.clone();
        new[10] = "edited".to_string();
        new.remove(20);
        new.insert(30, "added".to_string());
        let (o, n) = (text(&old), text(&new));
        let ops = big(&o, &n);
        check(&o, &n, &ops);
        assert_eq!(ops, TextDiff::from_lines(&o, &n).ops().to_vec());
    }

    #[test]
    fn with_unique_lines_it_matches_as_many_lines_as_myers() {
        let mut rng = Lcg(0xD1FF);
        for round in 0..200 {
            let n = 1 + rng.next(60);
            let old: Vec<String> = (0..n).map(|i| format!("o{i}")).collect();
            let mut new: Vec<String> = Vec::new();
            for l in &old {
                match rng.next(6) {
                    0 => {}
                    1 => new.push(format!("{l} edited")),
                    2 => {
                        new.push(l.clone());
                        new.push(format!("fresh{}", rng.next(1000)));
                    }
                    _ => new.push(l.clone()),
                }
            }
            // A few lines moved elsewhere.
            for _ in 0..rng.next(4) {
                if new.len() > 2 {
                    let l = new.remove(rng.next(new.len()));
                    new.insert(rng.next(new.len() + 1), l);
                }
            }
            let (o, nn) = (text(&old), text(&new));
            let ops = big(&o, &nn);
            check(&o, &nn, &ops);
            let myers = TextDiff::from_lines(&o, &nn).ops().to_vec();
            assert_eq!(matched(&ops), matched(&myers), "round {round}");
        }
    }

    #[test]
    fn with_repeated_lines_it_is_still_valid_and_close() {
        let mut rng = Lcg(0xBEEF);
        for round in 0..200 {
            let mk = |rng: &mut Lcg| -> String {
                (0..rng.next(80))
                    .map(|_| format!("v{}\n", rng.next(4)))
                    .collect()
            };
            let (o, n) = (mk(&mut rng), mk(&mut rng));
            let ops = big(&o, &n);
            check(&o, &n, &ops);
            let myers = matched(TextDiff::from_lines(&o, &n).ops());
            // Never claims more than the best there is.
            assert!(matched(&ops) <= myers, "round {round}");
        }
    }

    #[test]
    fn a_stretch_that_is_too_big_is_one_replacement_not_a_hang() {
        // Two huge stretches with only repeated lines between two unique ones.
        let n = 3000;
        let mut old: Vec<String> = vec!["start".into()];
        old.extend((0..n).map(|i| format!("r{}", i % 2)));
        old.push("end".into());
        let mut new: Vec<String> = vec!["start".into()];
        new.extend((0..n).map(|i| format!("r{}", (i + 1) % 2)));
        new.push("end".into());
        let (o, nn) = (text(&old), text(&new));
        let ops = big(&o, &nn);
        check(&o, &nn, &ops);
        // The unique lines still anchor the ends.
        assert!(matched(&ops) >= 2);
    }

    #[test]
    fn a_big_input_takes_the_fast_path_and_stays_valid() {
        let mut rng = Lcg(7);
        let n = 30_000;
        let old: Vec<String> = (0..n)
            .map(|i| format!("line {i} {}", rng.next(1000)))
            .collect();
        let mut new = old.clone();
        for _ in 0..200 {
            let i = rng.next(n);
            new[i].push_str(" edited");
        }
        new.splice(0..0, (0..50).map(|k| format!("top {k}")));
        let (o, nn) = (text(&old), text(&new));
        assert!(old.len() + new.len() > SMALL);
        let ops = line_diff(&o, &nn);
        check(&o, &nn, &ops);
        // Everything unedited is matched.
        assert!(matched(&ops) >= n - 200);
    }

    #[test]
    fn a_total_rewrite_of_a_big_file_is_quick() {
        let mut rng = Lcg(9);
        let n = 60_000;
        let old: Vec<String> = (0..n)
            .map(|i| format!("old {i} {}", rng.next(1_000_000)))
            .collect();
        let new: Vec<String> = (0..n)
            .map(|i| format!("new {i} {}", rng.next(1_000_000)))
            .collect();
        let (o, nn) = (text(&old), text(&new));
        let start = std::time::Instant::now();
        let ops = line_diff(&o, &nn);
        check(&o, &nn, &ops);
        assert_eq!(matched(&ops), 0);
        assert!(start.elapsed().as_secs() < 5, "took {:?}", start.elapsed());
    }

    #[test]
    fn scripts_from_the_small_path_are_contiguous_too() {
        // similar's own removal indices can be off (here the first removal is
        // reported at new line 1 though the script is still at 0).
        let (o, n) = ("v2\nv2\nv1\nv2\n", "v1\nv1\n");
        let ops = line_diff(o, n);
        check(o, n, &ops);
        let mut rng = Lcg(0xFACE);
        for round in 0..500 {
            let mk = |rng: &mut Lcg| -> String {
                (0..rng.next(14))
                    .map(|_| format!("v{}\n", rng.next(3)))
                    .collect()
            };
            let (o, n) = (mk(&mut rng), mk(&mut rng));
            let ops = line_diff(&o, &n);
            let a: Vec<&str> = o.split_inclusive('\n').collect();
            assert!(a.len() + n.split_inclusive('\n').count() <= SMALL);
            check(&o, &n, &ops);
            let _ = round;
        }
    }
}
