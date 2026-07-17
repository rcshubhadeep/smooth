use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::{app_meta_value, open_database, resolved_llama_config};

const LEGACY_INCEPTION_BASE_URL: &str = "https://api.inceptionlabs.ai";
const LEGACY_INCEPTION_MODEL: &str = "mercury-2";
pub(crate) const REMOTE_DEFAULT_CONTEXT_TOKENS: usize = 128_000;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LlmProvider {
    #[default]
    Local,
    #[serde(alias = "inception")]
    Remote,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct LlmSelection {
    #[serde(default)]
    pub(crate) provider: Option<LlmProvider>,
    #[serde(default)]
    pub(crate) model: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct LlmTarget {
    pub(crate) provider: LlmProvider,
    pub(crate) base_url: String,
    pub(crate) model: Option<String>,
    pub(crate) context_tokens: Option<usize>,
    api_key: Option<String>,
}

impl LlmTarget {
    pub(crate) fn provider_name(&self) -> &'static str {
        match self.provider {
            LlmProvider::Local => "local model server",
            LlmProvider::Remote => "remote OpenAI-compatible API",
        }
    }

    pub(crate) fn client_builder(&self) -> Result<reqwest::ClientBuilder, String> {
        let mut builder = reqwest::Client::builder();
        if let Some(api_key) = self.api_key.as_deref() {
            let mut headers = HeaderMap::new();
            let value = HeaderValue::from_str(&format!("Bearer {api_key}"))
                .map_err(|_| "The remote API key contains invalid characters".to_string())?;
            headers.insert(AUTHORIZATION, value);
            builder = builder.default_headers(headers);
        }
        Ok(builder)
    }
}

pub(crate) fn resolve_target(
    app: &AppHandle,
    selection: Option<&LlmSelection>,
) -> Result<LlmTarget, String> {
    let connection = open_database(app)?;
    let saved_default_provider = app_meta_value(&connection, "llm_default_provider")?;
    let legacy_inception_config = saved_default_provider.as_deref() == Some("inception");
    let default_provider = match saved_default_provider.as_deref() {
        Some("remote" | "inception") => LlmProvider::Remote,
        _ => LlmProvider::Local,
    };
    let always_obey_global = app_meta_value(&connection, "always_obey_global_llm")?
        .map(|value| value == "true")
        .unwrap_or(false);
    let effective_selection = if always_obey_global { None } else { selection };
    let provider = effective_selection
        .and_then(|value| value.provider)
        .unwrap_or(default_provider);
    let model_override = effective_selection
        .and_then(|value| value.model.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    match provider {
        LlmProvider::Local => {
            drop(connection);
            let config = resolved_llama_config(app)?;
            Ok(LlmTarget {
                provider,
                base_url: config.base_url,
                model: model_override.or(config.preferred_model),
                context_tokens: None,
                api_key: None,
            })
        }
        LlmProvider::Remote => {
            let base_url = app_meta_value(&connection, "remote_base_url")?
                .or(app_meta_value(&connection, "inception_base_url")?)
                .or_else(|| legacy_inception_config.then(|| LEGACY_INCEPTION_BASE_URL.to_string()))
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    "Configure a remote OpenAI-compatible API URL in Settings".to_string()
                })?;
            let model = model_override.or_else(|| {
                app_meta_value(&connection, "remote_model")
                    .ok()
                    .flatten()
                    .or_else(|| {
                        app_meta_value(&connection, "inception_model")
                            .ok()
                            .flatten()
                    })
                    .filter(|value| !value.trim().is_empty())
            });
            let api_key = std::env::var("OPENAI_API_KEY")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .or(std::env::var("INCEPTION_API_KEY")
                    .ok()
                    .filter(|value| !value.trim().is_empty()))
                .or(app_meta_value(&connection, "remote_api_key")?
                    .filter(|value| !value.trim().is_empty()))
                .or(app_meta_value(&connection, "inception_api_key")?
                    .filter(|value| !value.trim().is_empty()));
            let context_tokens = app_meta_value(&connection, "remote_context_tokens")?
                .and_then(|value| value.parse().ok())
                .filter(|value| *value >= 1024)
                .unwrap_or(REMOTE_DEFAULT_CONTEXT_TOKENS);
            Ok(LlmTarget {
                provider,
                base_url,
                model: Some(
                    model
                        .or_else(|| {
                            legacy_inception_config.then(|| LEGACY_INCEPTION_MODEL.to_string())
                        })
                        .ok_or_else(|| "Configure a remote model name in Settings".to_string())?,
                ),
                context_tokens: Some(context_tokens),
                api_key,
            })
        }
    }
}

pub(crate) fn is_mercury_model(model: &str) -> bool {
    model.eq_ignore_ascii_case(LEGACY_INCEPTION_MODEL)
}
