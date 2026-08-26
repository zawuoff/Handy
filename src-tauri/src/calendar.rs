//! Read-only view of the user's GNOME calendar (Evolution Data Server).
//!
//! Counterpart to the `cal-add` write path: upcoming events are fetched over
//! the same D-Bus service via `gdbus`, so the Home screen can show a
//! mockup-style "Coming up" card with a Record button. Linux/GNOME only —
//! other platforms simply return no events.

use serde::Serialize;
use specta::Type;

#[derive(Clone, Debug, Serialize, Type)]
pub struct CalendarEvent {
    pub summary: String,
    pub start_unix: i64,
    pub end_unix: i64,
    pub all_day: bool,
}

/// Upcoming events within the next `hours` hours, soonest first (max 5).
#[tauri::command]
#[specta::specta]
pub async fn get_upcoming_events(hours: Option<u32>) -> Result<Vec<CalendarEvent>, String> {
    let hours = hours.unwrap_or(48).min(24 * 14);
    tauri::async_runtime::spawn_blocking(move || fetch_upcoming(hours))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(not(target_os = "linux"))]
fn fetch_upcoming(_hours: u32) -> Result<Vec<CalendarEvent>, String> {
    Ok(Vec::new())
}

#[cfg(target_os = "linux")]
fn fetch_upcoming(hours: u32) -> Result<Vec<CalendarEvent>, String> {
    use std::process::Command;

    let dest = "org.gnome.evolution.dataserver.Calendar8";
    let open = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            dest,
            "--object-path",
            "/org/gnome/evolution/dataserver/CalendarFactory",
            "--method",
            "org.gnome.evolution.dataserver.CalendarFactory.OpenCalendar",
            "system-calendar",
        ])
        .output()
        .map_err(|e| format!("gdbus not available: {e}"))?;
    if !open.status.success() {
        return Err("could not open the system calendar".to_string());
    }
    let open_out = String::from_utf8_lossy(&open.stdout);
    let object_path = open_out
        .split('\'')
        .nth(1)
        .ok_or("unexpected OpenCalendar reply")?
        .to_string();

    let now = chrono::Utc::now();
    let until = now + chrono::Duration::hours(hours as i64);
    let sexp = format!(
        "(occur-in-time-range? (make-time \"{}\") (make-time \"{}\"))",
        now.format("%Y%m%dT%H%M%SZ"),
        until.format("%Y%m%dT%H%M%SZ"),
    );
    let list = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            dest,
            "--object-path",
            &object_path,
            "--method",
            "org.gnome.evolution.dataserver.Calendar.GetObjectList",
            &sexp,
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !list.status.success() {
        return Err("could not list calendar events".to_string());
    }

    let raw = String::from_utf8_lossy(&list.stdout);
    let now_unix = now.timestamp();
    let until_unix = until.timestamp();
    let mut events: Vec<CalendarEvent> = gvariant_strings(&raw)
        .iter()
        .filter_map(|ical| parse_vevent(ical))
        .filter(|event| event.end_unix > now_unix && event.start_unix < until_unix)
        .collect();
    events.sort_by_key(|event| event.start_unix);
    events.dedup_by(|a, b| a.summary == b.summary && a.start_unix == b.start_unix);
    events.truncate(5);
    Ok(events)
}

/// Extract the string items of a printed GVariant `([ '...', '...' ],)`,
/// undoing gdbus escaping.
fn gvariant_strings(printed: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut inside = false;
    let mut escape = false;
    for ch in printed.chars() {
        if !inside {
            if ch == '\'' {
                inside = true;
                current.clear();
            }
            continue;
        }
        if escape {
            match ch {
                'n' => current.push('\n'),
                'r' => current.push('\r'),
                't' => current.push('\t'),
                other => current.push(other),
            }
            escape = false;
            continue;
        }
        match ch {
            '\\' => escape = true,
            '\'' => {
                inside = false;
                items.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    items
}

/// Minimal VEVENT parse: SUMMARY plus DTSTART/DTEND in the common shapes
/// (floating local, UTC `Z`, `TZID=` treated as local, all-day `DATE`).
fn parse_vevent(ical: &str) -> Option<CalendarEvent> {
    let mut summary = None;
    let mut start = None;
    let mut end = None;
    let mut all_day = false;
    for line in ical.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(value) = line.strip_prefix("SUMMARY:") {
            summary = Some(
                value
                    .replace("\\,", ",")
                    .replace("\\;", ";")
                    .replace("\\\\", "\\"),
            );
        } else if line.starts_with("DTSTART") {
            let (ts, is_date) = parse_ical_time(line)?;
            start = Some(ts);
            all_day = is_date;
        } else if line.starts_with("DTEND") {
            if let Some((ts, _)) = parse_ical_time(line) {
                end = Some(ts);
            }
        }
    }
    let start = start?;
    Some(CalendarEvent {
        summary: summary.unwrap_or_default(),
        start_unix: start,
        end_unix: end.unwrap_or(start + 3600),
        all_day,
    })
}

fn parse_ical_time(line: &str) -> Option<(i64, bool)> {
    use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
    let value = line.split(':').nth(1)?.trim();
    if let Ok(dt) = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ") {
        return Some((Utc.from_utc_datetime(&dt).timestamp(), false));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S") {
        return Some((
            Local
                .from_local_datetime(&dt)
                .single()
                .unwrap_or_else(|| Local.from_utc_datetime(&dt))
                .timestamp(),
            false,
        ));
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y%m%d") {
        let dt = date.and_hms_opt(0, 0, 0)?;
        return Some((
            Local
                .from_local_datetime(&dt)
                .single()
                .unwrap_or_else(|| Local.from_utc_datetime(&dt))
                .timestamp(),
            true,
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gvariant_list_parses_with_escapes() {
        let printed = r"(['BEGIN:VEVENT\nSUMMARY:Team sync\nEND:VEVENT', 'BEGIN:VEVENT\nSUMMARY:One \'quoted\'\nEND:VEVENT'],)";
        let items = gvariant_strings(printed);
        assert_eq!(items.len(), 2);
        assert!(items[0].contains("SUMMARY:Team sync"));
        assert!(items[1].contains("One 'quoted'"));
    }

    #[test]
    fn vevent_parses_summary_and_times() {
        let ical = "BEGIN:VEVENT\nUID:x\nDTSTART:20260828T150000\nDTEND:20260828T160000\nSUMMARY:Quotation discussion\\, Pillr\nEND:VEVENT";
        let event = parse_vevent(ical).expect("parses");
        assert_eq!(event.summary, "Quotation discussion, Pillr");
        assert!(!event.all_day);
        assert_eq!(event.end_unix - event.start_unix, 3600);
    }

    #[test]
    fn all_day_and_tzid_forms_parse() {
        let event =
            parse_vevent("BEGIN:VEVENT\nDTSTART;VALUE=DATE:20260830\nSUMMARY:Birthday\nEND:VEVENT")
                .expect("parses");
        assert!(event.all_day);
        let event = parse_vevent(
            "BEGIN:VEVENT\nDTSTART;TZID=Europe/London:20260828T090000\nSUMMARY:Standup\nEND:VEVENT",
        )
        .expect("parses");
        assert!(!event.all_day);
    }
}
