//! AI actions end to end: a real store, the real service and provider
//! clients, and local mock servers speaking each protocol.

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fennec::ai::service::Scope;
use fennec::ai::{
    AiError, AiService, AiSettings, Cancel, Locality, MemorySecrets, Protocol, ProviderConfig, connect,
    privacy::PrivacyError,
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
