//! Timings for the line diff on big inputs. Run by hand:
//! `cargo test --release --test perf -- --ignored --nocapture`

use diffnote::linediff::line_diff;
use std::time::Instant;

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

fn text(ls: &[String]) -> String {
    ls.iter().map(|l| format!("{l}\n")).collect()
}

#[test]
#[ignore]
fn line_diff_timings() {
    let mut rng = Lcg(42);
    let n = 100_000;
    let base: Vec<String> = (0..n)
        .map(|i| format!("line {} {i}", rng.next(1_000_000)))
        .collect();
    let old = text(&base);

    let mut few = base.clone();
    for _ in 0..300 {
        let i = rng.next(n);
        few[i].push_str(" edited");
    }
    let mut third = base.clone();
    for line in third.iter_mut() {
        if rng.next(3) == 0 {
            line.push_str(" edited");
        }
    }
    let rewrite: Vec<String> = (0..n)
        .map(|i| format!("different {} {i}", rng.next(1_000_000)))
        .collect();
    let mut shuffled = base.clone();
    for i in (1..n).rev() {
        shuffled.swap(i, rng.next(i + 1));
    }
    let mut appended = base.clone();
    appended.extend((0..1000).map(|i| format!("appended {i}")));

    for (label, new) in [
        ("300 edits", &few),
        ("a third of the lines edited", &third),
        ("total rewrite", &rewrite),
        ("same lines, shuffled", &shuffled),
        ("1000 lines appended", &appended),
    ] {
        let new = text(new);
        let start = Instant::now();
        let ops = line_diff(&old, &new);
        println!(
            "{label:<30} {:>7.3}s  ({} ops)",
            start.elapsed().as_secs_f64(),
            ops.len()
        );
    }
}
