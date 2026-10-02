use std::path::Path;
use std::sync::Once;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

use super::{EngineError, Segment, TranscribeOptions, Transcriber};

/// whisper.cpp through whisper-rs. Owns one loaded model and its decoding state.
pub struct WhisperEngine {
    ctx: WhisperContext,
    state: WhisperState,
}

static LOG_HOOK: Once = Once::new();

impl WhisperEngine {
    /// Loads a GGML model. `use_gpu` only has an effect in builds with the
    /// `vulkan` or `cuda` feature.
    pub fn load(path: &Path, use_gpu: bool) -> Result<Self, EngineError> {
        // Route whisper.cpp's chatter into `tracing` instead of stderr.
        LOG_HOOK.call_once(whisper_rs::install_logging_hooks);
        if !path.exists() {
            return Err(EngineError::ModelMissing(path.to_path_buf()));
        }
        let mut params = WhisperContextParameters::default();
        params.use_gpu(use_gpu);
        let ctx = WhisperContext::new_with_params(path, params).map_err(|source| EngineError::Load {
            path: path.to_path_buf(),
            source,
        })?;
        let state = ctx.create_state().map_err(|source| EngineError::Load {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self { ctx, state })
    }
}

impl Transcriber for WhisperEngine {
    fn transcribe(&mut self, pcm: &[f32], opts: &TranscribeOptions) -> Result<Vec<Segment>, EngineError> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(&opts.language));
        params.set_translate(false);
        params.set_n_threads(opts.threads as i32);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        // The Danish fine-tunes were trained without timestamp tokens; asking
        // for them garbles the first words (FLEURS WER: Edda 18.7% → 7.9%,
        // Røst 26.1% → 11.5%). Times come from voice detection instead.
        params.set_no_timestamps(true);
        if let Some(prompt) = &opts.initial_prompt {
            params.set_initial_prompt(prompt);
        }

        self.state.full(params, pcm)?;

        let eot = self.ctx.token_eot();
        let duration_ms = pcm.len() as i64 * 1000 / i64::from(super::SAMPLE_RATE);
        let mut segments = Vec::new();
        let mut window_start = 0;
        for seg in self.state.as_iter() {
            let mut text = String::new();
            let mut low_confidence = Vec::new();
            for i in 0..seg.n_tokens() {
                let Some(token) = seg.get_token(i) else { continue };
                // Ids at or above end-of-text are control and timestamp tokens.
                if token.token_id() >= eot {
                    continue;
                }
                let piece = token.to_str_lossy()?;
                let start = text.len();
                text.push_str(&piece);
                if token.token_probability() < opts.low_confidence_threshold && !piece.trim().is_empty() {
                    let lead = piece.len() - piece.trim_start().len();
                    low_confidence.push(start + lead..text.len());
                }
            }
            let trimmed_start = text.len() - text.trim_start().len();
            let text_trimmed = text.trim().to_string();
            let low_confidence = merge_ranges(low_confidence)
                .into_iter()
                .filter_map(|r| {
                    let s = r.start.checked_sub(trimmed_start)?;
                    let e = (r.end - trimmed_start).min(text_trimmed.len());
                    (s < e).then_some(s..e)
                })
                .collect();
            // Without timestamp tokens each segment is one 30 s window: its
            // end (centiseconds) is the window edge, its start is not
            // meaningful, so it starts where the previous one ended.
            let start_ms = window_start;
            let end_ms = (seg.end_timestamp() * 10).clamp(start_ms, duration_ms);
            window_start = end_ms;
            if text_trimmed.is_empty() {
                continue;
            }
            segments.push(Segment {
                start_ms,
                end_ms,
                text: text_trimmed,
                low_confidence,
            });
        }
        Ok(segments)
    }
}

/// Joins adjacent or overlapping ranges so a word split over several unsure
/// tokens becomes one span.
fn merge_ranges(mut ranges: Vec<std::ops::Range<usize>>) -> Vec<std::ops::Range<usize>> {
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::merge_ranges;

    #[test]
    fn adjacent_ranges_merge_into_one_word() {
        assert_eq!(merge_ranges(vec![5..8, 0..3, 3..5]), vec![0..8]);
    }

    #[test]
    fn separate_ranges_stay_separate() {
        assert_eq!(merge_ranges(vec![0..2, 4..6]), vec![0..2, 4..6]);
    }
}
