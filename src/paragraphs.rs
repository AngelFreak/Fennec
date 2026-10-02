//! Groups engine segments into paragraphs: a long pause starts a new one.

use crate::engine::Segment;
use crate::store::Paragraph;

pub struct ParagraphBuilder {
    /// A pause at least this long starts a new paragraph.
    pub gap_ms: i64,
    /// Paragraphs are closed once they span this long.
    pub max_ms: i64,
    current: Option<Paragraph>,
}

impl Default for ParagraphBuilder {
    fn default() -> Self {
        Self {
            gap_ms: 1_500,
            max_ms: 90_000,
            current: None,
        }
    }
}

impl ParagraphBuilder {
    pub fn new(gap_ms: i64, max_ms: i64) -> Self {
        Self {
            gap_ms,
            max_ms,
            current: None,
        }
    }

    /// Adds a segment whose times are relative to `offset_ms`. Returns the
    /// previous paragraph if this segment started a new one.
    pub fn push(&mut self, seg: &Segment, offset_ms: i64) -> Option<Paragraph> {
        let (start, end) = (seg.start_ms + offset_ms, seg.end_ms + offset_ms);
        let breaks = self.current.as_ref().is_some_and(|p| {
            start - p.end_ms.unwrap_or(start) >= self.gap_ms
                || end - p.start_ms.unwrap_or(start) > self.max_ms
        });
        let finished = if breaks { self.current.take() } else { None };
        match &mut self.current {
            Some(p) => {
                let shift = p.text.len() + 1;
                p.text.push(' ');
                p.text.push_str(&seg.text);
                p.low_confidence
                    .extend(seg.low_confidence.iter().map(|r| r.start + shift..r.end + shift));
                p.end_ms = Some(end);
            }
            None => {
                self.current = Some(Paragraph {
                    start_ms: Some(start),
                    end_ms: Some(end),
                    low_confidence: seg.low_confidence.clone(),
                    ..Paragraph::new(&seg.text)
                })
            }
        }
        finished
    }

    /// The paragraph still being built.
    pub fn current(&self) -> Option<&Paragraph> {
        self.current.as_ref()
    }

    pub fn finish(&mut self) -> Option<Paragraph> {
        self.current.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: i64, end: i64, text: &str) -> Segment {
        Segment {
            start_ms: start,
            end_ms: end,
            text: text.into(),
            low_confidence: vec![],
        }
    }

    #[test]
    fn short_pauses_continue_the_paragraph() {
        let mut b = ParagraphBuilder::default();
        assert!(b.push(&seg(0, 2_000, "Første."), 0).is_none());
        assert!(b.push(&seg(2_500, 4_000, "Anden."), 0).is_none());
        let p = b.finish().unwrap();
        assert_eq!(p.text, "Første. Anden.");
        assert_eq!((p.start_ms, p.end_ms), (Some(0), Some(4_000)));
    }

    #[test]
    fn a_long_pause_closes_the_paragraph() {
        let mut b = ParagraphBuilder::default();
        b.push(&seg(0, 2_000, "Første."), 0);
        let done = b.push(&seg(5_000, 6_000, "Nyt."), 0).unwrap();
        assert_eq!(done.text, "Første.");
        assert_eq!(b.current().unwrap().text, "Nyt.");
    }

    #[test]
    fn offsets_make_times_absolute_and_spans_shift_with_the_text() {
        let mut b = ParagraphBuilder::default();
        b.push(&seg(0, 1_000, "en"), 10_000);
        let mut s = seg(1_100, 2_000, "utæt brønd");
        s.low_confidence = vec![0..5]; // "utæt" is 5 bytes
        b.push(&s, 10_000);
        let p = b.finish().unwrap();
        assert_eq!(p.start_ms, Some(10_000));
        assert_eq!(&p.text[p.low_confidence[0].clone()], "utæt");
    }

    #[test]
    fn very_long_speech_is_split() {
        let mut b = ParagraphBuilder::new(1_500, 10_000);
        b.push(&seg(0, 6_000, "a"), 0);
        assert!(b.push(&seg(6_100, 12_000, "b"), 0).is_some());
    }
}
