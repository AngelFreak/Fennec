//! Word-level diff for the clean-up review.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Same(String),
    Removed(String),
    Added(String),
}

/// Splits into words, keeping the whitespace after each word attached so
/// joining the pieces gives back the original text.
fn words(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_space = false;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            in_space = true;
        } else if in_space {
            out.push(&text[start..i]);
            start = i;
            in_space = false;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Longest-common-subsequence diff over words. Neighbouring changes of the
/// same kind are merged so the view shows phrases, not single words.
pub fn word_diff(old: &str, new: &str) -> Vec<Change> {
    let a = words(old);
    let b = words(new);
    let key = |w: &str| w.trim_end().to_string();
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if key(a[i]) == key(b[j]) {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out: Vec<Change> = Vec::new();
    let mut push = |c: Change| match (out.last_mut(), c) {
        (Some(Change::Same(s)), Change::Same(t))
        | (Some(Change::Removed(s)), Change::Removed(t))
        | (Some(Change::Added(s)), Change::Added(t)) => s.push_str(&t),
        (_, c) => out.push(c),
    };
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && key(a[i]) == key(b[j]) {
            // Keep the new spacing so accepted text reads as the model wrote it.
            push(Change::Same(b[j].to_string()));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            push(Change::Removed(a[i].to_string()));
            i += 1;
        } else {
            push(Change::Added(b[j].to_string()));
            j += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use Change::*;

    #[test]
    fn identical_text_is_one_same_run() {
        assert_eq!(
            word_diff("hej med dig", "hej med dig"),
            vec![Same("hej med dig".into())]
        );
    }

    #[test]
    fn a_replaced_word_shows_as_removed_then_added() {
        assert_eq!(
            word_diff(
                "vi mødes på mandag øh klokken ti",
                "vi mødes på mandag klokken 10"
            ),
            vec![
                Same("vi mødes på mandag ".into()),
                Removed("øh ".into()),
                Same("klokken ".into()),
                Removed("ti".into()),
                Added("10".into()),
            ]
        );
    }

    #[test]
    fn the_new_side_reassembles_the_new_text() {
        let new = "Det er godt, at vi ses.";
        let rebuilt: String = word_diff("det er godt at vi ses", new)
            .into_iter()
            .filter_map(|c| match c {
                Same(s) | Added(s) => Some(s),
                Removed(_) => None,
            })
            .collect();
        assert_eq!(rebuilt, new);
    }
}
