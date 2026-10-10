//! Store behaviour through its public API, on a real SQLite file.
// A single low-confidence span is intended, not a range of values.
#![allow(clippy::single_range_in_vec_init)]

use std::collections::BTreeMap;

use fennec::store::{
    DocumentFilter, NewActionItem, NewDocument, NewSummary, Paragraph, ProjectFilter, Source, Store,
};

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("fennec.db")).unwrap();
    (dir, store)
}

fn para(text: &str, start: i64, end: i64) -> Paragraph {
    Paragraph {
        start_ms: Some(start),
        end_ms: Some(end),
        ..Paragraph::new(text)
    }
}

#[test]
fn reopening_the_database_keeps_data_and_does_not_rerun_migrations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fennec.db");
    let id = {
        let s = Store::open(&path).unwrap();
        s.create_document(&NewDocument::dictation("Notat")).unwrap()
    };
    let s = Store::open(&path).unwrap();
    assert_eq!(s.document(id).unwrap().title, "Notat");
}

#[test]
fn threads_opening_a_new_database_together_all_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fennec.db");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let (path, barrier) = (path.clone(), std::sync::Arc::clone(&barrier));
            std::thread::spawn(move || {
                barrier.wait();
                Store::open(&path).map(|_| ()).map_err(|e| e.to_string())
            })
        })
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap(), Ok(()));
    }
}

#[test]
fn documents_belong_to_one_project_and_list_counts_follow() {
    let (_d, s) = store();
    let harbour = s.create_project("Operation Harbour", "#1D4ED8").unwrap();
    let acme = s.create_project("Vendor – Acme", "#0F766E").unwrap();
    let a = s
        .create_document(&NewDocument {
            project_id: Some(harbour),
            ..NewDocument::dictation("Kickoff")
        })
        .unwrap();
    s.create_document(&NewDocument {
        project_id: Some(harbour),
        ..NewDocument::file("Interview")
    })
    .unwrap();
    s.create_document(&NewDocument::dictation("Løs note")).unwrap();

    s.move_document(a, Some(acme)).unwrap();

    let counts: BTreeMap<String, usize> = s
        .projects()
        .unwrap()
        .into_iter()
        .map(|p| (p.name, p.document_count))
        .collect();
    assert_eq!(counts["Operation Harbour"], 1);
    assert_eq!(counts["Vendor – Acme"], 1);
    let unsorted = s
        .documents(&DocumentFilter {
            project: ProjectFilter::Unsorted,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        unsorted.iter().map(|d| d.title.as_str()).collect::<Vec<_>>(),
        ["Løs note"]
    );
}

#[test]
fn deleting_a_project_keeps_its_documents_as_unsorted() {
    let (_d, s) = store();
    let p = s.create_project("Midlertidig", "#C2410C").unwrap();
    let doc = s
        .create_document(&NewDocument {
            project_id: Some(p),
            ..NewDocument::dictation("Note")
        })
        .unwrap();
    s.delete_project(p).unwrap();
    assert_eq!(s.document(doc).unwrap().project_id, None);
}

#[test]
fn tags_are_shared_across_projects_and_filter_documents() {
    let (_d, s) = store();
    let p1 = s.create_project("A", "#000000").unwrap();
    let p2 = s.create_project("B", "#000000").unwrap();
    let d1 = s
        .create_document(&NewDocument {
            project_id: Some(p1),
            ..NewDocument::dictation("Møde 1")
        })
        .unwrap();
    let d2 = s
        .create_document(&NewDocument {
            project_id: Some(p2),
            ..NewDocument::dictation("Møde 2")
        })
        .unwrap();
    s.set_tags(d1, &["meeting".into(), "vendor".into()]).unwrap();
    s.set_tags(d2, &["vendor".into(), "Vendor".into()]).unwrap();

    let vendor = s
        .documents(&DocumentFilter {
            tag: Some("vendor".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(vendor.len(), 2, "tags are case-insensitive and cross projects");
    let tags: BTreeMap<String, usize> = s.tags().unwrap().into_iter().collect();
    assert_eq!(tags["vendor"], 2);
    assert_eq!(tags["meeting"], 1);
    assert_eq!(s.document(d2).unwrap().tags, ["vendor"]);
}

#[test]
fn paragraphs_round_trip_with_timestamps_and_low_confidence_spans() {
    let (_d, s) = store();
    let doc = s.create_document(&NewDocument::file("Interview")).unwrap();
    let mut p = para("Fugten kommer fra en utæt nedløbsbrønd.", 0, 4_200);
    p.low_confidence = vec![26..40];
    s.replace_paragraphs(doc, &[p.clone(), para("Andet afsnit.", 4_200, 6_000)])
        .unwrap();

    let got = s.paragraphs(doc).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].text, p.text);
    assert_eq!(got[0].low_confidence, vec![26..40]);
    assert_eq!((got[1].start_ms, got[1].end_ms), (Some(4_200), Some(6_000)));
    assert!(got[0].id.is_some());
}

#[test]
fn appended_paragraphs_keep_order_and_update_the_document_duration() {
    let (_d, s) = store();
    let doc = s.create_document(&NewDocument::dictation("Diktat")).unwrap();
    s.append_paragraph(doc, &para("Første.", 0, 2_000)).unwrap();
    s.append_paragraph(doc, &para("Anden.", 2_500, 5_000)).unwrap();
    let texts: Vec<String> = s.paragraphs(doc).unwrap().into_iter().map(|p| p.text).collect();
    assert_eq!(texts, ["Første.", "Anden."]);
    let summary = s.documents(&DocumentFilter::default()).unwrap().remove(0);
    assert_eq!(summary.duration_ms, Some(5_000));
}

#[test]
fn full_text_search_finds_danish_words_by_prefix_within_a_project() {
    let (_d, s) = store();
    let p = s.create_project("Nørregade 14", "#C2410C").unwrap();
    let inside = s
        .create_document(&NewDocument {
            project_id: Some(p),
            ..NewDocument::dictation("Besigtigelse")
        })
        .unwrap();
    let outside = s.create_document(&NewDocument::dictation("Andet")).unwrap();
    s.replace_paragraphs(inside, &[para("I kælderen er der fugt langs ydermuren.", 0, 1)])
        .unwrap();
    s.replace_paragraphs(outside, &[para("Kælderen her er tør.", 0, 1)])
        .unwrap();

    let all = s.search("kælder", ProjectFilter::All).unwrap();
    assert_eq!(all.len(), 2);
    let scoped = s.search("kælder fugt", ProjectFilter::Project(p)).unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].document_id, inside);

    let by_text = s
        .documents(&DocumentFilter {
            text: Some("ydermur".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(by_text.iter().map(|d| d.id).collect::<Vec<_>>(), [inside]);
}

#[test]
fn search_treats_quotes_and_operators_in_user_input_as_plain_text() {
    let (_d, s) = store();
    let doc = s.create_document(&NewDocument::dictation("x")).unwrap();
    s.replace_paragraphs(doc, &[para("Han sagde \"OR\" og gik.", 0, 1)])
        .unwrap();
    assert_eq!(s.search("\"OR\" (", ProjectFilter::All).unwrap().len(), 1);
    assert!(
        s.search("NOT AND", ProjectFilter::All).unwrap().is_empty(),
        "operators are words, not syntax"
    );
    assert!(s.search("   ", ProjectFilter::All).unwrap().is_empty());
}

#[test]
fn editing_paragraphs_keeps_the_search_index_in_sync() {
    let (_d, s) = store();
    let doc = s.create_document(&NewDocument::dictation("x")).unwrap();
    s.replace_paragraphs(doc, &[para("gammel tekst", 0, 1)]).unwrap();
    s.replace_paragraphs(doc, &[para("ny tekst", 0, 1)]).unwrap();
    assert!(s.search("gammel", ProjectFilter::All).unwrap().is_empty());
    assert_eq!(s.search("ny", ProjectFilter::All).unwrap().len(), 1);
    s.delete_document(doc).unwrap();
    assert!(s.search("ny", ProjectFilter::All).unwrap().is_empty());
}

#[test]
fn template_fields_and_metadata_are_stored_per_document() {
    let (_d, s) = store();
    let doc = s.create_document(&NewDocument::dictation("Notat")).unwrap();
    let mut fields = BTreeMap::new();
    fields.insert("sagsnr".to_string(), "2026-0412".to_string());
    s.update_document(doc, "Besigtigelse", Some("notat"), &fields)
        .unwrap();
    s.set_audio_path(doc, Some(std::path::Path::new("/tmp/a.flac")))
        .unwrap();

    let d = s.document(doc).unwrap();
    assert_eq!(d.title, "Besigtigelse");
    assert_eq!(d.template_id.as_deref(), Some("notat"));
    assert_eq!(d.fields["sagsnr"], "2026-0412");
    assert_eq!(d.audio_path.as_deref(), Some(std::path::Path::new("/tmp/a.flac")));
    assert_eq!(d.source, Source::Dictation);
}

#[test]
fn local_only_flag_is_stored_on_the_project() {
    let (_d, s) = store();
    let p = s.create_project("Hemmelig", "#000000").unwrap();
    s.set_project_local_only(p, true).unwrap();
    assert!(s.projects().unwrap()[0].local_only);
}

#[test]
fn summaries_and_action_items_roll_up_per_project() {
    let (_d, s) = store();
    let p = s.create_project("Harbour", "#000000").unwrap();
    let d1 = s
        .create_document(&NewDocument {
            project_id: Some(p),
            ..NewDocument::dictation("Møde")
        })
        .unwrap();
    let d2 = s.create_document(&NewDocument::dictation("Andet")).unwrap();
    s.add_summary(&NewSummary {
        document_id: Some(d1),
        project_id: None,
        prompt_id: "kort".into(),
        provider: "Claude".into(),
        model: "claude-opus-5-5".into(),
        text: "Kort resumé.".into(),
    })
    .unwrap();
    let items = [
        NewActionItem {
            what: "Ring til Acme".into(),
            who: Some("Projektleder".into()),
            due: Some("2026-10-15".into()),
            paragraph_id: None,
            provider: Some("Claude".into()),
        },
        NewActionItem {
            what: "Send plan".into(),
            who: None,
            due: None,
            paragraph_id: None,
            provider: Some("Claude".into()),
        },
    ];
    s.replace_action_items(d1, &items).unwrap();
    s.replace_action_items(
        d2,
        &[NewActionItem {
            what: "Ikke med".into(),
            who: None,
            due: None,
            paragraph_id: None,
            provider: None,
        }],
    )
    .unwrap();

    let roll_up = s.action_items(ProjectFilter::Project(p)).unwrap();
    assert_eq!(
        roll_up.iter().map(|a| a.what.as_str()).collect::<Vec<_>>(),
        ["Ring til Acme", "Send plan"]
    );
    assert!(
        roll_up
            .iter()
            .all(|a| a.provider.as_deref() == Some("Claude") && a.created_at > 0),
        "items remember who found them, and when"
    );
    s.set_action_done(roll_up[0].id, true).unwrap();
    let after = s.action_items(ProjectFilter::Project(p)).unwrap();
    assert_eq!(
        after
            .iter()
            .map(|a| (a.what.as_str(), a.done))
            .collect::<Vec<_>>(),
        [("Send plan", false), ("Ring til Acme", true)],
        "done items sort last"
    );

    let summaries = s.summaries_for_document(d1).unwrap();
    assert_eq!(summaries[0].text, "Kort resumé.");
    assert!(summaries[0].include_in_export);
}

#[test]
fn syncing_paragraphs_keeps_ids_and_search_in_step() {
    let (_d, s) = store();
    let doc = s.create_document(&NewDocument::dictation("x")).unwrap();
    s.sync_paragraphs(
        doc,
        &[Paragraph::new("en"), Paragraph::new("to"), Paragraph::new("tre")],
    )
    .unwrap();
    let before: Vec<_> = s.paragraphs(doc).unwrap().into_iter().map(|p| p.id).collect();
    s.sync_paragraphs(doc, &[Paragraph::new("et"), Paragraph::new("to")])
        .unwrap();
    let after = s.paragraphs(doc).unwrap();
    assert_eq!(after.iter().map(|p| p.id).collect::<Vec<_>>(), before[..2]);
    assert_eq!(after[0].text, "et");
    assert!(s.search("tre", ProjectFilter::All).unwrap().is_empty());
    assert!(s.search("en", ProjectFilter::All).unwrap().is_empty());
    assert_eq!(s.search("et", ProjectFilter::All).unwrap().len(), 1);
}

#[test]
fn cloud_consents_are_remembered_per_scope_and_provider() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("f.db");
    {
        let store = Store::open(&db).unwrap();
        assert!(!store.has_cloud_consent("document:1", "claude").unwrap());
        store.add_cloud_consent("document:1", "claude").unwrap();
        store.add_cloud_consent("document:1", "claude").unwrap();
    }
    let store = Store::open(&db).unwrap();
    assert!(store.has_cloud_consent("document:1", "claude").unwrap());
    assert!(!store.has_cloud_consent("document:1", "chatgpt").unwrap());
    assert!(!store.has_cloud_consent("document:2", "claude").unwrap());
    assert_eq!(store.clear_cloud_consents().unwrap(), 1);
    assert!(!store.has_cloud_consent("document:1", "claude").unwrap());
}

#[test]
fn corrections_are_counted_and_a_dismissed_one_stays_quiet() {
    let (_dir, s) = store();
    assert_eq!(s.record_correction("tonelighter", "toneleje").unwrap(), 1);
    assert_eq!(s.record_correction("tonelighter", "toneleje").unwrap(), 2);
    assert_eq!(
        s.record_correction("tonelighter", "tonelejet").unwrap(),
        1,
        "another wanted word"
    );
    s.dismiss_correction("tonelighter", "toneleje").unwrap();
    assert_eq!(
        s.record_correction("tonelighter", "toneleje").unwrap(),
        0,
        "dismissed: never offered again"
    );
}
