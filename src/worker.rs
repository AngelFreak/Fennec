//! One thread owns the loaded model. Jobs run by priority: live finals, then
//! file chunks, then live previews. Only the newest preview is kept, and a
//! preview never delays real text.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, bounded};

use crate::engine::{EngineError, Segment, TranscribeOptions, Transcriber};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    LiveFinal,
    File,
    LivePartial,
}

pub type Reply = Result<Vec<Segment>, EngineError>;

/// Decides from the first pass whether the second one is needed.
type NeedsSecond = Box<dyn Fn(&Reply) -> bool + Send>;

/// The text a live session has committed so far, for models trained to
/// take the previous sentence as context (Edda v0.2). A job reads it when
/// it starts and a final adds to it when it finishes, both on the engine
/// thread, so every job sees exactly the text before it, however early it
/// was queued.
#[derive(Debug, Clone, Default)]
pub struct Context(Arc<Mutex<String>>);

impl Context {
    const KEEP: usize = 200;

    pub fn new(seed: &str) -> Self {
        let c = Self::default();
        c.push(seed);
        c
    }

    /// The last 200 characters, or `None` while nothing is committed.
    pub fn prompt(&self) -> Option<String> {
        let text = self.0.lock().expect("context poisoned");
        let chars: Vec<char> = text.trim().chars().collect();
        (!chars.is_empty()).then(|| chars[chars.len().saturating_sub(Self::KEEP)..].iter().collect())
    }

    /// Adds committed text.
    pub fn push(&self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let mut c = self.0.lock().expect("context poisoned");
        if !c.is_empty() {
            c.push(' ');
        }
        c.push_str(text);
        // Only the tail is ever used.
        let n = c.chars().count();
        if n > 2 * Self::KEEP {
            *c = c.chars().skip(n - Self::KEEP).collect();
        }
    }
}

struct Job {
    pcm: Vec<f32>,
    opts: TranscribeOptions,
    reply: Sender<Reply>,
    /// A second pass over the same audio, run straight after the first.
    then: Option<(TranscribeOptions, NeedsSecond)>,
    /// Prompts each pass with it; a two-pass job's second pass adds its
    /// text to it.
    context: Option<Context>,
    /// A single pass whose text is committed (a live final), so it is added
    /// to the context.
    commits: bool,
}

#[derive(Default)]
struct Queues {
    finals: VecDeque<Job>,
    files: VecDeque<Job>,
    partial: Option<Job>,
    busy: bool,
    shutdown: bool,
    replace: Option<Box<dyn Transcriber>>,
}

pub struct EngineWorker {
    shared: Arc<(Mutex<Queues>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl EngineWorker {
    pub fn spawn(engine: Box<dyn Transcriber>) -> Self {
        let shared = Arc::new((Mutex::new(Queues::default()), Condvar::new()));
        let s = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("fennec-engine".into())
            .spawn(move || run(engine, s))
            .expect("spawning the engine thread");
        Self {
            shared,
            thread: Some(thread),
        }
    }

    /// Queues a job; the reply arrives on the returned channel. A replaced
    /// preview's channel closes without a reply.
    pub fn submit(&self, pcm: Vec<f32>, opts: TranscribeOptions, priority: Priority) -> Receiver<Reply> {
        enqueue(&self.shared, pcm, opts, priority, None, false)
    }

    /// A live final in two passes: `first`, then `second` over the same
    /// audio if `needs_second` says so, with nothing run in between. Each
    /// pass replies on the returned channel. With a context, both passes
    /// are prompted with it and the second pass's text is added to it.
    pub fn submit_two_pass(
        &self,
        pcm: Vec<f32>,
        first: TranscribeOptions,
        second: TranscribeOptions,
        needs_second: impl Fn(&Reply) -> bool + Send + 'static,
        context: Option<&Context>,
    ) -> Receiver<Reply> {
        let (tx, rx) = bounded(2);
        let job = Job {
            pcm,
            opts: first,
            reply: tx,
            then: Some((second, Box::new(needs_second))),
            context: context.cloned(),
            commits: false,
        };
        let (lock, cv) = &*self.shared;
        lock.lock().expect("engine queue poisoned").finals.push_back(job);
        cv.notify_one();
        rx
    }

    /// Like [`submit`](Self::submit), prompted with `context` when it runs.
    /// Adds nothing to the context.
    pub fn submit_with(
        &self,
        pcm: Vec<f32>,
        opts: TranscribeOptions,
        priority: Priority,
        context: Option<&Context>,
    ) -> Receiver<Reply> {
        enqueue(&self.shared, pcm, opts, priority, context.cloned(), false)
    }

    /// A live final in one pass, prompted with `context` and added to it.
    pub fn submit_final(
        &self,
        pcm: Vec<f32>,
        opts: TranscribeOptions,
        context: Option<&Context>,
    ) -> Receiver<Reply> {
        enqueue(
            &self.shared,
            pcm,
            opts,
            Priority::LiveFinal,
            context.cloned(),
            true,
        )
    }

    /// True while a job runs or real (non-preview) work is queued.
    pub fn is_busy(&self) -> bool {
        let q = self.shared.0.lock().expect("engine queue poisoned");
        q.busy || !q.finals.is_empty() || !q.files.is_empty()
    }

    /// Swaps the model after the current job (model or backend change).
    pub fn replace_engine(&self, engine: Box<dyn Transcriber>) {
        let (lock, cv) = &*self.shared;
        lock.lock().expect("engine queue poisoned").replace = Some(engine);
        cv.notify_one();
    }

    /// A blocking [`Transcriber`] that runs on this worker at `priority`, so
    /// file ingest shares the model with live dictation.
    pub fn transcriber(&self, priority: Priority) -> WorkerTranscriber {
        WorkerTranscriber {
            shared: Arc::clone(&self.shared),
            priority,
        }
    }
}

impl Drop for EngineWorker {
    fn drop(&mut self) {
        {
            let (lock, cv) = &*self.shared;
            if let Ok(mut q) = lock.lock() {
                q.shutdown = true;
            }
            cv.notify_one();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn enqueue(
    shared: &(Mutex<Queues>, Condvar),
    pcm: Vec<f32>,
    opts: TranscribeOptions,
    priority: Priority,
    context: Option<Context>,
    commits: bool,
) -> Receiver<Reply> {
    let (tx, rx) = bounded(1);
    let job = Job {
        pcm,
        opts,
        reply: tx,
        then: None,
        context,
        commits,
    };
    let (lock, cv) = shared;
    let mut q = lock.lock().expect("engine queue poisoned");
    match priority {
        Priority::LiveFinal => q.finals.push_back(job),
        Priority::File => q.files.push_back(job),
        Priority::LivePartial => q.partial = Some(job),
    }
    cv.notify_one();
    rx
}

fn run(mut engine: Box<dyn Transcriber>, shared: Arc<(Mutex<Queues>, Condvar)>) {
    let (lock, cv) = &*shared;
    loop {
        let job = {
            let mut q = lock.lock().expect("engine queue poisoned");
            loop {
                if let Some(e) = q.replace.take() {
                    engine = e;
                }
                if let Some(j) = q.finals.pop_front().or_else(|| q.files.pop_front()) {
                    q.busy = true;
                    break j;
                }
                // Queued real work finishes before shutdown; previews do not.
                if q.shutdown {
                    return;
                }
                if let Some(j) = q.partial.take() {
                    q.busy = true;
                    break j;
                }
                q = cv.wait(q).expect("engine queue poisoned");
            }
        };
        let prompted = |mut opts: TranscribeOptions| {
            if let Some(c) = &job.context {
                opts.initial_prompt = c.prompt();
            }
            opts
        };
        let commit = |result: &Reply| {
            if let (Some(c), Ok(segs)) = (&job.context, result) {
                let text: Vec<&str> = segs.iter().map(|s| s.text.trim()).collect();
                c.push(&text.join(" "));
            }
        };
        let result = engine.transcribe(&job.pcm, &prompted(job.opts.clone()));
        if job.commits {
            commit(&result);
        }
        let second = job
            .then
            .and_then(|(opts, needed)| needed(&result).then_some(opts));
        // The requester may have given up; that is fine.
        let _ = job.reply.send(result);
        if let Some(opts) = second {
            let result = engine.transcribe(&job.pcm, &prompted(opts));
            commit(&result);
            let _ = job.reply.send(result);
        }
        lock.lock().expect("engine queue poisoned").busy = false;
    }
}

pub struct WorkerTranscriber {
    shared: Arc<(Mutex<Queues>, Condvar)>,
    priority: Priority,
}

impl Transcriber for WorkerTranscriber {
    fn transcribe(&mut self, pcm: &[f32], opts: &TranscribeOptions) -> Result<Vec<Segment>, EngineError> {
        let rx = enqueue(
            &self.shared,
            pcm.to_vec(),
            opts.clone(),
            self.priority,
            None,
            false,
        );
        rx.recv().unwrap_or(Err(EngineError::WorkerStopped))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// Records the first sample of each job; blocks while `gate` is set.
    pub struct Recorder {
        pub seen: Arc<Mutex<Vec<f32>>>,
        pub gate: Arc<AtomicBool>,
    }

    impl Transcriber for Recorder {
        fn transcribe(&mut self, pcm: &[f32], _: &TranscribeOptions) -> Result<Vec<Segment>, EngineError> {
            while self.gate.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(2));
            }
            self.seen.lock().unwrap().push(pcm[0]);
            Ok(vec![Segment {
                start_ms: 0,
                end_ms: 1,
                text: format!("{}", pcm[0]),
                low_confidence: vec![],
            }])
        }
    }

    fn worker() -> (EngineWorker, Arc<Mutex<Vec<f32>>>, Arc<AtomicBool>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let gate = Arc::new(AtomicBool::new(true));
        let w = EngineWorker::spawn(Box::new(Recorder {
            seen: seen.clone(),
            gate: gate.clone(),
        }));
        (w, seen, gate)
    }

    /// Answers "word N" for a job whose first sample is N; records prompts.
    struct Prompts(Arc<Mutex<Vec<Option<String>>>>);

    impl Transcriber for Prompts {
        fn transcribe(&mut self, pcm: &[f32], o: &TranscribeOptions) -> Result<Vec<Segment>, EngineError> {
            self.0.lock().unwrap().push(o.initial_prompt.clone());
            Ok(vec![Segment {
                start_ms: 0,
                end_ms: 1,
                text: format!("Ord {}.", pcm[0]),
                low_confidence: vec![],
            }])
        }
    }

    #[test]
    fn each_final_is_prompted_with_the_text_committed_before_it() {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let w = EngineWorker::spawn(Box::new(Prompts(Arc::clone(&prompts))));
        let ctx = Context::new("Før.");
        let quick = TranscribeOptions {
            fast: true,
            ..Default::default()
        };
        // Both queued before either has run.
        let a = w.submit_two_pass(
            vec![1.0],
            quick.clone(),
            TranscribeOptions::default(),
            |_| true,
            Some(&ctx),
        );
        let b = w.submit_two_pass(
            vec![2.0],
            quick,
            TranscribeOptions::default(),
            |_| true,
            Some(&ctx),
        );
        for rx in [&a, &a, &b, &b] {
            rx.recv().unwrap().unwrap();
        }
        let p: Vec<Option<String>> = prompts.lock().unwrap().clone();
        assert_eq!(
            p,
            [
                Some("Før.".to_string()),
                Some("Før.".into()),
                Some("Før. Ord 1.".into()),
                Some("Før. Ord 1.".into())
            ]
        );
    }

    #[test]
    fn a_command_and_a_preview_add_nothing_to_the_context() {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let w = EngineWorker::spawn(Box::new(Prompts(Arc::clone(&prompts))));
        let ctx = Context::new("");
        let cmd = w.submit_two_pass(
            vec![1.0],
            TranscribeOptions::default(),
            TranscribeOptions::default(),
            |_| false,
            Some(&ctx),
        );
        cmd.recv().unwrap().unwrap();
        w.submit_with(
            vec![2.0],
            TranscribeOptions::default(),
            Priority::LivePartial,
            Some(&ctx),
        )
        .recv()
        .unwrap()
        .unwrap();
        w.submit_two_pass(
            vec![3.0],
            TranscribeOptions::default(),
            TranscribeOptions::default(),
            |_| true,
            Some(&ctx),
        );
        let last = w.submit_two_pass(
            vec![4.0],
            TranscribeOptions::default(),
            TranscribeOptions::default(),
            |_| true,
            Some(&ctx),
        );
        last.recv().unwrap().unwrap();
        last.recv().unwrap().unwrap();
        let p = prompts.lock().unwrap().clone();
        assert_eq!(p[0], None, "nothing committed yet");
        assert_eq!(p[1], None, "a command is not text");
        assert_eq!(p[2], None, "a preview is not committed");
        assert_eq!(p.last().unwrap().as_deref(), Some("Ord 3."));
    }

    #[test]
    fn the_context_is_the_last_two_hundred_characters() {
        let ctx = Context::new(&"a".repeat(300));
        assert_eq!(ctx.prompt().unwrap().chars().count(), 200);
        assert_eq!(Context::new("  ").prompt(), None);
    }

    #[test]
    fn a_two_pass_job_runs_both_passes_before_the_next_job() {
        let (w, seen, gate) = worker();
        let quick = TranscribeOptions {
            fast: true,
            ..Default::default()
        };
        let a = w.submit_two_pass(
            vec![1.0],
            quick.clone(),
            TranscribeOptions::default(),
            |_| true,
            None,
        );
        let b = w.submit_two_pass(vec![2.0], quick, TranscribeOptions::default(), |_| true, None);
        gate.store(false, Ordering::SeqCst);
        for rx in [&a, &a, &b, &b] {
            rx.recv().unwrap().unwrap();
        }
        assert_eq!(*seen.lock().unwrap(), [1.0, 1.0, 2.0, 2.0]);
    }

    #[test]
    fn the_second_pass_is_skipped_when_the_first_settles_it() {
        let (w, seen, gate) = worker();
        let rx = w.submit_two_pass(
            vec![3.0],
            TranscribeOptions::default(),
            TranscribeOptions::default(),
            |r| r.as_ref().map_or(true, |s| s[0].text != "3"),
            None,
        );
        gate.store(false, Ordering::SeqCst);
        rx.recv().unwrap().unwrap();
        assert!(rx.recv().is_err(), "no second reply");
        assert_eq!(*seen.lock().unwrap(), [3.0]);
    }

    #[test]
    fn finals_run_before_file_chunks_before_previews() {
        let (w, seen, gate) = worker();
        let first = w.submit(vec![0.0], TranscribeOptions::default(), Priority::File);
        std::thread::sleep(Duration::from_millis(20)); // job 0 is now running, held by the gate
        let p = w.submit(vec![3.0], TranscribeOptions::default(), Priority::LivePartial);
        let f = w.submit(vec![2.0], TranscribeOptions::default(), Priority::File);
        let l = w.submit(vec![1.0], TranscribeOptions::default(), Priority::LiveFinal);
        gate.store(false, Ordering::SeqCst);
        for rx in [first, l, f, p] {
            rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        }
        assert_eq!(*seen.lock().unwrap(), [0.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn a_newer_preview_replaces_a_waiting_one() {
        let (w, seen, gate) = worker();
        let busy = w.submit(vec![0.0], TranscribeOptions::default(), Priority::LiveFinal);
        std::thread::sleep(Duration::from_millis(20));
        let old = w.submit(vec![1.0], TranscribeOptions::default(), Priority::LivePartial);
        let new = w.submit(vec![2.0], TranscribeOptions::default(), Priority::LivePartial);
        gate.store(false, Ordering::SeqCst);
        busy.recv().unwrap().unwrap();
        assert!(old.recv().is_err(), "the replaced preview is dropped");
        new.recv().unwrap().unwrap();
        assert_eq!(*seen.lock().unwrap(), [0.0, 2.0]);
    }

    #[test]
    fn dropping_the_worker_still_finishes_queued_finals() {
        let (w, seen, gate) = worker();
        let a = w.submit(vec![1.0], TranscribeOptions::default(), Priority::LiveFinal);
        let b = w.submit(vec![2.0], TranscribeOptions::default(), Priority::LiveFinal);
        let p = w.submit(vec![3.0], TranscribeOptions::default(), Priority::LivePartial);
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            gate.store(false, Ordering::SeqCst);
        });
        drop(w);
        release.join().unwrap();
        assert!(a.recv().unwrap().is_ok() && b.recv().unwrap().is_ok());
        assert!(p.recv().is_err() || *seen.lock().unwrap() == [1.0, 2.0, 3.0]);
        assert!(seen.lock().unwrap().starts_with(&[1.0, 2.0]));
    }

    #[test]
    fn worker_transcriber_blocks_until_the_job_is_done() {
        let (w, _seen, gate) = worker();
        gate.store(false, Ordering::SeqCst);
        let mut t = w.transcriber(Priority::File);
        let segs = t.transcribe(&[7.0], &TranscribeOptions::default()).unwrap();
        assert_eq!(segs[0].text, "7");
        assert!(!w.is_busy());
    }
}
