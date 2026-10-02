//! Small formatting helpers shared by the UI and exports.

use chrono::{DateTime, Datelike, Local, TimeZone};

const MONTHS: [&str; 12] = [
    "januar",
    "februar",
    "marts",
    "april",
    "maj",
    "juni",
    "juli",
    "august",
    "september",
    "oktober",
    "november",
    "december",
];

/// "2. oktober 2026" for a unix timestamp in milliseconds, in local time.
pub fn danish_date(unix_ms: i64) -> String {
    match Local.timestamp_millis_opt(unix_ms).single() {
        Some(dt) => danish_date_of(&dt),
        None => String::new(),
    }
}

fn danish_date_of<Tz: TimeZone>(dt: &DateTime<Tz>) -> String {
    format!("{}. {} {}", dt.day(), MONTHS[dt.month0() as usize], dt.year())
}

/// "2. oktober 2026" for a calendar date.
pub fn danish_day(d: chrono::NaiveDate) -> String {
    format!("{}. {} {}", d.day(), MONTHS[d.month0() as usize], d.year())
}

/// Today's date in Danish, for `{today}`.
pub fn danish_today() -> String {
    danish_date_of(&Local::now())
}

/// "Today" or "Sep 30": dates in the interface's lists, as in the mockup.
pub fn short_date(unix_ms: i64) -> String {
    match Local.timestamp_millis_opt(unix_ms).single() {
        Some(dt) => short_day(dt.date_naive(), Local::now().date_naive()),
        None => String::new(),
    }
}

fn short_day(d: chrono::NaiveDate, today: chrono::NaiveDate) -> String {
    if d == today {
        "Today".into()
    } else {
        d.format("%b %-d").to_string()
    }
}

/// "2 h 57 min", or "48 min" under an hour: totals of audio.
pub fn hours_minutes(ms: i64) -> String {
    let min = (ms.max(0) + 30_000) / 60_000;
    if min >= 60 {
        format!("{} h {} min", min / 60, min % 60)
    } else {
        format!("{min} min")
    }
}

/// "00:12:31" (hours always shown, as in the transcript gutter).
pub fn clock(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// "48:12" or "1:12:40" for lengths in lists.
pub fn duration(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// A piece of a summary as the AI writes it: plain paragraphs, `**bold**`
/// lines as headings, and `-` or `*` bullets.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph(String),
    Heading(String),
    Bullet(String),
}

pub fn summary_blocks(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let flush = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Paragraph(para.join("\n")));
            para.clear();
        }
    };
    for line in text.lines().map(str::trim_end) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            flush(&mut para, &mut out);
        } else if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            flush(&mut para, &mut out);
            out.push(Block::Bullet(item.trim().into()));
        } else if let Some(h) = trimmed
            .strip_prefix("**")
            .and_then(|t| t.strip_suffix("**"))
            .filter(|h| !h.is_empty() && !h.contains("**"))
        {
            flush(&mut para, &mut out);
            out.push(Block::Heading(h.trim().into()));
        } else {
            para.push(line);
        }
    }
    flush(&mut para, &mut out);
    out
}

/// The text with `**bold**` markers removed, for plain-text output.
pub fn strip_bold(text: &str) -> String {
    let parts: Vec<&str> = text.split("**").collect();
    if parts.len() < 3 {
        return text.to_string();
    }
    let pairs = (parts.len() - 1) / 2 * 2;
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > pairs {
            out.push_str("**");
        }
        out.push_str(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn danish_dates_use_ordinal_day_and_lowercase_month() {
        let dt = Utc.with_ymd_and_hms(2026, 10, 2, 12, 0, 0).unwrap();
        assert_eq!(danish_date_of(&dt), "2. oktober 2026");
    }

    #[test]
    fn clock_always_shows_hours() {
        assert_eq!(clock(751_000), "00:12:31");
        assert_eq!(clock(4_360_000), "01:12:40");
    }

    #[test]
    fn duration_drops_hours_when_short() {
        assert_eq!(duration(2_892_000), "48:12");
        assert_eq!(duration(4_360_000), "1:12:40");
    }
    #[test]
    fn summaries_render_paragraphs_headings_and_bullets() {
        let text = "Første afsnit\nfortsætter her.\n\n**Opfølgning**\n- Undersøg brønden.\n* Følg revnerne.";
        assert_eq!(
            summary_blocks(text),
            vec![
                Block::Paragraph("Første afsnit\nfortsætter her.".into()),
                Block::Heading("Opfølgning".into()),
                Block::Bullet("Undersøg brønden.".into()),
                Block::Bullet("Følg revnerne.".into()),
            ]
        );
    }

    #[test]
    fn bold_markers_are_dropped_for_plain_text() {
        assert_eq!(strip_bold("a **b** c"), "a b c");
        assert_eq!(strip_bold("one ** left"), "one ** left");
    }
    #[test]
    fn list_dates_are_short_as_in_the_mockup() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
        assert_eq!(short_day(today, today), "Today");
        assert_eq!(
            short_day(chrono::NaiveDate::from_ymd_opt(2026, 9, 30).unwrap(), today),
            "Sep 30"
        );
    }

    #[test]
    fn audio_totals_read_in_hours_and_minutes() {
        assert_eq!(hours_minutes(((2 * 60 + 56) * 60 + 35) * 1000), "2 h 57 min");
        assert_eq!(hours_minutes(48 * 60 * 1000), "48 min");
        assert_eq!(hours_minutes(20 * 1000), "0 min");
    }
}
