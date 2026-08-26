//! MCP (Model Context Protocol) server for the notes database.
//!
//! `handy --mcp-serve` turns the binary into a read-only stdio MCP server so
//! AI assistants (Claude Code, Codex, anything MCP-capable) can look up the
//! user's meeting notes and transcripts — including the live transcript of a
//! meeting that is still being written, since the recording app commits
//! nothing until it stops but the assistant can read every finished meeting.
//!
//! Protocol: newline-delimited JSON-RPC 2.0 over stdin/stdout, MCP revision
//! 2024-11-05, tools capability only. Everything is read-only: the database
//! is opened with SQLITE_OPEN_READ_ONLY, so a running app instance is never
//! disturbed. No logging goes to stdout — stdout belongs to the protocol.

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

const PROTOCOL_VERSION: &str = "2024-11-05";
const HISTORY_COLUMNS: &str =
    "id, title, timestamp, transcription_text, ai_notes, user_notes, source";

/// Mirrors `portable::app_data_dir` without needing a Tauri handle: portable
/// installs resolve next to the executable, everything else uses the platform
/// data dir with the app identifier from tauri.conf.json.
fn resolve_app_data_dir() -> PathBuf {
    if let Some(dir) = crate::portable::data_dir() {
        return dir.clone();
    }
    dirs::data_dir()
        .map(|d| d.join("com.pais.handy"))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn open_db() -> Result<Connection, String> {
    let db_path = resolve_app_data_dir().join("history.db");
    Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("cannot open notes database at {}: {e}", db_path.display()))
}

/// Reads the live snapshot the app's meeting ticker writes once a second
/// while a session is recording (`meeting::persist_live_snapshot`). The file
/// is removed when the session ends and cleaned up at app startup, so its
/// presence — with a fresh timestamp — means a meeting is happening now.
fn current_meeting_text() -> Result<String, String> {
    let path = resolve_app_data_dir().join("live_meeting.json");
    let raw =
        match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(_) => return Ok(
                "No meeting is being recorded right now. Use list_meetings for finished meetings."
                    .to_string(),
            ),
        };
    let snap: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();
    let updated = snap
        .get("updated_unix")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if now - updated > 30 {
        return Ok(
            "No meeting is live: a leftover snapshot was found but it is stale (the recording app              likely stopped unexpectedly). Use list_meetings for finished meetings."
                .to_string(),
        );
    }
    let started = snap
        .get("started_unix")
        .and_then(|v| v.as_i64())
        .unwrap_or(now);
    let mins = ((now - started).max(0)) / 60;
    let secs = ((now - started).max(0)) % 60;
    let streaming = snap
        .get("streaming")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let transcript = snap
        .get("transcript")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    let mut out = format!(
        "A meeting is being recorded RIGHT NOW (started {} — running for {mins}m {secs}s).
",
        iso_date(started)
    );
    if let Some(notes) = snap.get("notes").and_then(|v| v.as_str()) {
        if !notes.trim().is_empty() {
            out.push_str("\nNotes the user has taken so far:\n");
            out.push_str(notes.trim());
            out.push('\n');
        }
    }
    if transcript.is_empty() {
        if streaming {
            out.push_str(
                "
Nothing has been said yet (or the meeting just started).",
            );
        } else {
            out.push_str(
                "
The active transcription model does not stream live text, so the transcript                  will only be available once the meeting is stopped.",
            );
        }
    } else {
        out.push_str(
            "
Live transcript so far (newest words last, still growing):

",
        );
        out.push_str(transcript);
    }
    Ok(out)
}

/// The note body as the user sees it: their edited text, falling back to the
/// generated notes.
fn note_body(user_notes: Option<&str>, ai_notes: Option<&str>) -> String {
    match user_notes {
        Some(text) if !text.trim().is_empty() => text.to_string(),
        _ => ai_notes.unwrap_or_default().to_string(),
    }
}

fn iso_date(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|utc| {
            utc.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| timestamp.to_string())
}

struct NoteRow {
    id: i64,
    title: String,
    timestamp: i64,
    transcript: String,
    ai_notes: Option<String>,
    user_notes: Option<String>,
}

fn map_note_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteRow> {
    Ok(NoteRow {
        id: row.get("id")?,
        title: row.get("title")?,
        timestamp: row.get("timestamp")?,
        transcript: row.get("transcription_text")?,
        ai_notes: row.get("ai_notes")?,
        user_notes: row.get("user_notes")?,
    })
}

fn summary_line(row: &NoteRow) -> String {
    let body = note_body(row.user_notes.as_deref(), row.ai_notes.as_deref());
    let preview_source = if body.trim().is_empty() {
        &row.transcript
    } else {
        &body
    };
    let preview: String = preview_source
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect();
    format!(
        "#{} · {} · {} · notes: {}\n  {}",
        row.id,
        iso_date(row.timestamp),
        row.title,
        if body.trim().is_empty() { "no" } else { "yes" },
        preview
    )
}

fn full_note_text(row: &NoteRow) -> String {
    let body = note_body(row.user_notes.as_deref(), row.ai_notes.as_deref());
    let mut out = format!("# {} ({})\n", row.title, iso_date(row.timestamp));
    if body.trim().is_empty() {
        out.push_str("\n(No notes were written for this meeting yet.)\n");
    } else {
        out.push_str("\n## Notes\n");
        out.push_str(&body);
        out.push('\n');
    }
    if row.transcript.trim().is_empty() {
        out.push_str("\n## Transcript\n(empty)\n");
    } else {
        out.push_str("\n## Transcript\n");
        out.push_str(&row.transcript);
        out.push('\n');
    }
    out
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "get_current_meeting",
            "description": "Check whether a meeting is being recorded RIGHT NOW and read its live transcript so far. Always use this first when the user asks about the current or ongoing meeting; list_meetings only shows finished meetings.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "list_meetings",
            "description": "List the user's FINISHED meetings (newest first): id, date, title, whether notes exist, and a short preview. Use get_meeting with an id for the full content, and get_current_meeting for a meeting happening right now.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "description": "Maximum number of meetings to return (default 10, max 50)" }
                }
            }
        },
        {
            "name": "get_meeting",
            "description": "Get one meeting by id: its notes (the user's edited version when present) and the full transcript. The most recent meeting is the first result of list_meetings.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "Meeting id from list_meetings or search_meetings" }
                },
                "required": ["id"]
            }
        },
        {
            "name": "search_meetings",
            "description": "Full-text search across meeting titles, notes and transcripts. Returns matching meetings with previews; use get_meeting for full content.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Text to search for" },
                    "limit": { "type": "integer", "description": "Maximum results (default 10, max 50)" }
                },
                "required": ["query"]
            }
        }
    ])
}

fn clamp_limit(args: &Value) -> i64 {
    args.get("limit")
        .and_then(|v| v.as_i64())
        .unwrap_or(10)
        .clamp(1, 50)
}

fn run_tool(name: &str, args: &Value) -> Result<String, String> {
    if name == "get_current_meeting" {
        return current_meeting_text();
    }
    let conn = open_db()?;
    match name {
        "list_meetings" => {
            let limit = clamp_limit(args);
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {HISTORY_COLUMNS} FROM transcription_history
                     WHERE source = 'meeting' ORDER BY id DESC LIMIT ?1"
                ))
                .map_err(|e| e.to_string())?;
            let rows: Vec<NoteRow> = stmt
                .query_map([limit], map_note_row)
                .map_err(|e| e.to_string())?
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?;
            if rows.is_empty() {
                return Ok("No meetings recorded yet.".to_string());
            }
            Ok(rows.iter().map(summary_line).collect::<Vec<_>>().join("\n"))
        }
        "get_meeting" => {
            let id = args
                .get("id")
                .and_then(|v| v.as_i64())
                .ok_or("missing required argument: id")?;
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {HISTORY_COLUMNS} FROM transcription_history WHERE id = ?1"
                ))
                .map_err(|e| e.to_string())?;
            let row = stmt
                .query_row([id], map_note_row)
                .map_err(|_| format!("no meeting with id {id}"))?;
            Ok(full_note_text(&row))
        }
        "search_meetings" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or("missing required argument: query")?;
            let limit = clamp_limit(args);
            let pattern = format!(
                "%{}%",
                query
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            );
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {HISTORY_COLUMNS} FROM transcription_history
                     WHERE source = 'meeting' AND (
                        title LIKE ?1 ESCAPE '\\'
                        OR transcription_text LIKE ?1 ESCAPE '\\'
                        OR ai_notes LIKE ?1 ESCAPE '\\'
                        OR user_notes LIKE ?1 ESCAPE '\\')
                     ORDER BY id DESC LIMIT ?2"
                ))
                .map_err(|e| e.to_string())?;
            let rows: Vec<NoteRow> = stmt
                .query_map(rusqlite::params![pattern, limit], map_note_row)
                .map_err(|e| e.to_string())?
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?;
            if rows.is_empty() {
                return Ok(format!("No meetings match \"{query}\"."));
            }
            Ok(rows.iter().map(summary_line).collect::<Vec<_>>().join("\n"))
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

fn handle_request(request: &Value) -> Option<Value> {
    let method = request.get("method").and_then(|m| m.as_str())?;
    let id = request.get("id");
    // Notifications (no id) get no response.
    id?;
    let id = id.unwrap().clone();

    let result = match method {
        "initialize" => json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "noted", "version": env!("CARGO_PKG_VERSION") }
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tool_definitions() }),
        "tools/call" => {
            let empty = json!({});
            let params = request.get("params").unwrap_or(&empty);
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match run_tool(name, &args) {
                Ok(text) => json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                }),
                Err(err) => json!({
                    "content": [{ "type": "text", "text": err }],
                    "isError": true
                }),
            }
        }
        other => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {other}") }
            }));
        }
    };

    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// Blocking stdio serve loop. Returns the process exit code.
pub fn serve() -> i32 {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(err) => {
                eprintln!("noted-mcp: ignoring malformed request: {err}");
                continue;
            }
        };
        if let Some(response) = handle_request(&request) {
            let mut out = stdout.lock();
            if serde_json::to_writer(&mut out, &response).is_err() {
                break;
            }
            let _ = out.write_all(b"\n");
            let _ = out.flush();
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_body_prefers_user_edits() {
        assert_eq!(note_body(Some("mine"), Some("generated")), "mine");
        assert_eq!(note_body(Some("   "), Some("generated")), "generated");
        assert_eq!(note_body(None, Some("generated")), "generated");
        assert_eq!(note_body(None, None), "");
    }

    #[test]
    fn initialize_and_tools_list_respond() {
        let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} });
        let response = handle_request(&init).expect("initialize gets a response");
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);

        let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        let response = handle_request(&list).expect("tools/list gets a response");
        let tools = response["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 4);
    }

    #[test]
    fn notifications_get_no_response() {
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle_request(&note).is_none());
    }

    #[test]
    fn unknown_method_returns_error() {
        let request = json!({ "jsonrpc": "2.0", "id": 3, "method": "bogus" });
        let response = handle_request(&request).expect("gets an error response");
        assert_eq!(response["error"]["code"], -32601);
    }
}
