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

/// Hits and misses for one kind of punctuation mark.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Marks {
    pub right: usize,
    /// In the reference, not the hypothesis.
    pub missed: usize,
    /// In the hypothesis, not the reference.
    pub extra: usize,
}

impl Marks {
    pub fn add(&mut self, o: Marks) {
        self.right += o.right;
        self.missed += o.missed;
        self.extra += o.extra;
    }

    pub fn precision(&self) -> f64 {
        ratio(self.right, self.right + self.extra)
    }

    pub fn recall(&self) -> f64 {
        ratio(self.right, self.right + self.missed)
    }

    pub fn f1(&self) -> f64 {
        ratio(2 * self.right, 2 * self.right + self.missed + self.extra)
    }
}

fn ratio(a: usize, b: usize) -> f64 {
    if b == 0 { 1.0 } else { a as f64 / b as f64 }
}

/// Commas and sentence ends (. ? !) compared word by word.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Punctuation {
    pub commas: Marks,
    pub ends: Marks,
}

impl Punctuation {
    pub fn add(&mut self, o: Punctuation) {
        self.commas.add(o.commas);
        self.ends.add(o.ends);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mark {
    Comma,
    End,
}

/// Normalized words, each with the mark that follows it.
fn marked_words(text: &str) -> Vec<(String, Option<Mark>)> {
    let mut out: Vec<(String, Option<Mark>)> = Vec::new();
    for token in text.split_whitespace() {
        let bare = token.trim_end_matches(['"', '\'', '»', '”', ')']);
        let mark = match bare.chars().last() {
            Some(',') => Some(Mark::Comma),
            Some('.' | '?' | '!') => Some(Mark::End),
            _ => None,
        };
        let words: Vec<String> = normalize(token).split_whitespace().map(String::from).collect();
        if words.is_empty() {
            // A lone dash or quote: its mark belongs to the word before.
            if let (Some(m), Some(last)) = (mark, out.last_mut()) {
                last.1 = Some(m);
            }
            continue;
        }
        let n = words.len();
        for (i, w) in words.into_iter().enumerate() {
            out.push((w, if i + 1 == n { mark } else { None }));
        }
    }
    out
}

/// Scores the hypothesis's punctuation against the reference's, on the
/// words the two share after alignment (misheard words count as shared).
/// The mark at the very end of the reference is not scored: every
/// transcript ends a sentence there.
pub fn punctuation(reference: &str, hypothesis: &str) -> Punctuation {
    let r = marked_words(reference);
    let h = marked_words(hypothesis);
    let rw: Vec<&str> = r.iter().map(|(w, _)| w.as_str()).collect();
    let hw: Vec<&str> = h.iter().map(|(w, _)| w.as_str()).collect();
    let mut p = Punctuation::default();
    for (i, j) in aligned_pairs(&rw, &hw) {
        if i + 1 == r.len() {
            continue;
        }
        for (kind, marks) in [(Mark::Comma, &mut p.commas), (Mark::End, &mut p.ends)] {
            match (r[i].1 == Some(kind), h[j].1 == Some(kind)) {
                (true, true) => marks.right += 1,
                (true, false) => marks.missed += 1,
                (false, true) => marks.extra += 1,
                (false, false) => {}
            }
        }
    }
    p
}

/// Index pairs of matched or substituted words in a minimal alignment.
pub(crate) fn aligned_pairs<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let sub = d[i - 1][j - 1] + usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = sub.min(d[i - 1][j] + 1).min(d[i][j - 1] + 1);
        }
    }
    let (mut i, mut j) = (a.len(), b.len());
    let mut pairs = Vec::new();
    while i > 0 && j > 0 {
        if d[i][j] == d[i - 1][j - 1] + usize::from(a[i - 1] != b[j - 1]) {
            pairs.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if d[i][j] == d[i - 1][j] + 1 {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    pairs.reverse();
    pairs
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
    fn punctuation_is_scored_on_the_words_both_texts_share() {
        let p = punctuation(
            "Det regner, og det blæser. Vi bliver hjemme.",
            "Det regner og det blæser, vi bliver hjemme.",
        );
        // The comma after "regner" was missed; the one after "blæser" sits
        // where a sentence should have ended.
        assert_eq!(
            p.commas,
            Marks {
                right: 0,
                missed: 1,
                extra: 1
            }
        );
        assert_eq!(
            p.ends,
            Marks {
                right: 0,
                missed: 1,
                extra: 0
            }
        );
    }

    #[test]
    fn the_final_mark_of_a_text_is_not_scored() {
        let p = punctuation("Hej. Farvel.", "Hej. Farvel");
        assert_eq!(
            p.ends,
            Marks {
                right: 1,
                missed: 0,
                extra: 0
            }
        );
    }

    #[test]
    fn misheard_words_still_carry_their_punctuation() {
        let p = punctuation("Goma er sikkert, siger de.", "Gome er sikkert, siger de.");
        assert_eq!(p.commas.right, 1);
    }

    #[test]
    fn f1_combines_hits_misses_and_extras() {
        let m = Marks {
            right: 2,
            missed: 2,
            extra: 0,
        };
        assert!((m.f1() - 2.0 / 3.0).abs() < 1e-9);
    }

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
