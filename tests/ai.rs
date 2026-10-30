//! AI actions end to end: a real store, the real service and provider
//! clients, and local mock servers speaking each protocol.

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fennec::ai::actions::Proofreading;
use fennec::ai::service::Scope;
use fennec::ai::{
    AiError, AiJob, AiService, AiSettings, Cancel, Locality, MemorySecrets, Protocol, ProviderConfig,
    connect, privacy::PrivacyError,
};
use fennec::store::{NewDocument, Paragraph, ProjectFilter, Store};
use fennec::template::Template;
use serde_json::json;
use support::{MockLlm, Recorded, Reply, Wire};

fn provider(id: &str, protocol: Protocol, url: &str, locality: Locality) -> ProviderConfig {
    ProviderConfig {
        id: id.into(),
        name: id.into(),
        protocol,
        base_url: url.into(),
        model: "test-model".into(),
        locality,
        context_chars: 60_000,
    }
}

fn service(p: ProviderConfig, key: Option<&str>) -> AiService {
    let secrets = match key {
        Some(k) => MemorySecrets::with(&p.id, k),
        None => MemorySecrets::default(),
    };
    AiService::new(
        AiSettings {
            enabled: true,
            default_provider: p.id.clone(),
            providers: vec![p],
            ..Default::default()
        },
        Arc::new(secrets),
    )
}

fn store_with_doc(texts: &[&str]) -> (Store, i64) {
    let store = Store::open_in_memory().unwrap();
    let doc = store
        .create_document(&NewDocument::dictation("Møde med leverandør"))
        .unwrap();
    let paragraphs: Vec<Paragraph> = texts.iter().map(|t| Paragraph::new(t)).collect();
    store.replace_paragraphs(doc, &paragraphs).unwrap();
    (store, doc)
}

fn paragraph_ids(store: &Store, doc: i64) -> Vec<i64> {
    store
        .paragraphs(doc)
        .unwrap()
        .iter()
        .map(|p| p.id.unwrap())
        .collect()
}

/// `[p12] text` lines in the user message.
fn tagged(rec: &Recorded) -> Vec<(i64, String)> {
    rec.prompt_text()
        .lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("[p")?;
            let (id, text) = rest.split_once("] ")?;
            Some((id.parse().ok()?, text.to_string()))
        })
        .collect()
}

#[test]
fn claude_summary_streams_with_the_right_headers_and_is_stored() {
    let mock = MockLlm::start(Wire::Anthropic, |_| Reply::Text("Kort resumé af mødet.".into()));
    let ai = service(
        provider("claude", Protocol::Anthropic, &mock.url, Locality::Cloud),
        Some("sk-test"),
    );
    let (store, doc) = store_with_doc(&["Vi aftalte en pris på 40.000 kr.", "Levering i uge 44."]);
    ai.consent(&store, &Scope::Document(doc)).unwrap();

    let mut streamed = String::new();
    let s = ai
        .summarize(&store, &Scope::Document(doc), &Cancel::default(), &mut |d| {
            streamed.push_str(d)
        })
        .unwrap();

    assert_eq!(s.text, "Kort resumé af mødet.");
    assert_eq!(streamed, s.text);
    let saved = store.summaries_for_document(doc).unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(
        (saved[0].provider.as_str(), saved[0].model.as_str()),
        ("claude", "test-model")
    );
    let req = &mock.requests()[0];
    assert_eq!(req.path, "/v1/messages");
    assert_eq!(req.header("x-api-key"), Some("sk-test"));
    assert_eq!(req.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(req.body["stream"], true);
    assert_eq!(req.body["model"], "test-model");
    assert!(req.prompt_text().contains("40.000 kr"));
}

#[test]
fn with_ai_off_no_action_makes_a_request() {
    let mock = MockLlm::start(Wire::OpenAi, |_| Reply::Text("x".into()));
    let mut settings = AiSettings {
        enabled: false,
        default_provider: "ollama".into(),
        ..Default::default()
    };
    settings.providers.push(provider(
        "ollama",
        Protocol::OpenAi,
        &mock.url,
        Locality::ThisComputer,
    ));
    let ai = AiService::new(settings, Arc::new(MemorySecrets::default()));
    let (store, doc) = store_with_doc(&["Hej."]);
    let c = Cancel::default();
    let scope = Scope::Document(doc);

    assert_eq!(
        ai.summarize(&store, &scope, &c, &mut |_| {}).unwrap_err(),
        AiError::Disabled
    );
    assert_eq!(ai.cleanup(&store, doc, &c).unwrap_err(), AiError::Disabled);
    assert_eq!(ai.action_items(&store, doc, &c).unwrap_err(), AiError::Disabled);
    assert_eq!(
        ai.ask(&store, &scope, "Hvad?", &c, &mut |_| {}).unwrap_err(),
        AiError::Disabled
    );
    assert_eq!(
        ai.suggest_fields(&store, doc, &Template::blank(), &c)
            .unwrap_err(),
        AiError::Disabled
    );
    assert!(!ai.enabled());
    assert_eq!(mock.count(), 0);
}

#[test]
fn a_cloud_send_needs_consent_and_nothing_is_sent_before_it() {
    let mock = MockLlm::start(Wire::OpenAi, |_| Reply::Text("ok".into()));
    let ai = service(
        provider("chatgpt", Protocol::OpenAi, &mock.url, Locality::Cloud),
        Some("k"),
    );
    let (store, doc) = store_with_doc(&["Hej."]);
    let scope = Scope::Document(doc);

    let err = ai
        .summarize(&store, &scope, &Cancel::default(), &mut |_| {})
        .unwrap_err();
    assert!(
        matches!(err, AiError::Privacy(PrivacyError::NeedsConsent { .. })),
        "{err:?}"
    );
    assert_eq!(mock.count(), 0);

    ai.consent(&store, &scope).unwrap();
    ai.summarize(&store, &scope, &Cancel::default(), &mut |_| {})
        .unwrap();
    assert_eq!(mock.count(), 1);
    assert_eq!(mock.requests()[0].header("authorization"), Some("Bearer k"));
}

#[test]
fn local_only_projects_reject_cloud_providers_even_after_consent() {
    let mock = MockLlm::start(Wire::Anthropic, |_| Reply::Text("ok".into()));
    let ai = service(
        provider("claude", Protocol::Anthropic, &mock.url, Locality::Cloud),
        Some("k"),
    );
    let (store, doc) = store_with_doc(&["Fortroligt."]);
    let project = store.create_project("Drift", "#888").unwrap();
    store.move_document(doc, Some(project)).unwrap();
    store.set_project_local_only(project, true).unwrap();
    ai.consent(&store, &Scope::Document(doc)).unwrap();
    ai.consent(&store, &Scope::Project(ProjectFilter::Project(project)))
        .unwrap();

    for scope in [
        Scope::Document(doc),
        Scope::Project(ProjectFilter::Project(project)),
    ] {
        let err = ai
            .summarize(&store, &scope, &Cancel::default(), &mut |_| {})
            .unwrap_err();
        assert!(
            matches!(err, AiError::Privacy(PrivacyError::LocalOnly { .. })),
            "{err:?}"
        );
    }
    // A tag spanning the local-only document is just as closed.
    store.set_tags(doc, &["leverandør".into()]).unwrap();
    let tag = Scope::Tag("leverandør".into());
    ai.consent(&store, &tag).unwrap();
    assert!(
        ai.ask(&store, &tag, "Hvad?", &Cancel::default(), &mut |_| {})
            .is_err()
    );
    assert_eq!(mock.count(), 0);
}

#[test]
fn local_providers_skip_consent_and_send_no_key() {
    let mock = MockLlm::start(Wire::OpenAi, |_| Reply::Text("Resumé.".into()));
    let ai = service(
        provider("ollama", Protocol::OpenAi, &mock.url, Locality::ThisComputer),
        None,
    );
    let (store, doc) = store_with_doc(&["Hej."]);
    ai.summarize(&store, &Scope::Document(doc), &Cancel::default(), &mut |_| {})
        .unwrap();
    let req = &mock.requests()[0];
    assert_eq!(req.path, "/v1/chat/completions");
    assert_eq!(req.header("authorization"), None);
    assert_eq!(req.body["messages"][0]["role"], "system");
}

#[test]
fn cleanup_returns_only_changed_paragraphs_with_known_ids() {
    let mock = MockLlm::start(Wire::OpenAi, |rec| {
        let mut paragraphs: Vec<_> = tagged(rec)
            .into_iter()
            .map(|(id, text)| json!({"id": id, "text": text.replace("øh ", "")}))
            .collect();
        paragraphs.push(json!({"id": 999_999, "text": "opdigtet"}));
        Reply::Text(json!({ "paragraphs": paragraphs }).to_string())
    });
    let ai = service(
        provider("ollama", Protocol::OpenAi, &mock.url, Locality::ThisComputer),
        None,
    );
    let (store, doc) = store_with_doc(&["vi øh mødes på mandag", "Alt er fint."]);
    let ids = paragraph_ids(&store, doc);

    let cleaned = ai.cleanup(&store, doc, &Cancel::default()).unwrap();

    assert_eq!(cleaned.len(), 1);
    assert_eq!(cleaned[0].paragraph_id, ids[0]);
    assert_eq!(cleaned[0].cleaned, "vi mødes på mandag");
    assert_eq!(cleaned[0].original, "vi øh mødes på mandag");
    // Nothing is written until the user accepts.
    assert_eq!(store.paragraphs(doc).unwrap()[0].text, "vi øh mødes på mandag");
    let req = &mock.requests()[0];
    assert_eq!(req.body["response_format"]["type"], "json_schema");
}

#[test]
fn long_texts_are_proofread_in_parts_at_once_and_reported_as_they_return() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
    static MOST: AtomicUsize = AtomicUsize::new(0);
    let mock = MockLlm::start(Wire::OpenAi, |rec| {
        let now = IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
        MOST.fetch_max(now, Ordering::SeqCst);
        let paragraphs = tagged(rec);
        // The first part is the slowest, so parts finish out of order.
        let first = paragraphs.iter().any(|(_, t)| t.starts_with("Afsnit 0 "));
        std::thread::sleep(std::time::Duration::from_millis(if first { 400 } else { 100 }));
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        let fixed: Vec<_> = paragraphs
            .into_iter()
            .filter(|(_, t)| t.contains("øh "))
            .map(|(id, t)| json!({"id": id, "text": t.replace("øh ", "")}))
            .collect();
        Reply::Text(json!({ "paragraphs": fixed }).to_string())
    });
    let ai = service(
        provider("ollama", Protocol::OpenAi, &mock.url, Locality::ThisComputer),
        None,
    );
    let texts: Vec<String> = (0..12)
        .map(|i| {
            format!(
                "Afsnit {i} øh handler om mødet og hvad der blev aftalt. {}",
                "Mere tekst her. ".repeat(18)
            )
        })
        .collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let (store, doc) = store_with_doc(&refs);
    let ids = paragraph_ids(&store, doc);

    let mut events = Vec::new();
    let cleaned = ai
        .cleanup_with_progress(&store, doc, &Cancel::default(), &mut |e| events.push(e))
        .unwrap();

    let parts = mock.count();
    assert!(parts > 1, "{parts} requests");
    assert!(MOST.load(Ordering::SeqCst) > 1, "parts run at once");
    assert_eq!(
        events.first(),
        Some(&Proofreading::Working {
            paragraphs: 12,
            parts
        })
    );
    let mut covered: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Proofreading::Part { covered, .. } => Some(covered.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(covered.len(), parts, "every part is reported");
    assert!(
        covered.first().is_some_and(|c| !c.contains(&ids[0])),
        "the slow first part is not reported first"
    );
    covered.sort();
    assert_eq!(covered.concat(), ids, "each paragraph is in exactly one part");
    // The result follows the text, whatever order the parts came back in.
    let got: Vec<_> = cleaned.iter().map(|c| c.paragraph_id).collect();
    assert_eq!(got, ids);
    assert!(cleaned.iter().all(|c| !c.cleaned.contains("øh")));
}

#[test]
fn a_server_without_schema_mode_gets_the_schema_in_the_prompt() {
    let mock = MockLlm::start(Wire::OpenAi, |rec| {
        if rec.body.get("response_format").is_some() {
            Reply::Status(400, "response_format is not supported".into())
        } else {
            Reply::Text("```json\n{\"paragraphs\": []}\n```".into())
        }
    });
    let ai = service(
        provider("lms", Protocol::OpenAi, &mock.url, Locality::Network),
        None,
    );
    let (store, doc) = store_with_doc(&["Hej."]);
    assert!(ai.cleanup(&store, doc, &Cancel::default()).unwrap().is_empty());
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 2);
    assert!(
        reqs[1].prompt_text().contains("\"paragraphs\""),
        "schema is in the prompt"
    );
}

#[test]
fn invalid_json_is_retried_once_then_reported() {
    let mock = MockLlm::start(Wire::Anthropic, |_| Reply::Text("Her er dine opgaver: …".into()));
    let ai = service(
        provider("claude", Protocol::Anthropic, &mock.url, Locality::Cloud),
        Some("k"),
    );
    let (store, doc) = store_with_doc(&["Hej."]);
    ai.consent(&store, &Scope::Document(doc)).unwrap();
    let err = ai.action_items(&store, doc, &Cancel::default()).unwrap_err();
    assert!(matches!(err, AiError::InvalidJson { .. }), "{err:?}");
    assert_eq!(mock.count(), 2);
    assert!(mock.requests()[0].body["output_config"]["format"]["schema"].is_object());
    assert!(store.document_action_items(doc).unwrap().is_empty());
}

#[test]
fn action_items_are_validated_and_stored() {
    let mock = MockLlm::start(Wire::Anthropic, |rec| {
        let first = tagged(rec)[0].0;
        Reply::Text(
            json!({"items": [
                {"what": "Send tilbud", "who": "Jens", "due": "2026-10-09", "paragraph": first},
                {"what": "Ring til Ane", "who": null, "due": "på fredag", "paragraph": 424242},
                {"what": "  ", "who": null, "due": null, "paragraph": null}
            ]})
            .to_string(),
        )
    });
    let ai = service(
        provider("claude", Protocol::Anthropic, &mock.url, Locality::Cloud),
        Some("k"),
    );
    let (store, doc) = store_with_doc(&["Jens sender tilbud på fredag.", "Nogen ringer til Ane."]);
    ai.consent(&store, &Scope::Document(doc)).unwrap();
    let ids = paragraph_ids(&store, doc);

    let items = ai.action_items(&store, doc, &Cancel::default()).unwrap();

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].what, "Send tilbud");
    assert_eq!(items[0].who.as_deref(), Some("Jens"));
    assert_eq!(items[0].due.as_deref(), Some("2026-10-09"));
    assert_eq!(items[0].paragraph_id, Some(ids[0]));
    assert_eq!(items[1].due, None, "not a date");
    assert_eq!(items[1].paragraph_id, None, "unknown paragraph");
    assert_eq!(store.document_action_items(doc).unwrap().len(), 2);
    assert!(mock.requests()[0].prompt_text().contains("regn relative datoer"));
}

#[test]
fn ask_the_project_keeps_only_real_citations() {
    let store = Store::open_in_memory().unwrap();
    let project = store.create_project("Leverandør A", "#888").unwrap();
    let mut first = None;
    for (title, text) in [
        ("Møde 1", "Prisen blev 40.000 kr."),
        ("Møde 2", "Leveringen flyttes til uge 46."),
    ] {
        let doc = store
            .create_document(&NewDocument {
                project_id: Some(project),
                ..NewDocument::dictation(title)
            })
            .unwrap();
        store.replace_paragraphs(doc, &[Paragraph::new(text)]).unwrap();
        first.get_or_insert((doc, paragraph_ids(&store, doc)[0]));
    }
    let (d, p) = first.unwrap();
    let answer = format!("Prisen er 40.000 kr. [d{d}:p{p}] og der er rabat [d77:p1].");
    let mock = MockLlm::start(Wire::OpenAi, move |_| Reply::Text(answer.clone()));
    let ai = service(
        provider("ollama", Protocol::OpenAi, &mock.url, Locality::ThisComputer),
        None,
    );

    let scope = Scope::Project(ProjectFilter::Project(project));
    let a = ai
        .ask(
            &store,
            &scope,
            "Hvad blev prisen?",
            &Cancel::default(),
            &mut |_| {},
        )
        .unwrap();

    assert_eq!(
        a.text,
        format!("Prisen er 40.000 kr. [d{d}:p{p}] og der er rabat.")
    );
    assert_eq!(a.citations.len(), 1);
    assert_eq!((a.citations[0].document_id, a.citations[0].paragraph_id), (d, p));
    let prompt = mock.requests()[0].prompt_text();
    assert!(
        prompt.contains(&format!("[d{d}:p{p}]")) && prompt.contains("uge 46"),
        "whole project fits"
    );
}

#[test]
fn ask_uses_search_hits_when_the_project_is_too_long() {
    let store = Store::open_in_memory().unwrap();
    let project = store.create_project("P", "#888").unwrap();
    let doc = store
        .create_document(&NewDocument {
            project_id: Some(project),
            ..NewDocument::dictation("Lang")
        })
        .unwrap();
    let mut paragraphs: Vec<Paragraph> = (0..200)
        .map(|i| {
            Paragraph::new(&format!(
                "Afsnit {i} handler om noget helt andet end det spurgte."
            ))
        })
        .collect();
    paragraphs.push(Paragraph::new("Kontrakten med leverandøren udløber i marts."));
    store.replace_paragraphs(doc, &paragraphs).unwrap();
    let mock = MockLlm::start(Wire::OpenAi, |_| Reply::Text("I marts.".into()));
    let mut p = provider("ollama", Protocol::OpenAi, &mock.url, Locality::ThisComputer);
    p.context_chars = 500;
    let ai = service(p, None);

    ai.ask(
        &store,
        &Scope::Project(ProjectFilter::Project(project)),
        "Hvornår udløber kontrakten?",
        &Cancel::default(),
        &mut |_| {},
    )
    .unwrap();

    let prompt = mock.requests()[0].prompt_text();
    assert!(prompt.contains("udløber i marts"), "{prompt}");
    assert!(!prompt.contains("Afsnit 150"));
}

#[test]
fn field_suggestions_fill_only_empty_fields_with_valid_values() {
    let mock = MockLlm::start(Wire::OpenAi, |rec| {
        let props = &rec.body["response_format"]["json_schema"]["schema"]["properties"];
        assert!(props.get("emne").is_none(), "filled fields are not asked for");
        Reply::Text(json!({"deltagere": "Jens og Ane", "dato": "2026-10-01", "type": "Fantasi"}).to_string())
    });
    let ai = service(
        provider("ollama", Protocol::OpenAi, &mock.url, Locality::ThisComputer),
        None,
    );
    let (store, doc) = store_with_doc(&["Jens og Ane mødtes i går om budgettet."]);
    let mut values = BTreeMap::new();
    values.insert("emne".to_string(), "Budget".to_string());
    store.update_document(doc, "Møde", None, &values).unwrap();
    let template = Template::parse(
        "t",
        r#"
name = "T"
heading = "T"
[[fields]]
key = "emne"
label = "Emne"
[[fields]]
key = "deltagere"
label = "Deltagere"
[[fields]]
key = "dato"
label = "Dato"
kind = "date"
[[fields]]
key = "type"
label = "Type"
kind = "list"
options = ["Møde", "Opkald"]
"#,
        std::path::Path::new("t.toml"),
    )
    .unwrap();

    let s = ai
        .suggest_fields(&store, doc, &template, &Cancel::default())
        .unwrap();

    assert_eq!(s.get("deltagere").map(String::as_str), Some("Jens og Ane"));
    assert_eq!(s.get("dato").map(String::as_str), Some("1. oktober 2026"));
    assert!(!s.contains_key("type"), "not one of the options");
    assert!(!s.contains_key("emne"));
    assert_eq!(store.document(doc).unwrap().fields, values, "nothing written");
}

#[test]
fn refusals_and_bad_keys_are_errors_and_store_nothing() {
    let mock = MockLlm::start(Wire::Anthropic, |rec| {
        if rec.header("x-api-key") == Some("bad") {
            Reply::Status(401, "invalid x-api-key".into())
        } else {
            Reply::Stop(String::new(), "refusal")
        }
    });
    let (store, doc) = store_with_doc(&["Hej."]);
    let scope = Scope::Document(doc);
    for (key, want) in [("good", "Refusal"), ("bad", "Auth")] {
        let ai = service(
            provider("claude", Protocol::Anthropic, &mock.url, Locality::Cloud),
            Some(key),
        );
        ai.consent(&store, &scope).unwrap();
        let err = ai
            .summarize(&store, &scope, &Cancel::default(), &mut |_| {})
            .unwrap_err();
        assert!(format!("{err:?}").starts_with(want), "{err:?}");
    }
    assert!(store.summaries_for_document(doc).unwrap().is_empty());
}

#[test]
fn long_documents_are_summarised_in_parts_then_combined() {
    let mock = MockLlm::start(Wire::OpenAi, |rec| {
        if rec.prompt_text().contains("Resuméer af tekstens dele") {
            Reply::Text("Samlet resumé.".into())
        } else {
            Reply::Text("Delresumé.".into())
        }
    });
    let mut p = provider("ollama", Protocol::OpenAi, &mock.url, Locality::ThisComputer);
    p.context_chars = 120;
    let ai = service(p, None);
    let texts: Vec<String> = (0..6)
        .map(|i| format!("Dette er afsnit nummer {i} i en lang tekst."))
        .collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let (store, doc) = store_with_doc(&refs);

    let s = ai
        .summarize(&store, &Scope::Document(doc), &Cancel::default(), &mut |_| {})
        .unwrap();

    assert_eq!(s.text, "Samlet resumé.");
    assert!(mock.count() >= 3, "{} requests", mock.count());
}

#[test]
fn model_lists_come_from_both_protocols() {
    for wire in [Wire::Anthropic, Wire::OpenAi] {
        let mock = MockLlm::start(wire, |rec| {
            assert!(rec.path.contains("/models"), "{}", rec.path);
            Reply::Models(vec!["b-model", "a-model"])
        });
        let protocol = if wire == Wire::Anthropic {
            Protocol::Anthropic
        } else {
            Protocol::OpenAi
        };
        let p = provider("x", protocol, &mock.url, Locality::Network);
        let models = connect(&p, &MemorySecrets::default()).list_models().unwrap();
        assert_eq!(models.len(), 2);
    }
}

#[test]
fn local_model_jobs_wait_while_dictation_runs() {
    let mock = MockLlm::start(Wire::OpenAi, |_| Reply::Text("ok".into()));
    let live = Arc::new(AtomicBool::new(true));
    let ai = service(
        provider("ollama", Protocol::OpenAi, &mock.url, Locality::ThisComputer),
        None,
    )
    .with_dictation_flag(Arc::clone(&live));
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("f.db");
    let doc = {
        let store = Store::open(&db).unwrap();
        let doc = store.create_document(&NewDocument::dictation("D")).unwrap();
        store.replace_paragraphs(doc, &[Paragraph::new("Hej.")]).unwrap();
        doc
    };
    let job = std::thread::spawn(move || {
        let store = Store::open(&db).unwrap();
        ai.summarize(&store, &Scope::Document(doc), &Cancel::default(), &mut |_| {})
    });
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(mock.count(), 0, "queued while dictating");
    live.store(false, Ordering::Relaxed);
    assert!(job.join().unwrap().is_ok());
    assert_eq!(mock.count(), 1);
}

/// One mock server per job, plus one for the default provider.
struct JobMocks {
    default: MockLlm,
    summaries: MockLlm,
    cleanup: MockLlm,
    items: MockLlm,
    ask: MockLlm,
    fields: MockLlm,
}

fn job_mocks() -> JobMocks {
    JobMocks {
        default: MockLlm::start(Wire::OpenAi, |_| Reply::Text("fra standard".into())),
        summaries: MockLlm::start(Wire::OpenAi, |_| Reply::Text("Resumé.".into())),
        cleanup: MockLlm::start(Wire::OpenAi, |rec| {
            let paragraphs: Vec<_> = tagged(rec)
                .into_iter()
                .map(|(id, text)| json!({"id": id, "text": text.replace("øh ", "")}))
                .collect();
            Reply::Text(json!({ "paragraphs": paragraphs }).to_string())
        }),
        items: MockLlm::start(Wire::OpenAi, |_| {
            Reply::Text(
                json!({"items": [{"what": "Send tilbud", "who": null, "due": null, "paragraph": null}]})
                    .to_string(),
            )
        }),
        ask: MockLlm::start(Wire::OpenAi, |_| Reply::Text("Jens.".into())),
        fields: MockLlm::start(Wire::Anthropic, |_| {
            Reply::Text(json!({"emne": "Tilbud"}).to_string())
        }),
    }
}

fn job_service(m: &JobMocks) -> AiService {
    let mut settings = AiSettings {
        enabled: true,
        default_provider: "default".into(),
        providers: vec![
            provider(
                "default",
                Protocol::OpenAi,
                &m.default.url,
                Locality::ThisComputer,
            ),
            provider("sum", Protocol::OpenAi, &m.summaries.url, Locality::ThisComputer),
            provider("clean", Protocol::OpenAi, &m.cleanup.url, Locality::Network),
            provider("items", Protocol::OpenAi, &m.items.url, Locality::ThisComputer),
            provider("ask", Protocol::OpenAi, &m.ask.url, Locality::Network),
            provider("fields", Protocol::Anthropic, &m.fields.url, Locality::Cloud),
        ],
        ..Default::default()
    };
    for (job, id) in [
        (AiJob::Summaries, "sum"),
        (AiJob::Cleanup, "clean"),
        (AiJob::ActionItems, "items"),
        (AiJob::Ask, "ask"),
        (AiJob::FillFields, "fields"),
    ] {
        settings.jobs.insert(job.key().into(), id.into());
    }
    AiService::new(settings, Arc::new(MemorySecrets::with("fields", "k")))
}

fn one_field_template() -> Template {
    Template::parse(
        "t",
        "name = \"T\"\nheading = \"T\"\n[[fields]]\nkey = \"emne\"\nlabel = \"Emne\"\n",
        std::path::Path::new("t.toml"),
    )
    .unwrap()
}

#[test]
fn each_action_goes_to_the_provider_set_for_its_job() {
    let m = job_mocks();
    let ai = job_service(&m);
    let (store, doc) = store_with_doc(&["Jens sender øh tilbuddet."]);
    let scope = Scope::Document(doc);
    let c = Cancel::default();

    let s = ai.summarize(&store, &scope, &c, &mut |_| {}).unwrap();
    assert_eq!((s.text.as_str(), s.provider.as_str()), ("Resumé.", "sum"));
    assert_eq!(m.summaries.count(), 1);

    assert_eq!(ai.cleanup(&store, doc, &c).unwrap().len(), 1);
    assert_eq!(m.cleanup.count(), 1);

    assert_eq!(ai.action_items(&store, doc, &c).unwrap()[0].what, "Send tilbud");
    assert_eq!(m.items.count(), 1);

    ai.ask(&store, &scope, "Hvem sender tilbuddet?", &c, &mut |_| {})
        .unwrap();
    assert_eq!(m.ask.count(), 1);

    // The fields job runs on a cloud provider: it asks first, naming that provider.
    let err = ai
        .suggest_fields(&store, doc, &one_field_template(), &c)
        .unwrap_err();
    assert!(
        matches!(
            &err,
            AiError::Privacy(PrivacyError::NeedsConsent { provider_id, .. }) if provider_id == "fields"
        ),
        "{err:?}"
    );
    assert_eq!(m.fields.count(), 0, "nothing is sent before consent");
    ai.consent_to(&store, &scope, "fields").unwrap();
    let s = ai.suggest_fields(&store, doc, &one_field_template(), &c).unwrap();
    assert_eq!(s.get("emne").map(String::as_str), Some("Tilbud"));
    assert_eq!(m.fields.count(), 1);

    assert_eq!(m.default.count(), 0, "the default provider was never used");
}

#[test]
fn a_local_only_project_sends_a_cloud_job_to_a_local_provider_instead() {
    // As the mockup says: "Local-only projects always use Workstation,
    // whatever is set here." The cloud provider is never contacted.
    let m = job_mocks();
    let ai = job_service(&m);
    let (store, doc) = store_with_doc(&["Fortroligt."]);
    let project = store.create_project("Drift", "#888").unwrap();
    store.move_document(doc, Some(project)).unwrap();
    store.set_project_local_only(project, true).unwrap();
    ai.consent_to(&store, &Scope::Document(doc), "fields").unwrap();

    let _ = ai.suggest_fields(&store, doc, &one_field_template(), &Cancel::default());
    assert_eq!(m.fields.count(), 0, "the cloud provider was contacted");
    assert!(
        m.default.count() > 0,
        "the local default provider answered instead"
    );
    // Jobs on local and network providers keep working in the same project.
    ai.summarize(&store, &Scope::Document(doc), &Cancel::default(), &mut |_| {})
        .unwrap();
    assert_eq!(m.fields.count(), 0);
    assert_eq!(m.summaries.count(), 1);
}

#[test]
fn a_job_without_a_choice_uses_the_default_provider() {
    let m = job_mocks();
    let mut settings = job_service(&m).settings().clone();
    settings.jobs.remove(AiJob::Summaries.key());
    let ai = AiService::new(settings, Arc::new(MemorySecrets::default()));
    let (store, doc) = store_with_doc(&["Hej."]);
    let s = ai
        .summarize(&store, &Scope::Document(doc), &Cancel::default(), &mut |_| {})
        .unwrap();
    assert_eq!(s.provider, "default");
    assert_eq!((m.default.count(), m.summaries.count()), (1, 0));
}

#[test]
fn meeting_notes_are_checked_stored_as_the_summary_and_fill_the_action_items() {
    let mock = MockLlm::start(Wire::OpenAi, |rec| {
        let first = tagged(rec)[0].0;
        Reply::Text(
            json!({
                "summary": "Mødet handlede om bookingsystemet.",
                "topics": [{"heading": "Bookingsystemet", "points": [
                    {"text": "Appen viser en tom side.", "paragraph": first},
                    {"text": "  ", "paragraph": null}
                ]}],
                "decisions": [{"text": "Anbefalingen sendes til udvalget.", "paragraph": 424242}],
                "action_items": [
                    {"what": "Skriv til leverandøren", "who": "Mette", "due": "2026-11-06", "paragraph": first},
                    {"what": "Ring til vagtfirmaet", "who": "null", "due": "i morgen", "paragraph": null}
                ],
                "open_questions": [{"text": "Hvad gør vi med de sene hold?", "paragraph": null}]
            })
            .to_string(),
        )
    });
    let ai = service(
        provider("local", Protocol::OpenAi, &mock.url, Locality::ThisComputer),
        None,
    );
    let (store, doc) = store_with_doc(&[
        "Appen viser bare en tom side, og det er et problem for foreningerne.",
        "Mette skriver til leverandøren og beder om svar inden fredag i næste uge.",
    ]);
    let ids = paragraph_ids(&store, doc);

    let notes = ai.meeting_notes(&store, doc, &Cancel::default()).unwrap();

    assert_eq!(notes.prompt_id, "meeting-notes");
    let text = &notes.text;
    for part in [
        "**Resumé**\nMødet handlede om bookingsystemet.",
        "**Bookingsystemet**\n- Appen viser en tom side.",
        "**Beslutninger**\n- Anbefalingen sendes til udvalget.",
        "- Skriv til leverandøren (Mette, frist 6. november 2026)",
        "- Ring til vagtfirmaet\n",
        "**Åbne spørgsmål**\n- Hvad gør vi med de sene hold?",
    ] {
        assert!(text.contains(part), "missing {part:?} in:\n{text}");
    }
    let stored = store.summaries_for_document(doc).unwrap();
    assert_eq!(stored[0].prompt_id, "meeting-notes");
    let items = store.document_action_items(doc).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].paragraph_id, Some(ids[0]));
    assert_eq!(items[1].who, None, "\"null\" as a word is no name");
    assert_eq!(items[1].due, None, "not a date");
    let prompt = mock.requests()[0].prompt_text();
    assert!(prompt.contains("Gæt aldrig, hvem der sagde noget"), "{prompt}");
    assert!(prompt.contains("Kalender:"), "{prompt}");
}

#[test]
fn english_transcripts_are_proofread_and_noted_in_english() {
    let mock = MockLlm::start(Wire::OpenAi, |rec| {
        if rec.prompt_text().contains("minutes") {
            Reply::Text(
                json!({"summary": "The launch stays.", "topics": [], "decisions": [],
                       "action_items": [{"what": "Update the plan", "who": "Tom", "due": "2026-10-30", "paragraph": null}],
                       "open_questions": []})
                .to_string(),
            )
        } else {
            Reply::Text(json!({"paragraphs": []}).to_string())
        }
    });
    let ai = service(
        provider("local", Protocol::OpenAi, &mock.url, Locality::ThisComputer),
        None,
    );
    let (store, doc) = store_with_doc(&[
        "Okay so we keep the launch date and we launch without card payments.",
        "Tom can you update the plan and tell the support team by Friday.",
    ]);

    ai.cleanup(&store, doc, &Cancel::default()).unwrap();
    let notes = ai.meeting_notes(&store, doc, &Cancel::default()).unwrap();

    let requests = mock.requests();
    let system = |i: usize| {
        requests[i].body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert!(
        system(0).starts_with("You work with transcripts of English speech"),
        "{}",
        system(0)
    );
    assert!(system(0).contains("Proofread each paragraph"));
    assert!(system(1).contains("Calendar:"), "{}", system(1));
    assert!(
        notes
            .text
            .contains("**Action items**\n- Update the plan (Tom, due 30 October 2026)"),
        "{}",
        notes.text
    );
}

#[test]
fn a_built_in_model_talks_to_its_server_without_thinking() {
    let mock = MockLlm::start(Wire::OpenAi, |_| {
        Reply::Text(json!({"paragraphs": []}).to_string())
    });
    let dir = tempfile::tempdir().unwrap();
    let model = &fennec::ai::local::catalog()[1];
    let file = fennec::ai::local::path_of(model, dir.path());
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"stand-in").unwrap();
    let cfg = fennec::ai::local::provider_for(model, dir.path());
    // The server this test stands in for; Fennec would start llama-server.
    // SAFETY: no other test in this binary reads this variable.
    unsafe { std::env::set_var("FENNEC_LLAMA_SERVER_URL", mock.url.as_str()) };
    let ai = service(cfg.clone(), None);
    let (store, doc) = store_with_doc(&["Det her er en test."]);

    ai.cleanup(&store, doc, &Cancel::default()).unwrap();

    let req = &mock.requests()[0];
    assert_eq!(req.body["chat_template_kwargs"]["enable_thinking"], false);
    assert_eq!(
        connect(&cfg, &MemorySecrets::default()).list_models().unwrap(),
        [model.id]
    );
}

/// Meeting notes and proofreading with a real model: start llama-server by
/// hand, then
/// `FENNEC_LLAMA_SERVER_URL=http://127.0.0.1:PORT/v1 cargo test --release --test ai -- --ignored --nocapture`.
/// The transcript is a made-up Danish meeting with typical recognition errors.
#[test]
#[ignore]
fn real_built_in_model_writes_meeting_notes() {
    assert!(
        std::env::var("FENNEC_LLAMA_SERVER_URL").is_ok(),
        "start llama-server and set FENNEC_LLAMA_SERVER_URL"
    );
    let dir = tempfile::tempdir().unwrap();
    let model = &fennec::ai::local::catalog()[0];
    let file = fennec::ai::local::path_of(model, dir.path());
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"stand-in").unwrap();
    let ai = service(fennec::ai::local::provider_for(model, dir.path()), None);
    let text = std::fs::read_to_string("tests/data/meeting-da.txt").unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let (store, doc) = store_with_doc(&lines);

    let started = std::time::Instant::now();
    let notes = ai.meeting_notes(&store, doc, &Cancel::default()).unwrap();
    println!("{}\n\n({:.0} s)", notes.text, started.elapsed().as_secs_f32());
    let started = std::time::Instant::now();
    let fixes = ai.cleanup(&store, doc, &Cancel::default()).unwrap();
    for c in &fixes {
        println!("- {}\n+ {}", c.original, c.cleaned);
    }
    println!("({:.0} s)", started.elapsed().as_secs_f32());

    let items = store.document_action_items(doc).unwrap();
    assert!(
        items.iter().any(|i| i.who.as_deref() == Some("Jonas")),
        "{items:?}"
    );
    assert!(notes.text.contains("**Beslutninger**"));
    assert!(fixes.len() > lines.len() / 2, "most paragraphs need commas");
}

/// Times proofreading on a running llama-server, to compare its settings:
/// `FENNEC_LLAMA_SERVER_URL=http://127.0.0.1:8090 cargo test --release
/// --test ai real_built_in_model_proofreads -- --ignored --nocapture`.
#[test]
#[ignore]
fn real_built_in_model_proofreads() {
    assert!(
        std::env::var("FENNEC_LLAMA_SERVER_URL").is_ok(),
        "start llama-server and set FENNEC_LLAMA_SERVER_URL"
    );
    let dir = tempfile::tempdir().unwrap();
    let model = &fennec::ai::local::catalog()[0];
    let file = fennec::ai::local::path_of(model, dir.path());
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"stand-in").unwrap();
    let ai = service(fennec::ai::local::provider_for(model, dir.path()), None);
    let text = std::fs::read_to_string("tests/data/meeting-da.txt").unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let (store, doc) = store_with_doc(&lines);

    let started = std::time::Instant::now();
    let fixes = ai
        .cleanup_with_progress(&store, doc, &Cancel::default(), &mut |e| {
            if let Proofreading::Part { covered, .. } = e {
                println!(
                    "{:5.1} s  part of {} paragraphs back",
                    started.elapsed().as_secs_f32(),
                    covered.len()
                );
            }
        })
        .unwrap();
    for c in &fixes {
        println!("- {}\n+ {}", c.original, c.cleaned);
    }
    println!(
        "{} of {} paragraphs changed in {:.0} s",
        fixes.len(),
        lines.len(),
        started.elapsed().as_secs_f32()
    );
    assert!(fixes.len() > lines.len() / 2, "most paragraphs need commas");
}
