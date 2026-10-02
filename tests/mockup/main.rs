//! The mockup's example content, staged in the real window and captured in
//! the same states as the mockup's own screenshots, for a side-by-side check
//! (scripts/mockup/README.md). Each screen's states live in their own module.
//!
//! Runs only when FENNEC_MOCKUP_DIR names an output folder; needs a Wayland
//! display of 1280×800 (the mockup's size) and `grim`.

mod cleanup;
mod dictate;
mod export;
mod files;
mod project;
mod settings;
#[path = "../support/mod.rs"]
mod support;
mod templates;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fennec::ai::{Locality, MemorySecrets, Protocol, ProviderConfig};
use fennec::audio::capture::{AudioSource, PcmSource};
use fennec::config::{Paths, Settings};
use fennec::engine::{EngineError, Segment, TranscribeOptions, Transcriber};
use fennec::store::{DocumentId, NewActionItem, NewDocument, NewSummary, Paragraph, ProjectId, Store};
use fennec::ui::{self, Deps};
use fennec::utterance::{EnergyVad, FrameVad};
use fennec::vad::SpeechDetector;
use gtk::glib;
use gtk::prelude::*;
use support::{MockLlm, Recorded, Reply, Wire};

/// The mockup's "today": 2 October 2026, 10:52.
pub const TODAY: &str = "2026-10-02";

pub struct Scene {
    pub out: PathBuf,
    pub store: Store,
    pub w: Rc<ui::MainWindow>,
    pub besigtigelse: DocumentId,
    pub harbour: ProjectId,
    pub interview: DocumentId,
    _llm: MockLlm,
}

impl Scene {
    /// Lets GTK lay out and draw, then captures the whole display (popovers
    /// included) as `<name>.png`.
    pub fn shot(&self, name: &str) {
        pump(Duration::from_millis(600));
        let path = self.out.join(format!("{name}.png"));
        let ok = std::process::Command::new("grim")
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        println!("{} {name}", if ok { "shot" } else { "FAILED" });
    }
}

pub fn pump(d: Duration) {
    pump_until(d, || false);
}

pub fn pump_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    let ctx = glib::MainContext::default();
    while !done() {
        if start.elapsed() > timeout {
            return false;
        }
        while ctx.iteration(false) {}
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

/// Transcribes the Files scene: the interview (the loudest file) gets the
/// mockup's sentences and then stalls mid-way, so it stays "Transcribing";
/// other files get one short line.
struct Scripted;
impl Transcriber for Scripted {
    fn transcribe(&mut self, pcm: &[f32], _: &TranscribeOptions) -> Result<Vec<Segment>, EngineError> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static INTERVIEW: AtomicUsize = AtomicUsize::new(0);
        let peak = pcm.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let text = if peak > files::INTERVIEW_LEVEL - 0.01 {
            let n = INTERVIEW.fetch_add(1, Ordering::SeqCst);
            match files::INTERVIEW_LINES.get(n) {
                Some(line) => *line,
                None => loop {
                    std::thread::sleep(Duration::from_secs(3600));
                },
            }
        } else {
            "Kort diktat fra bilen om dagens aftaler."
        };
        Ok(vec![Segment {
            start_ms: 0,
            end_ms: 900,
            text: text.into(),
            low_confidence: vec![],
        }])
    }
}

fn ms(h: i64, m: i64, s: i64) -> i64 {
    ((h * 60 + m) * 60 + s) * 1000
}

/// A paragraph with a timestamp, as transcription leaves them.
fn timed(text: &str, start: i64, end: i64) -> Paragraph {
    Paragraph {
        start_ms: Some(start),
        end_ms: Some(end),
        ..Paragraph::new(text)
    }
}

/// Byte range of `word` in `text`.
fn range_of(text: &str, word: &str) -> std::ops::Range<usize> {
    let i = text.find(word).expect("word in text");
    i..i + word.len()
}

/// Milliseconds since the epoch for a September/October 2026 day at 10:00.
fn day(month: u32, d: u32) -> i64 {
    use chrono::TimeZone;
    chrono::Local
        .with_ymd_and_hms(2026, month, d, 10, 0, 0)
        .single()
        .expect("a valid local time")
        .timestamp_millis()
}

struct Doc<'a> {
    title: &'a str,
    project: Option<ProjectId>,
    template: &'a str,
    file: bool,
    tags: &'a [&'a str],
    created: i64,
    paragraphs: Vec<Paragraph>,
}

fn add(store: &Store, db: &rusqlite::Connection, d: Doc) -> DocumentId {
    let new = if d.file {
        NewDocument::file(d.title)
    } else {
        NewDocument::dictation(d.title)
    };
    let id = store
        .create_document(&NewDocument {
            project_id: d.project,
            template_id: Some(d.template.into()),
            ..new
        })
        .unwrap();
    store.replace_paragraphs(id, &d.paragraphs).unwrap();
    let tags: Vec<String> = d.tags.iter().map(|t| t.to_string()).collect();
    store.set_tags(id, &tags).unwrap();
    db.execute(
        "UPDATE documents SET created_at = ?2, updated_at = ?2 WHERE id = ?1",
        rusqlite::params![id, d.created],
    )
    .unwrap();
    id
}

/// One paragraph spanning `length`, so lists show the recording's length.
fn spanning(text: &str, length: i64) -> Vec<Paragraph> {
    vec![timed(text, 0, length)]
}

pub const P1: &str = "Besigtigelsen blev foretaget tirsdag formiddag sammen med ejendommens vicevært. \
                      Der var adgang til kælderen, trappeopgangen og to lejligheder på tredje sal.";
pub const P2: &str = "I kælderen er der konstateret fugt langs den nordlige ydermur. Fugten ser ud til at \
                      komme fra en utæt nedløbsbrønd, og der er tydelige saltudtræk på murværket.";
pub const P3: &str =
    "Trappeopgangen fremstår velholdt. Der er dog revner i pudsen over hoveddøren, som bør følges.";
pub const PREVIEW: &str =
    "Viceværten oplyste, at problemet med fugt har været kendt siden sidste vinter, men at";

pub const SUMMARY: &str = "Besigtigelse af Nørregade 14 sammen med ejendommens vicevært. I kælderen er der \
fugt langs den nordlige ydermur, sandsynligvis fra en utæt nedløbsbrønd, med saltudtræk på murværket. \
Trappeopgangen er velholdt, men der er revner i pudsen over hoveddøren. Fugtproblemet har ifølge \
viceværten været kendt siden sidste vinter.\n\n**Opfølgning**\n- Undersøg nedløbsbrønden for utætheder.\n\
- Følg udviklingen i revnerne over hoveddøren.";

/// Seeds the database the way the mockup's sidebar and lists show it.
fn seed(root: &Path) -> (DocumentId, ProjectId, DocumentId) {
    std::fs::create_dir_all(root.join("data/models")).unwrap();
    std::fs::write(root.join("data/models/edda-v0.1-q5_0.bin"), b"stand-in").unwrap();
    let path = root.join("data/fennec.db");
    let store = Store::open(&path).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    let norregade = store.create_project("Nørregade 14", "#C2410C").unwrap();
    let harbour = store.create_project("Operation Harbour", "#1D4ED8").unwrap();
    let acme = store.create_project("Vendor – Acme", "#0F766E").unwrap();
    store.set_project_local_only(harbour, true).unwrap();
    store
        .update_project(harbour, "Operation Harbour", "#1D4ED8", Some("moedereferat"))
        .unwrap();

    // Unsorted (2) and Vendor – Acme (4), oldest first so the mockup's
    // documents are the newest.
    for (i, t) in ["Noter fra telefonmøde", "Idéer til kvartalsrapport"]
        .iter()
        .enumerate()
    {
        add(
            &store,
            &db,
            Doc {
                title: t,
                project: None,
                template: "notat",
                file: false,
                tags: &[],
                created: day(8, 20 + i as u32),
                paragraphs: spanning("Kort note.", ms(0, 2, 10)),
            },
        );
    }
    for (i, t) in [
        "Kontraktgennemgang",
        "Leveringsplan",
        "Prisforhandling",
        "Kvalitetsaftale",
    ]
    .iter()
    .enumerate()
    {
        add(
            &store,
            &db,
            Doc {
                title: t,
                project: Some(acme),
                template: "moedereferat",
                file: i % 2 == 0,
                tags: &[],
                created: day(9, 1 + i as u32),
                paragraphs: spanning("Gennemgang af aftalen.", ms(0, 20, 0)),
            },
        );
    }

    // Operation Harbour (5): the Project screen's table.
    let kickoff = add(
        &store,
        &db,
        Doc {
            title: "Kickoff",
            project: Some(harbour),
            template: "moedereferat",
            file: true,
            tags: &["meeting"],
            created: day(9, 12),
            paragraphs: vec![
                timed(
                    "Velkommen til projektet og en gennemgang af planen.",
                    0,
                    ms(0, 41, 0),
                ),
                timed(
                    "Vi udsender referatet fra kickoff i morgen.",
                    ms(0, 41, 20),
                    ms(0, 42, 10),
                ),
            ],
        },
    );
    let ressource = add(
        &store,
        &db,
        Doc {
            title: "Ressourceplan, første udkast",
            project: Some(harbour),
            template: "notat",
            file: false,
            tags: &["planning"],
            created: day(9, 18),
            paragraphs: spanning("Udkast til fordeling af folk på opgaverne.", ms(0, 5, 2)),
        },
    );
    let acme_followup = add(
        &store,
        &db,
        Doc {
            title: "Opfølgning med Acme",
            project: Some(harbour),
            template: "moedereferat",
            file: false,
            tags: &["meeting", "vendor"],
            created: day(9, 24),
            paragraphs: vec![
                timed(
                    "Acme lovede at levere den første del af udstyret inden udgangen af oktober.",
                    ms(0, 2, 14),
                    ms(0, 2, 30),
                ),
                timed(
                    "De tog forbehold for, at reservedele kunne blive forsinket.",
                    ms(0, 5, 40),
                    ms(0, 8, 31),
                ),
            ],
        },
    );
    let planning = add(
        &store,
        &db,
        Doc {
            title: "Planlægningsmøde, uge 40",
            project: Some(harbour),
            template: "moedereferat",
            file: true,
            tags: &["meeting", "planning"],
            created: day(9, 30),
            paragraphs: vec![
                timed("Gennemgang af tidsplanen for efteråret.", 0, ms(0, 38, 0)),
                timed(
                    "Det blev aftalt at følge op, hvis der ikke var en bekræftet dato inden den 15. oktober.",
                    ms(0, 38, 2),
                    ms(1, 12, 40),
                ),
            ],
        },
    );
    let interview = add(
        &store,
        &db,
        Doc {
            title: "Interview, afdelingsleder",
            project: Some(harbour),
            template: "notat",
            file: true,
            tags: &["interview"],
            created: day(10, 2),
            paragraphs: vec![
                timed(
                    "Vi startede med at kortlægge, hvordan sagerne faktisk flyttede sig mellem afdelingerne.",
                    ms(0, 11, 48),
                    ms(0, 12, 5),
                ),
                timed(
                    "Det viste sig, at meget af tiden gik med at vente på svar fra andre.",
                    ms(0, 12, 6),
                    ms(0, 12, 30),
                ),
                timed(
                    "Så vi besluttede at samle det hele i ét system i stedet for at sende mails frem og tilbage.",
                    ms(0, 12, 31),
                    ms(0, 12, 51),
                ),
                timed(
                    "Den første måned var svær. Folk var vant til deres egne regneark, og det tog tid at vænne sig til den nye måde at arbejde på.",
                    ms(0, 12, 52),
                    ms(0, 13, 19),
                ),
                timed(
                    "Men efter et par uger kunne vi begynde at se, hvor sagerne hobede sig op. Og det var egentlig dér, det gav mening for de fleste.",
                    ms(0, 13, 20),
                    ms(0, 48, 12),
                ),
            ],
        },
    );
    let para = |doc, n: usize| store.paragraphs(doc).unwrap()[n].id;
    let item = |what: &str, who: Option<&str>, due: Option<&str>, p| NewActionItem {
        what: what.into(),
        who: who.map(Into::into),
        due: due.map(Into::into),
        paragraph_id: p,
        provider: Some("Claude".into()),
    };
    store
        .replace_action_items(
            planning,
            &[item(
                "Få bekræftet leveringsdato fra Acme",
                Some("Projektleder"),
                Some("2026-10-15"),
                para(planning, 1),
            )],
        )
        .unwrap();
    store
        .replace_action_items(
            ressource,
            &[item(
                "Send opdateret ressourceplan til afdelingerne",
                Some("Afdelingsleder"),
                Some("2026-10-04"),
                para(ressource, 0),
            )],
        )
        .unwrap();
    store
        .replace_action_items(
            interview,
            &[item(
                "Book opfølgningsmøde efter første måned",
                None,
                None,
                para(interview, 2),
            )],
        )
        .unwrap();
    store
        .replace_action_items(
            kickoff,
            &[item(
                "Udsend referat fra kickoff",
                None,
                Some("2026-09-13"),
                para(kickoff, 1),
            )],
        )
        .unwrap();
    let done = store.document_action_items(kickoff).unwrap()[0].id;
    store.set_action_done(done, true).unwrap();
    let _ = acme_followup;

    // Nørregade 14 (3): the dictation being recorded is the newest.
    for (i, t) in ["Opmåling af kælder", "Tilbud på fugtsikring"].iter().enumerate() {
        add(
            &store,
            &db,
            Doc {
                title: t,
                project: Some(norregade),
                template: "notat",
                file: false,
                tags: &["inspection"],
                created: day(9, 26 + i as u32),
                paragraphs: spanning("Noter fra stedet.", ms(0, 3, 0)),
            },
        );
    }
    let mut p2 = timed(P2, ms(0, 1, 40), ms(0, 3, 10));
    p2.low_confidence = vec![range_of(P2, "nedløbsbrønd"), range_of(P2, "saltudtræk")];
    let besigtigelse = add(
        &store,
        &db,
        Doc {
            title: "Besigtigelse Nørregade 14",
            project: Some(norregade),
            template: "notat",
            file: false,
            tags: &["inspection", "moisture"],
            created: day(10, 2),
            paragraphs: vec![
                timed(P1, 0, ms(0, 1, 38)),
                p2,
                timed(P3, ms(0, 3, 12), ms(0, 4, 12)),
            ],
        },
    );
    let mut fields = BTreeMap::new();
    fields.insert("sagsnr".to_string(), "2026-0412".to_string());
    fields.insert("dato".to_string(), "2. oktober 2026".to_string());
    store
        .update_document(besigtigelse, "Besigtigelse Nørregade 14", Some("notat"), &fields)
        .unwrap();
    store
        .add_summary(&NewSummary {
            document_id: Some(besigtigelse),
            project_id: None,
            prompt_id: "summary".into(),
            provider: "Claude".into(),
            model: "claude-opus-5-5".into(),
            text: SUMMARY.into(),
        })
        .unwrap();
    store
        .replace_action_items(
            besigtigelse,
            &[
                item(
                    "Få nedløbsbrønden undersøgt for utætheder",
                    None,
                    None,
                    para(besigtigelse, 1),
                ),
                item(
                    "Følg op på revner i pudsen over hoveddøren",
                    None,
                    None,
                    para(besigtigelse, 2),
                ),
                item(
                    "Spørg, hvad der er gjort ved fugten siden sidste vinter",
                    Some("Vicevært"),
                    None,
                    para(besigtigelse, 2),
                ),
            ],
        )
        .unwrap();
    (besigtigelse, harbour, interview)
}

/// Answers the project question the way the mockup shows it.
fn model(rec: &Recorded) -> Reply {
    let prompt = rec.prompt_text();
    if prompt.contains("Besvar spørgsmålet") {
        let marker = |text: &str| {
            prompt
                .lines()
                .find(|l| l.contains(text))
                .and_then(|l| l.split_once(']').map(|(m, _)| format!("{m}]")))
                .unwrap_or_default()
        };
        return Reply::Text(format!(
            "Acme lovede at levere den første del af udstyret inden udgangen af oktober {}, men tog \
             forbehold for, at reservedele kunne blive forsinket {}. På planlægningsmødet blev det \
             aftalt at følge op, hvis der ikke var en bekræftet dato inden den 15. oktober {}.",
            marker("inden udgangen af oktober"),
            marker("reservedele"),
            marker("15. oktober"),
        ));
    }
    if prompt.contains("Ret hvert afsnit") {
        // Small grammar edits, as the mockup's review shows them.
        let paragraphs: Vec<serde_json::Value> = prompt
            .lines()
            .filter_map(|l| {
                let (id, text) = l.strip_prefix("[p")?.split_once("] ")?;
                let text = text
                    .replace("Fugten ser ud til at komme", "Fugten kommer sandsynligvis")
                    .replace("fremstår velholdt", "er velholdt");
                Some(serde_json::json!({"id": id.parse::<i64>().ok()?, "text": text}))
            })
            .collect();
        return Reply::Text(serde_json::json!({ "paragraphs": paragraphs }).to_string());
    }
    Reply::Text("OK".into())
}

fn deps(root: &Path, llm: &MockLlm) -> Deps {
    let provider = |id: &str, name: &str, protocol, url: &str, model: &str, locality| ProviderConfig {
        id: id.into(),
        name: name.into(),
        protocol,
        base_url: url.into(),
        model: model.into(),
        locality,
        context_chars: 60_000,
    };
    let mut settings = Settings {
        vocabulary: "Nørregade, nedløbsbrønd, saltudtræk, BBR, tv-inspektion".into(),
        ..Default::default()
    };
    settings.ai.enabled = true;
    settings.ai.providers = vec![
        provider(
            "claude",
            "Claude",
            Protocol::Anthropic,
            "https://api.anthropic.com",
            "claude-opus-5-5",
            Locality::Cloud,
        ),
        provider(
            "chatgpt",
            "ChatGPT",
            Protocol::OpenAi,
            "https://api.openai.com/v1",
            "",
            Locality::Cloud,
        ),
        // Answers the Ask view; the mockup's address is 192.168.1.40:8000.
        provider(
            "workstation",
            "Workstation",
            Protocol::OpenAi,
            &llm.url,
            "local model",
            Locality::Network,
        ),
        provider(
            "ollama",
            "Ollama",
            Protocol::OpenAi,
            "http://localhost:11434/v1",
            "",
            Locality::ThisComputer,
        ),
    ];
    settings.ai.default_provider = "claude".into();
    for (job, p) in [
        ("summaries", "claude"),
        ("cleanup", "workstation"),
        ("action-items", "claude"),
        ("ask", "claude"),
        ("fill-fields", "workstation"),
    ] {
        settings.ai.jobs.insert(job.into(), p.into());
    }
    Deps {
        paths: Paths::under(root),
        settings: Rc::new(RefCell::new(settings)),
        engine: Arc::new(|_, _| Ok(Box::new(Scripted) as Box<dyn Transcriber>)),
        audio: Arc::new(|_| Ok(Box::new(PcmSource::new(vec![0.0; 16_000])) as Box<dyn AudioSource>)),
        vad: Arc::new(|_, _| Ok(Box::new(EnergyVad::default()) as Box<dyn FrameVad>)),
        file_vad: Arc::new(|_, _| Box::new(files::EverySecond) as Box<dyn SpeechDetector>),
        secrets: Arc::new(MemorySecrets::with("claude", "sk-ant-mockup")),
        confirm_cloud: Rc::new(|_, _, answer| answer(true)),
        dictation_live: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    }
}

fn main() {
    let Some(out) = std::env::var_os("FENNEC_MOCKUP_DIR").map(PathBuf::from) else {
        println!("FENNEC_MOCKUP_DIR is not set; nothing to capture");
        return;
    };
    std::fs::create_dir_all(&out).unwrap();
    let only = std::env::var("FENNEC_MOCKUP_ONLY").ok();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let (besigtigelse, harbour, interview) = seed(&root);
    let llm = MockLlm::start(Wire::OpenAi, model);
    adw::init().expect("a display is available");
    ui::load_css();
    let w = ui::build_window(deps(&root, &llm));
    w.window.present();
    pump(Duration::from_millis(800));
    let scene = Scene {
        out,
        store: Store::open(&root.join("data/fennec.db")).unwrap(),
        w,
        besigtigelse,
        harbour,
        interview,
        _llm: llm,
    };
    let run = |name: &str| only.as_deref().is_none_or(|o| o == name);
    if run("dictate") {
        dictate::capture(&scene);
    }
    if run("cleanup") {
        cleanup::capture(&scene);
    }
    if run("export") {
        export::capture(&scene);
    }
    if run("project") {
        project::capture(&scene);
    }
    if run("templates") {
        templates::capture(&scene);
    }
    if run("settings") {
        settings::capture(&scene);
    }
    // Last: it adds three documents, which would change other screens' counts.
    if run("files") {
        files::capture(&scene, &root);
    }
}
