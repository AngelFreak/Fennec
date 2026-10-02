//! The one loaded speech model, shared by dictation and file import. It is
//! loaded on first use, off the main thread; callers queue until it is ready.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gtk::glib;

use super::Deps;
use crate::worker::EngineWorker;

type Waiter = Box<dyn FnOnce(Result<Arc<EngineWorker>, String>)>;

pub struct EngineHolder {
    deps: Deps,
    worker: RefCell<Option<Arc<EngineWorker>>>,
    waiting: RefCell<Vec<Waiter>>,
    loading: Cell<bool>,
}

impl EngineHolder {
    pub fn new(deps: Deps) -> Rc<Self> {
        Rc::new(Self {
            deps,
            worker: RefCell::default(),
            waiting: RefCell::default(),
            loading: Cell::new(false),
        })
    }

    pub fn is_loaded(&self) -> bool {
        self.worker.borrow().is_some()
    }

    /// Runs `f` with the worker, loading the model first if needed.
    pub fn with_worker(self: &Rc<Self>, f: impl FnOnce(Result<Arc<EngineWorker>, String>) + 'static) {
        if let Some(w) = self.worker.borrow().clone() {
            f(Ok(w));
            return;
        }
        self.waiting.borrow_mut().push(Box::new(f));
        if self.loading.replace(true) {
            return;
        }
        let (tx, rx) = async_channel::bounded(1);
        let factory = Arc::clone(&self.deps.engine);
        let settings = self.deps.settings.clone();
        let paths = self.deps.paths.clone();
        std::thread::spawn(move || {
            let _ = tx.send_blocking(factory(&settings, &paths));
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(result) = rx.recv().await else { return };
            let Some(me) = weak.upgrade() else { return };
            me.loading.set(false);
            let result = result.map(|engine| {
                let w = Arc::new(EngineWorker::spawn(engine));
                *me.worker.borrow_mut() = Some(Arc::clone(&w));
                w
            });
            let waiting = std::mem::take(&mut *me.waiting.borrow_mut());
            for f in waiting {
                f(result.clone());
            }
        });
    }

    /// Forgets the loaded model (after a model or backend change); the next
    /// use loads it again.
    pub fn reset(&self) {
        *self.worker.borrow_mut() = None;
    }
}
