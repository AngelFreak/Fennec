//! The user's vocabulary: names and terms spelled their way. It corrects
//! the transcript afterwards instead of prompting the model, because a
//! prompt wrecks the Danish fine-tunes (Edda's FLEURS WER went from 7.8% to
//! 87.7% with "Nørregade, Vicevært, toneleje" as the prompt).

use std::ops::Range;

/// Danish endings kept when the stem matches a term: "Nørregaden",
/// "tonelejet". The closest stem wins.
const ENDINGS: [&str; 18] = [
    "", "s", "t", "n", "r", "e", "en", "et", "er", "ne", "ns", "ts", "rne", "ens", "ets", "ene", "erne",
    "ernes",
];

#[derive(Debug, Clone)]
struct Term {
    text: String,
    /// Lower case, without spaces or hyphens.
    key: String,
}

#[derive(Debug, Clone, Default)]
pub struct Vocabulary {
    terms: Vec<Term>,
}

impl Vocabulary {
    /// Terms separated by commas or new lines.
    pub fn parse(text: &str) -> Self {
        let terms = text
            .split([',', '\n', ';'])
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| Term {
                text: t.to_string(),
                key: key(t),
            })
            .filter(|t| !t.key.is_empty())
            .collect();
        Self { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// `text` with near-misses of the terms replaced, and `spans` (byte
    /// ranges of unsure words) moved to match.
    pub fn correct(&self, text: &str, spans: &[Range<usize>]) -> (String, Vec<Range<usize>>) {
        if self.terms.is_empty() {
            return (text.to_string(), spans.to_vec());
        }
        let words = words(text);
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        let mut i = 0;
        while i < words.len() {
            // One word, or two heard for one term ("Nørre gade"), whichever
            // is closer; one word on a tie.
            let found = [1, 2]
                .into_iter()
                .filter(|n| i + n <= words.len())
                .filter_map(|n| {
                    let span = words[i].start..words[i + n - 1].end;
                    let heard: String = words[i..i + n].iter().map(|w| &text[w.clone()]).collect();
                    // An ending lies inside the last word, never a whole word.
                    let last = key(&text[words[i + n - 1].clone()]).chars().count();
                    let (d, t) = self.best(&heard, last)?;
                    // "fra Nyiragongovulkanen" is not one term: the second
                    // word matches on its own at least as well.
                    if n == 2
                        && self
                            .best(&text[words[i + 1].clone()], last)
                            .is_some_and(|(d2, _)| d2 <= d)
                    {
                        return None;
                    }
                    Some((d, n, span, t))
                })
                .min_by_key(|(d, n, _, _)| (*d, *n))
                .map(|(_, n, span, t)| (n, span, t));
            match found {
                Some((n, span, replacement)) => {
                    if text[span.clone()] != replacement {
                        edits.push((span, replacement));
                    }
                    i += n;
                }
                None => i += 1,
            }
        }
        apply(text, spans, &edits)
    }

    /// The term `heard` is a near-miss of, spelled the user's way and with
    /// the heard Danish ending, and how far off it was.
    fn best(&self, heard: &str, last_word_chars: usize) -> Option<(usize, String)> {
        let heard_key = key(heard);
        let mut best: Option<(usize, String)> = None;
        for term in &self.terms {
            for ending in ENDINGS {
                let Some(stem) = heard_key.strip_suffix(ending) else {
                    continue;
                };
                if ending.chars().count() >= last_word_chars {
                    continue;
                }
                let d = distance(stem, &term.key);
                let len = term.key.chars().count();
                // Short words only get their spelling fixed, never swapped.
                let allowed = if len < 5 { 0 } else { (len / 5).max(1) };
                if d <= allowed && best.as_ref().is_none_or(|(b, _)| d < *b) {
                    let tail: String = heard
                        .chars()
                        .skip(heard.chars().count() - ending.chars().count())
                        .collect();
                    best = Some((d, format!("{}{tail}", term.text)));
                }
            }
        }
        best
    }
}

/// Byte ranges of the words (letters, digits and inner hyphens).
fn words(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        let part = c.is_alphanumeric() || (c == '-' && start.is_some());
        match (part, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push(s..trim_hyphen(text, s, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(s..trim_hyphen(text, s, text.len()));
    }
    out
}

fn trim_hyphen(text: &str, start: usize, end: usize) -> usize {
    start + text[start..end].trim_end_matches('-').len()
}

fn key(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(x != y))
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Applies non-overlapping edits in order, moving spans along. A span
/// touching an edited word covers its replacement.
fn apply(
    text: &str,
    spans: &[Range<usize>],
    edits: &[(Range<usize>, String)],
) -> (String, Vec<Range<usize>>) {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    // (old range, new range) of each edit.
    let mut moved = Vec::new();
    for (range, replacement) in edits {
        out.push_str(&text[at..range.start]);
        let start = out.len();
        out.push_str(replacement);
        moved.push((range.clone(), start..out.len()));
        at = range.end;
    }
    out.push_str(&text[at..]);
    let map = |pos: usize, is_end: bool| -> usize {
        let mut shift: isize = 0;
        for (old, new) in &moved {
            if pos < old.start || (pos == old.start && !is_end) {
                break;
            }
            if pos <= old.end && !(pos == old.start && is_end) {
                return if is_end { new.end } else { new.start };
            }
            shift += new.len() as isize - old.len() as isize;
        }
        (pos as isize + shift) as usize
    };
    let spans = spans
        .iter()
        .map(|r| map(r.start, false)..map(r.end, true))
        .collect();
    (out, spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(vocab: &str, text: &str) -> String {
        Vocabulary::parse(vocab).correct(text, &[]).0
    }

    #[test]
    fn a_near_miss_takes_the_users_spelling() {
        assert_eq!(
            fix("Nørregade", "Vi mødes på Nørregarde 14."),
            "Vi mødes på Nørregade 14."
        );
        assert_eq!(
            fix("Rossby-tallet", "Jo mindre Rosbytallet er"),
            "Jo mindre Rossby-tallet er"
        );
    }

    #[test]
    fn a_term_heard_as_two_words_is_joined() {
        assert_eq!(fix("Nørregade", "på Nørre gade i dag"), "på Nørregade i dag");
    }

    #[test]
    fn a_word_before_a_term_is_not_swallowed_into_it() {
        assert_eq!(
            fix("Nyiragongo-vulkanen", "lava fra Nyiragongovulkanen som"),
            "lava fra Nyiragongo-vulkanen som"
        );
    }

    #[test]
    fn danish_endings_stay() {
        assert_eq!(fix("toneleje", "i et tonelejet"), "i et tonelejet");
        assert_eq!(fix("toneleje", "i et tonelejer"), "i et tonelejer");
        assert_eq!(fix("Nørregade", "hele Nørregarden"), "hele Nørregaden");
    }

    #[test]
    fn short_terms_only_fix_their_spelling() {
        assert_eq!(fix("BBR", "tjek bbr og bar"), "tjek BBR og bar");
    }

    #[test]
    fn unrelated_words_are_left_alone() {
        let text = "Fugten kommer fra en utæt nedløbsbrønd.";
        assert_eq!(fix("Nørregade, Vicevært, BBR", text), text);
    }

    #[test]
    fn unsure_word_spans_follow_the_text() {
        let text = "på Nørre garde i dag med regn";
        let regn = text.find("regn").unwrap();
        let garde = text.find("garde").unwrap();
        let (out, spans) =
            Vocabulary::parse("Nørregade").correct(text, &[garde..garde + "garde".len(), regn..regn + 4]);
        assert_eq!(&out[spans[0].clone()], "Nørregade");
        assert_eq!(&out[spans[1].clone()], "regn");
    }
}
