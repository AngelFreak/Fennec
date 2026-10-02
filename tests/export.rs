//! Export end to end: store → template → TXT / DOCX / PDF files, read back.
#![allow(clippy::single_range_in_vec_init)]

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::Command;

use fennec::export::{ExportError, ExportOptions, Format, Report, write};
use fennec::store::{NewDocument, NewSummary, Paragraph, Store};
use fennec::template::Template;

const NOTAT: &str = r#"
name = "Notat"
heading = "NOTAT"
footer = "Fortroligt"
summary_heading = "Resumé"

[[fields]]
key = "sagsnr"
label = "Sagsnr."
required = true

[[fields]]
key = "emne"
label = "Emne"
"#;

fn template() -> Template {
    Template::parse("notat", NOTAT, Path::new("/tpl/notat.toml")).unwrap()
}

fn seeded() -> (Store, i64) {
    let s = Store::open_in_memory().unwrap();
    let doc = s
        .create_document(&NewDocument::dictation("Besigtigelse Nørregade 14"))
        .unwrap();
    let mut fields = BTreeMap::new();
    fields.insert("sagsnr".to_string(), "2026-0412".to_string());
    fields.insert("emne".to_string(), "Fugt i kælder".to_string());
    s.update_document(doc, "Besigtigelse Nørregade 14", Some("notat"), &fields)
        .unwrap();
    let mut p1 = Paragraph::new("I kælderen er der fugt fra en utæt nedløbsbrønd.");
    p1.start_ms = Some(751_000);
    p1.low_confidence = vec![37..51]; // "nedløbsbrønd" (bytes, æ and ø are two each)
    let p2 = Paragraph {
        start_ms: Some(760_000),
        ..Paragraph::new("Trappeopgangen fremstår velholdt.")
    };
    s.replace_paragraphs(doc, &[p1, p2]).unwrap();
    (s, doc)
}

fn pdf_text(path: &Path) -> String {
    let out = Command::new("pdftotext")
        .arg(path)
        .arg("-")
        .output()
        .expect("pdftotext (poppler-utils) is installed");
    assert!(out.status.success(), "pdftotext failed");
    String::from_utf8(out.stdout).unwrap()
}

fn docx_xml(path: &Path) -> String {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut xml = String::new();
    zip.by_name("word/document.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    xml
}

#[test]
fn txt_export_contains_heading_fields_title_and_paragraphs_in_order() {
    let (s, doc) = seeded();
    let report = Report::for_document(&s, doc, &template(), ExportOptions::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notat.txt");
    write(&report, Format::Txt, &path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let order = [
        "NOTAT",
        "Sagsnr.: 2026-0412",
        "Emne:    Fugt i kælder",
        "Besigtigelse Nørregade 14",
        "I kælderen",
        "Trappeopgangen",
        "Fortroligt",
    ];
    let mut at = 0;
    for needle in order {
        let found = text[at..]
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} missing or out of order in:\n{text}"));
        at += found + needle.len();
    }
}

#[test]
fn timestamps_are_included_only_when_asked_for() {
    let (s, doc) = seeded();
    let on = ExportOptions {
        timestamps: true,
        ..Default::default()
    };
    let txt = fennec::export::render_txt(&Report::for_document(&s, doc, &template(), on).unwrap());
    assert!(txt.contains("[00:12:31] I kælderen"), "{txt}");
    let off = fennec::export::render_txt(
        &Report::for_document(&s, doc, &template(), ExportOptions::default()).unwrap(),
    );
    assert!(!off.contains("[00:12:31]"));
}

#[test]
fn export_is_blocked_while_required_fields_are_empty() {
    let (s, doc) = seeded();
    s.update_document(doc, "x", Some("notat"), &BTreeMap::new())
        .unwrap();
    let err = Report::for_document(&s, doc, &template(), ExportOptions::default()).unwrap_err();
    assert!(
        matches!(&err, ExportError::MissingFields(f) if f == &["Sagsnr."]),
        "{err}"
    );
}

#[test]
fn docx_has_fields_text_and_highlights_and_is_a_valid_zip_package() {
    let (s, doc) = seeded();
    let opts = ExportOptions {
        highlight_low_confidence: true,
        ..Default::default()
    };
    let report = Report::for_document(&s, doc, &template(), opts).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notat.docx");
    write(&report, Format::Docx, &path).unwrap();
    let xml = docx_xml(&path);
    for needle in [
        "NOTAT",
        "Sagsnr.",
        "2026-0412",
        "Besigtigelse Nørregade 14",
        "Trappeopgangen fremstår velholdt.",
    ] {
        assert!(xml.contains(needle), "{needle:?} missing");
    }
    assert!(
        xml.contains(r#"<w:highlight w:val="yellow"/></w:rPr><w:t xml:space="preserve">nedløbsbrønd</w:t>"#)
    );
    assert!(
        !dir.path().join("notat.docx.part").exists(),
        "temporary file is cleaned up"
    );
}

#[test]
fn pdf_text_is_extractable_with_danish_letters_and_page_footer() {
    let (s, doc) = seeded();
    let report = Report::for_document(&s, doc, &template(), ExportOptions::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notat.pdf");
    write(&report, Format::Pdf, &path).unwrap();
    let text = pdf_text(&path);
    for needle in [
        "NOTAT",
        "2026-0412",
        "Fugt i kælder",
        "nedløbsbrønd",
        "Side 1 af 1",
        "Fortroligt",
    ] {
        assert!(text.contains(needle), "{needle:?} missing in:\n{text}");
    }
}

#[test]
fn long_documents_paginate_and_number_every_page() {
    let s = Store::open_in_memory().unwrap();
    let doc = s.create_document(&NewDocument::file("Langt interview")).unwrap();
    let para = Paragraph::new(&"Det her er en lang sætning om ingenting i særdeleshed. ".repeat(12));
    s.replace_paragraphs(doc, &vec![para; 40]).unwrap();
    let report = Report::for_document(&s, doc, &Template::blank(), ExportOptions::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lang.pdf");
    write(&report, Format::Pdf, &path).unwrap();
    let text = pdf_text(&path);
    let pages = text.matches("Side ").count();
    assert!(pages >= 3, "expected several pages, got {pages}");
    assert!(text.contains(&format!("Side {pages} af {pages}")), "{text}");
}

#[test]
fn included_summary_appears_under_its_heading() {
    let (s, doc) = seeded();
    s.add_summary(&NewSummary {
        document_id: Some(doc),
        project_id: None,
        prompt_id: "kort".into(),
        provider: "Claude".into(),
        model: "claude-opus-5-5".into(),
        text: "Fugt i kælderen og revner i pudsen.".into(),
    })
    .unwrap();
    let txt = fennec::export::render_txt(
        &Report::for_document(&s, doc, &template(), ExportOptions::default()).unwrap(),
    );
    let h = txt.find("Resumé").expect("summary heading");
    assert!(
        txt[h..].starts_with("Resumé\nFugt i kælderen og revner i pudsen."),
        "{txt}"
    );
}

#[test]
fn project_export_orders_documents_oldest_first_with_a_table_of_contents() {
    let s = Store::open_in_memory().unwrap();
    let first = s.create_document(&NewDocument::dictation("Kickoff")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let second = s.create_document(&NewDocument::dictation("Opfølgning")).unwrap();
    s.replace_paragraphs(first, &[Paragraph::new("Første møde.")])
        .unwrap();
    s.replace_paragraphs(second, &[Paragraph::new("Andet møde.")])
        .unwrap();

    let report = Report::for_documents(
        &s,
        &[second, first],
        "Operation Harbour",
        &Template::blank(),
        ExportOptions::default(),
    )
    .unwrap();
    let txt = fennec::export::render_txt(&report);
    let toc_first = txt.find("1. Kickoff").expect("toc entry 1");
    let toc_second = txt.find("2. Opfølgning").expect("toc entry 2");
    let body_first = txt.find("Første møde.").unwrap();
    let body_second = txt.find("Andet møde.").unwrap();
    assert!(txt.starts_with("Operation Harbour"));
    assert!(
        toc_first < toc_second && toc_second < body_first && body_first < body_second,
        "{txt}"
    );

    let dir = tempfile::tempdir().unwrap();
    for format in [Format::Docx, Format::Pdf] {
        let path = dir.path().join(format!("p.{}", format.extension()));
        write(&report, format, &path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
    }
    assert!(
        pdf_text(&dir.path().join("p.pdf")).contains("Side 3 af 3"),
        "toc page + two documents"
    );
}

#[test]
fn a_non_png_logo_is_rejected_before_writing() {
    let (s, doc) = seeded();
    let mut t = template();
    t.logo = Some("logo.jpg".into());
    let report = Report::for_document(&s, doc, &t, ExportOptions::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let err = write(&report, Format::Pdf, &dir.path().join("x.pdf")).unwrap_err();
    assert!(matches!(err, ExportError::LogoNotPng { .. }));
}
