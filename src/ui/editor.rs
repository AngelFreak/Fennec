//! The document editor: a text view where each line is a paragraph.
//! Paragraph timestamps ride on text marks at line starts, so they survive
//! edits; unsure words are a text tag. Dictation inserts at the cursor.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gtk::prelude::*;

use crate::commands::Command;
use crate::store::Paragraph;
use crate::transcript::{last_sentence_start, punctuation_cut, sentence_case};

/// Mark name → (start_ms, end_ms) of the paragraph starting at that mark.
type ParagraphTimes = HashMap<String, (Option<i64>, Option<i64>)>;

/// Line breaks inside a paragraph (the "ny linje" command).
pub const LINE_BREAK: char = '\u{2028}';

pub struct Editor {
    pub view: gtk::TextView,
    buffer: gtk::TextBuffer,
    tag_preview: gtk::TextTag,
    tag_unsure: gtk::TextTag,
    times: RefCell<ParagraphTimes>,
    next_mark: Cell<u64>,
    force_new: Cell<bool>,
    last_end_ms: Cell<Option<i64>>,
    pub paragraph_gap_ms: Cell<i64>,
    /// Set while the editor itself changes the text, so autosave can tell
    /// dictation from typing if it needs to.
    programmatic: Cell<bool>,
}

impl Editor {
    pub fn new() -> Rc<Self> {
        let buffer = gtk::TextBuffer::new(None);
        let tag_preview = buffer
            .create_tag(
                Some("preview"),
                &[("style", &gtk::pango::Style::Italic), ("foreground", &"#6B7280")],
            )
            .expect("new tag");
        let tag_unsure = buffer
            .create_tag(
                Some("unsure"),
                &[
                    ("underline", &gtk::pango::Underline::Error),
                    ("underline-rgba", &gtk::gdk::RGBA::new(0.76, 0.25, 0.05, 1.0)),
                ],
            )
            .expect("new tag");
        let view = gtk::TextView::builder()
            .buffer(&buffer)
            .wrap_mode(gtk::WrapMode::WordChar)
            .pixels_below_lines(24)
            .pixels_inside_wrap(4)
            .left_margin(0)
            .right_margin(0)
            .css_classes(["fx-editor"])
            .vexpand(true)
            .build();
        view.update_property(&[gtk::accessible::Property::Label("Document text")]);
        Rc::new(Self {
            view,
            buffer,
            tag_preview,
            tag_unsure,
            times: RefCell::default(),
            next_mark: Cell::new(0),
            force_new: Cell::new(false),
            last_end_ms: Cell::new(None),
            paragraph_gap_ms: Cell::new(3_000),
            programmatic: Cell::new(false),
        })
    }

    pub fn buffer(&self) -> &gtk::TextBuffer {
        &self.buffer
    }

    pub fn is_programmatic(&self) -> bool {
        self.programmatic.get()
    }

    fn quietly(&self, f: impl FnOnce()) {
        self.programmatic.set(true);
        f();
        self.programmatic.set(false);
    }

    /// Replaces the text with `paragraphs`.
    pub fn load(&self, paragraphs: &[Paragraph]) {
        self.quietly(|| {
            for name in self.times.borrow_mut().drain().map(|(k, _)| k) {
                if let Some(m) = self.buffer.mark(&name) {
                    self.buffer.delete_mark(&m);
                }
            }
            self.buffer.set_text("");
            for (i, p) in paragraphs.iter().enumerate() {
                let mut end = self.buffer.end_iter();
                if i > 0 {
                    self.buffer.insert(&mut end, "\n");
                }
                let line_start = self.buffer.end_iter().offset();
                let mut end = self.buffer.end_iter();
                self.buffer
                    .insert(&mut end, &p.text.replace('\n', &LINE_BREAK.to_string()));
                self.add_mark(line_start, p.start_ms, p.end_ms);
                self.tag_spans(line_start, &p.text, &p.low_confidence);
            }
            self.buffer.place_cursor(&self.buffer.end_iter());
        });
        self.last_end_ms
            .set(paragraphs.iter().filter_map(|p| p.end_ms).max());
        self.force_new.set(false);
    }

    /// The text as paragraphs, with timestamps and unsure spans read back.
    pub fn paragraphs(&self) -> Vec<Paragraph> {
        let mut lines: Vec<Paragraph> = Vec::new();
        for line in 0..self.buffer.line_count() {
            let Some(start) = self.buffer.iter_at_line(line) else {
                continue;
            };
            let mut end = start;
            if !end.ends_line() {
                end.forward_to_line_end();
            }
            let text = self.text_without_preview(&start, &end);
            lines.push(Paragraph {
                low_confidence: self.unsure_spans(&start, &end),
                ..Paragraph::new(&text)
            });
        }
        for (name, (s, e)) in self.times.borrow().iter() {
            let Some(mark) = self.buffer.mark(name) else {
                continue;
            };
            let line = self.buffer.iter_at_mark(&mark).line() as usize;
            if let Some(p) = lines.get_mut(line) {
                p.start_ms = min_opt(p.start_ms, *s);
                p.end_ms = max_opt(p.end_ms, *e);
            }
        }
        lines
            .into_iter()
            .filter(|p| !p.text.trim().is_empty())
            .map(|mut p| {
                p.text = p.text.replace(LINE_BREAK, "\n");
                p
            })
            .collect()
    }

    /// Replaces paragraph `index` (as in [`Self::paragraphs`]) with `new`,
    /// but only if it still reads `expected`. The timestamps stay.
    pub fn replace_paragraph(&self, index: usize, expected: &str, new: &str) -> bool {
        let mut n = 0;
        for line in 0..self.buffer.line_count() {
            let Some(mut start) = self.buffer.iter_at_line(line) else {
                continue;
            };
            let mut end = start;
            if !end.ends_line() {
                end.forward_to_line_end();
            }
            let text = self.text_without_preview(&start, &end);
            if text.trim().is_empty() {
                continue;
            }
            if n < index {
                n += 1;
                continue;
            }
            if text.replace(LINE_BREAK, "\n") != expected {
                return false;
            }
            let offset = start.offset();
            self.buffer.delete(&mut start, &mut end);
            let mut at = self.buffer.iter_at_offset(offset);
            self.buffer
                .insert(&mut at, &new.replace('\n', &LINE_BREAK.to_string()));
            return true;
        }
        false
    }

    /// Puts the cursor at the start of paragraph `index` and scrolls to it.
    pub fn go_to_paragraph(&self, index: usize) {
        let mut n = 0;
        for line in 0..self.buffer.line_count() {
            let Some(start) = self.buffer.iter_at_line(line) else {
                continue;
            };
            let mut end = start;
            if !end.ends_line() {
                end.forward_to_line_end();
            }
            if self.text_without_preview(&start, &end).trim().is_empty() {
                continue;
            }
            if n == index {
                self.buffer.place_cursor(&start);
                self.view.scroll_to_iter(&mut start.clone(), 0.1, true, 0.0, 0.3);
                self.view.grab_focus();
                return;
            }
            n += 1;
        }
    }

    pub fn plain_text(&self) -> String {
        self.paragraphs()
            .into_iter()
            .map(|p| p.text)
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// The words currently marked unsure, in order.
    pub fn unsure_words(&self) -> Vec<String> {
        self.paragraphs()
            .iter()
            .flat_map(|p| {
                p.low_confidence
                    .iter()
                    .filter_map(|r| p.text.get(r.clone()).map(|s| s.trim().to_string()))
            })
            .filter(|w| !w.is_empty())
            .collect()
    }

    /// Selects the `nth` unsure word and scrolls to it.
    pub fn select_unsure(&self, nth: usize) {
        let mut iter = self.buffer.start_iter();
        let mut count = 0;
        loop {
            if iter.starts_tag(Some(&self.tag_unsure)) {
                if count == nth {
                    let mut end = iter;
                    end.forward_to_tag_toggle(Some(&self.tag_unsure));
                    self.buffer.select_range(&iter, &end);
                    self.view.scroll_to_iter(&mut iter.clone(), 0.1, false, 0.0, 0.0);
                    self.view.grab_focus();
                    return;
                }
                count += 1;
            }
            if !iter.forward_to_tag_toggle(Some(&self.tag_unsure)) {
                return;
            }
        }
    }

    /// Shows (or clears) the grey preview of what is being said, at the cursor.
    pub fn set_preview(&self, text: Option<&str>) {
        self.quietly(|| {
            self.remove_preview();
            let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) else {
                return;
            };
            let cursor = self.buffer.iter_at_mark(&self.buffer.get_insert());
            let text = format!("{}{text}", self.separator_before(&cursor));
            let offset = cursor.offset();
            let mut at = cursor;
            self.buffer.insert(&mut at, &text);
            let start = self.buffer.iter_at_offset(offset);
            self.buffer.apply_tag(&self.tag_preview, &start, &at);
            // Keep the cursor before the preview so finals land where it was.
            self.buffer.place_cursor(&self.buffer.iter_at_offset(offset));
        });
    }

    /// Inserts a finished utterance at the cursor.
    pub fn insert_final(&self, text: &str, start_ms: i64, end_ms: i64, low_confidence: &[Range<usize>]) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.quietly(|| {
            self.remove_preview();
            let gap = self
                .last_end_ms
                .get()
                .is_some_and(|e| start_ms - e >= self.paragraph_gap_ms.get());
            let mut cursor = self.buffer.iter_at_mark(&self.buffer.get_insert());
            let line_empty = line_is_blank(&cursor);
            if (self.force_new.get() || gap) && !line_empty {
                cursor.forward_to_line_end();
                self.buffer.insert(&mut cursor, "\n");
            }
            self.force_new.set(false);
            let text = &sentence_case(&self.line_before(&cursor), text);
            let sep = self.separator_before(&cursor);
            let line_start = {
                let mut s = cursor;
                s.set_line_offset(0);
                s.offset()
            };
            let has_mark = self.mark_on_line(cursor.line()).is_some();
            let insert_at = cursor.offset() + sep.chars().count() as i32;
            self.buffer.insert(&mut cursor, &format!("{sep}{text}"));
            self.tag_spans(insert_at, text, low_confidence);
            self.buffer.place_cursor(&cursor);
            match self.mark_on_line(cursor.line()) {
                Some(name) if has_mark => {
                    if let Some(t) = self.times.borrow_mut().get_mut(&name) {
                        t.0 = min_opt(t.0, Some(start_ms));
                        t.1 = max_opt(t.1, Some(end_ms));
                    }
                }
                _ => self.add_mark(line_start, Some(start_ms), Some(end_ms)),
            }
        });
        self.last_end_ms.set(Some(end_ms));
    }

    pub fn apply(&self, command: Command) {
        self.quietly(|| {
            self.remove_preview();
            match command {
                Command::NewParagraph => self.force_new.set(true),
                Command::NewLine => self.buffer.insert_at_cursor(&LINE_BREAK.to_string()),
                Command::DeleteLastSentence => {
                    let cursor = self.buffer.iter_at_mark(&self.buffer.get_insert());
                    let mut start = cursor;
                    start.set_line_offset(0);
                    let before = self.buffer.text(&start, &cursor, false).to_string();
                    let cut = last_sentence_start(&before);
                    let mut from = start;
                    from.forward_chars(before[..cut].chars().count() as i32);
                    let mut to = cursor;
                    self.buffer.delete(&mut from, &mut to);
                    // Drop a trailing space left behind.
                    let mut back = from;
                    if back.backward_char() && back.char() == ' ' {
                        self.buffer.delete(&mut back, &mut from);
                    }
                }
                Command::StopDictation => {}
                Command::Punctuate(mark) => {
                    let mut cursor = self.buffer.iter_at_mark(&self.buffer.get_insert());
                    let before = self.line_before(&cursor);
                    if before.trim().is_empty() {
                        return;
                    }
                    let mut from = cursor;
                    from.set_line_offset(0);
                    from.forward_chars(before[..punctuation_cut(&before)].chars().count() as i32);
                    self.buffer.delete(&mut from, &mut cursor);
                    self.buffer.insert(&mut cursor, &mark.to_string());
                    self.buffer.place_cursor(&cursor);
                }
            }
        });
    }

    fn remove_preview(&self) {
        let mut iter = self.buffer.start_iter();
        if !iter.starts_tag(Some(&self.tag_preview)) && !iter.forward_to_tag_toggle(Some(&self.tag_preview)) {
            return;
        }
        let mut end = iter;
        end.forward_to_tag_toggle(Some(&self.tag_preview));
        self.buffer.delete(&mut iter, &mut end);
    }

    fn text_without_preview(&self, start: &gtk::TextIter, end: &gtk::TextIter) -> String {
        let mut out = String::new();
        let mut i = *start;
        while i < *end {
            if !i.has_tag(&self.tag_preview) {
                out.push(i.char());
            }
            i.forward_char();
        }
        out
    }

    /// The text of the line up to `at`.
    fn line_before(&self, at: &gtk::TextIter) -> String {
        let mut start = *at;
        start.set_line_offset(0);
        self.buffer.text(&start, at, false).to_string()
    }

    fn separator_before(&self, at: &gtk::TextIter) -> &'static str {
        if at.starts_line() {
            return "";
        }
        let mut prev = *at;
        prev.backward_char();
        if prev.char().is_whitespace() || prev.char() == LINE_BREAK {
            ""
        } else {
            " "
        }
    }

    fn unsure_spans(&self, start: &gtk::TextIter, end: &gtk::TextIter) -> Vec<Range<usize>> {
        let mut spans = Vec::new();
        let mut i = *start;
        let mut open: Option<usize> = None;
        let mut bytes = 0usize;
        while i < *end {
            if i.has_tag(&self.tag_preview) {
                i.forward_char();
                continue;
            }
            let unsure = i.has_tag(&self.tag_unsure);
            match (unsure, open) {
                (true, None) => open = Some(bytes),
                (false, Some(s)) => {
                    spans.push(s..bytes);
                    open = None;
                }
                _ => {}
            }
            bytes += i.char().len_utf8();
            i.forward_char();
        }
        if let Some(s) = open {
            spans.push(s..bytes);
        }
        spans
    }

    /// Tags byte ranges of `text`, which starts at char `offset` in the buffer.
    fn tag_spans(&self, offset: i32, text: &str, spans: &[Range<usize>]) {
        for r in spans {
            let (Some(a), Some(b)) = (text.get(..r.start), text.get(..r.end.min(text.len()))) else {
                continue;
            };
            let s = self.buffer.iter_at_offset(offset + a.chars().count() as i32);
            let e = self.buffer.iter_at_offset(offset + b.chars().count() as i32);
            self.buffer.apply_tag(&self.tag_unsure, &s, &e);
        }
    }

    fn add_mark(&self, line_start_offset: i32, start_ms: Option<i64>, end_ms: Option<i64>) {
        if start_ms.is_none() && end_ms.is_none() {
            return;
        }
        let n = self.next_mark.get();
        self.next_mark.set(n + 1);
        let name = format!("para-{n}");
        self.buffer
            .create_mark(Some(&name), &self.buffer.iter_at_offset(line_start_offset), true);
        self.times.borrow_mut().insert(name, (start_ms, end_ms));
    }

    fn mark_on_line(&self, line: i32) -> Option<String> {
        self.times
            .borrow()
            .keys()
            .find(|name| {
                self.buffer
                    .mark(name)
                    .is_some_and(|m| self.buffer.iter_at_mark(&m).line() == line)
            })
            .cloned()
    }
}

fn line_is_blank(at: &gtk::TextIter) -> bool {
    let mut s = *at;
    s.set_line_offset(0);
    let mut e = s;
    if !e.ends_line() {
        e.forward_to_line_end();
    }
    s.buffer().text(&s, &e, false).trim().is_empty()
}

fn min_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, y) => x.or(y),
    }
}

fn max_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, y) => x.or(y),
    }
}
