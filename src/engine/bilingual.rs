//! Danish with the chosen Danish model, English with Whisper turbo. The
//! Danish fine-tunes cannot write English (Edda on English FLEURS: 56% WER,
//! and it hears every English clip as Danish), so English needs a model of
//! its own, and that model also tells the two languages apart.

use super::{EngineError, SAMPLE_RATE, Segment, TranscribeOptions, Transcriber};

type Loader = Box<dyn FnMut() -> Result<Box<dyn Transcriber>, EngineError> + Send>;

pub struct Bilingual {
    danish: Box<dyn Transcriber>,
    english: Option<Box<dyn Transcriber>>,
    /// Loads the English model the first time it is needed.
    load_english: Loader,
}

impl Bilingual {
    /// Speech shorter than this (2 s) is too little to settle a session's
    /// language on; it is still detected, but asked again next time.
    const SETTLE: usize = 2 * SAMPLE_RATE as usize;
    /// Detections less sure than this are not kept either.
    const SURE: f32 = 0.8;

    pub fn new(danish: Box<dyn Transcriber>, load_english: Loader) -> Self {
        Self {
            danish,
            english: None,
            load_english,
        }
    }

    fn english(&mut self) -> Result<&mut Box<dyn Transcriber>, EngineError> {
        if self.english.is_none() {
            self.english = Some((self.load_english)()?);
        }
        Ok(self.english.as_mut().expect("loaded above"))
    }

    fn language_of(&mut self, pcm: &[f32], opts: &TranscribeOptions) -> Result<String, EngineError> {
        let Some(spoken) = &opts.spoken else {
            return Ok(opts.language.clone());
        };
        if let Some(code) = spoken.current() {
            return Ok(code);
        }
        // A preview of the first second is replaced soon anyway; detecting
        // on it would cost a pass and could not settle the language.
        if opts.fast && pcm.len() < Self::SETTLE {
            return Ok("da".into());
        }
        let english = match self.english() {
            Ok(e) => e,
            Err(e) => {
                // Without the English model there is only Danish to write.
                tracing::warn!("cannot detect the language: {e}");
                spoken.remember("da");
                return Ok("da".into());
            }
        };
        let Some((code, sure)) = english.detect_language(pcm, &["da", "en"], opts.threads)? else {
            return Ok("da".into());
        };
        tracing::info!(
            "heard {code} ({sure:.2}) in {:.1} s",
            pcm.len() as f64 / f64::from(SAMPLE_RATE)
        );
        if pcm.len() >= Self::SETTLE && sure >= Self::SURE {
            spoken.remember(&code);
        }
        Ok(code)
    }
}

impl Transcriber for Bilingual {
    fn transcribe(&mut self, pcm: &[f32], opts: &TranscribeOptions) -> Result<Vec<Segment>, EngineError> {
        let language = self.language_of(pcm, opts)?;
        let opts = TranscribeOptions {
            language,
            ..opts.clone()
        };
        if opts.language == "en" {
            self.english()?.transcribe(pcm, &opts)
        } else {
            self.danish.transcribe(pcm, &opts)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::engine::SpokenLanguage;

    /// Writes which model ran, in which language; hears `hears`.
    struct Fake {
        name: &'static str,
        hears: Option<(&'static str, f32)>,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Transcriber for Fake {
        fn transcribe(
            &mut self,
            _pcm: &[f32],
            opts: &TranscribeOptions,
        ) -> Result<Vec<Segment>, EngineError> {
            self.log
                .lock()
                .unwrap()
                .push(format!("{} {}", self.name, opts.language));
            Ok(Vec::new())
        }

        fn detect_language(
            &mut self,
            _pcm: &[f32],
            _among: &[&str],
            _threads: usize,
        ) -> Result<Option<(String, f32)>, EngineError> {
            self.log.lock().unwrap().push("detect".into());
            Ok(self.hears.map(|(c, p)| (c.to_string(), p)))
        }
    }

    fn engines(hears: Option<(&'static str, f32)>) -> (Bilingual, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let danish = Fake {
            name: "edda",
            hears: None,
            log: Arc::clone(&log),
        };
        let l = Arc::clone(&log);
        let b = Bilingual::new(
            Box::new(danish),
            Box::new(move || {
                l.lock().unwrap().push("load turbo".into());
                Ok(Box::new(Fake {
                    name: "turbo",
                    hears,
                    log: Arc::clone(&l),
                }) as Box<dyn Transcriber>)
            }),
        );
        (b, log)
    }

    fn opts(spoken: &SpokenLanguage) -> TranscribeOptions {
        TranscribeOptions {
            spoken: Some(spoken.clone()),
            ..Default::default()
        }
    }

    static SECS_3: [f32; 48_000] = [0.0; 48_000];

    #[test]
    fn danish_by_choice_never_loads_the_english_model() {
        let (mut b, log) = engines(Some(("en", 1.0)));
        let da = SpokenLanguage::chosen("da");
        b.transcribe(&SECS_3, &opts(&da)).unwrap();
        assert_eq!(*log.lock().unwrap(), ["edda da"]);
    }

    #[test]
    fn english_by_choice_goes_to_the_english_model_without_detecting() {
        let (mut b, log) = engines(Some(("da", 1.0)));
        let en = SpokenLanguage::chosen("en");
        b.transcribe(&SECS_3, &opts(&en)).unwrap();
        b.transcribe(&SECS_3, &opts(&en)).unwrap();
        assert_eq!(*log.lock().unwrap(), ["load turbo", "turbo en", "turbo en"]);
    }

    #[test]
    fn detected_english_is_kept_for_the_rest_of_the_session() {
        let (mut b, log) = engines(Some(("en", 0.99)));
        let auto = SpokenLanguage::detect();
        b.transcribe(&SECS_3, &opts(&auto)).unwrap();
        b.transcribe(&SECS_3, &opts(&auto)).unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            ["load turbo", "detect", "turbo en", "turbo en"]
        );
        assert!(auto.is_english());
        assert_eq!(auto.fresh().current(), None, "a new session detects again");
    }

    #[test]
    fn detected_danish_goes_to_the_danish_model() {
        let (mut b, log) = engines(Some(("da", 0.99)));
        let auto = SpokenLanguage::detect();
        b.transcribe(&SECS_3, &opts(&auto)).unwrap();
        assert_eq!(*log.lock().unwrap(), ["load turbo", "detect", "edda da"]);
        assert_eq!(auto.current().as_deref(), Some("da"));
    }

    #[test]
    fn a_short_or_unsure_guess_is_used_but_not_kept() {
        let (mut b, _) = engines(Some(("en", 0.99)));
        let auto = SpokenLanguage::detect();
        b.transcribe(&[0.0; 16_000], &opts(&auto)).unwrap();
        assert_eq!(auto.current(), None, "one second is too short to settle on");

        let (mut b, log) = engines(Some(("en", 0.6)));
        b.transcribe(&SECS_3, &opts(&auto)).unwrap();
        assert_eq!(auto.current(), None, "unsure");
        assert_eq!(log.lock().unwrap().last().unwrap(), "turbo en");
    }

    #[test]
    fn a_short_preview_does_not_detect() {
        let (mut b, log) = engines(Some(("en", 0.99)));
        let auto = SpokenLanguage::detect();
        let preview = TranscribeOptions {
            fast: true,
            ..opts(&auto)
        };
        b.transcribe(&[0.0; 16_000], &preview).unwrap();
        assert_eq!(*log.lock().unwrap(), ["edda da"]);
        b.transcribe(&SECS_3, &preview).unwrap();
        assert!(auto.is_english(), "a longer preview settles it");
    }

    #[test]
    fn without_the_english_model_detection_settles_on_danish() {
        let mut b = Bilingual::new(
            Box::new(Fake {
                name: "edda",
                hears: None,
                log: Arc::default(),
            }),
            Box::new(|| Err(EngineError::Other("not downloaded".into()))),
        );
        let auto = SpokenLanguage::detect();
        b.transcribe(&SECS_3, &opts(&auto)).unwrap();
        assert_eq!(auto.current().as_deref(), Some("da"));
        assert!(
            b.transcribe(&SECS_3, &opts(&SpokenLanguage::chosen("en")))
                .is_err()
        );
    }

    #[test]
    fn without_a_spoken_language_the_code_decides() {
        let (mut b, log) = engines(None);
        let en = TranscribeOptions {
            language: "en".into(),
            ..Default::default()
        };
        b.transcribe(&SECS_3, &en).unwrap();
        b.transcribe(&SECS_3, &TranscribeOptions::default()).unwrap();
        assert_eq!(*log.lock().unwrap(), ["load turbo", "turbo en", "edda da"]);
    }
}
