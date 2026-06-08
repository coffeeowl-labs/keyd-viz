//! Ranking for the searchable key picker (Phase 6 E2).
//!
//! Slint has no sort/filter primitives, so the full key vocabulary lives Rust-side and
//! only the ranked, capped slice is pushed to the UI per keystroke. This module is the
//! pure core of that: given the vocabulary and a query, produce the best `cap` matches.

use slint::SharedString;

/// How many results the picker shows at once. Enough to browse a category without
/// scrolling forever; anything past it is reported as "+N more — refine search".
pub const RESULT_CAP: usize = 80;

/// How well a candidate matched the query, best first. Drives the primary sort tier.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    Exact,
    Prefix,
    Substring,
}

fn tier(name: &str, q: &str) -> Option<Tier> {
    if name == q {
        Some(Tier::Exact)
    } else if name.starts_with(q) {
        Some(Tier::Prefix)
    } else if name.contains(q) {
        Some(Tier::Substring)
    } else {
        None
    }
}

/// Rank `keys` against `query` and keep the best `cap`. Returns `(results, truncated)`
/// where `truncated` is how many matches were dropped past the cap (0 = all shown).
///
/// Matching is case-insensitive. An empty query returns the vocabulary in its original
/// (curated) order. A non-empty query keeps only matches, ordered by match quality
/// (exact > prefix > substring), then shortest name, then alphabetically — the cap is
/// applied *after* ranking, so the shown `cap` are the best `cap`, not the first `cap`.
pub fn rank_keys(keys: &[SharedString], query: &str, cap: usize) -> (Vec<SharedString>, usize) {
    let q = query.trim().to_lowercase();

    if q.is_empty() {
        let truncated = keys.len().saturating_sub(cap);
        return (keys.iter().take(cap).cloned().collect(), truncated);
    }

    let mut matches: Vec<(Tier, &SharedString)> = keys
        .iter()
        .filter_map(|k| tier(&k.to_lowercase(), &q).map(|t| (t, k)))
        .collect();
    matches.sort_by(|(ta, a), (tb, b)| {
        ta.cmp(tb).then(a.len().cmp(&b.len())).then_with(|| a.as_str().cmp(b.as_str()))
    });

    let truncated = matches.len().saturating_sub(cap);
    (matches.into_iter().take(cap).map(|(_, k)| k.clone()).collect(), truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab(names: &[&str]) -> Vec<SharedString> {
        names.iter().map(|s| (*s).into()).collect()
    }

    fn names(r: &[SharedString]) -> Vec<&str> {
        r.iter().map(SharedString::as_str).collect()
    }

    #[test]
    fn exact_match_ranks_first() {
        let keys = vocab(&["escape", "esc", "noesc"]);
        let (res, trunc) = rank_keys(&keys, "esc", 10);
        assert_eq!(names(&res)[0], "esc"); // exact beats prefix ("escape") and substring ("noesc")
        assert_eq!(trunc, 0);
    }

    #[test]
    fn prefix_beats_substring() {
        let keys = vocab(&["volumeup", "volumedown", "kpequalvol"]);
        let (res, _) = rank_keys(&keys, "vol", 10);
        // both "volume*" are prefix matches (sorted by length then alpha), substring last
        assert_eq!(names(&res), vec!["volumeup", "volumedown", "kpequalvol"]);
    }

    #[test]
    fn case_insensitive() {
        let keys = vocab(&["VolumeUp", "esc"]);
        let (res, _) = rank_keys(&keys, "VOL", 10);
        assert_eq!(names(&res), vec!["VolumeUp"]);
    }

    #[test]
    fn cap_applied_after_ranking_with_truncated_count() {
        // "kp1".."kp4" all prefix-match "kp"; cap=2 keeps the two best (shortest+alpha).
        let keys = vocab(&["kp1", "kp2", "kp3", "kp4"]);
        let (res, trunc) = rank_keys(&keys, "kp", 2);
        assert_eq!(names(&res), vec!["kp1", "kp2"]);
        assert_eq!(trunc, 2);
    }

    #[test]
    fn empty_query_returns_capped_vocabulary_in_order() {
        let keys = vocab(&["esc", "1", "2", "3"]);
        let (res, trunc) = rank_keys(&keys, "", 2);
        assert_eq!(names(&res), vec!["esc", "1"]);
        assert_eq!(trunc, 2);
    }

    #[test]
    fn whitespace_only_query_is_treated_as_empty() {
        let keys = vocab(&["esc", "tab"]);
        let (res, _) = rank_keys(&keys, "   ", 10);
        assert_eq!(names(&res), vec!["esc", "tab"]);
    }

    #[test]
    fn no_match_yields_empty() {
        let keys = vocab(&["esc", "tab"]);
        let (res, trunc) = rank_keys(&keys, "zzz", 10);
        assert!(res.is_empty());
        assert_eq!(trunc, 0);
    }
}
