//! Spoken editing commands. A command only fires when the whole utterance is
//! the command, so "jeg skrev et nyt afsnit i går" stays text.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    NewParagraph,
    NewLine,
    DeleteLastSentence,
    StopDictation,
    /// "punktum", "komma" and friends: put this mark at the end of the text,
    /// in place of any mark the model already wrote there.
    Punctuate(char),
}

impl Command {
    pub fn label(self) -> &'static str {
        match self {
            Command::NewParagraph => "New paragraph",
            Command::NewLine => "Line break",
            Command::DeleteLastSentence => "Delete last sentence",
            Command::StopDictation => "Stop dictation",
            Command::Punctuate('.') => "Full stop",
            Command::Punctuate(',') => "Comma",
            Command::Punctuate('?') => "Question mark",
            Command::Punctuate('!') => "Exclamation mark",
            Command::Punctuate(':') => "Colon",
            Command::Punctuate(_) => "Punctuation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandTable {
    pub phrases: Vec<(String, Command)>,
}

impl Default for CommandTable {
    fn default() -> Self {
        Self {
            phrases: vec![
                ("nyt afsnit".into(), Command::NewParagraph),
                ("ny linje".into(), Command::NewLine),
                ("slet sidste sætning".into(), Command::DeleteLastSentence),
                ("stop diktat".into(), Command::StopDictation),
                ("ny paragraf".into(), Command::NewParagraph),
                ("linjeskift".into(), Command::NewLine),
                ("stop optagelse".into(), Command::StopDictation),
                ("stop optagelsen".into(), Command::StopDictation),
                ("stop diktering".into(), Command::StopDictation),
                ("punktum".into(), Command::Punctuate('.')),
                ("komma".into(), Command::Punctuate(',')),
                ("spørgsmålstegn".into(), Command::Punctuate('?')),
                ("udråbstegn".into(), Command::Punctuate('!')),
                ("kolon".into(), Command::Punctuate(':')),
            ],
        }
    }
}

impl CommandTable {
    pub fn match_utterance(&self, text: &str) -> Option<Command> {
        let said = normalize(text);
        if said.is_empty() {
            return None;
        }
        self.phrases
            .iter()
            .find(|(phrase, _)| normalize(phrase) == said)
            .map(|(_, c)| *c)
    }

    /// This table plus any built-in phrase it lacks, so settings saved by an
    /// older version still get commands added since.
    pub fn with_defaults(mut self) -> Self {
        for (phrase, c) in Self::default().phrases {
            if !self
                .phrases
                .iter()
                .any(|(p, _)| normalize(p) == normalize(&phrase))
            {
                self.phrases.push((phrase, c));
            }
        }
        self
    }
}

/// Lowercase words without punctuation or spaces: the model writes
/// "stop optagelse" as "stopoptagelse" now and then.
fn normalize(text: &str) -> String {
    crate::eval::normalize(text).replace(' ', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_utterance_commands_match_regardless_of_case_and_punctuation() {
        let t = CommandTable::default();
        assert_eq!(t.match_utterance("Nyt afsnit."), Some(Command::NewParagraph));
        assert_eq!(
            t.match_utterance("  slet sidste sætning! "),
            Some(Command::DeleteLastSentence)
        );
    }

    #[test]
    fn a_command_phrase_inside_a_sentence_is_text() {
        let t = CommandTable::default();
        assert_eq!(t.match_utterance("Jeg skrev et nyt afsnit i går."), None);
        assert_eq!(t.match_utterance(""), None);
    }

    #[test]
    fn everyday_phrasings_are_commands_too() {
        let t = CommandTable::default();
        assert_eq!(t.match_utterance("Ny paragraf."), Some(Command::NewParagraph));
        assert_eq!(t.match_utterance("Stop optagelse."), Some(Command::StopDictation));
        assert_eq!(t.match_utterance("Linjeskift"), Some(Command::NewLine));
    }

    #[test]
    fn a_command_written_as_one_word_still_matches() {
        let t = CommandTable::default();
        assert_eq!(t.match_utterance("stopoptagelse"), Some(Command::StopDictation));
        assert_eq!(t.match_utterance("Nytafsnit."), Some(Command::NewParagraph));
    }

    #[test]
    fn spoken_punctuation_is_a_command() {
        let t = CommandTable::default();
        assert_eq!(t.match_utterance("Punktum."), Some(Command::Punctuate('.')));
        assert_eq!(t.match_utterance("komma"), Some(Command::Punctuate(',')));
        assert_eq!(
            t.match_utterance("Spørgsmålstegn?"),
            Some(Command::Punctuate('?'))
        );
        assert_eq!(t.match_utterance("Udråbstegn!"), Some(Command::Punctuate('!')));
        assert_eq!(t.match_utterance("Kolon"), Some(Command::Punctuate(':')));
    }

    #[test]
    fn a_saved_table_gains_new_built_in_phrases_and_keeps_its_own() {
        let saved = CommandTable {
            phrases: vec![
                ("nyt afsnit".into(), Command::NewParagraph),
                ("færdig".into(), Command::StopDictation),
            ],
        };
        let t = saved.with_defaults();
        assert_eq!(t.match_utterance("færdig"), Some(Command::StopDictation));
        assert_eq!(t.match_utterance("punktum"), Some(Command::Punctuate('.')));
        assert_eq!(
            t.phrases.iter().filter(|(p, _)| p == "nyt afsnit").count(),
            1,
            "no duplicates"
        );
    }
}
