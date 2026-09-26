//! Fuzzy matching for the command palette: the query's characters must
//! appear in order, and matches at word starts and in runs score higher, so
//! `nt` finds "New Tab" before "Next Match".

/// How well `query` matches `candidate`, higher being better, or `None` when
/// it doesn't. Case and spaces in the query are ignored; an empty query
/// matches everything equally.
pub fn score(query: &str, candidate: &str) -> Option<i32> {
    let query: Vec<char> = query
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let chars: Vec<char> = candidate.chars().collect();
    let lowered: Vec<char> = chars
        .iter()
        .map(|ch| ch.to_lowercase().next().unwrap_or(*ch))
        .collect();
    let word_start = |j: usize| {
        j == 0
            || !chars[j - 1].is_alphanumeric()
            || (chars[j].is_uppercase() && chars[j - 1].is_lowercase())
    };

    // best[j]: the best score with the query so far matched, its last
    // character at `j`.
    const NONE: i32 = i32::MIN / 2;
    let mut best = vec![NONE; chars.len()];
    for (i, wanted) in query.iter().enumerate() {
        let mut next = vec![NONE; chars.len()];
        let mut before = NONE;
        for j in 0..chars.len() {
            if lowered[j] == *wanted {
                let bonus = if word_start(j) { 10 } else { 1 };
                next[j] = if i == 0 {
                    // Earlier first matches read as more relevant.
                    bonus - j as i32 / 4
                } else {
                    let run = if j > 0 && best[j - 1] > NONE {
                        best[j - 1] + bonus + 8
                    } else {
                        NONE
                    };
                    let jump = if before > NONE {
                        before + bonus - 2
                    } else {
                        NONE
                    };
                    run.max(jump)
                };
            }
            before = before.max(best[j]);
        }
        best = next;
    }
    let top = best.into_iter().max().filter(|&score| score > NONE / 2)?;
    // Among equals, the shorter candidate is the closer match.
    Some(top * 4 - chars.len() as i32)
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn characters_must_appear_in_order() {
        assert!(score("nt", "New Tab").is_some());
        assert!(score("tn", "New Tab").is_none());
        assert!(score("xyz", "New Tab").is_none());
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("  ", "anything"), Some(0));
    }

    #[test]
    fn case_and_spaces_are_ignored() {
        assert!(score("NEW tab", "New Tab").is_some());
        assert!(score("exp csv", "Export Results as CSV…").is_some());
    }

    #[test]
    fn word_starts_and_runs_rank_first() {
        let rank = |query, a, b| score(query, a).unwrap() > score(query, b).unwrap();
        assert!(rank("nt", "New Tab", "Next Match"));
        assert!(rank("tab", "Next Tab", "Toggle Match Case Table"));
        assert!(rank("case", "Match Case", "Close Tab Searches"));
        assert!(rank("theme", "Toggle Theme", "The Home Menu"));
        // Camel case counts as word starts.
        assert!(rank("fb", "FooBar", "foobar"));
    }

    #[test]
    fn shorter_candidates_win_ties() {
        let rank = |query, a, b| score(query, a).unwrap() > score(query, b).unwrap();
        assert!(rank("tab", "Tab", "Tab strip"));
    }
}
