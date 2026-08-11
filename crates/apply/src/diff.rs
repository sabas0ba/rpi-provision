//! A small line diff for dry-run output.
//!
//! Uses the classic longest-common-subsequence dynamic program. The files
//! involved are configuration files of at most a few hundred lines, so the
//! quadratic cost is irrelevant; above [`MAX_LINES`] the diff degrades to a
//! summary rather than consuming memory.

pub const MAX_LINES: usize = 2_000;
const CONTEXT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Keep,
    Insert,
    Delete,
}

/// Render a unified-style diff of `before` and `after`.
///
/// Returns `None` when the two are identical.
pub fn unified(before: &str, after: &str) -> Option<String> {
    if before == after {
        return None;
    }

    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();

    if old.len() > MAX_LINES || new.len() > MAX_LINES {
        return Some(format!(
            "  (content differs; {} lines before, {} lines after, too large to diff)\n",
            old.len(),
            new.len()
        ));
    }

    let script = build_script(&old, &new);
    Some(render(&script))
}

fn build_script<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<(Op, &'a str)> {
    // lcs[i][j] = length of the longest common subsequence of old[i..], new[j..]
    let mut lcs = vec![vec![0usize; new.len() + 1]; old.len() + 1];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut script = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            script.push((Op::Keep, old[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            script.push((Op::Delete, old[i]));
            i += 1;
        } else {
            script.push((Op::Insert, new[j]));
            j += 1;
        }
    }
    while i < old.len() {
        script.push((Op::Delete, old[i]));
        i += 1;
    }
    while j < new.len() {
        script.push((Op::Insert, new[j]));
        j += 1;
    }
    script
}

/// Print changed regions with a few lines of context, eliding the rest.
fn render(script: &[(Op, &str)]) -> String {
    let interesting: Vec<bool> = script.iter().map(|(op, _)| *op != Op::Keep).collect();
    let mut show = vec![false; script.len()];
    for (index, changed) in interesting.iter().enumerate() {
        if !changed {
            continue;
        }
        let start = index.saturating_sub(CONTEXT);
        let end = (index + CONTEXT + 1).min(script.len());
        show[start..end].iter_mut().for_each(|slot| *slot = true);
    }

    let mut out = String::new();
    let mut skipped = 0usize;
    for (index, (op, line)) in script.iter().enumerate() {
        if !show[index] {
            skipped += 1;
            continue;
        }
        if skipped > 0 {
            out.push_str(&format!("  ... {skipped} unchanged line(s)\n"));
            skipped = 0;
        }
        let marker = match op {
            Op::Keep => ' ',
            Op::Insert => '+',
            Op::Delete => '-',
        };
        out.push(marker);
        out.push(' ');
        out.push_str(line);
        out.push('\n');
    }
    if skipped > 0 {
        out.push_str(&format!("  ... {skipped} unchanged line(s)\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_input_has_no_diff() {
        assert_eq!(unified("a\nb\n", "a\nb\n"), None);
    }

    #[test]
    fn shows_insertions_and_deletions() {
        let diff = unified("a\nb\nc\n", "a\nB\nc\n").unwrap();
        assert!(diff.contains("- b"));
        assert!(diff.contains("+ B"));
        assert!(diff.contains("  a"));
        assert!(diff.contains("  c"));
    }

    #[test]
    fn pure_append_is_reported_as_insertions() {
        let diff = unified("a\n", "a\nb\n").unwrap();
        assert!(diff.contains("+ b"));
        assert!(!diff.contains("- "));
    }

    #[test]
    fn elides_distant_unchanged_lines() {
        let before: String = (0..40).map(|n| format!("line{n}\n")).collect();
        let after = before.replace("line20", "CHANGED");
        let diff = unified(&before, &after).unwrap();
        assert!(diff.contains("unchanged line(s)"));
        assert!(diff.contains("+ CHANGED"));
        assert!(diff.contains("- line20"));
        assert!(!diff.contains("line0\n"), "distant lines must be elided");
    }

    #[test]
    fn creation_from_empty() {
        let diff = unified("", "a\nb\n").unwrap();
        assert!(diff.contains("+ a"));
        assert!(diff.contains("+ b"));
    }

    #[test]
    fn oversized_files_fall_back_to_a_summary() {
        let before: String = (0..MAX_LINES + 1).map(|n| format!("{n}\n")).collect();
        let diff = unified(&before, "x\n").unwrap();
        assert!(diff.contains("too large to diff"));
    }
}
