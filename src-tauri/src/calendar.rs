use chrono::{DateTime, Duration, SecondsFormat, Utc};
use reqwest::StatusCode;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::{db_error, now_string, open_database};

const LOOKAHEAD_DAYS: i64 = 14;
const MAX_EVENTS_PER_CALENDAR: usize = 12;
const MAX_RETURNED_EVENTS: usize = 30;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CalendarConfig {
    client_id: String,
    client_secret: String,
    has_access_token: bool,
    has_refresh_token: bool,
    access_token_expires_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CalendarTokenPayload {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CalendarEvent {
    id: String,
    calendar_id: String,
    calendar_name: String,
    title: String,
    starts_at: String,
    ends_at: Option<String>,
    is_all_day: bool,
    location: Option<String>,
    html_link: Option<String>,
    video_link: Option<String>,
    attendee_count: usize,
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

#[derive(Debug, Deserialize)]
struct GoogleCalendarListResponse {
    items: Option<Vec<GoogleCalendarListEntry>>,
}

#[derive(Debug, Deserialize)]
struct GoogleCalendarListEntry {
    id: String,
    summary: Option<String>,
    hidden: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct GoogleEventsResponse {
    items: Option<Vec<GoogleEvent>>,
}

#[derive(Debug, Deserialize)]
struct GoogleEvent {
    id: Option<String>,
    summary: Option<String>,
    status: Option<String>,
    start: Option<GoogleEventTime>,
    end: Option<GoogleEventTime>,
    location: Option<String>,
    #[serde(rename = "htmlLink")]
    html_link: Option<String>,
    #[serde(rename = "hangoutLink")]
    hangout_link: Option<String>,
    #[serde(rename = "conferenceData")]
    conference_data: Option<GoogleConferenceData>,
    attendees: Option<Vec<GoogleAttendee>>,
}

#[derive(Debug, Deserialize)]
struct GoogleEventTime {
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleConferenceData {
    #[serde(rename = "entryPoints")]
    entry_points: Option<Vec<GoogleConferenceEntryPoint>>,
}

#[derive(Debug, Deserialize)]
struct GoogleConferenceEntryPoint {
    #[serde(rename = "entryPointType")]
    entry_point_type: Option<String>,
    uri: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleAttendee {
    #[allow(dead_code)]
    email: Option<String>,
}

#[tauri::command]
pub(crate) fn get_calendar_config(app: AppHandle) -> Result<CalendarConfig, String> {
    let connection = open_database(&app)?;
    load_calendar_config(&connection)
}

#[tauri::command]
pub(crate) fn save_calendar_tokens(
    app: AppHandle,
    tokens: CalendarTokenPayload,
) -> Result<CalendarConfig, String> {
    if tokens.access_token.trim().is_empty() {
        return Err("Google did not return a Calendar access token".to_string());
    }

    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    set_meta(
        &transaction,
        "calendar_access_token",
        tokens.access_token.trim(),
    )?;
    if let Some(refresh_token) = tokens
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        set_meta(&transaction, "calendar_refresh_token", refresh_token)?;
    }
    set_meta(
        &transaction,
        "calendar_access_token_expires_at",
        &tokens.expires_at.unwrap_or(0).to_string(),
    )?;
    transaction.commit().map_err(db_error)?;
    load_calendar_config(&connection)
}

#[tauri::command]
pub(crate) fn clear_calendar_auth(app: AppHandle) -> Result<CalendarConfig, String> {
    let connection = open_database(&app)?;
    connection
        .execute(
            "
            DELETE FROM app_meta
            WHERE key IN (
                'calendar_access_token',
                'calendar_refresh_token',
                'calendar_access_token_expires_at'
            )
            ",
            [],
        )
        .map_err(db_error)?;
    load_calendar_config(&connection)
}

#[tauri::command]
pub(crate) async fn list_upcoming_calendar_events(
    app: AppHandle,
) -> Result<Vec<CalendarEvent>, String> {
    let access_token = valid_calendar_access_token(&app).await?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(25))
        .build()
        .map_err(|error| error.to_string())?;

    let calendars = list_calendars(&client, &access_token).await?;
    let now = Utc::now();
    let time_min = now.to_rfc3339_opts(SecondsFormat::Secs, true);
    let time_max =
        (now + Duration::days(LOOKAHEAD_DAYS)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut events = Vec::new();

    for calendar in calendars {
        let calendar_events =
            list_calendar_events(&client, &access_token, &calendar, &time_min, &time_max).await?;
        events.extend(calendar_events);
    }

    events.sort_by(|left, right| left.starts_at.cmp(&right.starts_at));
    events.truncate(MAX_RETURNED_EVENTS);
    Ok(events)
}

fn load_calendar_config(connection: &Connection) -> Result<CalendarConfig, String> {
    Ok(CalendarConfig {
        client_id: meta_value(connection, "gmail_client_id")?.unwrap_or_default(),
        client_secret: meta_value(connection, "gmail_client_secret")?.unwrap_or_default(),
        has_access_token: meta_value(connection, "calendar_access_token")?
            .map(|value| !value.is_empty())
            .unwrap_or(false),
        has_refresh_token: meta_value(connection, "calendar_refresh_token")?
            .map(|value| !value.is_empty())
            .unwrap_or(false),
        access_token_expires_at: meta_value(connection, "calendar_access_token_expires_at")?
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0),
    })
}

async fn valid_calendar_access_token(app: &AppHandle) -> Result<String, String> {
    let connection = open_database(app)?;
    let access_token =
        meta_value(&connection, "calendar_access_token")?.filter(|value| !value.trim().is_empty());
    let expires_at = meta_value(&connection, "calendar_access_token_expires_at")?
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
        .ok_or_else(|| "Configure Google OAuth client ID in Settings".to_string())?;
    let client_secret = meta_value(&connection, "gmail_client_secret")?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Configure Google OAuth client secret in Settings".to_string())?;
    let refresh_token = meta_value(&connection, "calendar_refresh_token")?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Connect Calendar in Settings before loading meetings".to_string())?;
    drop(connection);

    refresh_calendar_access_token(app, &client_id, &client_secret, &refresh_token).await
}

async fn refresh_calendar_access_token(
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
        .map_err(|error| format!("Failed to refresh Calendar access token: {error}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "Calendar token refresh failed: {}",
            google_error_message(status, &body)
        ));
    }

    let token = serde_json::from_str::<GoogleRefreshResponse>(&body)
        .map_err(|error| format!("Google returned an unexpected token response: {error}"))?;
    let expires_at = current_unix_seconds() + token.expires_in.unwrap_or(3600);
    let mut connection = open_database(app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    set_meta(&transaction, "calendar_access_token", &token.access_token)?;
    set_meta(
        &transaction,
        "calendar_access_token_expires_at",
        &expires_at.to_string(),
    )?;
    transaction.commit().map_err(db_error)?;
    Ok(token.access_token)
}

async fn list_calendars(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<Vec<GoogleCalendarListEntry>, String> {
    let response = client
        .get("https://www.googleapis.com/calendar/v3/users/me/calendarList")
        .bearer_auth(access_token)
        .query(&[("minAccessRole", "reader")])
        .send()
        .await
        .map_err(|error| format!("Failed to load Calendar list: {error}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "Calendar list failed: {}",
            google_error_message(status, &body)
        ));
    }

    let list = serde_json::from_str::<GoogleCalendarListResponse>(&body)
        .map_err(|error| format!("Google returned an unexpected Calendar list: {error}"))?;
    Ok(list
        .items
        .unwrap_or_default()
        .into_iter()
        .filter(|calendar| !calendar.hidden.unwrap_or(false))
        .collect())
}

async fn list_calendar_events(
    client: &reqwest::Client,
    access_token: &str,
    calendar: &GoogleCalendarListEntry,
    time_min: &str,
    time_max: &str,
) -> Result<Vec<CalendarEvent>, String> {
    let url = format!(
        "https://www.googleapis.com/calendar/v3/calendars/{}/events",
        url_encode(&calendar.id)
    );
    let max_results = MAX_EVENTS_PER_CALENDAR.to_string();
    let response = client
        .get(url)
        .bearer_auth(access_token)
        .query(&[
            ("singleEvents", "true"),
            ("orderBy", "startTime"),
            ("showDeleted", "false"),
            ("timeMin", time_min),
            ("timeMax", time_max),
            ("maxResults", &max_results),
        ])
        .send()
        .await
        .map_err(|error| {
            format!(
                "Failed to load events for {}: {error}",
                calendar_name(calendar)
            )
        })?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "Calendar events failed for {}: {}",
            calendar_name(calendar),
            google_error_message(status, &body)
        ));
    }

    let parsed = serde_json::from_str::<GoogleEventsResponse>(&body).map_err(|error| {
        format!(
            "Google returned unexpected events for {}: {error}",
            calendar_name(calendar)
        )
    })?;
    Ok(parsed
        .items
        .unwrap_or_default()
        .into_iter()
        .filter_map(|event| convert_event(calendar, event))
        .collect())
}

fn convert_event(calendar: &GoogleCalendarListEntry, event: GoogleEvent) -> Option<CalendarEvent> {
    if event.status.as_deref() == Some("cancelled") {
        return None;
    }

    let start = event.start?;
    let end = event.end;
    let (starts_at, is_all_day) = google_event_time(start)?;
    let ends_at = end.and_then(google_event_time).map(|(value, _)| value);
    let video_link = event
        .hangout_link
        .or_else(|| conference_video_link(event.conference_data));

    Some(CalendarEvent {
        id: event
            .id
            .unwrap_or_else(|| format!("{}:{starts_at}", calendar.id)),
        calendar_id: calendar.id.clone(),
        calendar_name: calendar_name(calendar),
        title: event
            .summary
            .unwrap_or_else(|| "Untitled event".to_string()),
        starts_at,
        ends_at,
        is_all_day,
        location: event.location.filter(|value| !value.trim().is_empty()),
        html_link: event.html_link,
        video_link,
        attendee_count: event
            .attendees
            .map(|attendees| attendees.len())
            .unwrap_or(0),
    })
}

fn google_event_time(time: GoogleEventTime) -> Option<(String, bool)> {
    if let Some(date_time) = time.date_time {
        return Some((normalize_rfc3339(&date_time), false));
    }
    time.date.map(|date| (date, true))
}

fn conference_video_link(conference_data: Option<GoogleConferenceData>) -> Option<String> {
    conference_data?
        .entry_points?
        .into_iter()
        .find(|entry| entry.entry_point_type.as_deref() == Some("video"))
        .and_then(|entry| entry.uri)
}

fn normalize_rfc3339(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|date| {
            date.with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Secs, true)
        })
        .unwrap_or_else(|_| value.to_string())
}

fn calendar_name(calendar: &GoogleCalendarListEntry) -> String {
    calendar
        .summary
        .clone()
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or_else(|| calendar.id.clone())
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

fn url_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => {
                let encoded = format!("%{byte:02X}");
                encoded.chars().collect()
            }
        })
        .collect()
}
