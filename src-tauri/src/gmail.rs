use reqwest::StatusCode;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::{db_error, now_string, open_database};

const GMAIL_DRAFT_SCOPE: &str = "https://www.googleapis.com/auth/gmail.drafts.create";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct GmailConfig {
    client_id: String,
    client_secret: String,
    has_access_token: bool,
    has_refresh_token: bool,
    access_token_expires_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GmailTokenPayload {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GmailDraftInput {
    to: Option<String>,
    subject: String,
    body: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct GmailDraftResult {
    id: String,
    message_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GmailDraftResponse {
    id: String,
    message: Option<GmailMessageResponse>,
}

#[derive(Debug, Deserialize)]
struct GmailMessageResponse {
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleRefreshResponse {
    access_token: String,
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GoogleErrorResponse {
    error: Option<String>,
    error_description: Option<String>,
}

#[tauri::command]
pub(crate) fn get_gmail_config(app: AppHandle) -> Result<GmailConfig, String> {
    let connection = open_database(&app)?;
    load_gmail_config(&connection)
}

#[tauri::command]
pub(crate) fn save_gmail_config(
    app: AppHandle,
    config: GmailConfig,
) -> Result<GmailConfig, String> {
    let client_id = config.client_id.trim();
    let client_secret = config.client_secret.trim();
    if client_id.is_empty() || client_secret.is_empty() {
        return Err("Gmail client ID and client secret are required".to_string());
    }

    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    set_meta(&transaction, "gmail_client_id", client_id)?;
    set_meta(&transaction, "gmail_client_secret", client_secret)?;
    transaction.commit().map_err(db_error)?;
    load_gmail_config(&connection)
}

#[tauri::command]
pub(crate) fn save_gmail_tokens(
    app: AppHandle,
    tokens: GmailTokenPayload,
) -> Result<GmailConfig, String> {
    if tokens.access_token.trim().is_empty() {
        return Err("Google did not return an access token".to_string());
    }

    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    set_meta(
        &transaction,
        "gmail_access_token",
        tokens.access_token.trim(),
    )?;
    if let Some(refresh_token) = tokens
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        set_meta(&transaction, "gmail_refresh_token", refresh_token)?;
    }
    set_meta(
        &transaction,
        "gmail_access_token_expires_at",
        &tokens.expires_at.unwrap_or(0).to_string(),
    )?;
    transaction.commit().map_err(db_error)?;
    load_gmail_config(&connection)
}

#[tauri::command]
pub(crate) fn clear_gmail_auth(app: AppHandle) -> Result<GmailConfig, String> {
    let connection = open_database(&app)?;
    connection
        .execute(
            "
            DELETE FROM app_meta
            WHERE key IN (
                'gmail_access_token',
                'gmail_refresh_token',
                'gmail_access_token_expires_at'
            )
            ",
            [],
        )
        .map_err(db_error)?;
    load_gmail_config(&connection)
}

#[tauri::command]
pub(crate) async fn create_gmail_draft(
    app: AppHandle,
    draft: GmailDraftInput,
) -> Result<GmailDraftResult, String> {
    let access_token = valid_gmail_access_token(&app).await?;
    let subject = sanitize_header_value(&draft.subject);
    if subject.trim().is_empty() {
        return Err("Email subject is required".to_string());
    }

    let raw_message = build_mime_message(draft.to.as_deref(), &subject, &draft.body);
    let request_body = serde_json::json!({
        "message": {
            "raw": base64url_encode(raw_message.as_bytes())
        }
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post("https://gmail.googleapis.com/gmail/v1/users/me/drafts")
        .bearer_auth(access_token)
        .json(&request_body)
        .send()
        .await
        .map_err(|error| format!("Failed to create Gmail draft: {error}"))?;

    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "Gmail draft creation failed: {}",
            google_error_message(status, &body)
        ));
    }

    let draft = serde_json::from_str::<GmailDraftResponse>(&body)
        .map_err(|error| format!("Gmail returned an unexpected response: {error}"))?;
    Ok(GmailDraftResult {
        id: draft.id,
        message_id: draft.message.and_then(|message| message.id),
    })
}

fn load_gmail_config(connection: &Connection) -> Result<GmailConfig, String> {
    Ok(GmailConfig {
        client_id: meta_value(connection, "gmail_client_id")?.unwrap_or_default(),
        client_secret: meta_value(connection, "gmail_client_secret")?.unwrap_or_default(),
        has_access_token: meta_value(connection, "gmail_access_token")?
            .map(|value| !value.is_empty())
            .unwrap_or(false),
        has_refresh_token: meta_value(connection, "gmail_refresh_token")?
            .map(|value| !value.is_empty())
            .unwrap_or(false),
        access_token_expires_at: meta_value(connection, "gmail_access_token_expires_at")?
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0),
    })
}

async fn valid_gmail_access_token(app: &AppHandle) -> Result<String, String> {
    let connection = open_database(app)?;
    let access_token =
        meta_value(&connection, "gmail_access_token")?.filter(|value| !value.trim().is_empty());
    let expires_at = meta_value(&connection, "gmail_access_token_expires_at")?
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let now = current_unix_seconds();
    if let Some(access_token) = access_token {
        if expires_at == 0 || expires_at > now + 60 {
            return Ok(access_token);
        }
    }

    let client_id = meta_value(&connection, "gmail_client_id")?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Configure Gmail client ID in Settings".to_string())?;
    let client_secret = meta_value(&connection, "gmail_client_secret")?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Configure Gmail client secret in Settings".to_string())?;
    let refresh_token = meta_value(&connection, "gmail_refresh_token")?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Connect Gmail in Settings before creating drafts".to_string())?;
    drop(connection);

    refresh_gmail_access_token(app, &client_id, &client_secret, &refresh_token).await
}

async fn refresh_gmail_access_token(
    app: &AppHandle,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|error| format!("Failed to refresh Gmail access token: {error}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "Gmail token refresh failed: {}",
            google_error_message(status, &body)
        ));
    }

    let token = serde_json::from_str::<GoogleRefreshResponse>(&body)
        .map_err(|error| format!("Google returned an unexpected token response: {error}"))?;
    let expires_at = current_unix_seconds() + token.expires_in.unwrap_or(3600);
    let mut connection = open_database(app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    set_meta(&transaction, "gmail_access_token", &token.access_token)?;
    set_meta(
        &transaction,
        "gmail_access_token_expires_at",
        &expires_at.to_string(),
    )?;
    transaction.commit().map_err(db_error)?;
    Ok(token.access_token)
}

fn build_mime_message(to: Option<&str>, subject: &str, body: &str) -> String {
    let mut headers = Vec::new();
    if let Some(to) = to
        .map(sanitize_header_value)
        .filter(|value| !value.is_empty())
    {
        headers.push(format!("To: {to}"));
    }
    headers.push(format!("Subject: {}", sanitize_header_value(subject)));
    headers.push("MIME-Version: 1.0".to_string());
    headers.push("Content-Type: text/plain; charset=UTF-8".to_string());
    headers.push("Content-Transfer-Encoding: 8bit".to_string());
    headers.push(format!("X-Smooth-Scope: {GMAIL_DRAFT_SCOPE}"));
    headers.push(String::new());
    headers.push(normalize_body_line_endings(body));
    headers.join("\r\n")
}

fn sanitize_header_value(value: &str) -> String {
    value
        .replace(['\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_body_line_endings(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

fn current_unix_seconds() -> u64 {
    now_string().parse::<u64>().unwrap_or(0) / 1000
}

fn meta_value(connection: &Connection, key: &str) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT value FROM app_meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)
}

fn set_meta(transaction: &rusqlite::Transaction<'_>, key: &str, value: &str) -> Result<(), String> {
    transaction
        .execute(
            "
            INSERT INTO app_meta (key, value)
            VALUES (?1, ?2)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            ",
            params![key, value],
        )
        .map_err(db_error)?;
    Ok(())
}

fn google_error_message(status: StatusCode, body: &str) -> String {
    match serde_json::from_str::<GoogleErrorResponse>(body) {
        Ok(error) => error
            .error_description
            .or(error.error)
            .unwrap_or_else(|| format!("HTTP {}", status.as_u16())),
        Err(_) => format!("HTTP {}: {}", status.as_u16(), body.trim()),
    }
}

fn base64url_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::with_capacity((data.len() * 4).div_ceil(3));
    let mut index = 0;
    while index + 3 <= data.len() {
        let chunk =
            ((data[index] as u32) << 16) | ((data[index + 1] as u32) << 8) | data[index + 2] as u32;
        output.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
        output.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
        output.push(TABLE[((chunk >> 6) & 0x3f) as usize] as char);
        output.push(TABLE[(chunk & 0x3f) as usize] as char);
        index += 3;
    }

    match data.len() - index {
        1 => {
            let chunk = (data[index] as u32) << 16;
            output.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
            output.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
        }
        2 => {
            let chunk = ((data[index] as u32) << 16) | ((data[index + 1] as u32) << 8);
            output.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
            output.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
            output.push(TABLE[((chunk >> 6) & 0x3f) as usize] as char);
        }
        _ => {}
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_has_no_padding() {
        assert_eq!(base64url_encode(b"hello?"), "aGVsbG8_");
        assert_eq!(base64url_encode(b"a"), "YQ");
        assert_eq!(base64url_encode(b"ab"), "YWI");
    }

    #[test]
    fn mime_sanitizes_headers_and_preserves_body_lines() {
        let message = build_mime_message(
            Some("test@example.com\nBcc: bad@example.com"),
            "Hello\r\nInjected: nope",
            "Line 1\nLine 2",
        );

        assert!(message.contains("To: test@example.com Bcc: bad@example.com\r\n"));
        assert!(message.contains("Subject: Hello Injected: nope\r\n"));
        assert!(message.ends_with("Line 1\r\nLine 2"));
    }
}
