//! Danish punctuation for dictated text, with Alvenir's BERT model
//! (Alvenir/bert-punct-restoration-da, Apache 2.0), run with candle.
//!
//! Edda leaves out punctuation in about half its sentences, and a pause in
//! the middle of a sentence splits it into utterances transcribed apart, so
//! sentence ends between them are never decided. This model labels every
//! word of a whole paragraph with the mark that follows it and whether it
//! starts with a capital. Marks are only added, never removed, so spoken
//! "komma" and the model's own marks stay; words are never changed.
//!
//! On 60 held-out FLEURS pairs transcribed by Edda: comma F1 0.64 → 0.79,
//! sentence-end F1 0.61 → 0.81, no word changed. Small chat models (Qwen 3.5
//! 2B/4B, Gemma 3n E4B) scored 0.72-0.77 and took 3-8 s per paragraph.

use std::path::Path;

use candle_core::{DType, Device, Module, Tensor};
use candle_nn::{Linear, VarBuilder};
use candle_transformers::models::bert::{BertModel, Config};
use tokenizers::models::wordpiece::WordPiece;
use tokenizers::normalizers::BertNormalizer;
use tokenizers::pre_tokenizers::bert::BertPreTokenizer;
use tokenizers::processors::bert::BertProcessing;
use tokenizers::{InputSequence, Tokenizer};

pub const REPO: &str = "Alvenir/bert-punct-restoration-da";
/// Folder under the models directory.
pub const DIR: &str = "punctuation-da";
pub const FILES: [&str; 3] = ["config.json", "vocab.txt", "pytorch_model.bin"];

/// The model's labels: the mark after the word, then U for a capital.
const LABELS: [&str; 15] = [
    "OU", "OO", ".O", "!O", ",O", ".U", "!U", ",U", ":O", ";O", ":U", "'O", "-O", "?O", "?U",
];
/// Words per window, and the step between windows (as Alvenir's punctfix).
const WINDOW: usize = 100;
const STEP: usize = 30;

#[derive(Debug, thiserror::Error)]
pub enum PunctuationError {
    #[error("the punctuation model is not downloaded ({0})")]
    Missing(std::path::PathBuf),
    #[error("loading the punctuation model: {0}")]
    Load(String),
    #[error("punctuating: {0}")]
    Run(String),
}

/// Labels the words of dictated text: the model, or a stand-in in tests.
pub trait Punctuate: Send + Sync {
    fn labels(&self, words: &[String]) -> Result<Vec<Label>, PunctuationError>;
}

/// `text` with marks and capitals added. `open` while it is still being
/// dictated: the last word then waits for what comes next.
pub fn punctuate(p: &dyn Punctuate, text: &str, open: bool) -> Result<String, PunctuationError> {
    let labels = p.labels(&model_words(text))?;
    Ok(if open {
        apply_open(text, &labels)
    } else {
        apply(text, &labels)
    })
}

/// Whether the model's files are all there.
pub fn installed(dir: &Path) -> bool {
    FILES.iter().all(|f| dir.join(f).is_file())
}

pub struct Punctuator {
    bert: BertModel,
    classifier: Linear,
    tokenizer: Tokenizer,
}

impl Punctuator {
    /// Loads the model from `dir` (the files in [`FILES`]).
    pub fn load(dir: &Path) -> Result<Self, PunctuationError> {
        if let Some(missing) = FILES.iter().map(|f| dir.join(f)).find(|p| !p.is_file()) {
            return Err(PunctuationError::Missing(missing));
        }
        let load = |e: &dyn std::fmt::Display| PunctuationError::Load(e.to_string());
        let text = std::fs::read_to_string(dir.join("config.json")).map_err(|e| load(&e))?;
        let config: Config = serde_json::from_str(&text).map_err(|e| load(&e))?;
        let vb = VarBuilder::from_pth(dir.join("pytorch_model.bin"), DType::F32, &Device::Cpu)
            .map_err(|e| load(&e))?;
        let bert = BertModel::load(vb.pp("bert"), &config).map_err(|e| load(&e))?;
        let classifier =
            candle_nn::linear(config.hidden_size, LABELS.len(), vb.pp("classifier")).map_err(|e| load(&e))?;
        let vocab = dir.join("vocab.txt");
        let wordpiece = WordPiece::from_file(&vocab.to_string_lossy())
            .unk_token("[UNK]".into())
            .build()
            .map_err(|e| load(&e))?;
        let mut tokenizer = Tokenizer::new(wordpiece);
        // As the model was trained: cased, accents kept.
        tokenizer
            .with_normalizer(Some(BertNormalizer::new(true, true, Some(false), false)))
            .map_err(|e| load(&e))?;
        tokenizer.with_pre_tokenizer(Some(BertPreTokenizer));
        let id = |t: &str| {
            tokenizer
                .token_to_id(t)
                .ok_or_else(|| PunctuationError::Load(format!("{t} is not in the vocabulary")))
        };
        let (cls, sep) = (id("[CLS]")?, id("[SEP]")?);
        tokenizer.with_post_processor(Some(BertProcessing::new(
            ("[SEP]".into(), sep),
            ("[CLS]".into(), cls),
        )));
        Ok(Self {
            bert,
            classifier,
            tokenizer,
        })
    }

    /// `text` with marks and capitals added; see [`apply`].
    pub fn punctuate(&self, text: &str) -> Result<String, PunctuationError> {
        punctuate(self, text, false)
    }

    /// One label per word: 100-word windows every 30 words, each word taking
    /// the label most windows give it (as punctfix does; the Rust port gave
    /// the same text as punctfix on 40 test paragraphs).
    pub fn labels(&self, words: &[String]) -> Result<Vec<Label>, PunctuationError> {
        let run = |e: &dyn std::fmt::Display| PunctuationError::Run(e.to_string());
        let mut votes: Vec<Vec<usize>> = vec![Vec::new(); words.len()];
        let starts: Vec<usize> = if words.len() <= WINDOW {
            vec![0]
        } else {
            (0..words.len()).step_by(STEP).collect()
        };
        for start in starts {
            let end = (start + WINDOW).min(words.len());
            let chunk: Vec<&str> = words[start..end].iter().map(String::as_str).collect();
            if chunk.is_empty() {
                continue;
            }
            let enc = self
                .tokenizer
                .encode(InputSequence::from(chunk), true)
                .map_err(|e| run(&e))?;
            let ids = Tensor::new(enc.get_ids(), &Device::Cpu)
                .and_then(|t| t.unsqueeze(0))
                .map_err(|e| run(&e))?;
            let types = ids.zeros_like().map_err(|e| run(&e))?;
            let best: Vec<u32> = self
                .bert
                .forward(&ids, &types, None)
                .and_then(|h| self.classifier.forward(&h))
                .and_then(|l| l.squeeze(0))
                .and_then(|l| l.argmax(1))
                .and_then(|l| l.to_vec1())
                .map_err(|e| run(&e))?;
            // A word's label is its first piece's.
            let mut last = None;
            for (pos, word) in enc.get_word_ids().iter().enumerate() {
                if let Some(w) = *word
                    && last != Some(w)
                {
                    last = Some(w);
                    votes[start + w as usize].push(best[pos] as usize);
                }
            }
        }
        Ok(votes.iter().map(|v| label_of(LABELS[majority(v)])).collect())
    }
}

impl Punctuate for Punctuator {
    fn labels(&self, words: &[String]) -> Result<Vec<Label>, PunctuationError> {
        Punctuator::labels(self, words)
    }
}

fn label_of(name: &str) -> Label {
    let mark = name.chars().next().filter(|c| *c != 'O');
    Label {
        mark,
        upper: name.ends_with('U'),
    }
}

/// The most common vote; a tie goes to the first seen (as Python's Counter).
fn majority(votes: &[usize]) -> usize {
    let mut best = (1, 0); // "OO" when a word got no vote
    for &l in votes {
        let n = votes.iter().filter(|&&x| x == l).count();
        if n > best.1 {
            best = (l, n);
        }
    }
    best.0
}

/// What the model says about one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label {
    /// The mark after the word, if any.
    pub mark: Option<char>,
    /// Starts with a capital (names, sentence starts).
    pub upper: bool,
}

impl Label {
    pub const NONE: Label = Label {
        mark: None,
        upper: false,
    };
}

/// Marks Fennec takes from the model. It also predicts ' and -, which
/// belong inside words.
const MARKS: [char; 6] = ['.', ',', '?', '!', ':', ';'];

/// Common words never written with a capital in the middle of a sentence;
/// Edda capitalises them now and then ("og Det er").
const LOWER: [&str; 46] = [
    "af", "at", "da", "de", "dem", "den", "der", "det", "dette", "du", "efter", "eller", "en", "er", "et",
    "for", "fra", "har", "havde", "hun", "hvad", "hvis", "hvor", "hvordan", "i", "ikke", "jeg", "kan", "man",
    "med", "men", "når", "og", "om", "på", "så", "som", "til", "var", "vi", "vil", "ville", "skal", "også",
    "bare", "nu",
];

/// The word as the model sees it: lower-case letters and digits only.
pub fn model_word(token: &str) -> String {
    token
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Byte ranges of whitespace-separated tokens.
fn tokens(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                out.push(s..i);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(s..text.len());
    }
    out
}

/// The tokens the model labels: those with letters or digits.
pub fn model_words(text: &str) -> Vec<String> {
    tokens(text)
        .into_iter()
        .map(|r| model_word(&text[r]))
        .filter(|w| !w.is_empty())
        .collect()
}

/// `text` with the model's marks and capitals added: one label per token
/// that has letters or digits (as [`model_words`] lists them). Every word,
/// every mark already there and all spacing stay.
pub fn apply(text: &str, labels: &[Label]) -> String {
    apply_with(text, labels, false)
}

/// Like [`apply`], for text still being dictated: the last word gets no
/// mark yet, since the sentence may go on.
pub fn apply_open(text: &str, labels: &[Label]) -> String {
    apply_with(text, labels, true)
}

fn apply_with(text: &str, labels: &[Label], leave_last: bool) -> String {
    let last = labels.len().saturating_sub(1);
    let mut out = String::with_capacity(text.len() + labels.len());
    let mut labels = labels.iter().enumerate();
    let mut at = 0;
    let mut sentence_start = true;
    for range in tokens(text) {
        out.push_str(&text[at..range.start]);
        at = range.end;
        let token = &text[range];
        if model_word(token).is_empty() {
            out.push_str(token);
            continue;
        }
        let (i, label) = labels.next().map_or((usize::MAX, Label::NONE), |(i, l)| (i, *l));
        let mut word = token.to_string();
        let first_upper = word.chars().next().is_some_and(char::is_uppercase);
        if (label.upper || sentence_start) && !first_upper {
            word = with_first(&word, true);
        } else if first_upper
            && !label.upper
            && !sentence_start
            && LOWER.contains(&model_word(&word).as_str())
            && word.chars().skip(1).all(|c| !c.is_uppercase())
        {
            word = with_first(&word, false);
        }
        let has_mark = word.trim_end_matches(['"', '\'', '»', '”', ')']).ends_with(MARKS);
        let open_end = leave_last && i == last;
        if !has_mark
            && !open_end
            && let Some(m) = label.mark.filter(|m| MARKS.contains(m))
        {
            word.push(m);
        }
        sentence_start = word
            .trim_end_matches(['"', '\'', '»', '”', ')'])
            .ends_with(['.', '?', '!']);
        out.push_str(&word);
    }
    out.push_str(&text[at..]);
    out
}

/// Byte ranges in `old` moved to the same text in `new`, where `new` is
/// `old` with marks inserted and first letters re-cased (as [`apply`]
/// makes it).
pub fn moved_spans(old: &str, new: &str, spans: &[std::ops::Range<usize>]) -> Vec<std::ops::Range<usize>> {
    // map[i] is where old byte i (on a char boundary) is in new.
    let mut map = vec![0; old.len() + 1];
    let mut n = new.char_indices().peekable();
    for (i, c) in old.char_indices() {
        // Skip the marks inserted before this character.
        while let Some(&(j, d)) = n.peek() {
            if d.to_lowercase().eq(c.to_lowercase()) {
                map[i] = j;
                n.next();
                break;
            }
            n.next();
        }
    }
    map[old.len()] = new.len();
    let at = |i: usize| map[i];
    // An end maps to just after the previous character, before any mark.
    let end = |i: usize| {
        let prev = old[..i].chars().next_back();
        prev.map_or(0, |c| {
            map[i - c.len_utf8()]
                + new[map[i - c.len_utf8()]..]
                    .chars()
                    .next()
                    .map_or(0, char::len_utf8)
        })
    };
    spans.iter().map(|r| at(r.start)..end(r.end)).collect()
}

fn with_first(word: &str, upper: bool) -> String {
    let mut c = word.chars();
    match c.next() {
        Some(f) if upper => f.to_uppercase().chain(c).collect(),
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const O: Label = Label::NONE;
    const COMMA: Label = Label {
        mark: Some(','),
        upper: false,
    };
    const STOP: Label = Label {
        mark: Some('.'),
        upper: false,
    };
    const ASK: Label = Label {
        mark: Some('?'),
        upper: false,
    };
    const NAME: Label = Label {
        mark: None,
        upper: true,
    };

    #[test]
    fn marks_and_sentence_capitals_are_added() {
        let text = "er det blevet bedre eller værre umiddelbart ser det fint ud";
        let labels = [O, O, O, O, O, ASK, O, O, O, O, STOP];
        assert_eq!(
            apply(text, &labels),
            "Er det blevet bedre eller værre? Umiddelbart ser det fint ud."
        );
    }

    #[test]
    fn marks_already_there_stay_even_if_the_model_disagrees() {
        // "komma" was spoken after "regner".
        let text = "Det regner, og vi bliver hjemme.";
        assert_eq!(apply(text, &[O, O, O, O, O, O]), text);
        assert_eq!(
            apply("Det regner, og vi bliver hjemme.", &[O, STOP, O, O, O, O]),
            text
        );
    }

    #[test]
    fn names_keep_their_capitals_and_the_model_adds_others() {
        let text = "vi mødes i aarhus hos Ane";
        assert_eq!(apply(text, &[O, O, O, NAME, O, O]), "Vi mødes i Aarhus hos Ane");
    }

    #[test]
    fn a_common_word_capitalised_mid_sentence_is_lowered() {
        let text = "det ser sådan ud og Det er positivt";
        assert_eq!(
            apply(text, &[O, O, O, COMMA, O, O, O, STOP]),
            "Det ser sådan ud, og det er positivt."
        );
    }

    #[test]
    fn spacing_and_line_breaks_are_kept() {
        let text = "første linje\u{2028}anden  linje";
        assert_eq!(apply(text, &[O, STOP, O, O]), "Første linje.\u{2028}Anden  linje");
    }

    #[test]
    fn while_dictating_the_last_word_waits_for_what_comes_next() {
        let text = "vi prøver for at finde ud af om";
        let labels = [O, O, O, O, O, O, O, STOP];
        assert_eq!(apply_open(text, &labels), "Vi prøver for at finde ud af om");
        assert_eq!(apply(text, &labels), "Vi prøver for at finde ud af om.");
    }

    #[test]
    fn unsure_word_spans_move_with_inserted_marks() {
        let old = "det regner i Aarhus i dag";
        let new = apply(old, &[O, COMMA, O, O, O, STOP]);
        assert_eq!(new, "Det regner, i Aarhus i dag.");
        let aarhus = old.find("Aarhus").unwrap();
        let spans = moved_spans(old, &new, &[aarhus..aarhus + 6, 0..3]);
        assert_eq!(&new[spans[0].clone()], "Aarhus");
        assert_eq!(&new[spans[1].clone()], "Det");
    }

    #[test]
    fn tokens_without_letters_take_no_label() {
        assert_eq!(model_words("klokken 14 – og så"), ["klokken", "14", "og", "så"]);
        assert_eq!(
            apply("klokken 14 – og så", &[O, COMMA, O, O]),
            "Klokken 14, – og så"
        );
    }
}
