//! The Files screen: a queue of audio/video files, the transcript of the
//! selected one filling in as it is transcribed, and a player to check it.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use super::engine::EngineHolder;
use super::{Deps, icon_button, label};
use crate::audio::decode::decode_file;
use crate::audio::playback::{Player, peaks};
use crate::ingest::{IngestError, IngestEvent, IngestOptions, Recognizer, ingest_file};
use crate::store::{DocumentId, NewDocument, Store};
use crate::text::{clock, duration};
use crate::worker::{EngineWorker, Priority};

/// Playback speeds the speed button cycles through, with their labels.
const SPEEDS: [(f64, &str); 4] = [(1.0, "1.0×"), (1.25, "1.25×"), (1.5, "1.5×"), (0.75, "0.75×")];
/// Bars in the waveform timeline.
const BARS: usize = 100;
/// Pause that starts a new paragraph when "New paragraph on long pauses" is on.
const PARAGRAPH_GAP_MS: i64 = 1_500;

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

    /// How much of the recording has a transcript, 0–1, where known.
    fn fraction(&self) -> Option<f64> {
        match self {
            ItemStatus::Running { done_ms, total_ms } if *total_ms > 0 => {
                Some((*done_ms as f64 / *total_ms as f64).clamp(0.0, 1.0))
            }
            ItemStatus::Running { .. } | ItemStatus::Waiting => Some(0.0),
            ItemStatus::Done => Some(1.0),
            ItemStatus::Failed(_) | ItemStatus::Cancelled => None,
        }
    }
}

struct Item {
    id: u64,
    path: PathBuf,
    doc: DocumentId,
    status: ItemStatus,
    duration_ms: Option<i64>,
    model: String,
    cancel: Arc<AtomicBool>,
    started: Option<Instant>,
    row: gtk::Box,
    status_label: gtk::Label,
    done_icon: gtk::Image,
    progress: gtk::ProgressBar,
    meta: gtk::Label,
}

struct Job {
    item: u64,
    path: PathBuf,
    doc: DocumentId,
    gap_ms: i64,
    cancel: Arc<AtomicBool>,
}

enum Msg {
    Event(u64, IngestEvent),
    Done(u64, Result<usize, String>),
}

/// What the waveform timeline draws.
#[derive(Default)]
struct Wave {
    peaks: Vec<f32>,
    played: f64,
    transcribed: f64,
}

/// The player bar under the transcript.
struct PlayerBar {
    root: gtk::Box,
    play: gtk::Button,
    play_icon: gtk::Image,
    time: gtk::Label,
    area: gtk::DrawingArea,
    speed: gtk::Button,
    wave: Rc<RefCell<Wave>>,
}

impl PlayerBar {
    fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        root.add_css_class("fx-player");
        let play_icon = gtk::Image::from_icon_name("fennec-play-symbolic");
        play_icon.set_pixel_size(18);
        let play = gtk::Button::builder()
            .child(&play_icon)
            .valign(gtk::Align::Center)
            .build();
        play.add_css_class("fx-play");
        play.set_tooltip_text(Some("Play"));
        play.update_property(&[gtk::accessible::Property::Label("Play")]);
        play.set_sensitive(false);
        let time = label("00:00 / 00:00", &["fx-mono", "fx-player-time"]);
        time.set_valign(gtk::Align::Center);

        let wave = Rc::new(RefCell::new(Wave::default()));
        let area = gtk::DrawingArea::builder()
            .content_height(44)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        area.add_css_class("fx-wave");
        area.update_property(&[gtk::accessible::Property::Label(
            "Audio timeline: played, transcribed and pending",
        )]);
        // Zero-size probes carry the other two bar colours from the theme.
        let transcribed_probe = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        transcribed_probe.add_css_class("fx-wave-transcribed");
        let pending_probe = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pending_probe.add_css_class("fx-wave-pending");
        let wave_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        wave_box.set_hexpand(true);
        wave_box.set_valign(gtk::Align::Center);
        wave_box.append(&area);
        wave_box.append(&transcribed_probe);
        wave_box.append(&pending_probe);
        let draw_wave = Rc::clone(&wave);
        area.set_draw_func(move |area, cr, w, h| {
            let wave = draw_wave.borrow();
            let colors = [area.color(), transcribed_probe.color(), pending_probe.color()];
            let (w, h) = (f64::from(w), f64::from(h));
            let gap = 2.0;
            let bar = ((w - gap * (BARS - 1) as f64) / BARS as f64).max(1.0);
            for i in 0..BARS {
                let f = i as f64 / BARS as f64;
                let amp = if wave.peaks.is_empty() {
                    0.0
                } else {
                    f64::from(wave.peaks[i * wave.peaks.len() / BARS]).sqrt()
                };
                let bar_h = (4.0 + amp * (h - 4.0)).min(h);
                let c = if f < wave.played {
                    &colors[0]
                } else if f < wave.transcribed {
                    &colors[1]
                } else {
                    &colors[2]
                };
                cr.set_source_rgba(
                    f64::from(c.red()),
                    f64::from(c.green()),
                    f64::from(c.blue()),
                    f64::from(c.alpha()),
                );
                cr.rectangle(i as f64 * (bar + gap), (h - bar_h) / 2.0, bar, bar_h);
                let _ = cr.fill();
            }
        });

        let speed = gtk::Button::with_label(SPEEDS[0].1);
        speed.add_css_class("fx-secondary");
        speed.add_css_class("fx-speed");
        speed.set_valign(gtk::Align::Center);
        speed.set_tooltip_text(Some("Playback speed"));

        root.append(&play);
        root.append(&time);
        root.append(&wave_box);
        root.append(&speed);
        Self {
            root,
            play,
            play_icon,
            time,
            area,
            speed,
            wave,
        }
    }
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
    transcript: gtk::Box,
    /// Paragraph rows of the shown transcript with their start times.
    rows: RefCell<Vec<(Option<i64>, gtk::Box)>>,
    current_row: Cell<Option<usize>>,
    banner: gtk::Box,
    banner_title: gtk::Label,
    banner_progress: gtk::ProgressBar,
    banner_eta: gtk::Label,
    empty: gtk::Label,
    bar: PlayerBar,
    player: RefCell<Option<Rc<Player>>>,
    /// Bumped on every file switch so a late decode is ignored.
    load_generation: Cell<u64>,
    speed: Cell<usize>,
    show_timestamps: gtk::CheckButton,
    paragraph_pauses: gtk::CheckButton,
    jobs: RefCell<Option<crossbeam_channel::Sender<Job>>>,
    on_open: super::Handler<DocumentId>,
}

impl FilesPage {
    pub fn new(store: Rc<Store>, deps: Deps, engine: Rc<EngineHolder>) -> Rc<Self> {
        let banner = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        banner.add_css_class("fx-files-banner");
        let banner_text = gtk::Box::new(gtk::Orientation::Vertical, 8);
        banner_text.set_hexpand(true);
        banner_text.set_valign(gtk::Align::Center);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let banner_title = label("", &["fx-banner-title"]);
        banner_title.set_hexpand(true);
        banner_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
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

        let transcript = gtk::Box::new(gtk::Orientation::Vertical, 4);
        transcript.add_css_class("fx-transcript-list");
        let empty = label("Add audio or video files to transcribe them.", &["fx-status"]);
        empty.set_xalign(0.5);
        empty.set_vexpand(true);
        let center_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        center_box.append(&empty);
        center_box.append(&transcript);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&center_box)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        let bar = PlayerBar::new();

        let center = gtk::Box::new(gtk::Orientation::Vertical, 0);
        center.set_hexpand(true);
        center.append(&banner);
        center.append(&scroller);
        center.append(&bar.root);

        let queue = gtk::Box::new(gtk::Orientation::Vertical, 14);
        queue.add_css_class("fx-inspector");
        queue.add_css_class("fx-files-aside");
        queue.set_size_request(300, -1);
        queue.set_hexpand(false);
        queue.update_property(&[gtk::accessible::Property::Label("Queue")]);
        queue.append(&label("QUEUE", &["fx-section-title"]));
        let drop_zone = gtk::Box::new(gtk::Orientation::Vertical, 10);
        drop_zone.add_css_class("fx-drop-zone");
        let drop_icon = gtk::Image::from_icon_name("fennec-upload-symbolic");
        drop_icon.set_pixel_size(24);
        drop_icon.add_css_class("fx-drop-icon");
        drop_zone.append(&drop_icon);
        let drop_text = label("Drop audio or video files here", &["fx-drop-text"]);
        drop_text.set_xalign(0.5);
        drop_zone.append(&drop_text);
        let choose = gtk::Button::with_label("Choose files…");
        choose.add_css_class("fx-secondary");
        choose.set_halign(gtk::Align::Center);
        drop_zone.append(&choose);
        let formats = label("mp3, m4a, wav, flac, ogg, mp4, mkv", &["fx-field-note", "small"]);
        formats.set_xalign(0.5);
        drop_zone.append(&formats);
        queue.append(&drop_zone);
        let queue_list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        // Room for the cards' rings, which the scroller would clip.
        queue_list.set_margin_start(1);
        queue_list.set_margin_end(1);
        queue_list.set_margin_top(1);
        queue_list.set_margin_bottom(1);
        let queue_scroll = gtk::ScrolledWindow::builder()
            .child(&queue_list)
            .propagate_natural_height(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        queue.append(&queue_scroll);
        queue.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let show_timestamps = gtk::CheckButton::with_label("Show timestamps");
        show_timestamps.set_active(true);
        queue.append(&show_timestamps);
        let paragraph_pauses = gtk::CheckButton::with_label("New paragraph on long pauses");
        paragraph_pauses.set_active(true);
        paragraph_pauses.set_tooltip_text(Some("Applies to files added from now on"));
        queue.append(&paragraph_pauses);

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.append(&center);
        root.append(&queue);

        let header_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let open_editor = icon_button("document-edit-symbolic", "Open in editor", &["fx-header-chip"]);
        open_editor.set_valign(gtk::Align::Center);
        header_actions.append(&open_editor);

        let page = Rc::new(Self {
            root,
            header_actions,
            on_header_changed: RefCell::default(),
            store,
            deps,
            engine,
            items: RefCell::default(),
            next_id: RefCell::new(0),
            selected: RefCell::default(),
            queue_list,
            transcript,
            rows: RefCell::default(),
            current_row: Cell::new(None),
            banner,
            banner_title,
            banner_progress,
            banner_eta,
            empty,
            bar,
            player: RefCell::default(),
            load_generation: Cell::new(0),
            speed: Cell::new(0),
            show_timestamps,
            paragraph_pauses,
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
        let weak = Rc::downgrade(&page);
        page.show_timestamps.connect_toggled(move |_| {
            if let Some(p) = weak.upgrade() {
                p.render_transcript();
            }
        });
        page.wire_player();
        let weak = Rc::downgrade(&page);
        choose.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.choose_files();
            }
        });
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let zone = drop_zone.clone();
        drop.connect_enter(move |_, _, _| {
            zone.add_css_class("hover");
            gdk::DragAction::COPY
        });
        let zone = drop_zone.clone();
        drop.connect_leave(move |_| zone.remove_css_class("hover"));
        let weak = Rc::downgrade(&page);
        drop.connect_drop(move |_, value, _, _| {
            drop_zone.remove_css_class("hover");
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

    fn wire_player(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.bar.play.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.toggle_play();
            }
        });
        let weak = Rc::downgrade(self);
        self.bar.speed.connect_clicked(move |b| {
            let Some(p) = weak.upgrade() else { return };
            let next = (p.speed.get() + 1) % SPEEDS.len();
            p.speed.set(next);
            b.set_label(SPEEDS[next].1);
            if let Some(player) = p.player.borrow().as_ref() {
                player.set_speed(SPEEDS[next].0);
            }
        });
        let seek = gtk::GestureClick::new();
        let weak = Rc::downgrade(self);
        seek.connect_pressed(move |g, _, x, _| {
            let Some(p) = weak.upgrade() else { return };
            let width = f64::from(g.widget().map(|w| w.width()).unwrap_or(1).max(1));
            p.seek_fraction(x / width);
        });
        self.bar.area.add_controller(seek);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(200), move || {
            let Some(p) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            p.tick();
            glib::ControlFlow::Continue
        });
    }

    pub fn connect_open_in_editor(&self, f: impl Fn(DocumentId) + 'static) {
        *self.on_open.borrow_mut() = Some(Rc::new(f));
    }

    /// Queues files for transcription, one new document per file.
    /// Opens the file chooser; the chosen files join the queue.
    pub fn choose_files(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder()
            .title("Choose audio or video files")
            .modal(true)
            .build();
        let window = self.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        dialog.open_multiple(window.as_ref(), gio::Cancellable::NONE, move |res| {
            let (Ok(files), Some(p)) = (res, weak.upgrade()) else {
                return;
            };
            let paths: Vec<PathBuf> = (0..files.n_items())
                .filter_map(|i| files.item(i).and_downcast::<gio::File>()?.path())
                .collect();
            p.add_paths(&paths);
        });
    }

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
        let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        row.add_css_class("fx-queue-item");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = label(
            &path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            &["fx-queue-name"],
        );
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let status = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let done_icon = gtk::Image::from_icon_name("fennec-check-symbolic");
        done_icon.set_pixel_size(14);
        done_icon.add_css_class("fx-queue-check");
        done_icon.set_visible(false);
        let status_label = label("Queued", &["fx-queue-status"]);
        status.append(&done_icon);
        status.append(&status_label);
        top.append(&name);
        top.append(&status);
        let progress = gtk::ProgressBar::new();
        progress.add_css_class("thin");
        progress.set_visible(false);
        let meta = label("", &["fx-field-note"]);
        meta.set_ellipsize(gtk::pango::EllipsizeMode::End);
        meta.set_visible(false);
        row.append(&top);
        row.append(&progress);
        row.append(&meta);
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
            model: model_name(&self.deps.settings.borrow()),
            cancel: Arc::new(AtomicBool::new(false)),
            started: None,
            row,
            status_label,
            done_icon,
            progress,
            meta,
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
                    let gap_ms = if p.paragraph_pauses.is_active() {
                        PARAGRAPH_GAP_MS
                    } else {
                        i64::MAX
                    };
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
                            gap_ms,
                            cancel: Arc::clone(&item.cancel),
                        });
                    }
                    p.refresh_rows();
                    p.render_transcript();
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
                    p.render_transcript();
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
        let punctuator_factory = Arc::clone(&self.deps.punctuator);
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
                let mut opts = IngestOptions {
                    vocabulary: settings.vocabulary.clone(),
                    context: crate::models::takes_context(&settings.model),
                    ..Default::default()
                };
                let punct_dir = paths.models().join(crate::punctuation::DIR);
                let punctuator = (settings.punctuate && crate::punctuation::installed(&punct_dir))
                    .then(|| punctuator_factory(&paths))
                    .and_then(|r| {
                        r.map_err(|e| tracing::warn!("no punctuation for imports: {e}"))
                            .ok()
                    });
                for job in job_rx {
                    opts.paragraph_gap_ms = job.gap_ms;
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
                            punctuator: punctuator.as_deref(),
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
        let mut header = false;
        {
            let mut items = self.items.borrow_mut();
            let (id, item) = match &msg {
                Msg::Event(id, _) | Msg::Done(id, _) => (*id, items.iter_mut().find(|i| i.id == *id)),
            };
            let Some(item) = item else { return };
            let is_selected = selected == Some(id);
            match msg {
                Msg::Event(_, IngestEvent::Decoded { duration_ms }) => {
                    item.duration_ms = Some(duration_ms);
                    item.started = Some(Instant::now());
                    item.status = ItemStatus::Running {
                        done_ms: 0,
                        total_ms: duration_ms,
                    };
                    header = is_selected;
                }
                Msg::Event(_, IngestEvent::Progress { done_ms, total_ms }) => {
                    item.status = ItemStatus::Running { done_ms, total_ms };
                }
                Msg::Event(_, IngestEvent::Paragraph(_)) => rerender = is_selected,
                Msg::Event(_, IngestEvent::Finished { .. }) => {}
                Msg::Done(_, result) => {
                    item.status = match result {
                        Ok(_) => ItemStatus::Done,
                        Err(e) if e == "cancelled" => ItemStatus::Cancelled,
                        Err(e) => ItemStatus::Failed(e),
                    };
                    rerender = is_selected;
                    header = is_selected;
                }
            }
        }
        self.refresh_rows();
        if rerender {
            self.render_transcript();
        }
        self.update_wave();
        if header {
            self.header_changed();
        }
    }

    fn refresh_rows(&self) {
        let selected = *self.selected.borrow();
        for item in self.items.borrow().iter() {
            item.status_label.set_text(&item.status.label());
            let running = matches!(item.status, ItemStatus::Running { .. });
            for (class, on) in [
                ("accent", running),
                ("done", item.status == ItemStatus::Done),
                ("error", matches!(item.status, ItemStatus::Failed(_))),
            ] {
                if on {
                    item.status_label.add_css_class(class);
                } else {
                    item.status_label.remove_css_class(class);
                }
            }
            item.done_icon.set_visible(item.status == ItemStatus::Done);
            let length = item.duration_ms.map(duration);
            let meta = match &item.status {
                ItemStatus::Running { .. } => Some(match &length {
                    Some(l) => format!("{l} · {}", item.model),
                    None => item.model.clone(),
                }),
                ItemStatus::Failed(e) => Some(e.clone()),
                _ => length,
            };
            item.meta.set_visible(meta.is_some());
            item.meta.set_text(meta.as_deref().unwrap_or_default());
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
            item.row.set_spacing(if running { 8 } else { 4 });
            if Some(item.id) == selected {
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
                self.banner_title
                    .set_markup(&format!("<span weight=\"600\">Transcribing</span> · {pct}%"));
                self.banner_progress.set_visible(true);
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
                self.banner_title.set_markup(&format!(
                    "<span weight=\"600\">Failed</span> · {}",
                    glib::markup_escape_text(e)
                ));
                self.banner_progress.set_visible(false);
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

    /// Selects the queued file with this name (tests).
    pub fn select_file(self: &Rc<Self>, name: &str) {
        let id = self
            .items
            .borrow()
            .iter()
            .find(|i| i.path.file_name().is_some_and(|n| n == name))
            .map(|i| i.id);
        if let Some(id) = id {
            self.select(id);
        }
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
        let length = self.selected_item_duration().map(duration);
        let details = match (file.is_empty(), length) {
            (false, Some(l)) => format!("{file} · {l}"),
            (true, Some(l)) => l,
            (_, None) => file,
        };
        (
            Some(project.unwrap_or_else(|| "Unsorted".into())),
            doc.title,
            details,
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

    fn selected_status(&self) -> Option<ItemStatus> {
        let id = (*self.selected.borrow())?;
        self.items
            .borrow()
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.status.clone())
    }

    fn selected_item_duration(&self) -> Option<i64> {
        let id = (*self.selected.borrow())?;
        self.items.borrow().iter().find(|i| i.id == id)?.duration_ms
    }

    /// Loads `path` for playback: decodes it off the main thread, then
    /// fills in the waveform and enables the play button.
    fn set_media(self: &Rc<Self>, path: &Path) {
        let generation = self.load_generation.get() + 1;
        self.load_generation.set(generation);
        if let Some(old) = self.player.borrow_mut().take() {
            old.pause();
        }
        self.bar.wave.borrow_mut().peaks.clear();
        self.bar.play.set_sensitive(false);
        self.set_play_icon(false);
        self.current_row.set(None);
        self.update_wave();
        let (tx, rx) = async_channel::bounded(1);
        let path = path.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("fennec-waveform".into())
            .spawn(move || {
                let result = decode_file(&path).map(|samples| {
                    let shape = peaks(&samples, BARS);
                    (Arc::new(samples), shape)
                });
                let _ = tx.send_blocking(result.map_err(|e| e.to_string()));
            });
        if let Err(e) = spawned {
            tracing::warn!("reading audio for playback: {e}");
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(result) = rx.recv().await else { return };
            let Some(p) = weak.upgrade() else { return };
            if p.load_generation.get() != generation {
                return;
            }
            match result {
                Ok((samples, shape)) => {
                    let player = Player::new(samples);
                    player.set_speed(SPEEDS[p.speed.get()].0);
                    *p.player.borrow_mut() = Some(Rc::new(player));
                    p.bar.wave.borrow_mut().peaks = shape;
                    p.bar.play.set_sensitive(true);
                    p.bar.play.set_tooltip_text(Some("Play"));
                }
                Err(e) => {
                    tracing::warn!("reading audio for playback: {e}");
                    p.bar.play.set_tooltip_text(Some("This file cannot be played"));
                }
            }
            p.tick();
            p.update_wave();
        });
    }

    fn toggle_play(&self) {
        let Some(player) = self.player.borrow().clone() else {
            return;
        };
        if player.is_playing() {
            player.pause();
        } else {
            player.play();
        }
        self.tick();
    }

    fn seek_fraction(&self, fraction: f64) {
        let Some(player) = self.player.borrow().clone() else {
            return;
        };
        player.seek_ms((fraction.clamp(0.0, 1.0) * player.duration_ms() as f64) as i64);
        self.tick();
    }

    fn play_from(&self, ms: i64) {
        let Some(player) = self.player.borrow().clone() else {
            return;
        };
        player.seek_ms(ms);
        player.play();
        self.tick();
    }

    fn set_play_icon(&self, playing: bool) {
        let (icon, name) = if playing {
            ("fennec-pause-symbolic", "Pause")
        } else {
            ("fennec-play-symbolic", "Play")
        };
        if self.bar.play_icon.icon_name().as_deref() != Some(icon) {
            self.bar.play_icon.set_icon_name(Some(icon));
            self.bar
                .play
                .update_property(&[gtk::accessible::Property::Label(name)]);
        }
    }

    /// Follows playback: time, play/pause icon, played part and current row.
    fn tick(&self) {
        let player = self.player.borrow().clone();
        let (pos, total, playing) = match &player {
            Some(p) => (p.position_ms(), p.duration_ms(), p.is_playing()),
            None => (0, self.selected_item_duration().unwrap_or(0), false),
        };
        let text = format!("{} / {}", duration(pos), duration(total));
        if self.bar.time.text() != text {
            self.bar.time.set_text(&text);
        }
        self.set_play_icon(playing);
        let played = if total > 0 { pos as f64 / total as f64 } else { 0.0 };
        if (self.bar.wave.borrow().played - played).abs() > f64::EPSILON {
            self.bar.wave.borrow_mut().played = played;
            self.bar.area.queue_draw();
        }
        let current = (player.is_some() && (playing || pos > 0))
            .then(|| {
                self.rows
                    .borrow()
                    .iter()
                    .rposition(|(start, _)| start.is_some_and(|s| s <= pos))
            })
            .flatten();
        if current != self.current_row.get() {
            let rows = self.rows.borrow();
            if let Some((_, row)) = self.current_row.get().and_then(|i| rows.get(i)) {
                row.remove_css_class("current");
            }
            if let Some((_, row)) = current.and_then(|i| rows.get(i)) {
                row.add_css_class("current");
            }
            self.current_row.set(current);
        }
    }

    /// Recomputes how much of the timeline is transcribed.
    fn update_wave(&self) {
        let status = self.selected_status();
        let transcribed = match status.as_ref().and_then(ItemStatus::fraction) {
            Some(f) => f,
            None => {
                let total = self
                    .player
                    .borrow()
                    .as_ref()
                    .map(|p| p.duration_ms())
                    .or_else(|| self.selected_item_duration())
                    .unwrap_or(0);
                let end = self
                    .rows
                    .borrow()
                    .last()
                    .and_then(|(start, _)| *start)
                    .unwrap_or(0);
                if total > 0 { end as f64 / total as f64 } else { 0.0 }
            }
        };
        self.bar.wave.borrow_mut().transcribed = transcribed;
        self.bar.area.queue_draw();
    }

    fn render_transcript(self: &Rc<Self>) {
        while let Some(c) = self.transcript.first_child() {
            self.transcript.remove(&c);
        }
        self.rows.borrow_mut().clear();
        self.current_row.set(None);
        let Some(doc) = self.selected_doc() else {
            self.empty
                .set_text("Add audio or video files to transcribe them.");
            self.empty.set_visible(true);
            return;
        };
        let status = self.selected_status();
        let pending = matches!(status, Some(ItemStatus::Running { .. } | ItemStatus::Waiting));
        let paragraphs = self.store.paragraphs(doc).unwrap_or_default();
        self.empty.set_visible(paragraphs.is_empty() && !pending);
        if paragraphs.is_empty() {
            self.empty.set_text(match status {
                Some(ItemStatus::Done) => "No speech was found in this file.",
                _ => "The transcript appears here as it is made.",
            });
        }
        let stamps = self.show_timestamps.is_active();
        let mut last_end = 0;
        for p in paragraphs {
            last_end = p.end_ms.or(p.start_ms).unwrap_or(last_end);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 20);
            row.add_css_class("fx-transcript-row");
            if stamps && let Some(ms) = p.start_ms {
                let ts = gtk::Button::with_label(&clock(ms));
                ts.add_css_class("fx-timestamp");
                ts.set_valign(gtk::Align::Start);
                ts.set_tooltip_text(Some("Play from here"));
                if let Some(l) = ts.child().and_downcast::<gtk::Label>() {
                    l.set_xalign(0.0);
                }
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
            self.rows.borrow_mut().push((p.start_ms, row));
        }
        if pending {
            self.append_pending(stamps, last_end);
        }
        self.update_wave();
        self.tick();
    }

    /// Placeholder lines for audio not yet transcribed, and a note.
    fn append_pending(&self, stamps: bool, from_ms: i64) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        row.add_css_class("fx-transcript-row");
        row.add_css_class("pending");
        let stamp_text = if stamps { clock(from_ms) } else { String::new() };
        let stamp = label(&stamp_text, &["fx-mono", "fx-stats", "fx-gutter"]);
        stamp.set_valign(gtk::Align::Center);
        row.append(&stamp);
        let lines = gtk::Box::new(gtk::Orientation::Vertical, 8);
        lines.set_hexpand(true);
        lines.set_valign(gtk::Align::Center);
        for rest in [12, 36] {
            // The bars fill 88% and 64% of the text column.
            let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            bar.add_css_class("fx-skeleton");
            let filled = 100 - rest;
            let line = gtk::Grid::builder()
                .column_homogeneous(true)
                .hexpand(true)
                .build();
            line.attach(&bar, 0, 0, filled, 1);
            line.attach(
                &gtk::Box::new(gtk::Orientation::Horizontal, 0),
                filled,
                0,
                rest,
                1,
            );
            lines.append(&line);
        }
        row.append(&lines);
        self.transcript.append(&row);
        let note = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        note.add_css_class("fx-pending-note");
        note.append(&label("", &["fx-gutter"]));
        note.append(&label(
            "The rest is still being transcribed. You can edit the text meanwhile.",
            &["fx-status"],
        ));
        self.transcript.append(&note);
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
        self.rows
            .borrow()
            .iter()
            .filter_map(|(_, r)| super::texts_in(r).into_iter().find(|t| !t.starts_with("00:")))
            .collect()
    }

    pub fn total_duration_label(&self) -> String {
        let total: i64 = self.items.borrow().iter().filter_map(|i| i.duration_ms).sum();
        duration(total)
    }

    /// The player's position and length in ms, once the audio is loaded (tests).
    pub fn playback(&self) -> Option<(i64, i64)> {
        self.player
            .borrow()
            .as_ref()
            .map(|p| (p.position_ms(), p.duration_ms()))
    }

    /// Seeks as a click at `fraction` of the timeline would (tests).
    pub fn seek_timeline(&self, fraction: f64) {
        self.seek_fraction(fraction);
    }

    /// Presses the speed button; returns its new label (tests).
    pub fn cycle_speed(&self) -> String {
        self.bar.speed.emit_clicked();
        self.bar.speed.label().map(|l| l.to_string()).unwrap_or_default()
    }
}

/// The speech model's catalog name, for queue cards.
fn model_name(settings: &crate::config::Settings) -> String {
    crate::models::catalog()
        .into_iter()
        .find(|m| m.file_name == settings.model)
        .map(|m| m.name.to_string())
        .unwrap_or_else(|| settings.model.trim_end_matches(".bin").to_string())
}
