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

/// Today's date in Danish, for `{today}`.
pub fn danish_today() -> String {
    danish_date_of(&Local::now())
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
}
