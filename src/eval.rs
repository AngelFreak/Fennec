//! Word and character error rates for comparing models against references.

/// Lower-cases, drops punctuation and collapses whitespace so that formatting
/// differences between models and references do not count as errors.
/// Letters (including æ, ø, å), digits and spaces survive.
pub fn normalize(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                ' '
            }
        })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Running totals of edits and reference length, so several utterances can be
/// combined into one corpus-level score (sum of errors / sum of lengths).
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ErrorCount {
    pub errors: usize,
    pub reference_len: usize,
}

impl ErrorCount {
    pub fn add(&mut self, other: ErrorCount) {
        self.errors += other.errors;
        self.reference_len += other.reference_len;
    }

    /// Error rate as a fraction. An empty reference scores 0 when the
    /// hypothesis is also empty, otherwise 1.
    pub fn rate(&self) -> f64 {
        if self.reference_len == 0 {
            return if self.errors == 0 { 0.0 } else { 1.0 };
        }
        self.errors as f64 / self.reference_len as f64
    }
}

/// Word-level edit count after normalization.
pub fn word_errors(reference: &str, hypothesis: &str) -> ErrorCount {
    let r = normalize(reference);
    let h = normalize(hypothesis);
    let rw: Vec<&str> = r.split_whitespace().collect();
    let hw: Vec<&str> = h.split_whitespace().collect();
    ErrorCount {
        errors: edit_distance(&rw, &hw),
        reference_len: rw.len(),
    }
}

/// Character-level edit count after normalization (spaces count as characters).
pub fn char_errors(reference: &str, hypothesis: &str) -> ErrorCount {
    let rc: Vec<char> = normalize(reference).chars().collect();
    let hc: Vec<char> = normalize(hypothesis).chars().collect();
    ErrorCount {
        errors: edit_distance(&rc, &hc),
        reference_len: rc.len(),
    }
}

fn edit_distance<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let substitution = prev[j] + usize::from(x != y);
            cur[j + 1] = substitution.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_punctuation_and_case_but_keeps_danish_letters() {
        assert_eq!(normalize("Ærø, Østergade -- Å!"), "ærø østergade å");
    }

    #[test]
    fn identical_text_has_no_word_errors() {
        assert_eq!(word_errors("Det regner i dag.", "det regner i dag").errors, 0);
    }

    #[test]
    fn one_substituted_word_in_four_is_25_percent() {
        let e = word_errors("det regner i dag", "det sner i dag");
        assert_eq!(
            e,
            ErrorCount {
                errors: 1,
                reference_len: 4
            }
        );
        assert!((e.rate() - 0.25).abs() < 1e-9);
    }

    #[test]
    fn insertions_and_deletions_both_count() {
        assert_eq!(word_errors("a b c", "a c").errors, 1);
        assert_eq!(word_errors("a b c", "a b x c").errors, 1);
    }

    #[test]
    fn char_errors_count_letter_level_differences() {
        assert_eq!(char_errors("kælder", "kelder").errors, 1);
    }

    #[test]
    fn corpus_rate_sums_errors_and_lengths() {
        let mut total = ErrorCount::default();
        total.add(ErrorCount {
            errors: 1,
            reference_len: 4,
        });
        total.add(ErrorCount {
            errors: 0,
            reference_len: 6,
        });
        assert!((total.rate() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn empty_reference_with_output_is_full_error() {
        assert_eq!(word_errors("", "noget").rate(), 1.0);
        assert_eq!(word_errors("", "").rate(), 0.0);
    }
}
