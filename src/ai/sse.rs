//! Server-sent events, read line by line from a blocking response.

use std::io::BufRead;

#[derive(Debug, Default, PartialEq)]
pub struct Event {
    pub event: String,
    pub data: String,
}

/// Calls `on_event` for each complete event. Stops early when it returns
/// `false`. Comment lines and unknown fields are ignored.
pub fn read_events(reader: impl BufRead, mut on_event: impl FnMut(Event) -> bool) -> std::io::Result<()> {
    let mut current = Event::default();
    let mut has_data = false;
    for line in reader.lines() {
        let line = line?;
        let line = line.strip_suffix('\r').unwrap_or(&line);
        if line.is_empty() {
            if has_data && !on_event(std::mem::take(&mut current)) {
                return Ok(());
            }
            current = Event::default();
            has_data = false;
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => current.event = value.to_string(),
            "data" => {
                if has_data {
                    current.data.push('\n');
                }
                current.data.push_str(value);
                has_data = true;
            }
            _ => {}
        }
    }
    if has_data {
        on_event(current);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(text: &str) -> Vec<Event> {
        let mut out = Vec::new();
        read_events(text.as_bytes(), |e| {
            out.push(e);
            true
        })
        .unwrap();
        out
    }

    #[test]
    fn events_split_on_blank_lines() {
        let got = collect("event: a\ndata: {\"x\":1}\n\n: ping\n\ndata: [DONE]\n\n");
        assert_eq!(
            got,
            vec![
                Event {
                    event: "a".into(),
                    data: "{\"x\":1}".into()
                },
                Event {
                    event: String::new(),
                    data: "[DONE]".into()
                },
            ]
        );
    }

    #[test]
    fn multi_line_data_and_crlf_are_joined() {
        let got = collect("data: one\r\ndata: two\r\n\r\n");
        assert_eq!(got[0].data, "one\ntwo");
    }

    #[test]
    fn a_final_event_without_blank_line_is_kept() {
        assert_eq!(collect("data: last").len(), 1);
    }
}
