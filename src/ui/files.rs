//! The Files screen: a queue of audio/video files, the transcript of the
//! selected one filling in as it is transcribed, and a player to check it.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::engine::EngineHolder;
use super::{Deps, label};
use crate::ingest::{IngestError, IngestEvent, IngestOptions, Recognizer, ingest_file};
use crate::store::{DocumentId, NewDocument, Store};
use crate::text::{clock, duration};
use crate::worker::{EngineWorker, Priority};

#[derive(Debug, Clone, PartialEq)]
pub enum ItemStatus {
    Waiting,
    Running { done_ms: i64, total_ms: i64 },
    Done,
    Failed(String),
    Cancelled,
}

impl ItemStatus {
    pub fn label(&self) -> String {
        match self {
            ItemStatus::Waiting => "Queued".into(),
            ItemStatus::Running { done_ms, total_ms } if *total_ms > 0 => {
                format!("{}%", (done_ms * 100 / total_ms).clamp(0, 100))
            }
            ItemStatus::Running { .. } => "Reading…".into(),
            ItemStatus::Done => "Done".into(),
            ItemStatus::Failed(_) => "Failed".into(),
            ItemStatus::Cancelled => "Cancelled".into(),
        }
    }
}

struct Item {
    id: u64,
    path: PathBuf,
    doc: DocumentId,
    status: ItemStatus,
    duration_ms: Option<i64>,
    cancel: Arc<AtomicBool>,
    started: Option<Instant>,
    row: gtk::Box,
    status_label: gtk::Label,
    progress: gtk::ProgressBar,
}

struct Job {
    item: u64,
    path: PathBuf,
    doc: DocumentId,
    cancel: Arc<AtomicBool>,
}

enum Msg {
    Event(u64, IngestEvent),
    Done(u64, Result<usize, String>),
}

pub struct FilesPage {
    pub root: gtk::Box,
    /// Extra buttons for the window header.
    pub header_actions: gtk::Box,
    on_header_changed: super::Handler<()>,
    store: Rc<Store>,
    deps: Deps,
    engine: Rc<EngineHolder>,
    items: RefCell<Vec<Item>>,
    next_id: RefCell<u64>,
    selected: RefCell<Option<u64>>,
    queue_list: gtk::Box,
    transcript: gtk::ListBox,
    banner: gtk::Box,
    banner_title: gtk::Label,
    banner_progress: gtk::ProgressBar,
    banner_eta: gtk::Label,
    empty: gtk::Label,
    player_box: gtk::Box,
    media: RefCell<Option<gtk::MediaFile>>,
    show_timestamps: gtk::CheckButton,
    jobs: RefCell<Option<crossbeam_channel::Sender<Job>>>,
    on_open: super::Handler<DocumentId>,
}

impl FilesPage {
    pub fn new(store: Rc<Store>, deps: Deps, engine: Rc<EngineHolder>) -> Rc<Self> {
        let banner = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        banner.add_css_class("fx-files-banner");
        let banner_text = gtk::Box::new(gtk::Orientation::Vertical, 8);
        banner_text.set_hexpand(true);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let banner_title = label("", &["fx-field-label"]);
        banner_title.set_hexpand(true);
        let banner_eta = label("", &["fx-status"]);
        top.append(&banner_title);
        top.append(&banner_eta);
        let banner_progress = gtk::ProgressBar::new();
        banner_text.append(&top);
        banner_text.append(&banner_progress);
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("fx-secondary");
        cancel.set_valign(gtk::Align::Center);
        banner.append(&banner_text);
        banner.append(&cancel);
        banner.set_visible(false);

        let transcript = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        transcript.add_css_class("fx-transcript");
        let empty = label("Add audio or video files to transcribe them.", &["fx-status"]);
        empty.set_xalign(0.5);
        empty.set_vexpand(true);
        let center_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        center_box.append(&empty);
        center_box.append(&transcript);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&center_box)
            .vexpand(true)
            .build();

        let player_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        player_box.add_css_class("fx-dock");
        let open_editor = gtk::Button::with_label("Open in editor");
        open_editor.add_css_class("fx-secondary");
        open_editor.set_valign(gtk::Align::Center);

        let center = gtk::Box::new(gtk::Orientation::Vertical, 0);
        center.set_hexpand(true);
        center.append(&banner);
        center.append(&scroller);
        center.append(&player_box);

        let queue = gtk::Box::new(gtk::Orientation::Vertical, 14);
        queue.add_css_class("fx-inspector");
        queue.set_size_request(300, -1);
        queue.append(&label("QUEUE", &["fx-section-title"]));
        let drop_zone = gtk::Box::new(gtk::Orientation::Vertical, 10);
        drop_zone.add_css_class("fx-drop-zone");
        let drop_icon = gtk::Image::from_icon_name("document-open-symbolic");
        drop_icon.set_pixel_size(24);
        drop_zone.append(&drop_icon);
        let drop_text = label("Drop audio or video files here", &["fx-field-label"]);
        drop_text.set_xalign(0.5);
        drop_zone.append(&drop_text);
        let choose = gtk::Button::with_label("Choose files…");
        choose.add_css_class("fx-secondary");
        choose.set_halign(gtk::Align::Center);
        drop_zone.append(&choose);
        let formats = label("mp3, m4a, wav, flac, ogg, mp4, mkv", &["fx-field-note"]);
        formats.set_xalign(0.5);
        drop_zone.append(&formats);
        queue.append(&drop_zone);
        let queue_list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let queue_scroll = gtk::ScrolledWindow::builder()
            .child(&queue_list)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        queue.append(&queue_scroll);
        queue.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let show_timestamps = gtk::CheckButton::with_label("Show timestamps");
        show_timestamps.set_active(true);
        queue.append(&show_timestamps);

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&center);
        root.append(&queue);

        let page = Rc::new(Self {
            root,
            header_actions: gtk::Box::new(gtk::Orientation::Horizontal, 8),
            on_header_changed: RefCell::default(),
            store,
            deps,
            engine,
            items: RefCell::default(),
            next_id: RefCell::new(0),
            selected: RefCell::default(),
            queue_list,
            transcript,
            banner,
            banner_title,
            banner_progress,
            banner_eta,
            empty,
            player_box,
            media: RefCell::default(),
            show_timestamps,
            jobs: RefCell::default(),
            on_open: RefCell::default(),
        });

        let weak = Rc::downgrade(&page);
        cancel.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.cancel_running();
            }
        });
        let weak = Rc::downgrade(&page);
        open_editor.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade()
                && let Some(doc) = p.selected_doc()
                && let Some(f) = p.on_open.borrow().clone()
            {
                f(doc);
            }
        });
        page.player_box.append(&open_editor);
        let weak = Rc::downgrade(&page);
        page.show_timestamps.connect_toggled(move |_| {
            if let Some(p) = weak.upgrade() {
                p.render_transcript();
            }
        });
        let weak = Rc::downgrade(&page);
        choose.connect_clicked(move |b| {
            let Some(p) = weak.upgrade() else { return };
            let dialog = gtk::FileDialog::builder()
                .title("Choose audio or video files")
                .modal(true)
                .build();
            let window = b.root().and_downcast::<gtk::Window>();
            let weak = Rc::downgrade(&p);
            dialog.open_multiple(window.as_ref(), gio::Cancellable::NONE, move |res| {
                let (Ok(files), Some(p)) = (res, weak.upgrade()) else {
                    return;
                };
                let paths: Vec<PathBuf> = (0..files.n_items())
                    .filter_map(|i| files.item(i).and_downcast::<gio::File>()?.path())
                    .collect();
                p.add_paths(&paths);
            });
        });
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let weak = Rc::downgrade(&page);
        drop.connect_drop(move |_, value, _, _| {
            let (Ok(list), Some(p)) = (value.get::<gdk::FileList>(), weak.upgrade()) else {
                return false;
            };
            let paths: Vec<PathBuf> = list.files().iter().filter_map(|f| f.path()).collect();
            p.add_paths(&paths);
            true
        });
        page.root.add_controller(drop);
        page
    }

    pub fn connect_open_in_editor(&self, f: impl Fn(DocumentId) + 'static) {
        *self.on_open.borrow_mut() = Some(Rc::new(f));
    }

    /// Queues files for transcription, one new document per file.
    pub fn add_paths(self: &Rc<Self>, paths: &[PathBuf]) {
        let mut first_new = None;
        for path in paths {
            let title = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Optagelse".into());
            let doc = match self.store.create_document(&NewDocument {
                template_id: Some(self.deps.settings.borrow().default_template.clone()),
                ..NewDocument::file(&title)
            }) {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!("creating a document for {}: {e}", path.display());
                    continue;
                }
            };
            let id = {
                let mut n = self.next_id.borrow_mut();
                *n += 1;
                *n
            };
            first_new.get_or_insert(id);
            let item = self.make_item(id, path.clone(), doc);
            self.queue_list.append(&item.row);
            self.items.borrow_mut().push(item);
        }
        if self.selected.borrow().is_none()
            && let Some(id) = first_new
        {
            self.select(id);
        }
        self.dispatch_waiting();
    }

    fn make_item(self: &Rc<Self>, id: u64, path: PathBuf, doc: DocumentId) -> Item {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 6);
        row.add_css_class("fx-queue-item");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = label(
            &path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            &["fx-field-label"],
        );
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let status_label = label("Queued", &["fx-stats"]);
        top.append(&name);
        top.append(&status_label);
        let progress = gtk::ProgressBar::new();
        progress.set_visible(false);
        row.append(&top);
        row.append(&progress);
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(self);
        click.connect_released(move |_, _, _, _| {
            if let Some(p) = weak.upgrade() {
                p.select(id);
            }
        });
        row.add_controller(click);
        Item {
            id,
            path,
            doc,
            status: ItemStatus::Waiting,
            duration_ms: None,
            cancel: Arc::new(AtomicBool::new(false)),
            started: None,
            row,
            status_label,
            progress,
        }
    }

    /// Sends waiting items to the ingest thread, loading the model first if needed.
    fn dispatch_waiting(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.engine.with_worker(move |result| {
            let Some(p) = weak.upgrade() else { return };
            match result {
                Ok(worker) => {
                    p.ensure_thread(worker);
                    let jobs = p.jobs.borrow().clone();
                    let Some(jobs) = jobs else { return };
                    for item in p
                        .items
                        .borrow_mut()
                        .iter_mut()
                        .filter(|i| i.status == ItemStatus::Waiting)
                    {
                        item.status = ItemStatus::Running {
                            done_ms: 0,
                            total_ms: 0,
                        };
                        let _ = jobs.send(Job {
                            item: item.id,
                            path: item.path.clone(),
                            doc: item.doc,
                            cancel: Arc::clone(&item.cancel),
                        });
                    }
                    p.refresh_rows();
                }
                Err(e) => {
                    for item in p
                        .items
                        .borrow_mut()
                        .iter_mut()
                        .filter(|i| i.status == ItemStatus::Waiting)
                    {
                        item.status = ItemStatus::Failed(format!("Could not load the speech model: {e}"));
                    }
                    p.refresh_rows();
                }
            }
        });
    }

    fn ensure_thread(self: &Rc<Self>, worker: Arc<EngineWorker>) {
        if self.jobs.borrow().is_some() {
            return;
        }
        let (job_tx, job_rx) = crossbeam_channel::unbounded::<Job>();
        let (msg_tx, msg_rx) = async_channel::unbounded::<Msg>();
        let db = self.deps.paths.database();
        let settings = self.deps.settings();
        let paths = self.deps.paths.clone();
        let file_vad = Arc::clone(&self.deps.file_vad);
        std::thread::Builder::new()
            .name("fennec-ingest".into())
            .spawn(move || {
                let store = match Store::open(&db) {
                    Ok(s) => s,
                    Err(e) => {
                        for job in job_rx {
                            let _ = msg_tx.send_blocking(Msg::Done(job.item, Err(e.to_string())));
                        }
                        return;
                    }
                };
                let opts = IngestOptions {
                    vocabulary: settings.vocabulary.clone(),
                    ..Default::default()
                };
                for job in job_rx {
                    let mut vad = file_vad(&settings, &paths);
                    let mut engine = worker.transcriber(Priority::File);
                    let tx = msg_tx.clone();
                    let result = ingest_file(
                        &job.path,
                        job.doc,
                        &store,
                        Recognizer {
                            engine: &mut engine,
                            vad: &mut *vad,
                        },
                        &opts,
                        &job.cancel,
                        |e| {
                            let _ = tx.send_blocking(Msg::Event(job.item, e));
                        },
                    );
                    let result = result.map_err(|e| match e {
                        IngestError::Cancelled => "cancelled".to_string(),
                        e => e.to_string(),
                    });
                    let _ = msg_tx.send_blocking(Msg::Done(job.item, result));
                }
            })
            .expect("spawning the ingest thread");
        *self.jobs.borrow_mut() = Some(job_tx);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(msg) = msg_rx.recv().await {
                let Some(p) = weak.upgrade() else { return };
                p.handle(msg);
            }
        });
    }

    fn handle(self: &Rc<Self>, msg: Msg) {
        let selected = *self.selected.borrow();
        let mut rerender = false;
        {
            let mut items = self.items.borrow_mut();
            let (id, item) = match &msg {
                Msg::Event(id, _) | Msg::Done(id, _) => (*id, items.iter_mut().find(|i| i.id == *id)),
            };
            let Some(item) = item else { return };
            match msg {
                Msg::Event(_, IngestEvent::Decoded { duration_ms }) => {
                    item.duration_ms = Some(duration_ms);
                    item.started = Some(Instant::now());
                    item.status = ItemStatus::Running {
                        done_ms: 0,
                        total_ms: duration_ms,
                    };
                }
                Msg::Event(_, IngestEvent::Progress { done_ms, total_ms }) => {
                    item.status = ItemStatus::Running { done_ms, total_ms };
                }
                Msg::Event(_, IngestEvent::Paragraph(_)) => rerender = selected == Some(id),
                Msg::Event(_, IngestEvent::Finished { .. }) => {}
                Msg::Done(_, Ok(_)) => item.status = ItemStatus::Done,
                Msg::Done(_, Err(e)) if e == "cancelled" => item.status = ItemStatus::Cancelled,
                Msg::Done(_, Err(e)) => item.status = ItemStatus::Failed(e),
            }
        }
        self.refresh_rows();
        if rerender || selected.is_some() {
            self.render_banner();
        }
        if rerender {
            self.render_transcript();
        }
    }

    fn refresh_rows(&self) {
        for item in self.items.borrow().iter() {
            item.status_label.set_text(&item.status.label());
            let tooltip = match &item.status {
                ItemStatus::Failed(e) => Some(e.as_str()),
                _ => None,
            };
            item.row.set_tooltip_text(tooltip);
            match item.status {
                ItemStatus::Running { done_ms, total_ms } if total_ms > 0 => {
                    item.progress.set_visible(true);
                    item.progress.set_fraction(done_ms as f64 / total_ms as f64);
                }
                _ => item.progress.set_visible(false),
            }
            if Some(item.id) == *self.selected.borrow() {
                item.row.add_css_class("selected");
            } else {
                item.row.remove_css_class("selected");
            }
        }
        self.render_banner();
    }

    fn render_banner(&self) {
        let items = self.items.borrow();
        let Some(item) = self
            .selected
            .borrow()
            .and_then(|id| items.iter().find(|i| i.id == id))
        else {
            self.banner.set_visible(false);
            return;
        };
        match &item.status {
            ItemStatus::Running { done_ms, total_ms } => {
                self.banner.set_visible(true);
                let pct = if *total_ms > 0 {
                    done_ms * 100 / total_ms
                } else {
                    0
                };
                self.banner_title.set_text(&format!("Transcribing · {pct}%"));
                self.banner_progress.set_fraction(if *total_ms > 0 {
                    *done_ms as f64 / *total_ms as f64
                } else {
                    0.0
                });
                let eta = item.started.filter(|_| *done_ms > 0).map(|s| {
                    let per_ms = s.elapsed().as_secs_f64() / *done_ms as f64;
                    ((total_ms - done_ms) as f64 * per_ms).round() as u64
                });
                self.banner_eta.set_text(&match eta {
                    Some(secs) if secs >= 60 => format!("about {} min left", secs / 60 + 1),
                    Some(secs) => format!("about {secs} s left"),
                    None => String::new(),
                });
            }
            ItemStatus::Failed(e) => {
                self.banner.set_visible(true);
                self.banner_title.set_text(&format!("Failed: {e}"));
                self.banner_progress.set_fraction(0.0);
                self.banner_eta.set_text("");
            }
            _ => self.banner.set_visible(false),
        }
    }

    pub fn select(self: &Rc<Self>, id: u64) {
        *self.selected.borrow_mut() = Some(id);
        let path = self
            .items
            .borrow()
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.path.clone());
        if let Some(path) = path {
            self.set_media(&path);
        }
        self.refresh_rows();
        self.render_transcript();
        self.header_changed();
    }

    /// The document of the selected file, for Export.
    pub fn current_document(&self) -> Option<DocumentId> {
        self.selected_doc()
    }

    /// Breadcrumb for the header: project, title and file details.
    pub fn header_info(&self) -> (Option<String>, String, String) {
        let Some(doc) = self.selected_doc().and_then(|id| self.store.document(id).ok()) else {
            return (None, "Files".into(), String::new());
        };
        let project = doc
            .project_id
            .and_then(|p| self.store.projects().ok()?.into_iter().find(|x| x.id == p))
            .map(|p| p.name);
        let file = doc
            .audio_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        (
            Some(project.unwrap_or_else(|| "Unsorted".into())),
            doc.title,
            file,
        )
    }

    pub fn connect_header_changed(&self, f: impl Fn() + 'static) {
        *self.on_header_changed.borrow_mut() = Some(Rc::new(move |()| f()));
    }

    fn header_changed(&self) {
        if let Some(f) = self.on_header_changed.borrow().clone() {
            f(());
        }
    }

    fn selected_doc(&self) -> Option<DocumentId> {
        let id = (*self.selected.borrow())?;
        self.items.borrow().iter().find(|i| i.id == id).map(|i| i.doc)
    }

    fn set_media(&self, path: &Path) {
        while let Some(c) = self.player_box.first_child() {
            if c.downcast_ref::<gtk::Button>().is_some() {
                break;
            }
            self.player_box.remove(&c);
        }
        let media = gtk::MediaFile::for_filename(path);
        let controls = gtk::MediaControls::new(Some(&media));
        controls.set_hexpand(true);
        self.player_box.prepend(&controls);
        *self.media.borrow_mut() = Some(media);
    }

    fn render_transcript(self: &Rc<Self>) {
        while let Some(c) = self.transcript.first_child() {
            self.transcript.remove(&c);
        }
        let Some(doc) = self.selected_doc() else {
            self.empty.set_visible(true);
            return;
        };
        let paragraphs = self.store.paragraphs(doc).unwrap_or_default();
        self.empty.set_visible(paragraphs.is_empty());
        if paragraphs.is_empty() {
            self.empty.set_text("The transcript appears here as it is made.");
        }
        let stamps = self.show_timestamps.is_active();
        for p in paragraphs {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 20);
            row.add_css_class("fx-transcript-row");
            if stamps && let Some(ms) = p.start_ms {
                let ts = gtk::Button::with_label(&clock(ms));
                ts.add_css_class("fx-timestamp");
                ts.set_valign(gtk::Align::Start);
                ts.set_tooltip_text(Some("Play from here"));
                let weak = Rc::downgrade(self);
                ts.connect_clicked(move |_| {
                    if let Some(page) = weak.upgrade() {
                        page.play_from(ms);
                    }
                });
                row.append(&ts);
            }
            let text = label(&p.text, &["fx-transcript-text"]);
            text.set_wrap(true);
            text.set_hexpand(true);
            text.set_selectable(true);
            row.append(&text);
            self.transcript.append(&row);
        }
    }

    fn play_from(&self, ms: i64) {
        if let Some(m) = self.media.borrow().as_ref() {
            m.seek(ms * 1000);
            m.play();
        }
    }

    fn cancel_running(&self) {
        for item in self.items.borrow().iter() {
            if matches!(item.status, ItemStatus::Running { .. } | ItemStatus::Waiting)
                && Some(item.id) == *self.selected.borrow()
            {
                item.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    /// (file name, status) for each queued file (tests).
    pub fn statuses(&self) -> Vec<(String, String)> {
        self.items
            .borrow()
            .iter()
            .map(|i| {
                (
                    i.path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    i.status.label(),
                )
            })
            .collect()
    }

    /// The selected transcript's paragraph texts as shown (tests).
    pub fn shown_texts(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut row = self.transcript.first_child();
        while let Some(r) = row {
            if let Some(t) = super::texts_in(&r).into_iter().find(|t| !t.starts_with("00:")) {
                out.push(t);
            }
            row = r.next_sibling();
        }
        out
    }

    pub fn total_duration_label(&self) -> String {
        let total: i64 = self.items.borrow().iter().filter_map(|i| i.duration_ms).sum();
        duration(total)
    }
}
