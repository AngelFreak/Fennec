use super::Report;

/// Plain UTF-8 text: heading, aligned fields, title, summary, paragraphs.
pub fn render_txt(report: &Report) -> String {
    let mut out = String::new();
    if let Some(title) = &report.collection_title {
        push_line(&mut out, title);
        if report.table_of_contents {
            out.push('\n');
            for (i, s) in report.sections.iter().enumerate() {
                push_line(&mut out, &format!("{}. {} ({})", i + 1, s.title, s.date));
            }
        }
        out.push('\n');
    }
    for (i, section) in report.sections.iter().enumerate() {
        if i > 0 || report.collection_title.is_some() {
            out.push_str("\n----------------------------------------\n\n");
        }
        if !report.heading.is_empty() {
            push_line(&mut out, &report.heading);
        }
        let width = section
            .fields
            .iter()
            .map(|(l, _)| l.chars().count())
            .max()
            .unwrap_or(0)
            + 2;
        for (label, value) in &section.fields {
            let label = format!("{label}:");
            push_line(&mut out, format!("{label:<width$}{value}").trim_end());
        }
        if !report.heading.is_empty() || !section.fields.is_empty() {
            out.push('\n');
        }
        push_line(&mut out, &section.title);
        out.push('\n');
        if let Some(summary) = &section.summary {
            push_line(&mut out, &report.summary_heading);
            summary_lines(&mut out, summary);
            out.push('\n');
        }
        for p in &section.paragraphs {
            match &p.timestamp {
                Some(ts) => push_line(&mut out, &format!("[{ts}] {}", p.text)),
                None => push_line(&mut out, &p.text),
            }
            out.push('\n');
        }
    }
    if !report.footer.is_empty() {
        push_line(&mut out, &report.footer);
    }
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    out.push('\n');
    out
}

/// The summary as plain text: headings on their own line after a blank
/// one, bullets as "- ", no `**` markers.
fn summary_lines(out: &mut String, summary: &str) {
    use crate::text::{Block, strip_bold, summary_blocks};
    for (i, block) in summary_blocks(summary).iter().enumerate() {
        match block {
            Block::Paragraph(t) => {
                if i > 0 {
                    out.push('\n');
                }
                push_line(out, &strip_bold(t));
            }
            Block::Heading(t) => {
                if i > 0 {
                    out.push('\n');
                }
                push_line(out, t);
            }
            Block::Bullet(t) => push_line(out, &format!("- {}", strip_bold(t))),
        }
    }
}

fn push_line(out: &mut String, line: &str) {
    out.push_str(line);
    out.push('\n');
}
