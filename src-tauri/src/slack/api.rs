use reqwest::Client;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SlackMessage {
    pub(crate) user: Option<String>,
    pub(crate) bot_id: Option<String>,
    pub(crate) text: Option<String>,
    pub(crate) ts: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SocketUrlResponse {
    ok: bool,
    url: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RepliesResponse {
    ok: bool,
    messages: Option<Vec<SlackMessage>>,
    error: Option<String>,
    response_metadata: Option<ResponseMetadata>,
}

#[derive(Debug, Deserialize)]
struct ResponseMetadata {
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PostMessageResponse {
    ok: bool,
    channel: Option<String>,
    ts: Option<String>,
    error: Option<String>,
}

pub(crate) struct PostedMessage {
    pub(crate) channel: String,
    pub(crate) ts: String,
}

pub(crate) async fn open_socket(client: &Client, app_token: &str) -> Result<String, String> {
    let response = client
        .post("https://slack.com/api/apps.connections.open")
        .bearer_auth(app_token)
        .send()
        .await
        .map_err(|error| format!("Slack connection request failed: {error}"))?
        .json::<SocketUrlResponse>()
        .await
        .map_err(|error| format!("Slack returned an invalid connection response: {error}"))?;
    if !response.ok {
        return Err(format!(
            "Slack connection failed: {}",
            response.error.unwrap_or_else(|| "unknown error".into())
        ));
    }
    response
        .url
        .ok_or_else(|| "Slack did not return a WebSocket URL".to_string())
}

pub(crate) async fn fetch_thread(
    client: &Client,
    bot_token: &str,
    channel: &str,
    thread_ts: &str,
) -> Result<Vec<SlackMessage>, String> {
    let mut messages = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut request = client
            .get("https://slack.com/api/conversations.replies")
            .bearer_auth(bot_token)
            .query(&[("channel", channel), ("ts", thread_ts), ("limit", "100")]);
        if let Some(value) = cursor.as_deref() {
            request = request.query(&[("cursor", value)]);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("Could not read Slack thread: {error}"))?
            .json::<RepliesResponse>()
            .await
            .map_err(|error| format!("Slack returned an invalid thread response: {error}"))?;
        if !response.ok {
            return Err(format!(
                "Could not read Slack thread: {}",
                response.error.unwrap_or_else(|| "unknown error".into())
            ));
        }
        messages.extend(response.messages.unwrap_or_default());
        cursor = response
            .response_metadata
            .and_then(|metadata| metadata.next_cursor)
            .filter(|value| !value.is_empty());
        if cursor.is_none() || messages.len() >= 500 {
            break;
        }
    }
    Ok(messages)
}

pub(crate) async fn post_confirmation(
    client: &Client,
    bot_token: &str,
    channel: &str,
    thread_ts: &str,
    title: &str,
) -> Result<(), String> {
    post_message(
        client,
        bot_token,
        channel,
        Some(thread_ts),
        &format!("Saved to Smooth as *{}*.", title.replace('*', "")),
    )
    .await
    .map(|_| ())
}

pub(crate) async fn post_message(
    client: &Client,
    bot_token: &str,
    channel: &str,
    thread_ts: Option<&str>,
    text: &str,
) -> Result<PostedMessage, String> {
    let mut body = serde_json::json!({ "channel": channel, "text": text });
    if let Some(thread_ts) = thread_ts {
        body["thread_ts"] = serde_json::Value::String(thread_ts.to_string());
    }
    let response = client
        .post("https://slack.com/api/chat.postMessage")
        .bearer_auth(bot_token)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("Could not confirm in Slack: {error}"))?
        .json::<PostMessageResponse>()
        .await
        .map_err(|error| format!("Slack returned an invalid confirmation response: {error}"))?;
    if response.ok {
        Ok(PostedMessage {
            channel: response.channel.unwrap_or_else(|| channel.to_string()),
            ts: response.ts.unwrap_or_default(),
        })
    } else {
        Err(format!(
            "Could not confirm in Slack: {}",
            response.error.unwrap_or_else(|| "unknown error".into())
        ))
    }
}
