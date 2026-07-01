use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    net::IpAddr,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

#[derive(Clone, Debug, Default, Deserialize)]
struct LegacyStore {
    notes: Vec<NoteMeta>,
    folders: Vec<Folder>,
    links: Vec<NoteLink>,
}

#[derive(Clone, Debug, Deserialize)]
struct NoteMeta {
    id: String,
    title: String,
    folder_id: Option<String>,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
    #[serde(default = "default_extraction_status")]
    extraction_status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Folder {
    id: String,
    name: String,
    created_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct NoteLink {
    source_id: String,
    target_id: String,
    created_at: String,
}

#[derive(Clone, Debug, Serialize)]
struct NoteListItem {
    id: String,
    title: String,
    folder_id: Option<String>,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
    excerpt: String,
    extraction_status: String,
}

#[derive(Clone, Debug, Serialize)]
struct NoteWithContent {
    id: String,
    title: String,
    folder_id: Option<String>,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
    content: String,
    extraction_status: String,
}

#[derive(Clone, Debug, Serialize)]
struct BankSnapshot {
    notes: Vec<NoteListItem>,
    folders: Vec<Folder>,
    links: Vec<NoteLink>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LlamaConfig {
    base_url: String,
    preferred_model: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct LlamaModel {
    id: String,
    owned_by: Option<String>,
    context_size: Option<u64>,
    parameter_count: Option<u64>,
    size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum LlamaConnectionState {
    Offline,
    Loading,
    Ready,
    Error,
}

#[derive(Clone, Debug, Serialize)]
struct LlamaStatus {
    state: LlamaConnectionState,
    base_url: String,
    message: String,
    latency_ms: Option<u64>,
    checked_at: String,
    models: Vec<LlamaModel>,
}

#[derive(Debug, Deserialize)]
struct LlamaModelsResponse {
    #[serde(default)]
    data: Vec<LlamaModelResponse>,
}

#[derive(Debug, Deserialize)]
struct LlamaModelResponse {
    id: String,
    owned_by: Option<String>,
    meta: Option<LlamaModelMeta>,
}

#[derive(Debug, Deserialize)]
struct LlamaModelMeta {
    n_ctx: Option<u64>,
    n_ctx_train: Option<u64>,
    n_params: Option<u64>,
    size: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
struct ExtractionQueueStatus {
    pending: u64,
    processing: u64,
    failed: u64,
    indexed: u64,
    not_indexed: u64,
}

#[derive(Clone, Debug)]
struct ExtractionJob {
    id: i64,
    note_id: String,
    content_hash: String,
    attempts: u32,
    max_attempts: u32,
    lease_token: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ExtractedEntities {
    #[serde(default)]
    entities: Vec<ExtractedEntity>,
}

#[derive(Clone, Debug, Deserialize)]
struct ExtractedEntity {
    name: String,
    entity_type: String,
    surface_text: String,
    context: Option<String>,
    confidence: Option<f64>,
    #[serde(default)]
    aliases: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
}

#[derive(Clone, Debug, Deserialize)]
struct ChatCompletionChoice {
    finish_reason: Option<String>,
    message: ChatCompletionMessage,
}

#[derive(Clone, Debug, Deserialize)]
struct ChatCompletionMessage {
    content: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct InputTokensResponse {
    input_tokens: usize,
}

#[derive(Clone, Debug, Serialize)]
struct NoteEntity {
    id: i64,
    name: String,
    entity_type: String,
    mention_count: u64,
}

#[derive(Clone, Debug, Serialize)]
struct NoteExtractionView {
    status: String,
    error: Option<String>,
    entities: Vec<NoteEntity>,
}

#[derive(Clone, Debug, Serialize)]
struct LinkSuggestion {
    note: NoteListItem,
    shared_entities: Vec<NoteEntity>,
    shared_entity_count: u64,
    shared_mention_count: u64,
}

#[derive(Debug)]
enum ChunkExtractionError {
    Split(String),
    Retry(String),
}

const EXTRACTION_INPUT_TOKEN_BUDGET: usize = 1400;
const EXTRACTION_MAX_OUTPUT_TOKENS: usize = 900;
const EXTRACTION_INITIAL_CHARS: usize = 2200;
const EXTRACTION_MIN_SPLIT_CHARS: usize = 300;
const EXTRACTION_POLL_SECONDS: u64 = 2;

const ENTITY_TYPES: [&str; 12] = [
    "PERSON",
    "ORGANIZATION",
    "LOCATION",
    "EVENT",
    "DATE",
    "TIME",
    "DATETIME",
    "PRODUCT",
    "PROJECT",
    "TECHNOLOGY",
    "CONCEPT",
    "OTHER",
];

fn default_extraction_status() -> String {
    "not_indexed".to_string()
}

fn now_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

fn new_id(prefix: &str) -> String {
    format!("{}-{}", prefix, now_string())
}

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(dir.join("notes")).map_err(|error| error.to_string())?;
    Ok(dir)
}

fn legacy_store_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("store.json"))
}

fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("smooth.db"))
}

fn note_path(app: &AppHandle, note_id: &str) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?
        .join("notes")
        .join(format!("{note_id}.md")))
}

fn open_database(app: &AppHandle) -> Result<Connection, String> {
    let mut connection = Connection::open(database_path(app)?).map_err(db_error)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(db_error)?;
    connection
        .execute_batch(
            "
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;

            CREATE TABLE IF NOT EXISTS app_meta (
                key TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS folders (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS notes (
                id TEXT PRIMARY KEY NOT NULL,
                title TEXT NOT NULL,
                folder_id TEXT REFERENCES folders(id) ON DELETE SET NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                deleted_at TEXT,
                content_hash TEXT,
                indexed_content_hash TEXT,
                extraction_status TEXT NOT NULL DEFAULT 'not_indexed',
                extraction_error TEXT
            );

            CREATE INDEX IF NOT EXISTS notes_folder_id_idx ON notes(folder_id);
            CREATE INDEX IF NOT EXISTS notes_updated_at_idx ON notes(updated_at);
            CREATE INDEX IF NOT EXISTS notes_deleted_at_idx ON notes(deleted_at);

            CREATE TABLE IF NOT EXISTS note_links (
                source_id TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
                target_id TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
                created_at TEXT NOT NULL,
                PRIMARY KEY (source_id, target_id),
                CHECK (source_id < target_id)
            );

            CREATE INDEX IF NOT EXISTS note_links_target_id_idx ON note_links(target_id);

            CREATE TABLE IF NOT EXISTS entities (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                canonical_name TEXT NOT NULL,
                normalized_name TEXT NOT NULL,
                entity_type TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE (normalized_name, entity_type)
            );

            CREATE TABLE IF NOT EXISTS entity_aliases (
                entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
                alias TEXT NOT NULL,
                normalized_alias TEXT NOT NULL,
                PRIMARY KEY (entity_id, normalized_alias)
            );

            CREATE INDEX IF NOT EXISTS entity_aliases_normalized_alias_idx
                ON entity_aliases(normalized_alias);

            CREATE TABLE IF NOT EXISTS entity_mentions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                note_id TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
                entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
                surface_text TEXT NOT NULL,
                context TEXT,
                chunk_index INTEGER NOT NULL,
                confidence REAL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS entity_mentions_note_id_idx
                ON entity_mentions(note_id);
            CREATE INDEX IF NOT EXISTS entity_mentions_entity_id_idx
                ON entity_mentions(entity_id);

            CREATE TABLE IF NOT EXISTS extraction_jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                note_id TEXT NOT NULL UNIQUE REFERENCES notes(id) ON DELETE CASCADE,
                content_hash TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'processing', 'failed')),
                priority INTEGER NOT NULL DEFAULT 0,
                attempts INTEGER NOT NULL DEFAULT 0,
                max_attempts INTEGER NOT NULL DEFAULT 3,
                available_at TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                started_at TEXT,
                lease_token TEXT,
                last_error TEXT
            );

            CREATE INDEX IF NOT EXISTS extraction_jobs_claim_idx
                ON extraction_jobs(status, available_at, priority, created_at);
            ",
        )
        .map_err(db_error)?;

    migrate_legacy_store(app, &mut connection)?;
    Ok(connection)
}

fn migrate_legacy_store(app: &AppHandle, connection: &mut Connection) -> Result<(), String> {
    let migration_complete = connection
        .query_row(
            "SELECT value FROM app_meta WHERE key = 'legacy_store_imported'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?
        .is_some();

    if migration_complete {
        return Ok(());
    }

    let path = legacy_store_path(app)?;
    let legacy_store = if path.exists() {
        let contents = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        serde_json::from_str::<LegacyStore>(&contents).map_err(|error| error.to_string())?
    } else {
        LegacyStore::default()
    };

    let folder_ids = legacy_store
        .folders
        .iter()
        .map(|folder| folder.id.clone())
        .collect::<HashSet<_>>();
    let note_ids = legacy_store
        .notes
        .iter()
        .map(|note| note.id.clone())
        .collect::<HashSet<_>>();

    let transaction = connection.transaction().map_err(db_error)?;

    for folder in legacy_store.folders {
        transaction
            .execute(
                "
                INSERT OR IGNORE INTO folders (id, name, created_at)
                VALUES (?1, ?2, ?3)
                ",
                params![folder.id, folder.name, folder.created_at],
            )
            .map_err(db_error)?;
    }

    for note in legacy_store.notes {
        let folder_id = note
            .folder_id
            .filter(|folder_id| folder_ids.contains(folder_id));
        transaction
            .execute(
                "
                INSERT OR IGNORE INTO notes (
                    id, title, folder_id, created_at, updated_at, deleted_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ",
                params![
                    note.id,
                    note.title,
                    folder_id,
                    note.created_at,
                    note.updated_at,
                    note.deleted_at
                ],
            )
            .map_err(db_error)?;
    }

    for link in legacy_store.links {
        let Some((source_id, target_id)) = normalize_pair(&link.source_id, &link.target_id) else {
            continue;
        };
        if !note_ids.contains(&source_id) || !note_ids.contains(&target_id) {
            continue;
        }

        transaction
            .execute(
                "
                INSERT OR IGNORE INTO note_links (source_id, target_id, created_at)
                VALUES (?1, ?2, ?3)
                ",
                params![source_id, target_id, link.created_at],
            )
            .map_err(db_error)?;
    }

    transaction
        .execute(
            "
            INSERT INTO app_meta (key, value)
            VALUES ('legacy_store_imported', ?1)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            ",
            params![now_string()],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "
            INSERT INTO app_meta (key, value)
            VALUES ('schema_version', '1')
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            ",
            [],
        )
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)
}

fn db_error(error: rusqlite::Error) -> String {
    error.to_string()
}

fn content_hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

fn recover_interrupted_extraction_jobs(connection: &Connection) -> Result<(), String> {
    let now = now_string();
    connection
        .execute(
            "
            UPDATE extraction_jobs
            SET status = 'pending',
                attempts = CASE
                    WHEN max_attempts > 0 AND attempts >= max_attempts THEN max_attempts - 1
                    WHEN attempts > 0 THEN attempts - 1
                    ELSE 0
                END,
                available_at = ?1,
                updated_at = ?1,
                started_at = NULL,
                lease_token = NULL,
                last_error = CASE
                    WHEN last_error IS NULL OR last_error = ''
                    THEN 'Interrupted by application shutdown'
                    ELSE last_error
                END
            WHERE status = 'processing'
            ",
            params![now],
        )
        .map_err(db_error)?;
    connection
        .execute(
            "
            UPDATE notes
            SET extraction_status = 'queued'
            WHERE extraction_status = 'processing'
              AND id IN (SELECT note_id FROM extraction_jobs WHERE status = 'pending')
            ",
            [],
        )
        .map_err(db_error)?;
    Ok(())
}

fn enqueue_extraction(
    transaction: &rusqlite::Transaction<'_>,
    note_id: &str,
    hash: &str,
) -> Result<(), String> {
    let indexed_hash = transaction
        .query_row(
            "SELECT indexed_content_hash FROM notes WHERE id = ?1",
            params![note_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(db_error)?
        .flatten();

    if indexed_hash.as_deref() == Some(hash) {
        transaction
            .execute(
                "DELETE FROM extraction_jobs WHERE note_id = ?1",
                params![note_id],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "
                UPDATE notes
                SET content_hash = ?1,
                    extraction_status = 'indexed',
                    extraction_error = NULL
                WHERE id = ?2
                ",
                params![hash, note_id],
            )
            .map_err(db_error)?;
        return Ok(());
    }

    let now = now_string();
    transaction
        .execute(
            "
            INSERT INTO extraction_jobs (
                note_id, content_hash, status, priority, attempts, max_attempts,
                available_at, created_at, updated_at, started_at, lease_token, last_error
            )
            VALUES (?1, ?2, 'pending', 0, 0, 3, ?3, ?3, ?3, NULL, NULL, NULL)
            ON CONFLICT(note_id) DO UPDATE SET
                content_hash = excluded.content_hash,
                status = 'pending',
                attempts = 0,
                available_at = excluded.available_at,
                updated_at = excluded.updated_at,
                started_at = NULL,
                lease_token = NULL,
                last_error = NULL
            ",
            params![note_id, hash, now],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "
            UPDATE notes
            SET content_hash = ?1,
                extraction_status = 'queued',
                extraction_error = NULL
            WHERE id = ?2
            ",
            params![hash, note_id],
        )
        .map_err(db_error)?;
    Ok(())
}

fn force_enqueue_extraction(
    transaction: &rusqlite::Transaction<'_>,
    note_id: &str,
    hash: &str,
) -> Result<(), String> {
    let now = now_string();
    transaction
        .execute(
            "
            INSERT INTO extraction_jobs (
                note_id, content_hash, status, priority, attempts, max_attempts,
                available_at, created_at, updated_at, started_at, lease_token, last_error
            )
            VALUES (?1, ?2, 'pending', 0, 0, 3, ?3, ?3, ?3, NULL, NULL, NULL)
            ON CONFLICT(note_id) DO UPDATE SET
                content_hash = excluded.content_hash,
                status = 'pending',
                attempts = 0,
                available_at = excluded.available_at,
                updated_at = excluded.updated_at,
                started_at = NULL,
                lease_token = NULL,
                last_error = NULL
            ",
            params![note_id, hash, now],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "
            UPDATE notes
            SET content_hash = ?1,
                extraction_status = 'queued',
                extraction_error = NULL
            WHERE id = ?2
            ",
            params![hash, note_id],
        )
        .map_err(db_error)?;
    Ok(())
}

fn clear_extraction_job(
    transaction: &rusqlite::Transaction<'_>,
    note_id: &str,
) -> Result<(), String> {
    transaction
        .execute(
            "DELETE FROM extraction_jobs WHERE note_id = ?1",
            params![note_id],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "
            UPDATE notes
            SET content_hash = NULL,
                extraction_status = 'not_indexed',
                extraction_error = NULL
            WHERE id = ?1
            ",
            params![note_id],
        )
        .map_err(db_error)?;
    Ok(())
}

fn extraction_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["entities"],
        "properties": {
            "entities": {
                "type": "array",
                "maxItems": 30,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": [
                        "name",
                        "entity_type",
                        "surface_text"
                    ],
                    "properties": {
                        "name": { "type": "string", "minLength": 1, "maxLength": 160 },
                        "entity_type": { "type": "string", "enum": ENTITY_TYPES },
                        "surface_text": { "type": "string", "minLength": 1, "maxLength": 240 }
                    }
                }
            }
        }
    })
}

fn extraction_messages(title: &str, chunk: &str) -> serde_json::Value {
    json!([
        {
            "role": "system",
            "content": concat!(
                "Extract important named entities and durable knowledge keywords from the note. ",
                "Use only these entity types: PERSON, ORGANIZATION, LOCATION, EVENT, DATE, TIME, ",
                "DATETIME, PRODUCT, PROJECT, TECHNOLOGY, CONCEPT, OTHER. ",
                "Canonicalize names conservatively. Do not merge people or organizations merely ",
                "because their names are similar. Include meaningful concepts, but omit generic ",
                "words and incidental nouns. surface_text must be text appearing in the input. ",
                "Return only the schema-constrained JSON."
            )
        },
        {
            "role": "user",
            "content": format!("Note title: {title}\n\nNote chunk:\n{chunk}")
        }
    ])
}

fn normalize_entity_name(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character.is_whitespace() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn split_text_near_middle(value: &str) -> Option<(String, String)> {
    let characters = value.chars().collect::<Vec<_>>();
    if characters.len() < 2 {
        return None;
    }

    let midpoint = characters.len() / 2;
    let split_at = (midpoint..characters.len())
        .find(|index| characters[*index] == '\n' || characters[*index].is_whitespace())
        .or_else(|| {
            (1..midpoint)
                .rev()
                .find(|index| characters[*index] == '\n' || characters[*index].is_whitespace())
        })
        .unwrap_or(midpoint);
    let first = characters[..split_at]
        .iter()
        .collect::<String>()
        .trim()
        .to_string();
    let second = characters[split_at..]
        .iter()
        .collect::<String>()
        .trim()
        .to_string();
    if first.is_empty() || second.is_empty() {
        None
    } else {
        Some((first, second))
    }
}

fn initial_note_chunks(content: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for paragraph in content
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
    {
        if paragraph.chars().count() > EXTRACTION_INITIAL_CHARS {
            if !current.is_empty() {
                chunks.push(current.trim().to_string());
                current.clear();
            }
            let mut pending = VecDeque::from([paragraph.to_string()]);
            while let Some(part) = pending.pop_front() {
                if part.chars().count() <= EXTRACTION_INITIAL_CHARS {
                    chunks.push(part);
                } else if let Some((first, second)) = split_text_near_middle(&part) {
                    pending.push_front(second);
                    pending.push_front(first);
                } else {
                    chunks.push(part);
                }
            }
            continue;
        }

        let separator = if current.is_empty() { 0 } else { 2 };
        if current.chars().count() + separator + paragraph.chars().count()
            > EXTRACTION_INITIAL_CHARS
            && !current.is_empty()
        {
            chunks.push(current.trim().to_string());
            current.clear();
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(paragraph);
    }

    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }
    chunks
}

async fn count_extraction_input_tokens(
    client: &reqwest::Client,
    config: &LlamaConfig,
    model: &str,
    title: &str,
    chunk: &str,
) -> Option<usize> {
    let response = client
        .post(llama_endpoint(
            &config.base_url,
            "/v1/chat/completions/input_tokens",
        ))
        .json(&json!({
            "model": model,
            "messages": extraction_messages(title, chunk)
        }))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response
        .json::<InputTokensResponse>()
        .await
        .ok()
        .map(|result| result.input_tokens)
}

async fn fit_chunks_to_context(
    client: &reqwest::Client,
    config: &LlamaConfig,
    model: &str,
    title: &str,
    content: &str,
) -> Result<Vec<String>, String> {
    let mut pending = VecDeque::from(initial_note_chunks(content));
    let mut chunks = Vec::new();

    while let Some(chunk) = pending.pop_front() {
        let token_count = count_extraction_input_tokens(client, config, model, title, &chunk).await;
        let fits = token_count
            .map(|count| count <= EXTRACTION_INPUT_TOKEN_BUDGET)
            .unwrap_or_else(|| chunk.chars().count() <= EXTRACTION_INITIAL_CHARS);
        if fits {
            chunks.push(chunk);
            continue;
        }

        if chunk.chars().count() <= EXTRACTION_MIN_SPLIT_CHARS {
            return Err(format!(
                "A note segment cannot fit within the {} token extraction budget",
                EXTRACTION_INPUT_TOKEN_BUDGET
            ));
        }
        let Some((first, second)) = split_text_near_middle(&chunk) else {
            return Err("Unable to split an oversized note segment".to_string());
        };
        pending.push_front(second);
        pending.push_front(first);
    }

    Ok(chunks)
}

fn parse_extracted_entities(content: &str) -> Result<ExtractedEntities, String> {
    if let Ok(parsed) = serde_json::from_str::<ExtractedEntities>(content) {
        return Ok(parsed);
    }

    let start = content
        .find('{')
        .ok_or_else(|| "llama.cpp returned no JSON object".to_string())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "llama.cpp returned incomplete JSON".to_string())?;
    serde_json::from_str::<ExtractedEntities>(&content[start..=end])
        .map_err(|error| format!("Invalid extraction JSON: {error}"))
}

fn validate_extracted_entity(mut entity: ExtractedEntity) -> Option<ExtractedEntity> {
    entity.name = truncate_chars(entity.name.trim(), 160);
    entity.entity_type = entity.entity_type.trim().to_uppercase();
    entity.surface_text = truncate_chars(entity.surface_text.trim(), 240);
    entity.context = entity
        .context
        .map(|value| truncate_chars(value.trim(), 500))
        .filter(|value| !value.is_empty());
    entity.confidence = entity.confidence.map(|value| value.clamp(0.0, 1.0));
    entity.aliases = entity
        .aliases
        .into_iter()
        .map(|alias| truncate_chars(alias.trim(), 160))
        .filter(|alias| !alias.is_empty())
        .collect();

    if entity.name.is_empty()
        || entity.surface_text.is_empty()
        || !ENTITY_TYPES.contains(&entity.entity_type.as_str())
        || normalize_entity_name(&entity.name).is_empty()
    {
        None
    } else {
        Some(entity)
    }
}

async fn extract_chunk_entities(
    client: &reqwest::Client,
    config: &LlamaConfig,
    model: &str,
    title: &str,
    chunk: &str,
) -> Result<Vec<ExtractedEntity>, ChunkExtractionError> {
    let response = client
        .post(llama_endpoint(&config.base_url, "/v1/chat/completions"))
        .json(&json!({
            "model": model,
            "messages": extraction_messages(title, chunk),
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "note_entities",
                    "strict": true,
                    "schema": extraction_schema()
                }
            },
            "temperature": 0.1,
            "top_p": 0.9,
            "max_tokens": EXTRACTION_MAX_OUTPUT_TOKENS,
            "stream": false,
            "reasoning_format": "none",
            "chat_template_kwargs": {
                "enable_thinking": false
            }
        }))
        .send()
        .await
        .map_err(|error| {
            ChunkExtractionError::Retry(format!("llama.cpp request failed: {error}"))
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let message = format!(
            "llama.cpp returned HTTP {}: {}",
            status.as_u16(),
            truncate_chars(body.trim(), 300)
        );
        return if status == reqwest::StatusCode::BAD_REQUEST
            || status == reqwest::StatusCode::PAYLOAD_TOO_LARGE
        {
            Err(ChunkExtractionError::Split(message))
        } else {
            Err(ChunkExtractionError::Retry(message))
        };
    }

    let response = response
        .json::<ChatCompletionResponse>()
        .await
        .map_err(|error| {
            ChunkExtractionError::Retry(format!("Invalid llama.cpp response: {error}"))
        })?;
    let choice = response.choices.first().ok_or_else(|| {
        ChunkExtractionError::Retry("llama.cpp returned no completion choice".to_string())
    })?;
    if choice.finish_reason.as_deref() == Some("length") {
        return Err(ChunkExtractionError::Split(
            "Entity JSON exceeded the output token limit".to_string(),
        ));
    }
    let content = choice.message.content.as_deref().ok_or_else(|| {
        ChunkExtractionError::Split("llama.cpp returned no extraction content".to_string())
    })?;
    Ok(parse_extracted_entities(content)
        .map_err(ChunkExtractionError::Split)?
        .entities
        .into_iter()
        .filter_map(validate_extracted_entity)
        .collect())
}

async fn extract_entities_adaptively(
    client: &reqwest::Client,
    config: &LlamaConfig,
    model: &str,
    title: &str,
    chunk: String,
) -> Result<Vec<ExtractedEntity>, String> {
    let mut pending = VecDeque::from([chunk]);
    let mut entities = Vec::new();

    while let Some(part) = pending.pop_front() {
        match extract_chunk_entities(client, config, model, title, &part).await {
            Ok(extracted) => entities.extend(extracted),
            Err(ChunkExtractionError::Retry(error)) => return Err(error),
            Err(ChunkExtractionError::Split(error)) => {
                if part.chars().count() <= EXTRACTION_MIN_SPLIT_CHARS {
                    return Err(format!(
                        "{error}; the failing segment is already at the minimum chunk size"
                    ));
                }
                let Some((first, second)) = split_text_near_middle(&part) else {
                    return Err(format!("{error}; unable to split the failing segment"));
                };
                pending.push_front(second);
                pending.push_front(first);
            }
        }
    }

    Ok(entities)
}

fn app_meta_value(connection: &Connection, key: &str) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT value FROM app_meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)
}

fn load_llama_config(connection: &Connection) -> Result<LlamaConfig, String> {
    Ok(LlamaConfig {
        base_url: app_meta_value(connection, "llama_base_url")?
            .unwrap_or_else(|| "http://127.0.0.1:8080".to_string()),
        preferred_model: app_meta_value(connection, "llama_preferred_model")?
            .filter(|value| !value.is_empty()),
    })
}

fn validate_llama_base_url(value: &str) -> Result<String, String> {
    let mut url = reqwest::Url::parse(value.trim())
        .map_err(|_| "Enter a valid llama.cpp server URL".to_string())?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err("The llama.cpp URL must use http or https".to_string());
    }

    let host = url
        .host_str()
        .ok_or_else(|| "The llama.cpp URL must include a host".to_string())?;
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .map(|address| address.is_loopback())
            .unwrap_or(false);
    if !is_loopback {
        return Err("Only localhost llama.cpp servers are allowed".to_string());
    }

    if url.query().is_some() || url.fragment().is_some() {
        return Err("The llama.cpp URL cannot include a query or fragment".to_string());
    }

    let normalized_path = url.path().trim_end_matches('/').to_string();
    url.set_path(if normalized_path.is_empty() {
        "/"
    } else {
        &normalized_path
    });
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn llama_endpoint(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn llama_status(
    config: &LlamaConfig,
    state: LlamaConnectionState,
    message: impl Into<String>,
    latency_ms: Option<u64>,
    models: Vec<LlamaModel>,
) -> LlamaStatus {
    LlamaStatus {
        state,
        base_url: config.base_url.clone(),
        message: message.into(),
        latency_ms,
        checked_at: now_string(),
        models,
    }
}

async fn llama_server_ready(client: &reqwest::Client, config: &LlamaConfig) -> bool {
    client
        .get(llama_endpoint(&config.base_url, "/health"))
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

async fn resolve_extraction_model(
    client: &reqwest::Client,
    config: &LlamaConfig,
) -> Result<String, String> {
    if let Some(model) = config.preferred_model.as_ref() {
        return Ok(model.clone());
    }

    let response = client
        .get(llama_endpoint(&config.base_url, "/v1/models"))
        .send()
        .await
        .map_err(|error| format!("Unable to discover llama.cpp models: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Model discovery returned HTTP {}",
            response.status().as_u16()
        ));
    }
    response
        .json::<LlamaModelsResponse>()
        .await
        .map_err(|error| format!("Invalid model discovery response: {error}"))?
        .data
        .into_iter()
        .next()
        .map(|model| model.id)
        .ok_or_else(|| "llama.cpp has no available model".to_string())
}

fn claim_extraction_job(app: &AppHandle) -> Result<Option<ExtractionJob>, String> {
    let mut connection = open_database(app)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let now = now_string();
    let job = transaction
        .query_row(
            "
            SELECT id, note_id, content_hash, attempts, max_attempts
            FROM extraction_jobs
            WHERE status = 'pending' AND available_at <= ?1
            ORDER BY priority DESC, created_at ASC
            LIMIT 1
            ",
            params![now],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, u32>(4)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?;
    let Some((id, note_id, content_hash, attempts, max_attempts)) = job else {
        transaction.commit().map_err(db_error)?;
        return Ok(None);
    };

    let lease_token = format!("{}-{id}", new_id("lease"));
    let changed = transaction
        .execute(
            "
            UPDATE extraction_jobs
            SET status = 'processing',
                attempts = attempts + 1,
                updated_at = ?1,
                started_at = ?1,
                lease_token = ?2,
                last_error = NULL
            WHERE id = ?3 AND status = 'pending'
            ",
            params![now, lease_token, id],
        )
        .map_err(db_error)?;
    if changed == 0 {
        transaction.commit().map_err(db_error)?;
        return Ok(None);
    }
    transaction
        .execute(
            "
            UPDATE notes
            SET extraction_status = 'processing', extraction_error = NULL
            WHERE id = ?1
            ",
            params![note_id],
        )
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)?;

    Ok(Some(ExtractionJob {
        id,
        note_id,
        content_hash,
        attempts: attempts + 1,
        max_attempts,
        lease_token,
    }))
}

fn extraction_job_is_current(app: &AppHandle, job: &ExtractionJob) -> Result<bool, String> {
    let connection = open_database(app)?;
    connection
        .query_row(
            "
            SELECT COUNT(*)
            FROM extraction_jobs
            WHERE id = ?1
              AND note_id = ?2
              AND content_hash = ?3
              AND status = 'processing'
              AND lease_token = ?4
            ",
            params![job.id, job.note_id, job.content_hash, job.lease_token],
            |row| row.get::<_, u64>(0),
        )
        .map(|count| count == 1)
        .map_err(db_error)
}

fn requeue_claimed_job_with_content(
    app: &AppHandle,
    job: &ExtractionJob,
    hash: &str,
) -> Result<(), String> {
    let mut connection = open_database(app)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    if extraction_job_matches(&transaction, job)? {
        enqueue_extraction(&transaction, &job.note_id, hash)?;
    }
    transaction.commit().map_err(db_error)
}

fn cancel_claimed_job(app: &AppHandle, job: &ExtractionJob) -> Result<(), String> {
    let mut connection = open_database(app)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    if extraction_job_matches(&transaction, job)? {
        clear_extraction_job(&transaction, &job.note_id)?;
    }
    transaction.commit().map_err(db_error)
}

fn extraction_job_matches(
    transaction: &rusqlite::Transaction<'_>,
    job: &ExtractionJob,
) -> Result<bool, String> {
    transaction
        .query_row(
            "
            SELECT COUNT(*)
            FROM extraction_jobs
            WHERE id = ?1
              AND note_id = ?2
              AND content_hash = ?3
              AND status = 'processing'
              AND lease_token = ?4
            ",
            params![job.id, job.note_id, job.content_hash, job.lease_token],
            |row| row.get::<_, u64>(0),
        )
        .map(|count| count == 1)
        .map_err(db_error)
}

fn persist_extraction_results(
    app: &AppHandle,
    job: &ExtractionJob,
    entities: Vec<(usize, ExtractedEntity)>,
) -> Result<bool, String> {
    let mut connection = open_database(app)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    if !extraction_job_matches(&transaction, job)? {
        transaction.commit().map_err(db_error)?;
        return Ok(false);
    }

    let note_state = transaction
        .query_row(
            "SELECT content_hash, deleted_at FROM notes WHERE id = ?1",
            params![job.note_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?;
    if !matches!(
        note_state,
        Some((Some(ref hash), None)) if hash == &job.content_hash
    ) {
        transaction.commit().map_err(db_error)?;
        return Ok(false);
    }

    transaction
        .execute(
            "DELETE FROM entity_mentions WHERE note_id = ?1",
            params![job.note_id],
        )
        .map_err(db_error)?;

    let now = now_string();
    let mut entity_ids = HashMap::<(String, String), i64>::new();
    let mut mention_keys = HashSet::new();
    for (chunk_index, entity) in entities {
        let normalized_name = normalize_entity_name(&entity.name);
        let entity_key = (normalized_name.clone(), entity.entity_type.clone());
        let entity_id = if let Some(id) = entity_ids.get(&entity_key) {
            *id
        } else {
            transaction
                .execute(
                    "
                    INSERT INTO entities (
                        canonical_name, normalized_name, entity_type, created_at, updated_at
                    )
                    VALUES (?1, ?2, ?3, ?4, ?4)
                    ON CONFLICT(normalized_name, entity_type) DO UPDATE SET
                        updated_at = excluded.updated_at
                    ",
                    params![entity.name, normalized_name, entity.entity_type, now],
                )
                .map_err(db_error)?;
            let id = transaction
                .query_row(
                    "
                    SELECT id FROM entities
                    WHERE normalized_name = ?1 AND entity_type = ?2
                    ",
                    params![entity_key.0, entity_key.1],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(db_error)?;
            entity_ids.insert(entity_key.clone(), id);
            id
        };

        for alias in
            std::iter::once(entity.name.as_str()).chain(entity.aliases.iter().map(String::as_str))
        {
            let normalized_alias = normalize_entity_name(alias);
            if normalized_alias.is_empty() {
                continue;
            }
            transaction
                .execute(
                    "
                    INSERT OR IGNORE INTO entity_aliases (
                        entity_id, alias, normalized_alias
                    )
                    VALUES (?1, ?2, ?3)
                    ",
                    params![entity_id, alias, normalized_alias],
                )
                .map_err(db_error)?;
        }

        let context = entity.context.as_deref().unwrap_or("");
        let mention_key = format!(
            "{}|{}|{}|{}",
            entity_key.0,
            entity_key.1,
            normalize_entity_name(&entity.surface_text),
            normalize_entity_name(context)
        );
        if !mention_keys.insert(mention_key) {
            continue;
        }
        transaction
            .execute(
                "
                INSERT INTO entity_mentions (
                    note_id, entity_id, surface_text, context, chunk_index,
                    confidence, created_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ",
                params![
                    job.note_id,
                    entity_id,
                    entity.surface_text,
                    entity.context,
                    chunk_index as i64,
                    entity.confidence,
                    now
                ],
            )
            .map_err(db_error)?;
    }

    transaction
        .execute(
            "
            UPDATE notes
            SET indexed_content_hash = ?1,
                extraction_status = 'indexed',
                extraction_error = NULL
            WHERE id = ?2
            ",
            params![job.content_hash, job.note_id],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "DELETE FROM extraction_jobs WHERE id = ?1 AND lease_token = ?2",
            params![job.id, job.lease_token],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "
            DELETE FROM entities
            WHERE id NOT IN (SELECT DISTINCT entity_id FROM entity_mentions)
            ",
            [],
        )
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)?;
    Ok(true)
}

fn fail_extraction_job(app: &AppHandle, job: &ExtractionJob, error: &str) -> Result<(), String> {
    let mut connection = open_database(app)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    if !extraction_job_matches(&transaction, job)? {
        transaction.commit().map_err(db_error)?;
        return Ok(());
    }

    let error = truncate_chars(error, 1000);
    let now_ms = now_string().parse::<u128>().unwrap_or_default();
    let retry_seconds =
        5_u128.saturating_mul(2_u128.saturating_pow(job.attempts.saturating_sub(1)));
    let available_at = (now_ms + retry_seconds.min(300) * 1000).to_string();
    let exhausted = job.attempts >= job.max_attempts;
    let status = if exhausted { "failed" } else { "pending" };
    let note_status = if exhausted { "failed" } else { "queued" };
    transaction
        .execute(
            "
            UPDATE extraction_jobs
            SET status = ?1,
                available_at = ?2,
                updated_at = ?3,
                started_at = NULL,
                lease_token = NULL,
                last_error = ?4
            WHERE id = ?5
            ",
            params![status, available_at, now_string(), error, job.id],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "
            UPDATE notes
            SET extraction_status = ?1, extraction_error = ?2
            WHERE id = ?3
            ",
            params![note_status, error, job.note_id],
        )
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)
}

async fn process_extraction_job(
    app: &AppHandle,
    client: &reqwest::Client,
    config: &LlamaConfig,
    model: &str,
    job: &ExtractionJob,
) -> Result<(), String> {
    let connection = open_database(app)?;
    let note = load_note_meta(&connection, &job.note_id)?;
    drop(connection);
    if note.deleted_at.is_some() {
        cancel_claimed_job(app, job)?;
        return Ok(());
    }

    let content = read_note_content(app, &job.note_id)?;
    let current_hash = content_hash(&content);
    if current_hash != job.content_hash {
        requeue_claimed_job_with_content(app, job, &current_hash)?;
        return Ok(());
    }
    if content.trim().is_empty() {
        cancel_claimed_job(app, job)?;
        return Ok(());
    }

    let chunks = fit_chunks_to_context(client, config, model, &note.title, &content).await?;
    let mut all_entities = Vec::new();
    for (chunk_index, chunk) in chunks.iter().enumerate() {
        if !extraction_job_is_current(app, job)? {
            return Ok(());
        }
        let entities =
            extract_entities_adaptively(client, config, model, &note.title, chunk.clone()).await?;
        all_entities.extend(entities.into_iter().map(|entity| (chunk_index, entity)));
    }
    persist_extraction_results(app, job, all_entities)?;
    Ok(())
}

async fn extraction_worker(app: AppHandle) {
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(180))
        .build()
    {
        Ok(client) => client,
        Err(_) => return,
    };

    loop {
        let config = match open_database(&app).and_then(|connection| load_llama_config(&connection))
        {
            Ok(config) => config,
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(EXTRACTION_POLL_SECONDS)).await;
                continue;
            }
        };
        if !llama_server_ready(&client, &config).await {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        let model = match resolve_extraction_model(&client, &config).await {
            Ok(model) => model,
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let job = match claim_extraction_job(&app) {
            Ok(Some(job)) => job,
            Ok(None) => {
                tokio::time::sleep(Duration::from_secs(EXTRACTION_POLL_SECONDS)).await;
                continue;
            }
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(EXTRACTION_POLL_SECONDS)).await;
                continue;
            }
        };

        if let Err(error) = process_extraction_job(&app, &client, &config, &model, &job).await {
            let _ = fail_extraction_job(&app, &job, &error);
        }
    }
}

fn read_note_content(app: &AppHandle, note_id: &str) -> Result<String, String> {
    let path = note_path(app, note_id)?;
    if !path.exists() {
        return Ok(String::new());
    }

    fs::read_to_string(path).map_err(|error| error.to_string())
}

fn write_note_content(app: &AppHandle, note_id: &str, content: &str) -> Result<(), String> {
    fs::write(note_path(app, note_id)?, content).map_err(|error| error.to_string())
}

fn infer_title(content: &str) -> String {
    content
        .lines()
        .map(|line| line.trim().trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(80).collect())
        .unwrap_or_else(|| "Untitled note".to_string())
}

fn excerpt(content: &str) -> String {
    content
        .lines()
        .map(|line| line.trim().trim_start_matches('#').trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(180)
        .collect()
}

fn normalize_pair(first_id: &str, second_id: &str) -> Option<(String, String)> {
    if first_id == second_id {
        return None;
    }

    if first_id < second_id {
        Some((first_id.to_string(), second_id.to_string()))
    } else {
        Some((second_id.to_string(), first_id.to_string()))
    }
}

fn load_note_meta(connection: &Connection, id: &str) -> Result<NoteMeta, String> {
    connection
        .query_row(
            "
            SELECT id, title, folder_id, created_at, updated_at, deleted_at,
                   extraction_status
            FROM notes
            WHERE id = ?1
            ",
            params![id],
            |row| {
                Ok(NoteMeta {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    folder_id: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    deleted_at: row.get(5)?,
                    extraction_status: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| "Note not found".to_string())
}

fn snapshot(app: &AppHandle, connection: &Connection) -> Result<BankSnapshot, String> {
    let notes_meta = {
        let mut statement = connection
            .prepare(
                "
                SELECT id, title, folder_id, created_at, updated_at, deleted_at,
                       extraction_status
                FROM notes
                ",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(NoteMeta {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    folder_id: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    deleted_at: row.get(5)?,
                    extraction_status: row.get(6)?,
                })
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        rows
    };

    let folders = {
        let mut statement = connection
            .prepare("SELECT id, name, created_at FROM folders ORDER BY created_at ASC")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(Folder {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        rows
    };

    let links = {
        let mut statement = connection
            .prepare(
                "
                SELECT source_id, target_id, created_at
                FROM note_links
                ORDER BY created_at ASC
                ",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(NoteLink {
                    source_id: row.get(0)?,
                    target_id: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        rows
    };

    let mut notes = Vec::with_capacity(notes_meta.len());
    for note in notes_meta {
        let content = read_note_content(app, &note.id)?;
        notes.push(NoteListItem {
            id: note.id,
            title: note.title,
            folder_id: note.folder_id,
            created_at: note.created_at,
            updated_at: note.updated_at,
            deleted_at: note.deleted_at,
            excerpt: excerpt(&content),
            extraction_status: note.extraction_status,
        });
    }

    Ok(BankSnapshot {
        notes,
        folders,
        links,
    })
}

#[tauri::command]
fn get_llama_config(app: AppHandle) -> Result<LlamaConfig, String> {
    let connection = open_database(&app)?;
    load_llama_config(&connection)
}

#[tauri::command]
fn save_llama_config(app: AppHandle, config: LlamaConfig) -> Result<LlamaConfig, String> {
    let base_url = validate_llama_base_url(&config.base_url)?;
    let preferred_model = config
        .preferred_model
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;

    transaction
        .execute(
            "
            INSERT INTO app_meta (key, value)
            VALUES ('llama_base_url', ?1)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            ",
            params![base_url],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "
            INSERT INTO app_meta (key, value)
            VALUES ('llama_preferred_model', ?1)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            ",
            params![preferred_model.as_deref().unwrap_or("")],
        )
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)?;

    Ok(LlamaConfig {
        base_url,
        preferred_model,
    })
}

#[tauri::command]
async fn get_llama_status(app: AppHandle) -> Result<LlamaStatus, String> {
    let config = {
        let connection = open_database(&app)?;
        load_llama_config(&connection)?
    };
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(4))
        .build()
        .map_err(|error| error.to_string())?;
    let started_at = Instant::now();
    let health_response = match client
        .get(llama_endpoint(&config.base_url, "/health"))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) if error.is_connect() || error.is_timeout() => {
            return Ok(llama_status(
                &config,
                LlamaConnectionState::Offline,
                "llama.cpp is not reachable",
                None,
                Vec::new(),
            ));
        }
        Err(error) => {
            return Ok(llama_status(
                &config,
                LlamaConnectionState::Error,
                error.to_string(),
                None,
                Vec::new(),
            ));
        }
    };
    let latency_ms = started_at.elapsed().as_millis() as u64;

    if health_response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
        return Ok(llama_status(
            &config,
            LlamaConnectionState::Loading,
            "llama.cpp is loading the model",
            Some(latency_ms),
            Vec::new(),
        ));
    }

    if !health_response.status().is_success() {
        return Ok(llama_status(
            &config,
            LlamaConnectionState::Error,
            format!(
                "Health check returned HTTP {}",
                health_response.status().as_u16()
            ),
            Some(latency_ms),
            Vec::new(),
        ));
    }

    let models_response = client
        .get(llama_endpoint(&config.base_url, "/v1/models"))
        .send()
        .await;
    let models = match models_response {
        Ok(response) if response.status().is_success() => response
            .json::<LlamaModelsResponse>()
            .await
            .map(|response| {
                response
                    .data
                    .into_iter()
                    .map(|model| LlamaModel {
                        id: model.id,
                        owned_by: model.owned_by,
                        context_size: model
                            .meta
                            .as_ref()
                            .and_then(|meta| meta.n_ctx.or(meta.n_ctx_train)),
                        parameter_count: model.meta.as_ref().and_then(|meta| meta.n_params),
                        size_bytes: model.meta.as_ref().and_then(|meta| meta.size),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    let message = if models.is_empty() {
        "llama.cpp is ready, but no model metadata was returned"
    } else {
        "llama.cpp is ready"
    };
    Ok(llama_status(
        &config,
        LlamaConnectionState::Ready,
        message,
        Some(latency_ms),
        models,
    ))
}

#[tauri::command]
fn get_extraction_queue_status(app: AppHandle) -> Result<ExtractionQueueStatus, String> {
    let connection = open_database(&app)?;
    let job_count = |status: &str| -> Result<u64, String> {
        connection
            .query_row(
                "SELECT COUNT(*) FROM extraction_jobs WHERE status = ?1",
                params![status],
                |row| row.get(0),
            )
            .map_err(db_error)
    };
    let note_count = |status: &str| -> Result<u64, String> {
        connection
            .query_row(
                "SELECT COUNT(*) FROM notes WHERE extraction_status = ?1",
                params![status],
                |row| row.get(0),
            )
            .map_err(db_error)
    };

    Ok(ExtractionQueueStatus {
        pending: job_count("pending")?,
        processing: job_count("processing")?,
        failed: job_count("failed")?,
        indexed: note_count("indexed")?,
        not_indexed: note_count("not_indexed")?,
    })
}

#[tauri::command]
fn get_note_extraction(app: AppHandle, id: String) -> Result<NoteExtractionView, String> {
    let connection = open_database(&app)?;
    let (status, error) = connection
        .query_row(
            "
            SELECT extraction_status, extraction_error
            FROM notes
            WHERE id = ?1
            ",
            params![id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| "Note not found".to_string())?;
    let entities = {
        let mut statement = connection
            .prepare(
                "
                SELECT entities.id,
                       entities.canonical_name,
                       entities.entity_type,
                       COUNT(entity_mentions.id) AS mention_count
                FROM entity_mentions
                JOIN entities ON entities.id = entity_mentions.entity_id
                WHERE entity_mentions.note_id = ?1
                GROUP BY entities.id, entities.canonical_name, entities.entity_type
                ORDER BY mention_count DESC, entities.canonical_name COLLATE NOCASE ASC
                ",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map(params![id], |row| {
                Ok(NoteEntity {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    entity_type: row.get(2)?,
                    mention_count: row.get(3)?,
                })
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        rows
    };

    Ok(NoteExtractionView {
        status,
        error,
        entities,
    })
}

#[tauri::command]
fn get_link_suggestions(
    app: AppHandle,
    note_id: String,
    limit: Option<u32>,
) -> Result<Vec<LinkSuggestion>, String> {
    let connection = open_database(&app)?;
    connection
        .query_row(
            "SELECT COUNT(*) FROM notes WHERE id = ?1 AND deleted_at IS NULL",
            params![note_id],
            |row| row.get::<_, u64>(0),
        )
        .map_err(db_error)
        .and_then(|count| {
            if count == 0 {
                Err("Note not found".to_string())
            } else {
                Ok(())
            }
        })?;

    let limit = i64::from(limit.unwrap_or(6).clamp(1, 20));
    let mut statement = connection
        .prepare(
            "
            WITH source_entities AS (
                SELECT entity_id, COUNT(*) AS source_mentions
                FROM entity_mentions
                WHERE note_id = ?1
                GROUP BY entity_id
            ),
            candidate_entities AS (
                SELECT mentions.note_id,
                       mentions.entity_id,
                       COUNT(*) AS candidate_mentions
                FROM entity_mentions AS mentions
                JOIN source_entities
                  ON source_entities.entity_id = mentions.entity_id
                JOIN notes
                  ON notes.id = mentions.note_id
                 AND notes.deleted_at IS NULL
                WHERE mentions.note_id != ?1
                  AND NOT EXISTS (
                    SELECT 1
                    FROM note_links
                    WHERE (source_id = ?1 AND target_id = mentions.note_id)
                       OR (source_id = mentions.note_id AND target_id = ?1)
                  )
                GROUP BY mentions.note_id, mentions.entity_id
            ),
            ranked_notes AS (
                SELECT note_id,
                       COUNT(*) AS shared_entity_count,
                       SUM(candidate_mentions) AS shared_mention_count
                FROM candidate_entities
                GROUP BY note_id
                ORDER BY shared_entity_count DESC,
                         shared_mention_count DESC,
                         MAX(note_id) ASC
                LIMIT ?2
            )
            SELECT note_id, shared_entity_count, shared_mention_count
            FROM ranked_notes
            ORDER BY shared_entity_count DESC, shared_mention_count DESC, note_id ASC
            ",
        )
        .map_err(db_error)?;
    let candidates = statement
        .query_map(params![note_id, limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, u64>(2)?,
            ))
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;

    let mut suggestions = Vec::with_capacity(candidates.len());
    for (candidate_id, shared_entity_count, shared_mention_count) in candidates {
        let meta = load_note_meta(&connection, &candidate_id)?;
        let content = read_note_content(&app, &candidate_id)?;
        let mut entity_statement = connection
            .prepare(
                "
                SELECT entities.id,
                       entities.canonical_name,
                       entities.entity_type,
                       COUNT(candidate_mentions.id) AS mention_count
                FROM entity_mentions AS source_mentions
                JOIN entity_mentions AS candidate_mentions
                  ON candidate_mentions.entity_id = source_mentions.entity_id
                 AND candidate_mentions.note_id = ?2
                JOIN entities ON entities.id = source_mentions.entity_id
                WHERE source_mentions.note_id = ?1
                GROUP BY entities.id, entities.canonical_name, entities.entity_type
                ORDER BY mention_count DESC, entities.canonical_name COLLATE NOCASE ASC
                LIMIT 5
                ",
            )
            .map_err(db_error)?;
        let shared_entities = entity_statement
            .query_map(params![note_id, candidate_id], |row| {
                Ok(NoteEntity {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    entity_type: row.get(2)?,
                    mention_count: row.get(3)?,
                })
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;

        suggestions.push(LinkSuggestion {
            note: NoteListItem {
                id: meta.id,
                title: meta.title,
                folder_id: meta.folder_id,
                created_at: meta.created_at,
                updated_at: meta.updated_at,
                deleted_at: meta.deleted_at,
                excerpt: excerpt(&content),
                extraction_status: meta.extraction_status,
            },
            shared_entities,
            shared_entity_count,
            shared_mention_count,
        });
    }

    Ok(suggestions)
}

#[tauri::command]
fn enqueue_note_extraction(app: AppHandle, id: String) -> Result<ExtractionQueueStatus, String> {
    let mut connection = open_database(&app)?;
    let note = load_note_meta(&connection, &id)?;
    if note.deleted_at.is_some() {
        return Err("Trashed notes cannot be queued for extraction".to_string());
    }

    let content = read_note_content(&app, &id)?;
    let transaction = connection.transaction().map_err(db_error)?;
    if content.trim().is_empty() {
        clear_extraction_job(&transaction, &id)?;
    } else {
        force_enqueue_extraction(&transaction, &id, &content_hash(&content))?;
    }
    transaction.commit().map_err(db_error)?;
    get_extraction_queue_status(app)
}

#[tauri::command]
fn enqueue_all_note_extractions(app: AppHandle) -> Result<ExtractionQueueStatus, String> {
    let mut connection = open_database(&app)?;
    let note_ids = {
        let mut statement = connection
            .prepare("SELECT id FROM notes WHERE deleted_at IS NULL")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        rows
    };
    let contents = note_ids
        .iter()
        .map(|id| Ok((id.clone(), read_note_content(&app, id)?)))
        .collect::<Result<Vec<_>, String>>()?;

    let transaction = connection.transaction().map_err(db_error)?;
    for (id, content) in contents {
        if content.trim().is_empty() {
            clear_extraction_job(&transaction, &id)?;
        } else {
            enqueue_extraction(&transaction, &id, &content_hash(&content))?;
        }
    }
    transaction.commit().map_err(db_error)?;
    get_extraction_queue_status(app)
}

#[tauri::command]
fn retry_failed_extractions(app: AppHandle) -> Result<ExtractionQueueStatus, String> {
    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    let now = now_string();
    transaction
        .execute(
            "
            UPDATE extraction_jobs
            SET status = 'pending',
                attempts = 0,
                available_at = ?1,
                updated_at = ?1,
                started_at = NULL,
                lease_token = NULL,
                last_error = NULL
            WHERE status = 'failed'
            ",
            params![now],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "
            UPDATE notes
            SET extraction_status = 'queued',
                extraction_error = NULL
            WHERE id IN (
                SELECT note_id FROM extraction_jobs WHERE status = 'pending'
            )
            ",
            [],
        )
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)?;
    get_extraction_queue_status(app)
}

#[tauri::command]
fn get_bank(app: AppHandle) -> Result<BankSnapshot, String> {
    let connection = open_database(&app)?;
    snapshot(&app, &connection)
}

#[tauri::command]
fn create_note(
    app: AppHandle,
    title: Option<String>,
    folder_id: Option<String>,
) -> Result<NoteWithContent, String> {
    let connection = open_database(&app)?;
    let now = now_string();
    let note = NoteMeta {
        id: new_id("note"),
        title: title
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "Untitled note".to_string()),
        folder_id,
        created_at: now.clone(),
        updated_at: now,
        deleted_at: None,
        extraction_status: default_extraction_status(),
    };

    write_note_content(&app, &note.id, "")?;
    if let Err(error) = connection.execute(
        "
        INSERT INTO notes (id, title, folder_id, created_at, updated_at, deleted_at)
        VALUES (?1, ?2, ?3, ?4, ?5, NULL)
        ",
        params![
            note.id,
            note.title,
            note.folder_id,
            note.created_at,
            note.updated_at
        ],
    ) {
        let _ = fs::remove_file(note_path(&app, &note.id)?);
        return Err(db_error(error));
    }

    get_note(app, note.id)
}

#[tauri::command]
fn get_note(app: AppHandle, id: String) -> Result<NoteWithContent, String> {
    let connection = open_database(&app)?;
    let note = load_note_meta(&connection, &id)?;
    let content = read_note_content(&app, &note.id)?;

    Ok(NoteWithContent {
        id: note.id,
        title: note.title,
        folder_id: note.folder_id,
        created_at: note.created_at,
        updated_at: note.updated_at,
        deleted_at: note.deleted_at,
        content,
        extraction_status: note.extraction_status,
    })
}

#[tauri::command]
fn save_note(
    app: AppHandle,
    id: String,
    title: String,
    content: String,
    folder_id: Option<String>,
) -> Result<NoteWithContent, String> {
    let mut connection = open_database(&app)?;
    load_note_meta(&connection, &id)?;

    let clean_title = title.trim();
    let saved_title = if clean_title.is_empty() {
        infer_title(&content)
    } else {
        clean_title.to_string()
    };
    let updated_at = now_string();
    let hash = content_hash(&content);

    write_note_content(&app, &id, &content)?;
    let transaction = connection.transaction().map_err(db_error)?;
    transaction
        .execute(
            "
            UPDATE notes
            SET title = ?1,
                folder_id = ?2,
                updated_at = ?3
            WHERE id = ?4
            ",
            params![saved_title, folder_id, updated_at, id],
        )
        .map_err(db_error)?;
    if content.trim().is_empty() {
        clear_extraction_job(&transaction, &id)?;
    } else {
        enqueue_extraction(&transaction, &id, &hash)?;
    }
    transaction.commit().map_err(db_error)?;

    get_note(app, id)
}

#[tauri::command]
fn create_folder(app: AppHandle, name: String) -> Result<BankSnapshot, String> {
    let clean_name = name.trim();
    if clean_name.is_empty() {
        return Err("Folder name is required".to_string());
    }

    let connection = open_database(&app)?;
    connection
        .execute(
            "INSERT INTO folders (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![new_id("folder"), clean_name, now_string()],
        )
        .map_err(db_error)?;
    snapshot(&app, &connection)
}

#[tauri::command]
fn delete_folder(app: AppHandle, id: String) -> Result<BankSnapshot, String> {
    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    transaction
        .execute(
            "
            UPDATE notes
            SET folder_id = NULL, updated_at = ?1
            WHERE folder_id = ?2
            ",
            params![now_string(), id],
        )
        .map_err(db_error)?;
    transaction
        .execute("DELETE FROM folders WHERE id = ?1", params![id])
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)?;
    snapshot(&app, &connection)
}

#[tauri::command]
fn move_note(
    app: AppHandle,
    id: String,
    folder_id: Option<String>,
) -> Result<BankSnapshot, String> {
    let connection = open_database(&app)?;
    let current_folder_id = connection
        .query_row(
            "SELECT folder_id FROM notes WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| "Note not found".to_string())?;

    if current_folder_id == folder_id {
        return snapshot(&app, &connection);
    }

    let changed = connection
        .execute(
            "
            UPDATE notes
            SET folder_id = ?1
            WHERE id = ?2
            ",
            params![folder_id, id],
        )
        .map_err(db_error)?;
    if changed == 0 {
        return Err("Note not found".to_string());
    }
    snapshot(&app, &connection)
}

#[tauri::command]
fn trash_note(app: AppHandle, id: String) -> Result<BankSnapshot, String> {
    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    let now = now_string();
    let changed = transaction
        .execute(
            "
            UPDATE notes
            SET deleted_at = ?1,
                updated_at = ?1,
                extraction_status = 'not_indexed',
                extraction_error = NULL
            WHERE id = ?2
            ",
            params![now, id],
        )
        .map_err(db_error)?;
    if changed == 0 {
        return Err("Note not found".to_string());
    }
    transaction
        .execute(
            "DELETE FROM extraction_jobs WHERE note_id = ?1",
            params![id],
        )
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)?;
    snapshot(&app, &connection)
}

#[tauri::command]
fn restore_note(app: AppHandle, id: String) -> Result<BankSnapshot, String> {
    let connection = open_database(&app)?;
    let changed = connection
        .execute(
            "
            UPDATE notes
            SET deleted_at = NULL, updated_at = ?1
            WHERE id = ?2
            ",
            params![now_string(), id],
        )
        .map_err(db_error)?;
    if changed == 0 {
        return Err("Note not found".to_string());
    }
    snapshot(&app, &connection)
}

#[tauri::command]
fn permanent_delete_note(app: AppHandle, id: String) -> Result<BankSnapshot, String> {
    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    let changed = transaction
        .execute("DELETE FROM notes WHERE id = ?1", params![id])
        .map_err(db_error)?;
    if changed == 0 {
        return Err("Note not found".to_string());
    }

    let path = note_path(&app, &id)?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }

    transaction.commit().map_err(db_error)?;
    snapshot(&app, &connection)
}

#[tauri::command]
fn link_notes(app: AppHandle, ids: Vec<String>) -> Result<BankSnapshot, String> {
    let mut connection = open_database(&app)?;
    let valid_ids = {
        let mut statement = connection
            .prepare("SELECT id FROM notes WHERE deleted_at IS NULL")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_error)?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(db_error)?;
        rows
    };
    let selected_ids = ids
        .into_iter()
        .filter(|id| valid_ids.contains(id))
        .collect::<Vec<_>>();

    if selected_ids.len() < 2 {
        return Err("Select at least two active notes to link".to_string());
    }

    let transaction = connection.transaction().map_err(db_error)?;
    for first_index in 0..selected_ids.len() {
        for second_index in (first_index + 1)..selected_ids.len() {
            if let Some((source_id, target_id)) =
                normalize_pair(&selected_ids[first_index], &selected_ids[second_index])
            {
                transaction
                    .execute(
                        "
                        INSERT OR IGNORE INTO note_links (source_id, target_id, created_at)
                        VALUES (?1, ?2, ?3)
                        ",
                        params![source_id, target_id, now_string()],
                    )
                    .map_err(db_error)?;
            }
        }
    }
    transaction.commit().map_err(db_error)?;
    snapshot(&app, &connection)
}

#[tauri::command]
fn unlink_notes(
    app: AppHandle,
    source_id: String,
    target_id: String,
) -> Result<BankSnapshot, String> {
    let Some((source_id, target_id)) = normalize_pair(&source_id, &target_id) else {
        return Err("Cannot unlink a note from itself".to_string());
    };

    let connection = open_database(&app)?;
    connection
        .execute(
            "
            DELETE FROM note_links
            WHERE source_id = ?1 AND target_id = ?2
            ",
            params![source_id, target_id],
        )
        .map_err(db_error)?;
    snapshot(&app, &connection)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let connection = open_database(app.handle()).map_err(std::io::Error::other)?;
            recover_interrupted_extraction_jobs(&connection).map_err(std::io::Error::other)?;
            tauri::async_runtime::spawn(extraction_worker(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_llama_config,
            save_llama_config,
            get_llama_status,
            get_extraction_queue_status,
            get_note_extraction,
            get_link_suggestions,
            enqueue_note_extraction,
            enqueue_all_note_extractions,
            retry_failed_extractions,
            get_bank,
            create_note,
            get_note,
            save_note,
            create_folder,
            delete_folder,
            move_note,
            trash_note,
            restore_note,
            permanent_delete_note,
            link_notes,
            unlink_notes
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue_test_database() -> Connection {
        let connection = Connection::open_in_memory().expect("open test database");
        connection
            .execute_batch(
                "
                CREATE TABLE notes (
                    id TEXT PRIMARY KEY NOT NULL,
                    content_hash TEXT,
                    indexed_content_hash TEXT,
                    extraction_status TEXT NOT NULL DEFAULT 'not_indexed',
                    extraction_error TEXT
                );

                CREATE TABLE extraction_jobs (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    note_id TEXT NOT NULL UNIQUE REFERENCES notes(id) ON DELETE CASCADE,
                    content_hash TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'pending',
                    priority INTEGER NOT NULL DEFAULT 0,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    max_attempts INTEGER NOT NULL DEFAULT 3,
                    available_at TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    started_at TEXT,
                    lease_token TEXT,
                    last_error TEXT
                );

                INSERT INTO notes (id) VALUES ('note-1');
                ",
            )
            .expect("create queue test schema");
        connection
    }

    #[test]
    fn extraction_jobs_coalesce_to_latest_content_hash() {
        let mut connection = queue_test_database();

        {
            let transaction = connection.transaction().expect("first transaction");
            enqueue_extraction(&transaction, "note-1", "hash-one").expect("first enqueue");
            transaction.commit().expect("commit first enqueue");
        }

        connection
            .execute(
                "
                UPDATE extraction_jobs
                SET attempts = 2, status = 'failed', last_error = 'temporary failure'
                WHERE note_id = 'note-1'
                ",
                [],
            )
            .expect("simulate failed job");

        {
            let transaction = connection.transaction().expect("second transaction");
            enqueue_extraction(&transaction, "note-1", "hash-two").expect("second enqueue");
            transaction.commit().expect("commit second enqueue");
        }

        let job = connection
            .query_row(
                "
                SELECT COUNT(*), content_hash, status, attempts, last_error
                FROM extraction_jobs
                WHERE note_id = 'note-1'
                ",
                [],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, u64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .expect("load coalesced job");

        assert_eq!(
            job,
            (1, "hash-two".to_string(), "pending".to_string(), 0, None)
        );
    }

    #[test]
    fn indexed_content_does_not_remain_queued() {
        let mut connection = queue_test_database();
        connection
            .execute(
                "
                UPDATE notes
                SET indexed_content_hash = 'indexed-hash'
                WHERE id = 'note-1'
                ",
                [],
            )
            .expect("set indexed hash");

        let transaction = connection.transaction().expect("transaction");
        enqueue_extraction(&transaction, "note-1", "indexed-hash").expect("enqueue");
        transaction.commit().expect("commit");

        let job_count = connection
            .query_row("SELECT COUNT(*) FROM extraction_jobs", [], |row| {
                row.get::<_, u64>(0)
            })
            .expect("count jobs");
        let status = connection
            .query_row(
                "SELECT extraction_status FROM notes WHERE id = 'note-1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("load note status");

        assert_eq!(job_count, 0);
        assert_eq!(status, "indexed");
    }

    #[test]
    fn parses_schema_json_after_model_channel_prefix() {
        let content = r#"<|channel>thought
<channel|>{"entities":[{"name":"Google","entity_type":"ORGANIZATION","surface_text":"Google","context":"Google announced","confidence":0.98,"aliases":[]}]}"#;
        let parsed = parse_extracted_entities(content).expect("parse prefixed JSON");

        assert_eq!(parsed.entities.len(), 1);
        assert_eq!(parsed.entities[0].name, "Google");
        assert_eq!(parsed.entities[0].entity_type, "ORGANIZATION");
    }

    #[test]
    fn paragraph_chunking_preserves_all_content() {
        let first = "A".repeat(EXTRACTION_INITIAL_CHARS - 100);
        let second = "B".repeat(250);
        let content = format!("{first}\n\n{second}");
        let chunks = initial_note_chunks(&content);

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], first);
        assert_eq!(chunks[1], second);
    }

    #[test]
    fn dense_large_paragraph_is_split_before_inference() {
        let content = "Entity-rich sentence. ".repeat(170);
        let chunks = initial_note_chunks(&content);

        assert!(chunks.len() >= 2);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.chars().count() <= EXTRACTION_INITIAL_CHARS));
    }
}
