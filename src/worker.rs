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

struct Job {
    pcm: Vec<f32>,
    opts: TranscribeOptions,
    reply: Sender<Reply>,
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
        enqueue(&self.shared, pcm, opts, priority)
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
) -> Receiver<Reply> {
    let (tx, rx) = bounded(1);
    let job = Job { pcm, opts, reply: tx };
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
        let result = engine.transcribe(&job.pcm, &job.opts);
        // The requester may have given up; that is fine.
        let _ = job.reply.send(result);
        lock.lock().expect("engine queue poisoned").busy = false;
    }
}

pub struct WorkerTranscriber {
    shared: Arc<(Mutex<Queues>, Condvar)>,
    priority: Priority,
}

impl Transcriber for WorkerTranscriber {
    fn transcribe(&mut self, pcm: &[f32], opts: &TranscribeOptions) -> Result<Vec<Segment>, EngineError> {
        let rx = enqueue(&self.shared, pcm.to_vec(), opts.clone(), self.priority);
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
