//! The dictated document as a list of paragraphs, built from live events.
//! The GTK editor mirrors this; it is kept separate so it can be tested.

use crate::commands::Command;
use crate::store::Paragraph;

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Text appended to paragraph `index` (which may be new).
    Appended {
        index: usize,
    },
    NewParagraph,
    Edited {
        index: usize,
    },
}

#[derive(Debug, Clone, Default)]
pub struct Transcript {
    pub paragraphs: Vec<Paragraph>,
    /// Grey preview of the utterance being spoken.
    pub preview: Option<String>,
    /// A pause this long (between utterances) starts a new paragraph.
    pub paragraph_gap_ms: i64,
    force_new: bool,
}

impl Transcript {
    pub fn new(paragraph_gap_ms: i64) -> Self {
        Self {
            paragraph_gap_ms,
            ..Default::default()
        }
    }

    pub fn set_preview(&mut self, text: Option<String>) {
        self.preview = text.filter(|t| !t.trim().is_empty());
    }

    /// Adds a finished utterance. Times are absolute within the document.
    pub fn add_final(
        &mut self,
        text: &str,
        start_ms: i64,
        end_ms: i64,
        low_confidence: &[std::ops::Range<usize>],
    ) -> Option<Change> {
        self.preview = None;
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let gap_breaks = self
            .paragraphs
            .last()
            .and_then(|p| p.end_ms)
            .is_some_and(|end| start_ms - end >= self.paragraph_gap_ms);
        if self.paragraphs.is_empty() || self.force_new || gap_breaks {
            self.force_new = false;
            self.paragraphs.push(Paragraph {
                start_ms: Some(start_ms),
                end_ms: Some(end_ms),
                low_confidence: low_confidence.to_vec(),
                ..Paragraph::new(&sentence_case("", text))
            });
            return Some(Change::Appended {
                index: self.paragraphs.len() - 1,
            });
        }
        let index = self.paragraphs.len() - 1;
        let p = &mut self.paragraphs[index];
        let sep = if p.text.ends_with('\n') { "" } else { " " };
        let shift = p.text.len() + sep.len();
        let text = sentence_case(&p.text, text);
        p.text.push_str(sep);
        p.text.push_str(&text);
        p.low_confidence
            .extend(low_confidence.iter().map(|r| r.start + shift..r.end + shift));
        p.end_ms = Some(end_ms);
        p.start_ms = p.start_ms.or(Some(start_ms));
        Some(Change::Appended { index })
    }

    /// Applies an editing command. `StopDictation` is the session's to handle.
    pub fn apply(&mut self, command: Command) -> Option<Change> {
        self.preview = None;
        match command {
            Command::NewParagraph => {
                self.force_new = true;
                Some(Change::NewParagraph)
            }
            Command::NewLine => {
                let index = self.paragraphs.len().checked_sub(1)?;
                self.paragraphs[index].text.push('\n');
                Some(Change::Edited { index })
            }
            Command::DeleteLastSentence => {
                let index = self.paragraphs.len().checked_sub(1)?;
                let p = &mut self.paragraphs[index];
                let cut = last_sentence_start(&p.text);
                p.text.truncate(cut);
                let len = p.text.trim_end().len();
                p.text.truncate(len);
                p.low_confidence.retain(|r| r.end <= len);
                if p.text.is_empty() {
                    self.paragraphs.pop();
                }
                Some(Change::Edited { index })
            }
            Command::StopDictation => None,
            Command::Punctuate(mark) => {
                let index = self.paragraphs.len().checked_sub(1)?;
                let p = &mut self.paragraphs[index];
                p.text.truncate(punctuation_cut(&p.text));
                p.text.push(mark);
                Some(Change::Edited { index })
            }
        }
    }
}

/// Byte length of `before` to keep when a spoken mark goes at its end: drops
/// trailing spaces and the mark the model may already have written.
pub fn punctuation_cut(before: &str) -> usize {
    let body = before.trim_end();
    body.strip_suffix(['.', ',', '!', '?', ':', ';'])
        .unwrap_or(body)
        .len()
}

/// `text` with a capital first letter when it starts a sentence after
/// `before` (nothing, or text ending in . ! ?).
pub fn sentence_case(before: &str, text: &str) -> String {
    let starts = before.trim_end().is_empty() || before.trim_end().ends_with(['.', '!', '?']);
    let mut chars = text.chars();
    match chars.next() {
        // Same byte length, so unsure-word ranges stay valid.
        Some(c) if starts && c.is_lowercase() && c.to_uppercase().count() == 1 => {
            let upper = c.to_uppercase().next().unwrap_or(c);
            if upper.len_utf8() == c.len_utf8() {
                format!("{upper}{}", chars.as_str())
            } else {
                text.to_string()
            }
        }
        _ => text.to_string(),
    }
}

/// Byte index where the last sentence begins (after the previous . ! ? or newline).
pub fn last_sentence_start(text: &str) -> usize {
    let body = text.trim_end().trim_end_matches(['.', '!', '?']);
    body.rfind(['.', '!', '?', '\n']).map(|i| i + 1).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(t: &Transcript) -> Vec<&str> {
        t.paragraphs.iter().map(|p| p.text.as_str()).collect()
    }

    #[test]
    fn utterances_join_until_a_long_pause_or_new_paragraph_command() {
        let mut t = Transcript::new(3_000);
        t.add_final("Første.", 0, 1_000, &[]);
        t.add_final("Anden.", 1_500, 2_500, &[]);
        t.apply(Command::NewParagraph);
        t.add_final("Tredje.", 3_000, 4_000, &[]);
        t.add_final("Fjerde.", 8_000, 9_000, &[]);
        assert_eq!(texts(&t), ["Første. Anden.", "Tredje.", "Fjerde."]);
        assert_eq!(t.paragraphs[0].end_ms, Some(2_500));
    }

    #[test]
    fn delete_last_sentence_removes_only_the_last_one() {
        let mut t = Transcript::new(3_000);
        t.add_final("Det regner. Det sner meget.", 0, 1_000, &[]);
        t.apply(Command::DeleteLastSentence);
        assert_eq!(texts(&t), ["Det regner."]);
        t.apply(Command::DeleteLastSentence);
        assert!(t.paragraphs.is_empty());
        assert_eq!(t.apply(Command::DeleteLastSentence), None);
    }

    #[test]
    fn new_line_keeps_the_paragraph_and_the_next_text_follows_it() {
        let mut t = Transcript::new(3_000);
        t.add_final("Punkt et.", 0, 1_000, &[]);
        t.apply(Command::NewLine);
        t.add_final("Punkt to.", 1_200, 2_000, &[]);
        assert_eq!(texts(&t), ["Punkt et.\nPunkt to."]);
    }

    #[test]
    fn low_confidence_spans_shift_when_joined() {
        let mut t = Transcript::new(3_000);
        t.add_final("En", 0, 500, &[]);
        t.add_final("utæt brønd", 600, 1_000, &[0..5]);
        let p = &t.paragraphs[0];
        assert_eq!(&p.text[p.low_confidence[0].clone()], "utæt");
    }

    #[test]
    fn a_final_clears_the_preview() {
        let mut t = Transcript::new(3_000);
        t.set_preview(Some("Det reg".into()));
        t.add_final("Det regner.", 0, 1_000, &[]);
        assert!(t.preview.is_none());
    }

    #[test]
    fn punctuation_replaces_the_mark_the_model_already_wrote() {
        assert_eq!(punctuation_cut("Jeg prøver lige."), "Jeg prøver lige".len());
        assert_eq!(punctuation_cut("Jeg prøver lige "), "Jeg prøver lige".len());
        assert_eq!(punctuation_cut("Jeg prøver lige"), "Jeg prøver lige".len());
        assert_eq!(punctuation_cut(""), 0);
    }

    #[test]
    fn a_new_sentence_starts_with_a_capital() {
        assert_eq!(sentence_case("Det regner.", "hvad nu hvis"), "Hvad nu hvis");
        assert_eq!(sentence_case("", "ølet er koldt"), "Ølet er koldt");
        assert_eq!(sentence_case("Det regner,", "og det sner"), "og det sner");
        assert_eq!(sentence_case("Hvad?", "1 time"), "1 time");
    }

    #[test]
    fn spoken_punctuation_ends_the_sentence() {
        let mut t = Transcript::new(3_000);
        t.add_final("Jeg prøver lige", 0, 1_000, &[]);
        t.apply(Command::Punctuate('.'));
        t.add_final("hvad nu hvis", 1_100, 2_000, &[]);
        assert_eq!(texts(&t), ["Jeg prøver lige. Hvad nu hvis"]);
    }
}
