//! Numbers typed a digit at a time, for the places that number things on
//! screen (the pickers, `display-panes`): past nine, a number takes more than
//! one key, so the keys are put together here, the same way everywhere.
//!
//! The numbers on screen run `lo..=hi`. A digit is kept while what has been
//! typed is one of them or the first digits of one (`1` with `lo` 5 and `hi`
//! 14 is the start of 10..14, though there is no 1).

use std::time::{Duration, Instant};

/// How long after one digit the next still adds to the same number.
pub const GAP: Duration = Duration::from_secs(1);

/// What has been typed so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Typed {
    pub n: usize,
    pub at: Instant,
}

/// The number after digit `d`: the typed one with `d` after it when that
/// leads somewhere (see `leads_to`) and came within `GAP`, else `d` on its
/// own when that does, else none (and nothing is kept).
pub fn push(typed: Option<Typed>, d: usize, now: Instant, lo: usize, hi: usize) -> Option<Typed> {
    let longer = typed
        .filter(|t| now.saturating_duration_since(t.at) < GAP)
        .and_then(|t| t.n.checked_mul(10)?.checked_add(d))
        .filter(|&n| leads_to(n, lo, hi));
    longer.or(Some(d).filter(|&n| leads_to(n, lo, hi))).map(|n| Typed { n, at: now })
}

/// Whether `n` is in `lo..=hi`, or the first digits of a number that is.
pub fn leads_to(n: usize, lo: usize, hi: usize) -> bool {
    (lo..=hi).contains(&n) || longer(n, lo, hi)
}

/// Whether some number in `lo..=hi` is `n` with more digits after it: then
/// `display-panes` keeps its numbers up a moment for them. (`0` starts
/// nothing.)
pub fn longer(n: usize, lo: usize, hi: usize) -> bool {
    if n == 0 {
        return false;
    }
    // The numbers that start with n's digits, one digit longer each time.
    let (mut a, mut b) = (n, n);
    loop {
        let (Some(a2), Some(b2)) = (a.checked_mul(10), b.checked_mul(10).and_then(|b| b.checked_add(9))) else {
            return false;
        };
        (a, b) = (a2, b2);
        if a > hi {
            return false;
        }
        if b >= lo {
            return true;
        }
    }
}

/// The `(n)` a picker line starts with, padded to the widest number among
/// `rows` lines so the text after it lines up; `None` is blank (a line that
/// cannot be picked).
pub fn label(n: Option<usize>, rows: usize) -> String {
    let width = rows.saturating_sub(1).to_string().len() + 2;
    match n {
        Some(n) => format!("{:<width$}", format!("({n})")),
        None => " ".repeat(width),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number a run of digits makes, typed quickly, with `lo..=hi` up.
    fn keys(ds: &[usize], lo: usize, hi: usize) -> Option<usize> {
        let now = Instant::now();
        ds.iter().fold(None, |t, &d| push(t, d, now, lo, hi)).map(|t| t.n)
    }

    #[test]
    fn under_ten_numbers_a_digit_is_the_number_as_before() {
        for d in 0..10 {
            assert_eq!(keys(&[d], 0, 9), Some(d));
        }
        // A second digit can never make a number that exists: it starts over.
        assert_eq!(keys(&[1, 2], 0, 9), Some(2));
        assert_eq!(keys(&[3, 0], 0, 9), Some(0));
        // Base 1, nine of them: 0 is nothing, 1 is not the start of 10.
        assert_eq!(keys(&[0], 1, 9), None);
        assert!(!longer(1, 1, 9));
    }

    #[test]
    fn past_nine_two_digits_make_one_number() {
        assert_eq!(keys(&[1, 2], 0, 14), Some(12));
        assert_eq!(keys(&[1, 0], 0, 10), Some(10));
        assert_eq!(keys(&[1, 4, 3], 0, 149), Some(143));
        // Too big to exist: the new digit starts a number of its own.
        assert_eq!(keys(&[1, 5], 0, 14), Some(5));
        assert_eq!(keys(&[2, 3], 0, 14), Some(3));
    }

    #[test]
    fn a_number_that_leads_nowhere_is_nothing() {
        assert_eq!(keys(&[7], 0, 4), None);
        assert_eq!(keys(&[], 0, 4), None);
        // Nothing kept: the next digit starts afresh.
        assert_eq!(keys(&[7, 3], 0, 4), Some(3));
    }

    #[test]
    fn a_high_base_still_reaches_the_two_digit_numbers() {
        // pane-base-index 5, ten panes: 5..14. 1 is no pane but starts 10..14.
        assert_eq!(keys(&[1], 5, 14), Some(1));
        assert_eq!(keys(&[1, 2], 5, 14), Some(12));
        assert!(longer(1, 5, 14) && !(5..=14).contains(&1));
        assert_eq!(keys(&[2], 5, 14), None);
        assert_eq!(keys(&[5], 5, 14), Some(5));
        assert!(!longer(5, 5, 14));
        // Base 100: only three-digit numbers; 1 and 10 lead there.
        assert_eq!(keys(&[1, 0, 3], 100, 105), Some(103));
        assert!(longer(1, 100, 105) && longer(10, 100, 105) && !longer(11, 100, 105));
    }

    #[test]
    fn a_pause_starts_a_new_number() {
        let t0 = Instant::now();
        let one = push(None, 1, t0, 0, 19);
        assert_eq!(push(one, 2, t0 + GAP / 2, 0, 19).map(|t| t.n), Some(12));
        assert_eq!(push(one, 2, t0 + GAP, 0, 19).map(|t| t.n), Some(2));
    }

    #[test]
    fn longer_only_while_a_longer_number_exists() {
        assert!(!longer(1, 0, 9));
        assert!(longer(1, 0, 10));
        assert!(longer(1, 0, 19));
        assert!(!longer(2, 0, 19));
        assert!(!longer(0, 0, 99));
        assert!(longer(9, 0, 99));
        assert!(!longer(10, 0, 99));
        assert!(longer(10, 0, 100));
        // Overflow is no number at all.
        assert!(!longer(usize::MAX, 0, usize::MAX));
        assert!(!longer(usize::MAX / 10 + 1, 0, usize::MAX));
        let big = Some(Typed { n: usize::MAX, at: Instant::now() });
        assert_eq!(push(big, 5, Instant::now(), 0, 9).map(|t| t.n), Some(5));
    }

    #[test]
    fn labels_line_up_and_stay_as_they_were_under_ten() {
        assert_eq!(label(Some(0), 10), "(0)");
        assert_eq!(label(None, 10), "   ");
        assert_eq!(label(Some(3), 0), "(3)");
        assert_eq!(label(Some(3), 11), "(3) ");
        assert_eq!(label(Some(10), 11), "(10)");
        assert_eq!(label(None, 11), "    ");
        assert_eq!(label(Some(7), 101), "(7)  ");
        assert_eq!(label(Some(100), 101), "(100)");
    }
}
