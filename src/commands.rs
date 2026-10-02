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
}

impl Command {
    pub fn label(self) -> &'static str {
        match self {
            Command::NewParagraph => "New paragraph",
            Command::NewLine => "Line break",
            Command::DeleteLastSentence => "Delete last sentence",
            Command::StopDictation => "Stop dictation",
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
}

fn normalize(text: &str) -> String {
    crate::eval::normalize(text)
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
}
