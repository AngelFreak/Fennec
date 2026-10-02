//! Minimal Office Open XML writer: styles, a fields table, highlighted runs,
//! optional logo and a footer with page numbers.

use std::fmt::Write as _;
use std::io::Write;
use std::path::Path;

use zip::write::SimpleFileOptions;

use super::{Report, ReportParagraph, Section};

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing""#;

pub fn write(report: &Report, path: &Path) -> std::io::Result<()> {
    let logo = match &report.logo {
        Some(p) => Some(std::fs::read(p)?),
        None => None,
    };
    let file = std::fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, body: &[u8]| -> std::io::Result<()> {
        zip.start_file(name, opts).map_err(std::io::Error::other)?;
        zip.write_all(body)
    };
    put("[Content_Types].xml", CONTENT_TYPES.as_bytes())?;
    put("_rels/.rels", ROOT_RELS.as_bytes())?;
    put(
        "word/_rels/document.xml.rels",
        document_rels(logo.is_some()).as_bytes(),
    )?;
    put("word/styles.xml", styles(report).as_bytes())?;
    put("word/footer1.xml", footer(report).as_bytes())?;
    put("word/document.xml", document(report, logo.as_deref()).as_bytes())?;
    if let Some(bytes) = &logo {
        put("word/media/logo.png", bytes)?;
    }
    zip.finish().map_err(std::io::Error::other)?;
    Ok(())
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/></Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

fn document_rels(logo: bool) -> String {
    let mut s = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rIdFooter" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer1.xml"/>"#,
    );
    if logo {
        s.push_str(r#"<Relationship Id="rIdLogo" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/logo.png"/>"#);
    }
    s.push_str("</Relationships>");
    s
}

fn styles(report: &Report) -> String {
    let font = esc(&report.body_font);
    let size = (report.body_size_pt * 2.0).round() as i64;
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles {W_NS}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="{font}" w:hAnsi="{font}" w:cs="{font}"/><w:sz w:val="{size}"/><w:lang w:val="da-DK"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="300" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="240"/></w:pPr><w:rPr><w:b/><w:sz w:val="40"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="ReportHeading"><w:name w:val="Report Heading"/><w:basedOn w:val="Normal"/><w:pPr><w:jc w:val="right"/><w:pBdr><w:bottom w:val="single" w:sz="12" w:space="4" w:color="15171C"/></w:pBdr><w:spacing w:after="240"/></w:pPr><w:rPr><w:b/><w:spacing w:val="40"/><w:sz w:val="22"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="160"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="160" w:after="80"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="24"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Footer"><w:name w:val="footer"/><w:basedOn w:val="Normal"/><w:pPr><w:tabs><w:tab w:val="right" w:pos="9026"/></w:tabs></w:pPr><w:rPr><w:color w:val="5A6170"/><w:sz w:val="16"/></w:rPr></w:style></w:styles>"#
    )
}

fn footer(report: &Report) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:ftr {W_NS}><w:p><w:pPr><w:pStyle w:val="Footer"/></w:pPr>{text}<w:r><w:tab/><w:t xml:space="preserve">Side </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> af </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>NUMPAGES</w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:ftr>"#,
        text = run(&report.footer, "")
    )
}

fn document(report: &Report, logo: Option<&[u8]>) -> String {
    let mut body = String::new();
    if let Some(title) = &report.collection_title {
        para(&mut body, "Title", &run(title, ""));
        if report.table_of_contents {
            for (i, s) in report.sections.iter().enumerate() {
                para(
                    &mut body,
                    "Normal",
                    &run(&format!("{}. {} ({})", i + 1, s.title, s.date), ""),
                );
            }
        }
    }
    for (i, section) in report.sections.iter().enumerate() {
        if i > 0 || report.collection_title.is_some() {
            body.push_str(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
        }
        if let (Some(bytes), true) = (logo, i == 0 || report.collection_title.is_some()) {
            body.push_str(&logo_paragraph(bytes));
        }
        if !report.heading.is_empty() {
            para(&mut body, "ReportHeading", &run(&report.heading, ""));
        }
        section_body(&mut body, report, section);
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document {W_NS}><w:body>{body}<w:sectPr><w:footerReference w:type="default" r:id="rIdFooter"/><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1418" w:right="1418" w:bottom="1418" w:left="1418" w:header="709" w:footer="709" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    )
}

fn section_body(body: &mut String, report: &Report, section: &Section) {
    if !section.fields.is_empty() {
        body.push_str(r#"<w:tbl><w:tblPr><w:tblW w:w="5000" w:type="pct"/><w:tblLook w:val="0000"/></w:tblPr><w:tblGrid><w:gridCol w:w="2600"/><w:gridCol w:w="6400"/></w:tblGrid>"#);
        for (label, value) in &section.fields {
            let _ = write!(
                body,
                r#"<w:tr><w:tc><w:p><w:pPr><w:spacing w:after="40"/></w:pPr>{}</w:p></w:tc><w:tc><w:p><w:pPr><w:spacing w:after="40"/></w:pPr>{}</w:p></w:tc></w:tr>"#,
                run(label, r#"<w:color w:val="5A6170"/>"#),
                multiline_runs(value)
            );
        }
        body.push_str("</w:tbl>");
    }
    para(body, "Heading1", &run(&section.title, ""));
    if let Some(summary) = &section.summary {
        para(body, "Heading2", &run(&report.summary_heading, ""));
        for line in summary.lines().filter(|l| !l.trim().is_empty()) {
            para(body, "Normal", &run(line.trim(), ""));
        }
    }
    for p in &section.paragraphs {
        para(body, "Normal", &paragraph_runs(p));
    }
}

fn paragraph_runs(p: &ReportParagraph) -> String {
    let mut out = String::new();
    if let Some(ts) = &p.timestamp {
        out.push_str(&run(&format!("[{ts}] "), r#"<w:color w:val="5A6170"/>"#));
    }
    let mut pos = 0;
    for r in &p.highlight {
        let (start, end) = (r.start.min(p.text.len()), r.end.min(p.text.len()));
        if start < pos || !p.text.is_char_boundary(start) || !p.text.is_char_boundary(end) {
            continue;
        }
        out.push_str(&run(&p.text[pos..start], ""));
        out.push_str(&run(&p.text[start..end], r#"<w:highlight w:val="yellow"/>"#));
        pos = end;
    }
    out.push_str(&run(&p.text[pos..], ""));
    out
}

fn multiline_runs(text: &str) -> String {
    text.lines()
        .map(|l| run(l, ""))
        .collect::<Vec<_>>()
        .join("<w:r><w:br/></w:r>")
}

fn para(body: &mut String, style: &str, runs: &str) {
    let _ = write!(
        body,
        r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr>{runs}</w:p>"#
    );
}

fn run(text: &str, rpr: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let rpr = if rpr.is_empty() {
        String::new()
    } else {
        format!("<w:rPr>{rpr}</w:rPr>")
    };
    format!(r#"<w:r>{rpr}<w:t xml:space="preserve">{}</w:t></w:r>"#, esc(text))
}

/// Inline logo, 1.2 cm high, width from the PNG's aspect ratio.
fn logo_paragraph(png: &[u8]) -> String {
    let (w, h) = png_size(png).unwrap_or((1, 1));
    let cy: u64 = 432_000; // EMU, 1.2 cm
    let cx = cy * u64::from(w) / u64::from(h.max(1));
    format!(
        r#"<w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="1" name="Logo"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="0" name="logo.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rIdLogo"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
    )
}

fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(png[20..24].try_into().ok()?);
    Some((w, h))
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // XML 1.0 forbids most control characters; drop them.
            c if (c as u32) < 0x20 && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_covers_markup_and_drops_control_characters() {
        assert_eq!(esc("a<b>&\"c\"\u{1}"), "a&lt;b&gt;&amp;&quot;c&quot;");
    }

    #[test]
    fn highlighted_spans_become_separate_runs() {
        let p = ReportParagraph {
            text: "en utæt brønd".into(),
            timestamp: None,
            highlight: vec![3..8],
        };
        let xml = paragraph_runs(&p);
        assert!(
            xml.contains(r#"<w:highlight w:val="yellow"/></w:rPr><w:t xml:space="preserve">utæt</w:t>"#),
            "{xml}"
        );
        assert!(
            xml.contains(">en </w:t>") && xml.contains("> brønd</w:t>"),
            "{xml}"
        );
    }

    #[test]
    fn spans_off_char_boundaries_are_ignored_rather_than_panicking() {
        let p = ReportParagraph {
            text: "æøå".into(),
            timestamp: None,
            highlight: vec![1..2],
        };
        assert!(paragraph_runs(&p).contains("æøå"));
    }
}
