//! Composio integration: sync meeting notes to Google Docs.
//!
//! Talks to Composio's REST API directly (no SDK). The user pastes their
//! Composio API key in Settings → Integrations, connects Google Docs once
//! via a browser OAuth flow (Composio-managed auth), and can then sync a
//! note — or all notes — to Google Docs. Each entry remembers its doc id
//! (`gdoc_id`) so re-syncing updates the same document.

use crate::managers::history::{HistoryEntry, HistoryManager};
use crate::settings::{get_settings, write_settings};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};

const BASE: &str = "https://backend.composio.dev/api/v3.1";
pub(crate) const TOOLKIT_GDOCS: &str = "googledocs";
pub(crate) const TOOLKIT_GMAIL: &str = "gmail";
// Tool slugs + argument names; verify against GET /tools/{slug} if a sync
// errors with an argument complaint.
const CREATE_TOOL: &str = "GOOGLEDOCS_CREATE_DOCUMENT_MARKDOWN";
const UPDATE_TOOL: &str = "GOOGLEDOCS_UPDATE_DOCUMENT_MARKDOWN";

static CLIENT: Lazy<reqwest::Client> = Lazy::new(reqwest::Client::new);

/// Entries currently syncing (single-flight; the user can double-click).
static SYNC_IN_FLIGHT: Lazy<Mutex<HashSet<i64>>> = Lazy::new(|| Mutex::new(HashSet::new()));
/// One sync-all at a time. No atomic DB claim needed: these commands are
/// awaited (the button disables while pending) and the output is idempotent
/// (the same doc id gets updated).
static SYNC_ALL_RUNNING: AtomicBool = AtomicBool::new(false);

async fn api_request(
    api_key: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let mut req = CLIENT
        .request(method, format!("{BASE}{path}"))
        .header("x-api-key", api_key);
    if let Some(body) = body {
        req = req.json(&body);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| crate::llm_client::report_reqwest_error("Composio request failed", &e))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let snippet: String = text.chars().take(300).collect();
        return Err(format!("Composio API error {status}: {snippet}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("Composio returned invalid JSON: {e}"))
}

/// Depth-first search for the first string value under any of `keys`.
/// Composio's response field names vary between API revisions (e.g.
/// `redirect_url` vs `redirectUrl`, nested under `connection_data`), so we
/// probe rather than hard-code one path.
fn find_string<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(Value::String(s)) = map.get(*key) {
                    return Some(s);
                }
            }
            map.values().find_map(|v| find_string(v, keys))
        }
        Value::Array(items) => items.iter().find_map(|v| find_string(v, keys)),
        _ => None,
    }
}

#[tauri::command]
#[specta::specta]
pub fn change_composio_api_key_setting(app: AppHandle, api_key: String) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.composio_api_key = api_key.trim().to_string().into();
    write_settings(&app, settings);
    Ok(())
}

/// (auth_config_id, account_id) stored for a toolkit; errors on unknown slugs
/// so a typo'd frontend call can't silently read the wrong fields.
fn get_toolkit_ids(
    settings: &crate::settings::AppSettings,
    toolkit: &str,
) -> Result<(String, String), String> {
    match toolkit {
        TOOLKIT_GDOCS => Ok((
            settings.composio_gdocs_auth_config_id.clone(),
            settings.composio_gdocs_account_id.clone(),
        )),
        TOOLKIT_GMAIL => Ok((
            settings.composio_gmail_auth_config_id.clone(),
            settings.composio_gmail_account_id.clone(),
        )),
        other => Err(format!("Unknown Composio toolkit: {other}")),
    }
}

fn store_toolkit_ids(app: &AppHandle, toolkit: &str, auth_config_id: &str, account_id: &str) {
    let mut settings = get_settings(app);
    match toolkit {
        TOOLKIT_GDOCS => {
            settings.composio_gdocs_auth_config_id = auth_config_id.to_string();
            settings.composio_gdocs_account_id = account_id.to_string();
        }
        TOOLKIT_GMAIL => {
            settings.composio_gmail_auth_config_id = auth_config_id.to_string();
            settings.composio_gmail_account_id = account_id.to_string();
        }
        _ => return,
    }
    write_settings(app, settings);
}

/// Start a toolkit's OAuth flow: create the auth config on first use, create
/// a connected account, and return the URL the user must open in a browser
/// to approve access. The frontend polls the status afterwards.
#[tauri::command]
#[specta::specta]
pub async fn connect_composio_toolkit(app: AppHandle, toolkit: String) -> Result<String, String> {
    let settings = get_settings(&app);
    let api_key = settings.composio_api_key.to_string();
    if api_key.is_empty() {
        return Err("Composio API key is not set".to_string());
    }

    let (mut auth_config_id, account_id) = get_toolkit_ids(&settings, &toolkit)?;
    if auth_config_id.is_empty() {
        let resp = api_request(
            &api_key,
            reqwest::Method::POST,
            "/auth_configs",
            Some(json!({
                "toolkit": { "slug": toolkit },
                "auth_config": { "type": "use_composio_managed_auth" },
            })),
        )
        .await?;
        auth_config_id = resp
            .get("auth_config")
            .and_then(|c| c.get("id"))
            .and_then(Value::as_str)
            .or_else(|| find_string(&resp, &["id"]))
            .ok_or("Composio auth config response had no id")?
            .to_string();
        store_toolkit_ids(&app, &toolkit, &auth_config_id, &account_id);
    }

    // Managed-OAuth connections must go through the link endpoint (the plain
    // /connected_accounts POST returns 400 for them). Verified live:
    // { auth_config_id, user_id } -> { redirect_url, connected_account_id }.
    let resp = api_request(
        &api_key,
        reqwest::Method::POST,
        "/connected_accounts/link",
        Some(json!({
            "auth_config_id": auth_config_id,
            "user_id": "default",
        })),
    )
    .await?;
    let account_id = resp
        .get("connected_account_id")
        .and_then(Value::as_str)
        .ok_or("Composio link response had no connected account id")?
        .to_string();
    let redirect_url = resp
        .get("redirect_url")
        .and_then(Value::as_str)
        .ok_or("Composio link response had no redirect URL")?
        .to_string();

    store_toolkit_ids(&app, &toolkit, &auth_config_id, &account_id);
    Ok(redirect_url)
}

/// Returns `not_connected` when no key/account is stored, otherwise the
/// account's status string from Composio (`ACTIVE` once OAuth completes).
#[tauri::command]
#[specta::specta]
pub async fn get_composio_connection_status(
    app: AppHandle,
    toolkit: String,
) -> Result<String, String> {
    let settings = get_settings(&app);
    let api_key = settings.composio_api_key.to_string();
    let (_, account_id) = get_toolkit_ids(&settings, &toolkit)?;
    if api_key.is_empty() || account_id.is_empty() {
        return Ok("not_connected".to_string());
    }
    let resp = api_request(
        &api_key,
        reqwest::Method::GET,
        &format!("/connected_accounts/{account_id}"),
        None,
    )
    .await?;
    Ok(resp
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN")
        .to_string())
}

/// The note body the user actually sees: their edited notes when present,
/// else the generated notes (same rule as the note view).
fn sync_body(entry: &HistoryEntry) -> Option<&str> {
    [entry.user_notes.as_deref(), entry.ai_notes.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|body| !body.is_empty())
}

async fn execute_tool(
    api_key: &str,
    account_id: &str,
    slug: &str,
    args: Value,
) -> Result<Value, String> {
    let resp = api_request(
        api_key,
        reqwest::Method::POST,
        &format!("/tools/execute/{slug}"),
        Some(json!({
            "connected_account_id": account_id,
            "user_id": "default",
            "arguments": args,
        })),
    )
    .await?;
    if resp.get("successful").and_then(Value::as_bool) != Some(true) {
        let error = resp
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        let snippet: String = error.chars().take(300).collect();
        return Err(format!("{slug} failed: {snippet}"));
    }
    Ok(resp)
}

/// Create or update the Google Doc for one entry. Callers hold the
/// SYNC_IN_FLIGHT guard.
async fn sync_entry(
    hm: &HistoryManager,
    api_key: &str,
    account_id: &str,
    entry: &HistoryEntry,
) -> Result<(), String> {
    let body = sync_body(entry).ok_or("This note has no content to sync yet")?;
    match &entry.gdoc_id {
        Some(doc_id) => {
            execute_tool(
                api_key,
                account_id,
                UPDATE_TOOL,
                json!({ "id": doc_id, "markdown": body }),
            )
            .await?;
        }
        None => {
            let resp = execute_tool(
                api_key,
                account_id,
                CREATE_TOOL,
                json!({ "title": entry.title, "markdown_text": body }),
            )
            .await?;
            let doc_id = resp
                .get("data")
                .and_then(|d| find_string(d, &["documentId", "document_id", "id"]))
                .ok_or("Google Docs create response had no document id")?
                .to_string();
            hm.set_gdoc_id(entry.id, doc_id)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// (api_key, account_id) for a connected toolkit; Err when not connected.
pub(crate) fn toolkit_credentials(
    app: &AppHandle,
    toolkit: &str,
) -> Result<(String, String), String> {
    let settings = get_settings(app);
    let api_key = settings.composio_api_key.to_string();
    let (_, account_id) = get_toolkit_ids(&settings, toolkit)?;
    if api_key.is_empty() || account_id.is_empty() {
        return Err(format!(
            "{toolkit} is not connected (see Settings → Integrations)"
        ));
    }
    Ok((api_key, account_id))
}

fn gdocs_credentials(app: &AppHandle) -> Result<(String, String), String> {
    toolkit_credentials(app, TOOLKIT_GDOCS)
}

/// The authenticated Gmail account's own address — the placeholder recipient
/// when no actual email address is known.
pub(crate) async fn gmail_self_address(api_key: &str, account_id: &str) -> Result<String, String> {
    let resp = execute_tool(
        api_key,
        account_id,
        "GMAIL_GET_PROFILE",
        json!({ "user_id": "me" }),
    )
    .await?;
    resp.get("data")
        .and_then(|d| find_string(d, &["emailAddress", "email_address", "email"]))
        .map(str::to_string)
        .ok_or_else(|| "Gmail profile had no email address".to_string())
}

/// Create a Gmail draft. Drafts never send on their own — the user reviews
/// and sends from Gmail — which is what makes auto-drafting safe.
pub(crate) async fn create_gmail_draft(
    api_key: &str,
    account_id: &str,
    recipient: &str,
    subject: &str,
    body: &str,
) -> Result<(), String> {
    execute_tool(
        api_key,
        account_id,
        "GMAIL_CREATE_EMAIL_DRAFT",
        json!({
            "user_id": "me",
            "recipient_email": recipient,
            "subject": subject,
            "body": body,
            "is_html": false,
        }),
    )
    .await
    .map(|_| ())
}

/// Actually send an email through the user's Gmail. Only the task key calls
/// this, and only when the user explicitly said to send.
pub(crate) async fn send_gmail_email(
    api_key: &str,
    account_id: &str,
    recipient: &str,
    subject: &str,
    body: &str,
) -> Result<(), String> {
    execute_tool(
        api_key,
        account_id,
        "GMAIL_SEND_EMAIL",
        json!({
            "user_id": "me",
            "recipient_email": recipient,
            "subject": subject,
            "body": body,
            "is_html": false,
        }),
    )
    .await
    .map(|_| ())
}

#[tauri::command]
#[specta::specta]
pub fn change_gmail_contacts_setting(
    app: AppHandle,
    contacts: std::collections::HashMap<String, String>,
) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.gmail_contacts = contacts
        .into_iter()
        .map(|(name, email)| (name.trim().to_string(), email.trim().to_string()))
        .filter(|(name, email)| !name.is_empty() && !email.is_empty())
        .collect();
    write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_gmail_signature_name_setting(app: AppHandle, name: String) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.gmail_signature_name = name.trim().to_string();
    write_settings(&app, settings);
    Ok(())
}

/// Resolve the recipient for a drafted email: a spoken address wins,
/// otherwise the draft is addressed to the user themself as a placeholder.
pub(crate) async fn resolve_draft_recipient(
    api_key: &str,
    account_id: &str,
    hint: Option<&str>,
) -> Result<String, String> {
    match hint.map(str::trim).filter(|h| h.contains('@')) {
        Some(address) => Ok(address.to_string()),
        None => gmail_self_address(api_key, account_id).await,
    }
}

/// "Draft email" button on a note: a Gmail draft with the note body (and the
/// Google Doc link when one exists), addressed to the user to fill in.
#[tauri::command]
#[specta::specta]
pub async fn draft_note_email(app: AppHandle, id: i64) -> Result<(), String> {
    let (api_key, account_id) = toolkit_credentials(&app, TOOLKIT_GMAIL)?;
    let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());
    let entry = hm
        .get_entry_by_id(id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("History entry {id} not found"))?;
    let mut body = sync_body(&entry)
        .ok_or("This note has no content yet")?
        .to_string();
    if let Some(doc_id) = &entry.gdoc_id {
        body.push_str(&format!(
            "\n\nGoogle Doc: https://docs.google.com/document/d/{doc_id}"
        ));
    }
    let recipient = gmail_self_address(&api_key, &account_id).await?;
    create_gmail_draft(&api_key, &account_id, &recipient, &entry.title, &body).await
}

#[tauri::command]
#[specta::specta]
pub async fn sync_note_to_gdocs(app: AppHandle, id: i64) -> Result<(), String> {
    let (api_key, account_id) = gdocs_credentials(&app)?;
    if !SYNC_IN_FLIGHT.lock().unwrap().insert(id) {
        return Err("This note is already syncing".to_string());
    }
    let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());
    let result = match hm.get_entry_by_id(id).await {
        Ok(Some(entry)) => sync_entry(&hm, &api_key, &account_id, &entry).await,
        Ok(None) => Err(format!("History entry {id} not found")),
        Err(e) => Err(e.to_string()),
    };
    SYNC_IN_FLIGHT.lock().unwrap().remove(&id);
    result
}

#[derive(serde::Serialize, specta::Type)]
pub struct GdocsSyncSummary {
    pub synced: u32,
    pub failed: u32,
}

/// Sync every meeting note that has content, sequentially. Entries without
/// a body yet (no notes generated) are skipped and not counted as failures.
#[tauri::command]
#[specta::specta]
pub async fn sync_all_notes_to_gdocs(app: AppHandle) -> Result<GdocsSyncSummary, String> {
    let (api_key, account_id) = gdocs_credentials(&app)?;
    if SYNC_ALL_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("A sync of all notes is already running".to_string());
    }

    let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());
    let result = async {
        let entries = hm
            .get_meeting_entries(None, None)
            .await
            .map_err(|e| e.to_string())?
            .entries;
        let mut summary = GdocsSyncSummary {
            synced: 0,
            failed: 0,
        };
        for entry in &entries {
            if sync_body(entry).is_none() {
                continue;
            }
            if !SYNC_IN_FLIGHT.lock().unwrap().insert(entry.id) {
                summary.failed += 1;
                continue;
            }
            let synced = sync_entry(&hm, &api_key, &account_id, entry).await;
            SYNC_IN_FLIGHT.lock().unwrap().remove(&entry.id);
            match synced {
                Ok(()) => summary.synced += 1,
                Err(e) => {
                    log::error!("Google Docs sync failed for entry {}: {e}", entry.id);
                    summary.failed += 1;
                }
            }
        }
        Ok(summary)
    }
    .await;
    SYNC_ALL_RUNNING.store(false, Ordering::SeqCst);
    result
}
