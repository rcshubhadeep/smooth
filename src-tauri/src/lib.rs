mod agents;
mod audio_capture;
mod audio_preprocess;
mod calendar;
mod chat;
mod diarization;
mod export_notes;
mod gmail;
mod imports;
mod llama_runtime;
mod llm;
mod mcp;
mod meeting_notes;
mod reminders;
mod semantic_search;
mod slack;
mod stt;
mod system_audio;

use audio_capture::{
    flush_audio_capture_chunk, get_audio_capture_status, start_audio_capture, stop_audio_capture,
    AudioCaptureState,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    net::IpAddr,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use stt::{
    enqueue_stt_job, get_stt_config, get_stt_queue_status, get_stt_status,
    recover_interrupted_stt_jobs, save_stt_config, transcribe_capture_file,
    transcribe_last_capture, SttRuntime,
};
use system_audio::{
    capture_meeting_snapshot, check_system_audio_permission, get_system_audio_capture_status,
    list_meeting_visual_sources, start_system_audio_capture, stop_system_audio_capture,
    SystemAudioCaptureState,
};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Clone, Debug, Default, Deserialize)]
struct LegacyStore {
    notes: Vec<NoteMeta>,
    folders: Vec<Folder>,
    links: Vec<NoteLink>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct NoteMeta {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) folder_id: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) deleted_at: Option<String>,
    #[serde(default = "default_extraction_status")]
    pub(crate) extraction_status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Folder {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) created_at: String,
    pub(crate) system_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NoteLink {
    pub(crate) source_id: String,
    pub(crate) target_id: String,
    pub(crate) created_at: String,
    #[serde(default)]
    pub(crate) label: Option<String>,
    #[serde(default = "default_link_kind")]
    pub(crate) link_kind: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct NoteListItem {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) folder_id: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) deleted_at: Option<String>,
    pub(crate) excerpt: String,
    pub(crate) extraction_status: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct NoteWithContent {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) folder_id: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) deleted_at: Option<String>,
    pub(crate) content: String,
    pub(crate) extraction_status: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BankSnapshot {
    pub(crate) notes: Vec<NoteListItem>,
    pub(crate) folders: Vec<Folder>,
    pub(crate) links: Vec<NoteLink>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum LlamaMode {
    Managed,
    External,
}

fn default_remote_base_url() -> String {
    String::new()
}

fn default_remote_model() -> String {
    String::new()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LlamaConfig {
    #[serde(default)]
    default_provider: llm::LlmProvider,
    #[serde(default)]
    always_obey_global_llm: bool,
    mode: LlamaMode,
    base_url: String,
    preferred_model: Option<String>,
    managed_model: String,
    context_size: u32,
    gpu_layers: i32,
    flash_attention: bool,
    parallel: u16,
    cache_ram_mb: u32,
    context_checkpoints: u16,
    cache_type_k: String,
    cache_type_v: String,
    spec_type: String,
    spec_draft_n_max: u16,
    #[serde(default = "default_remote_base_url", alias = "inception_base_url")]
    remote_base_url: String,
    #[serde(default = "default_remote_model", alias = "inception_model")]
    remote_model: String,
    #[serde(default)]
    #[serde(alias = "inception_api_key")]
    remote_api_key: Option<String>,
    #[serde(default)]
    #[serde(alias = "clear_inception_api_key")]
    clear_remote_api_key: bool,
    #[serde(default)]
    #[serde(alias = "inception_api_key_configured")]
    remote_api_key_configured: bool,
    #[serde(default = "default_remote_context_tokens")]
    remote_context_tokens: u32,
}

fn default_remote_context_tokens() -> u32 {
    llm::REMOTE_DEFAULT_CONTEXT_TOKENS as u32
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
    managed: Option<llama_runtime::ManagedLlamaSnapshot>,
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
    selection: Option<llm::LlmSelection>,
}

#[derive(Clone, Debug)]
struct ExtractionModel {
    id: String,
    context_tokens: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
struct ExtractionBudget {
    input_tokens: usize,
    max_output_tokens: usize,
    initial_chars: usize,
    max_entities: usize,
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

#[derive(Clone, Debug)]
struct MentionLocation {
    start_offset: Option<i64>,
    end_offset: Option<i64>,
    match_status: String,
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
pub(crate) struct NoteEntity {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) entity_type: String,
    pub(crate) mention_count: u64,
}

#[derive(Clone, Debug, Serialize)]
struct NoteEntityMention {
    id: i64,
    entity_id: i64,
    surface_text: String,
    context: Option<String>,
    start_offset: Option<i64>,
    end_offset: Option<i64>,
    match_status: String,
}

#[derive(Clone, Debug, Serialize)]
struct NoteExtractionView {
    status: String,
    error: Option<String>,
    entities: Vec<NoteEntity>,
    mentions: Vec<NoteEntityMention>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct EntityInterestDefinition {
    id: Option<i64>,
    name: String,
    description: String,
    enabled: bool,
    sort_order: i64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct LinkSuggestion {
    pub(crate) note: NoteListItem,
    pub(crate) shared_entities: Vec<NoteEntity>,
    pub(crate) shared_entity_count: u64,
    pub(crate) shared_mention_count: u64,
}

#[derive(Debug)]
enum ChunkExtractionError {
    Split(String),
    Retry(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExtractionResponseMode {
    StrictSchema,
    JsonObject,
    PlainJson,
}

impl ExtractionResponseMode {
    fn log_name(self) -> &'static str {
        match self {
            Self::StrictSchema => "strict_schema",
            Self::JsonObject => "json_object_fallback",
            Self::PlainJson => "plain_json_fallback",
        }
    }
}

const DEFAULT_EXTRACTION_CONTEXT_TOKENS: usize = 8192;
const EXTRACTION_CONTEXT_RESERVE_TOKENS: usize = 256;
const EXTRACTION_MIN_OUTPUT_TOKENS: usize = 700;
const EXTRACTION_MAX_OUTPUT_TOKENS: usize = 1600;
const EXTRACTION_MIN_INPUT_TOKENS: usize = 700;
const EXTRACTION_MIN_SPLIT_CHARS: usize = 300;
const EXTRACTION_POLL_SECONDS: u64 = 2;
const EXTRACTION_CHARS_PER_ENTITY_BUDGET: usize = 180;
const EXTRACTION_TOKENS_PER_ENTITY_BUDGET: usize = 48;

static EXTRACTION_RESPONSE_MODES: OnceLock<Mutex<HashMap<String, ExtractionResponseMode>>> =
    OnceLock::new();

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

fn default_link_kind() -> String {
    "manual".to_string()
}

fn disabled_extraction_status() -> String {
    "disabled".to_string()
}

pub(crate) fn now_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

pub(crate) fn new_id(prefix: &str) -> String {
    format!("{}-{}", prefix, now_string())
}

pub(crate) fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
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

pub(crate) fn open_database(app: &AppHandle) -> Result<Connection, String> {
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
                created_at TEXT NOT NULL,
                system_key TEXT
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
                label TEXT,
                link_kind TEXT NOT NULL DEFAULT 'manual'
                    CHECK (link_kind IN ('manual', 'entity_sharing')),
                PRIMARY KEY (source_id, target_id),
                CHECK (source_id != target_id)
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
                entity_type TEXT,
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
                start_offset INTEGER,
                end_offset INTEGER,
                match_status TEXT NOT NULL DEFAULT 'unresolved',
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS entity_mentions_note_id_idx
                ON entity_mentions(note_id);
            CREATE INDEX IF NOT EXISTS entity_mentions_entity_id_idx
                ON entity_mentions(entity_id);

            CREATE TABLE IF NOT EXISTS entity_interest_definitions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                enabled INTEGER NOT NULL DEFAULT 1,
                sort_order INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(name COLLATE NOCASE)
            );

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
                last_error TEXT,
                llm_provider TEXT,
                llm_model TEXT
            );

            CREATE INDEX IF NOT EXISTS extraction_jobs_claim_idx
                ON extraction_jobs(status, available_at, priority, created_at);
            ",
        )
        .map_err(db_error)?;

    chat::init_schema(&connection)?;
    agents::init_schema(&connection)?;
    stt::init_schema(&connection)?;
    meeting_notes::init_schema(&connection)?;
    mcp::init_schema(&connection)?;
    semantic_search::init_schema(&connection)?;
    slack::init_schema(&connection)?;
    imports::init_schema(&connection)?;
    reminders::init_schema(&connection)?;
    agents::reminder_workflows::init_schema(&connection)?;
    migrate_note_links_schema(&connection)?;
    migrate_entity_schema(&connection)?;
    migrate_extraction_jobs_schema(&connection)?;
    seed_default_entity_interests(&connection)?;
    migrate_legacy_store(app, &mut connection)?;
    Ok(connection)
}

const DEFAULT_ENTITY_INTERESTS: &[(&str, &str)] = &[
    (
        "People",
        "Named people, speakers, buyers, stakeholders, and owners.",
    ),
    (
        "Organizations",
        "Companies, customers, partners, vendors, and institutions.",
    ),
    (
        "Products",
        "Products, product areas, SKUs, packages, and named offerings.",
    ),
    (
        "Projects",
        "Internal or customer-facing projects, initiatives, and workstreams.",
    ),
    (
        "Customers",
        "Customer names, accounts, segments, and customer teams.",
    ),
    ("Competitors", "Competitor names and competing products."),
    (
        "Objections",
        "Sales objections, blockers, concerns, and risks.",
    ),
    (
        "Requirements",
        "Requested capabilities, acceptance criteria, and constraints.",
    ),
    (
        "Decisions",
        "Explicit decisions, approvals, rejections, and commitments.",
    ),
    (
        "Follow-ups",
        "Action items, next steps, owners, and due dates.",
    ),
    (
        "Technologies",
        "Frameworks, tools, models, APIs, and infrastructure.",
    ),
    (
        "Dates",
        "Important dates, times, deadlines, and meeting references.",
    ),
];

fn migrate_note_links_schema(connection: &Connection) -> Result<(), String> {
    let existing_columns = {
        let mut statement = connection
            .prepare("PRAGMA table_info(note_links)")
            .map_err(db_error)?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(db_error)?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(db_error)?;
        columns
    };

    if !existing_columns.contains("label") {
        connection
            .execute("ALTER TABLE note_links ADD COLUMN label TEXT", [])
            .map_err(db_error)?;
    }

    if !existing_columns.contains("link_kind") {
        connection
            .execute(
                "ALTER TABLE note_links ADD COLUMN link_kind TEXT NOT NULL DEFAULT 'manual'",
                [],
            )
            .map_err(db_error)?;
    }

    let create_sql = connection
        .query_row(
            "
            SELECT sql
            FROM sqlite_master
            WHERE type = 'table' AND name = 'note_links'
            ",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?
        .unwrap_or_default();

    if create_sql.contains("source_id < target_id") {
        connection
            .execute_batch(
                "
                DROP TABLE IF EXISTS note_links_directed_migration;

                CREATE TABLE note_links_directed_migration (
                    source_id TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
                    target_id TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
                    created_at TEXT NOT NULL,
                    label TEXT,
                    link_kind TEXT NOT NULL DEFAULT 'manual'
                        CHECK (link_kind IN ('manual', 'entity_sharing')),
                    PRIMARY KEY (source_id, target_id),
                    CHECK (source_id != target_id)
                );

                INSERT OR IGNORE INTO note_links_directed_migration (
                    source_id, target_id, created_at, label, link_kind
                )
                SELECT source_id, target_id, created_at, label, link_kind
                FROM note_links
                WHERE source_id != target_id;

                INSERT OR IGNORE INTO note_links_directed_migration (
                    source_id, target_id, created_at, label, link_kind
                )
                SELECT target_id, source_id, created_at, label, link_kind
                FROM note_links
                WHERE source_id != target_id;

                DROP TABLE note_links;
                ALTER TABLE note_links_directed_migration RENAME TO note_links;
                ",
            )
            .map_err(db_error)?;
    }

    connection
        .execute(
            "CREATE INDEX IF NOT EXISTS note_links_target_id_idx ON note_links(target_id)",
            [],
        )
        .map_err(db_error)?;

    Ok(())
}

fn table_columns(connection: &Connection, table: &str) -> Result<HashSet<String>, String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(db_error)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(db_error)?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(db_error)?;
    Ok(columns)
}

fn migrate_entity_schema(connection: &Connection) -> Result<(), String> {
    let mention_columns = table_columns(connection, "entity_mentions")?;
    if !mention_columns.contains("start_offset") {
        connection
            .execute(
                "ALTER TABLE entity_mentions ADD COLUMN start_offset INTEGER",
                [],
            )
            .map_err(db_error)?;
    }
    if !mention_columns.contains("end_offset") {
        connection
            .execute(
                "ALTER TABLE entity_mentions ADD COLUMN end_offset INTEGER",
                [],
            )
            .map_err(db_error)?;
    }
    if !mention_columns.contains("match_status") {
        connection
            .execute(
                "ALTER TABLE entity_mentions ADD COLUMN match_status TEXT NOT NULL DEFAULT 'unresolved'",
                [],
            )
            .map_err(db_error)?;
    }

    let alias_columns = table_columns(connection, "entity_aliases")?;
    if !alias_columns.contains("entity_type") {
        connection
            .execute("ALTER TABLE entity_aliases ADD COLUMN entity_type TEXT", [])
            .map_err(db_error)?;
    }
    connection
        .execute(
            "
            UPDATE entity_aliases
            SET entity_type = (
                SELECT entities.entity_type
                FROM entities
                WHERE entities.id = entity_aliases.entity_id
            )
            WHERE entity_type IS NULL OR entity_type = ''
            ",
            [],
        )
        .map_err(db_error)?;
    connection
        .execute(
            "
            DELETE FROM entity_aliases
            WHERE rowid NOT IN (
                SELECT MIN(rowid)
                FROM entity_aliases
                GROUP BY normalized_alias, entity_type
            )
            ",
            [],
        )
        .map_err(db_error)?;
    connection
        .execute(
            "
            CREATE UNIQUE INDEX IF NOT EXISTS entity_aliases_lookup_unique_idx
                ON entity_aliases(normalized_alias, entity_type)
            ",
            [],
        )
        .map_err(db_error)?;
    connection
        .execute(
            "
            CREATE INDEX IF NOT EXISTS entity_mentions_offsets_idx
                ON entity_mentions(note_id, start_offset)
            ",
            [],
        )
        .map_err(db_error)?;

    Ok(())
}

fn migrate_extraction_jobs_schema(connection: &Connection) -> Result<(), String> {
    let columns = table_columns(connection, "extraction_jobs")?;
    if !columns.contains("llm_provider") {
        connection
            .execute(
                "ALTER TABLE extraction_jobs ADD COLUMN llm_provider TEXT",
                [],
            )
            .map_err(db_error)?;
    }
    if !columns.contains("llm_model") {
        connection
            .execute("ALTER TABLE extraction_jobs ADD COLUMN llm_model TEXT", [])
            .map_err(db_error)?;
    }
    Ok(())
}

fn seed_default_entity_interests(connection: &Connection) -> Result<(), String> {
    let now = now_string();
    for (index, (name, description)) in DEFAULT_ENTITY_INTERESTS.iter().enumerate() {
        connection
            .execute(
                "
                INSERT INTO entity_interest_definitions (
                    name, description, enabled, sort_order, created_at, updated_at
                )
                VALUES (?1, ?2, 1, ?3, ?4, ?4)
                ON CONFLICT(name) DO NOTHING
                ",
                params![name, description, index as i64, now],
            )
            .map_err(db_error)?;
    }
    Ok(())
}

/// Note title + full markdown body, for use as chat context.
pub(crate) fn note_context(app: &AppHandle, note_id: &str) -> Result<(String, String), String> {
    let connection = open_database(app)?;
    let meta = load_note_meta(&connection, note_id)?;
    if meta.deleted_at.is_some() {
        return Err("This note is in the trash".to_string());
    }
    let content = read_note_content(app, note_id)?;
    Ok((meta.title, content))
}

fn resolved_llama_config(app: &AppHandle) -> Result<LlamaConfig, String> {
    let connection = open_database(app)?;
    let mut config = load_llama_config(&connection)?;
    drop(connection);
    if config.mode == LlamaMode::Managed {
        config.base_url = app
            .state::<llama_runtime::LlamaRuntimeState>()
            .ensure_running(app, &managed_llama_launch_config(&config))?;
    }
    Ok(config)
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
        let Some((source_id, target_id)) =
            normalize_directed_pair(&link.source_id, &link.target_id)
        else {
            continue;
        };
        if !note_ids.contains(&source_id) || !note_ids.contains(&target_id) {
            continue;
        }

        let label = normalize_link_label(link.label);
        let link_kind = normalize_link_kind(Some(link.link_kind))?;
        transaction
            .execute(
                "
                INSERT OR IGNORE INTO note_links (source_id, target_id, created_at, label, link_kind)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ",
                params![
                    &source_id,
                    &target_id,
                    &link.created_at,
                    label.as_deref(),
                    link_kind.as_str(),
                ],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "
                INSERT OR IGNORE INTO note_links (source_id, target_id, created_at, label, link_kind)
                VALUES (?1, ?2, ?3, ?4, ?5)
                ",
                params![
                    &target_id,
                    &source_id,
                    &link.created_at,
                    label.as_deref(),
                    link_kind.as_str(),
                ],
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

pub(crate) fn db_error(error: rusqlite::Error) -> String {
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
                available_at, created_at, updated_at, started_at, lease_token, last_error,
                llm_provider, llm_model
            )
            VALUES (?1, ?2, 'pending', 0, 0, 3, ?3, ?3, ?3, NULL, NULL, NULL, NULL, NULL)
            ON CONFLICT(note_id) DO UPDATE SET
                content_hash = excluded.content_hash,
                status = 'pending',
                attempts = 0,
                available_at = excluded.available_at,
                updated_at = excluded.updated_at,
                started_at = NULL,
                lease_token = NULL,
                last_error = NULL,
                llm_provider = NULL,
                llm_model = NULL
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
    force_enqueue_extraction_with_selection(transaction, note_id, hash, None)
}

fn force_enqueue_extraction_with_selection(
    transaction: &rusqlite::Transaction<'_>,
    note_id: &str,
    hash: &str,
    selection: Option<&llm::LlmSelection>,
) -> Result<(), String> {
    let now = now_string();
    let provider =
        selection
            .and_then(|selection| selection.provider)
            .map(|provider| match provider {
                llm::LlmProvider::Local => "local",
                llm::LlmProvider::Remote => "remote",
            });
    let model = selection
        .and_then(|selection| selection.model.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    transaction
        .execute(
            "
            INSERT INTO extraction_jobs (
                note_id, content_hash, status, priority, attempts, max_attempts,
                available_at, created_at, updated_at, started_at, lease_token, last_error,
                llm_provider, llm_model
            )
            VALUES (?1, ?2, 'pending', 0, 0, 3, ?3, ?3, ?3, NULL, NULL, NULL, ?4, ?5)
            ON CONFLICT(note_id) DO UPDATE SET
                content_hash = excluded.content_hash,
                status = 'pending',
                attempts = 0,
                available_at = excluded.available_at,
                updated_at = excluded.updated_at,
                started_at = NULL,
                lease_token = NULL,
                last_error = NULL,
                llm_provider = excluded.llm_provider,
                llm_model = excluded.llm_model
            ",
            params![note_id, hash, now, provider, model],
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

fn extraction_budget(context_tokens: Option<usize>, model: &str) -> ExtractionBudget {
    let context_tokens = context_tokens.unwrap_or(DEFAULT_EXTRACTION_CONTEXT_TOKENS);
    let reserve_tokens = EXTRACTION_CONTEXT_RESERVE_TOKENS.min(context_tokens / 8);
    let output_ceiling =
        context_tokens.saturating_sub(reserve_tokens + EXTRACTION_MIN_INPUT_TOKENS);
    let max_output_tokens = (context_tokens / 5)
        .clamp(EXTRACTION_MIN_OUTPUT_TOKENS, EXTRACTION_MAX_OUTPUT_TOKENS)
        .min(output_ceiling);
    let input_tokens = context_tokens
        .saturating_sub(max_output_tokens + reserve_tokens)
        .max(EXTRACTION_MIN_INPUT_TOKENS);

    if !is_bonsai_model(model) && !llm::is_mercury_model(model) {
        return ExtractionBudget {
            input_tokens,
            max_output_tokens,
            initial_chars: input_tokens.saturating_mul(4),
            max_entities: (max_output_tokens / 25).clamp(30, 80),
        };
    }

    let max_entities = (max_output_tokens / EXTRACTION_TOKENS_PER_ENTITY_BUDGET).clamp(12, 32);
    ExtractionBudget {
        input_tokens,
        max_output_tokens,
        initial_chars: input_tokens
            .saturating_mul(4)
            .min(max_entities.saturating_mul(EXTRACTION_CHARS_PER_ENTITY_BUDGET)),
        max_entities,
    }
}

fn model_context_tokens(model: &LlamaModelResponse) -> Option<usize> {
    model
        .meta
        .as_ref()
        .and_then(|meta| meta.n_ctx.or(meta.n_ctx_train))
        .and_then(|value| usize::try_from(value).ok())
}

fn extraction_schema(max_entities: usize) -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["entities"],
        "properties": {
            "entities": {
                "type": "array",
                "maxItems": max_entities,
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

fn extraction_messages(
    model: &str,
    title: &str,
    chunk: &str,
    interests: &[EntityInterestDefinition],
    max_entities: usize,
) -> serde_json::Value {
    let interest_guidance = if interests.is_empty() {
        "No custom user interests are configured.".to_string()
    } else {
        interests
            .iter()
            .filter(|interest| interest.enabled)
            .map(|interest| {
                if interest.description.trim().is_empty() {
                    format!("- {}", interest.name.trim())
                } else {
                    format!(
                        "- {}: {}",
                        interest.name.trim(),
                        interest.description.trim()
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let system_content = if is_bonsai_model(model) || llm::is_mercury_model(model) {
        concat!(
            "Extract important named entities and durable knowledge keywords from the note. ",
            "Use only these entity types: PERSON, ORGANIZATION, LOCATION, EVENT, DATE, TIME, ",
            "DATETIME, PRODUCT, PROJECT, TECHNOLOGY, CONCEPT, OTHER. ",
            "Canonicalize names conservatively. Do not merge people or organizations merely ",
            "because their names are similar. Include meaningful concepts, but omit generic ",
            "words and incidental nouns. surface_text must be text appearing in the input. ",
            "Return one JSON object with exactly this shape: ",
            "{\"entities\":[{\"name\":\"canonical name\",\"entity_type\":\"PERSON\",",
            "\"surface_text\":\"exact input text\"}]}. Every entity must contain exactly the ",
            "keys name, entity_type, and surface_text. Do not use a key named type. Do not ",
            "repeat an entity or list category labels as entities. Stop immediately after the ",
            "closing JSON brace. Do not include markdown, explanations, thinking text, or ",
            "thinking tags."
        )
    } else {
        concat!(
            "Extract important named entities and durable knowledge keywords from the note. ",
            "Use only these entity types: PERSON, ORGANIZATION, LOCATION, EVENT, DATE, TIME, ",
            "DATETIME, PRODUCT, PROJECT, TECHNOLOGY, CONCEPT, OTHER. ",
            "Canonicalize names conservatively. Do not merge people or organizations merely ",
            "because their names are similar. Include meaningful concepts, but omit generic ",
            "words and incidental nouns. surface_text must be text appearing in the input. ",
            "Return only the schema-constrained JSON. Do not include markdown, explanations, ",
            "thinking text, or thinking tags."
        )
    };
    let user_content = if is_bonsai_model(model) || llm::is_mercury_model(model) {
        format!(
            "Return at most {max_entities} unique entities.\n\nUser is especially interested in these entity categories:\n{interest_guidance}\n\nNote title: {title}\n\nNote chunk:\n{chunk}"
        )
    } else {
        format!(
            "User is especially interested in these entity categories:\n{interest_guidance}\n\nNote title: {title}\n\nNote chunk:\n{chunk}"
        )
    };

    json!([
        {
            "role": "system",
            "content": system_content
        },
        {
            "role": "user",
            "content": user_content
        }
    ])
}

fn extraction_request_payload(
    model: &str,
    is_remote: bool,
    budget: ExtractionBudget,
    title: &str,
    chunk: &str,
    interests: &[EntityInterestDefinition],
    response_mode: ExtractionResponseMode,
) -> serde_json::Value {
    let mut payload = json!({
        "model": model,
        "messages": extraction_messages(model, title, chunk, interests, budget.max_entities),
        "temperature": if is_bonsai_model(model) { 0.0 } else { 0.1 },
        "top_p": 0.9,
        "max_tokens": budget.max_output_tokens,
        "stream": false
    });
    if !is_remote {
        payload["reasoning_format"] = json!("none");
        payload["chat_template_kwargs"] = json!({ "enable_thinking": false });
    }

    let response_format = match response_mode {
        ExtractionResponseMode::StrictSchema => Some(json!({
            "type": "json_schema",
            "json_schema": {
                "name": "note_entities",
                "strict": true,
                "schema": extraction_schema(budget.max_entities)
            }
        })),
        ExtractionResponseMode::JsonObject => Some(json!({ "type": "json_object" })),
        ExtractionResponseMode::PlainJson => None,
    };
    if let Some(response_format) = response_format {
        payload
            .as_object_mut()
            .expect("payload is an object")
            .insert("response_format".to_string(), response_format);
    }

    payload
}

fn is_grammar_sampler_error(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("grammar sampler")
        || lower.contains("error initializing grammar")
        || lower.contains("failed to initialize samplers")
        || lower.contains("common_sampler_init")
        || lower.contains("generation prompt:")
}

fn extraction_response_mode(config: &LlamaConfig, model: &str) -> ExtractionResponseMode {
    if !is_bonsai_model(model) && config.default_provider != llm::LlmProvider::Remote {
        return ExtractionResponseMode::StrictSchema;
    }
    let key = format!("{}\n{model}", config.base_url.trim_end_matches('/'));
    EXTRACTION_RESPONSE_MODES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|modes| modes.get(&key).copied())
        .unwrap_or(ExtractionResponseMode::StrictSchema)
}

fn remember_extraction_response_mode(
    config: &LlamaConfig,
    model: &str,
    response_mode: ExtractionResponseMode,
) {
    if !is_bonsai_model(model) && config.default_provider != llm::LlmProvider::Remote {
        return;
    }
    let key = format!("{}\n{model}", config.base_url.trim_end_matches('/'));
    if let Ok(mut modes) = EXTRACTION_RESPONSE_MODES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        modes.insert(key, response_mode);
    }
}

fn next_extraction_response_mode(
    model: &str,
    is_remote: bool,
    response_mode: ExtractionResponseMode,
) -> Option<ExtractionResponseMode> {
    match response_mode {
        ExtractionResponseMode::StrictSchema if is_bonsai_model(model) || is_remote => {
            Some(ExtractionResponseMode::JsonObject)
        }
        ExtractionResponseMode::StrictSchema => Some(ExtractionResponseMode::PlainJson),
        ExtractionResponseMode::JsonObject => Some(ExtractionResponseMode::PlainJson),
        ExtractionResponseMode::PlainJson => None,
    }
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

fn find_mention_location(
    content: &str,
    surface_text: &str,
    context: Option<&str>,
    used_ranges: &[(usize, usize)],
) -> MentionLocation {
    let surface = surface_text.trim();
    if surface.is_empty() {
        return MentionLocation {
            start_offset: None,
            end_offset: None,
            match_status: "missing".to_string(),
        };
    }

    let mut candidates = find_case_insensitive_ranges(content, surface);
    let mut status = "exact";
    if candidates.is_empty() {
        candidates = find_case_insensitive_ranges(content, &normalize_entity_name(surface));
        status = "approximate";
    }

    if candidates.is_empty() {
        return MentionLocation {
            start_offset: None,
            end_offset: None,
            match_status: "missing".to_string(),
        };
    }

    let context_center = context.and_then(|value| {
        let context_ranges = find_case_insensitive_ranges(content, value.trim());
        context_ranges
            .first()
            .map(|(start, end)| start.saturating_add((end - start) / 2))
    });

    candidates.sort_by_key(|(start, end)| {
        let used_penalty = if used_ranges
            .iter()
            .any(|(used_start, used_end)| start < used_end && end > used_start)
        {
            content.len()
        } else {
            0
        };
        let context_distance = context_center
            .map(|center| {
                let candidate_center = start.saturating_add((end - start) / 2);
                candidate_center.abs_diff(center)
            })
            .unwrap_or(0);
        used_penalty + context_distance + *start
    });

    let (start, end) = candidates[0];
    MentionLocation {
        start_offset: Some(start as i64),
        end_offset: Some(end as i64),
        match_status: status.to_string(),
    }
}

fn find_case_insensitive_ranges(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Vec::new();
    }

    let haystack_lower = haystack.to_lowercase();
    let needle_lower = needle.to_lowercase();
    let mut ranges = Vec::new();
    let mut search_from = 0;
    while let Some(relative) = haystack_lower[search_from..].find(&needle_lower) {
        let start = search_from + relative;
        let end = start + needle_lower.len();
        if haystack.is_char_boundary(start) && haystack.is_char_boundary(end) {
            ranges.push((start, end));
        }
        search_from = end;
        if search_from >= haystack_lower.len() {
            break;
        }
    }
    ranges
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

fn initial_note_chunks(content: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for paragraph in content
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
    {
        if paragraph.chars().count() > max_chars {
            if !current.is_empty() {
                chunks.push(current.trim().to_string());
                current.clear();
            }
            let mut pending = VecDeque::from([paragraph.to_string()]);
            while let Some(part) = pending.pop_front() {
                if part.chars().count() <= max_chars {
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
        if current.chars().count() + separator + paragraph.chars().count() > max_chars
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
    interests: &[EntityInterestDefinition],
    max_entities: usize,
) -> Option<usize> {
    if config.default_provider == llm::LlmProvider::Remote {
        return None;
    }
    let response = client
        .post(llama_endpoint(
            &config.base_url,
            "/v1/chat/completions/input_tokens",
        ))
        .json(&json!({
            "model": model,
            "messages": extraction_messages(model, title, chunk, interests, max_entities)
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
    budget: ExtractionBudget,
    title: &str,
    content: &str,
    interests: &[EntityInterestDefinition],
) -> Result<Vec<String>, String> {
    let mut pending = VecDeque::from(initial_note_chunks(content, budget.initial_chars));
    let mut chunks = Vec::new();

    while let Some(chunk) = pending.pop_front() {
        let token_count = count_extraction_input_tokens(
            client,
            config,
            model,
            title,
            &chunk,
            interests,
            budget.max_entities,
        )
        .await;
        let fits = token_count
            .map(|count| count <= budget.input_tokens)
            .unwrap_or_else(|| chunk.chars().count() <= budget.initial_chars);
        if fits {
            chunks.push(chunk);
            continue;
        }

        if chunk.chars().count() <= EXTRACTION_MIN_SPLIT_CHARS {
            return Err(format!(
                "A note segment cannot fit within the {} token extraction budget",
                budget.input_tokens
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
    budget: ExtractionBudget,
    title: &str,
    chunk: &str,
    interests: &[EntityInterestDefinition],
) -> Result<Vec<ExtractedEntity>, ChunkExtractionError> {
    async fn request_completion(
        client: &reqwest::Client,
        config: &LlamaConfig,
        model: &str,
        budget: ExtractionBudget,
        title: &str,
        chunk: &str,
        interests: &[EntityInterestDefinition],
        response_mode: ExtractionResponseMode,
    ) -> Result<(reqwest::StatusCode, String), ChunkExtractionError> {
        let response = client
            .post(llama_endpoint(&config.base_url, "/v1/chat/completions"))
            .json(&extraction_request_payload(
                model,
                config.default_provider == llm::LlmProvider::Remote,
                budget,
                title,
                chunk,
                interests,
                response_mode,
            ))
            .send()
            .await
            .map_err(|error| {
                ChunkExtractionError::Retry(format!("llama.cpp request failed: {error}"))
            })?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Ok((status, body))
    }

    fn http_extraction_error(status: reqwest::StatusCode, body: &str) -> ChunkExtractionError {
        let message = format!(
            "llama.cpp returned HTTP {}: {}",
            status.as_u16(),
            truncate_chars(body.trim(), 300)
        );
        if status == reqwest::StatusCode::BAD_REQUEST
            || status == reqwest::StatusCode::PAYLOAD_TOO_LARGE
        {
            ChunkExtractionError::Split(message)
        } else {
            ChunkExtractionError::Retry(message)
        }
    }

    let mut response_mode = extraction_response_mode(config, model);
    let response = loop {
        let (status, response) = request_completion(
            client,
            config,
            model,
            budget,
            title,
            chunk,
            interests,
            response_mode,
        )
        .await?;
        if status.is_success() {
            break response;
        }
        let is_remote = config.default_provider == llm::LlmProvider::Remote;
        let unsupported_format = is_remote && status == reqwest::StatusCode::BAD_REQUEST;
        if !is_grammar_sampler_error(&response) && !unsupported_format {
            return Err(http_extraction_error(status, &response));
        }

        let fallback_mode = match next_extraction_response_mode(model, is_remote, response_mode) {
            Some(fallback_mode) => fallback_mode,
            None => {
                return Err(http_extraction_error(status, &response));
            }
        };
        #[cfg(debug_assertions)]
        eprintln!(
            "[smooth:llm-extraction-fallback]\nmodel={model}\nfrom={}\nto={}\nerror={}",
            response_mode.log_name(),
            fallback_mode.log_name(),
            truncate_chars(response.trim(), 1000)
        );
        remember_extraction_response_mode(config, model, fallback_mode);
        response_mode = fallback_mode;
    };

    #[cfg(debug_assertions)]
    println!(
        "[smooth:llm-extraction-response]\nmodel={model}\nmode={}\ninput_budget={}\noutput_budget={}\nchunk_chars={}\nraw_response={response}",
        response_mode.log_name(),
        budget.input_tokens,
        budget.max_output_tokens,
        chunk.chars().count(),
        response = truncate_chars(&response, 4000)
    );

    let response = serde_json::from_str::<ChatCompletionResponse>(&response).map_err(|error| {
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
    budget: ExtractionBudget,
    title: &str,
    chunk: String,
    interests: &[EntityInterestDefinition],
) -> Result<Vec<ExtractedEntity>, String> {
    let mut pending = VecDeque::from([chunk]);
    let mut entities = Vec::new();

    while let Some(part) = pending.pop_front() {
        match extract_chunk_entities(client, config, model, budget, title, &part, interests).await {
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

pub(crate) fn app_meta_value(connection: &Connection, key: &str) -> Result<Option<String>, String> {
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
    let saved_default_provider = app_meta_value(connection, "llm_default_provider")?;
    let legacy_inception_config = saved_default_provider.as_deref() == Some("inception");
    Ok(LlamaConfig {
        default_provider: match saved_default_provider.as_deref() {
            Some("remote" | "inception") => llm::LlmProvider::Remote,
            _ => llm::LlmProvider::Local,
        },
        always_obey_global_llm: app_meta_value(connection, "always_obey_global_llm")?
            .map(|value| value == "true")
            .unwrap_or(false),
        mode: match app_meta_value(connection, "llama_mode")?.as_deref() {
            Some("external") => LlamaMode::External,
            _ => LlamaMode::Managed,
        },
        base_url: app_meta_value(connection, "llama_base_url")?
            .unwrap_or_else(|| "http://127.0.0.1:8080".to_string()),
        preferred_model: app_meta_value(connection, "llama_preferred_model")?
            .filter(|value| !value.is_empty()),
        managed_model: app_meta_value(connection, "llama_managed_model")?
            .unwrap_or_else(|| "unsloth/gemma-4-12B-it-qat-GGUF:UD-Q4_K_XL".to_string()),
        context_size: app_meta_value(connection, "llama_context_size")?
            .and_then(|value| value.parse().ok())
            .unwrap_or(8192),
        gpu_layers: app_meta_value(connection, "llama_gpu_layers")?
            .and_then(|value| value.parse().ok())
            .unwrap_or(999),
        flash_attention: app_meta_value(connection, "llama_flash_attention")?
            .map(|value| value != "false")
            .unwrap_or(true),
        parallel: app_meta_value(connection, "llama_parallel")?
            .and_then(|value| value.parse().ok())
            .unwrap_or(1),
        cache_ram_mb: app_meta_value(connection, "llama_cache_ram_mb")?
            .and_then(|value| value.parse().ok())
            .unwrap_or(2048),
        context_checkpoints: app_meta_value(connection, "llama_context_checkpoints")?
            .and_then(|value| value.parse().ok())
            .unwrap_or(2),
        cache_type_k: app_meta_value(connection, "llama_cache_type_k")?
            .unwrap_or_else(|| "q8_0".to_string()),
        cache_type_v: app_meta_value(connection, "llama_cache_type_v")?
            .unwrap_or_else(|| "q8_0".to_string()),
        spec_type: app_meta_value(connection, "llama_spec_type")?
            .unwrap_or_else(|| "draft-mtp".to_string()),
        spec_draft_n_max: app_meta_value(connection, "llama_spec_draft_n_max")?
            .and_then(|value| value.parse().ok())
            .unwrap_or(2),
        remote_base_url: app_meta_value(connection, "remote_base_url")?
            .or(app_meta_value(connection, "inception_base_url")?)
            .or_else(|| legacy_inception_config.then(|| "https://api.inceptionlabs.ai".to_string()))
            .unwrap_or_else(default_remote_base_url),
        remote_model: app_meta_value(connection, "remote_model")?
            .or(app_meta_value(connection, "inception_model")?)
            .or_else(|| legacy_inception_config.then(|| "mercury-2".to_string()))
            .unwrap_or_else(default_remote_model),
        remote_api_key: None,
        clear_remote_api_key: false,
        remote_api_key_configured: std::env::var("OPENAI_API_KEY")
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
            || std::env::var("INCEPTION_API_KEY")
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
            || app_meta_value(connection, "remote_api_key")?
                .or(app_meta_value(connection, "inception_api_key")?)
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false),
        remote_context_tokens: app_meta_value(connection, "remote_context_tokens")?
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(default_remote_context_tokens),
    })
}

fn managed_llama_launch_config(config: &LlamaConfig) -> llama_runtime::ManagedLlamaLaunchConfig {
    llama_runtime::ManagedLlamaLaunchConfig {
        model: config.managed_model.clone(),
        context_size: config.context_size,
        gpu_layers: config.gpu_layers,
        flash_attention: config.flash_attention,
        parallel: config.parallel,
        cache_ram_mb: config.cache_ram_mb,
        context_checkpoints: config.context_checkpoints,
        cache_type_k: config.cache_type_k.clone(),
        cache_type_v: config.cache_type_v.clone(),
        spec_type: config.spec_type.clone(),
        spec_draft_n_max: config.spec_draft_n_max,
    }
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

fn validate_remote_llm_base_url(value: &str) -> Result<String, String> {
    let mut url = reqwest::Url::parse(value.trim())
        .map_err(|_| "Enter a valid OpenAI-compatible API URL".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("The remote API URL must use http or https".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "The remote API URL must include a host".to_string())?;
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .map(|address| address.is_loopback())
            .unwrap_or(false);
    if url.scheme() == "http" && !is_loopback {
        return Err(
            "Remote API URLs must use https; http is allowed only for localhost".to_string(),
        );
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.as_str().trim_end_matches('/').to_string())
}

pub(crate) fn llama_endpoint(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    let path = if base.ends_with("/v1") {
        path.strip_prefix("v1/").unwrap_or(path)
    } else {
        path
    };
    format!("{base}/{path}")
}

pub(crate) fn is_bonsai_model(model: &str) -> bool {
    model.to_ascii_lowercase().contains("bonsai")
}

fn llama_status(
    config: &LlamaConfig,
    state: LlamaConnectionState,
    message: impl Into<String>,
    latency_ms: Option<u64>,
    models: Vec<LlamaModel>,
    managed: Option<llama_runtime::ManagedLlamaSnapshot>,
) -> LlamaStatus {
    LlamaStatus {
        state,
        base_url: config.base_url.clone(),
        message: message.into(),
        latency_ms,
        checked_at: now_string(),
        models,
        managed,
    }
}

async fn llama_server_ready(client: &reqwest::Client, config: &LlamaConfig) -> bool {
    let path = if config.default_provider == llm::LlmProvider::Remote {
        "/v1/models"
    } else {
        "/health"
    };
    client
        .get(llama_endpoint(&config.base_url, path))
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

async fn fetch_llama_models(
    client: &reqwest::Client,
    config: &LlamaConfig,
) -> Result<Vec<LlamaModelResponse>, String> {
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
        .map(|response| response.data)
        .map_err(|error| format!("Invalid model discovery response: {error}"))
}

async fn resolve_extraction_model(
    client: &reqwest::Client,
    config: &LlamaConfig,
) -> Result<ExtractionModel, String> {
    if let Some(model) = config.preferred_model.as_ref() {
        let context_tokens = fetch_llama_models(client, config)
            .await
            .ok()
            .and_then(|models| {
                models
                    .into_iter()
                    .find(|candidate| candidate.id == *model)
                    .and_then(|candidate| model_context_tokens(&candidate))
            });
        return Ok(ExtractionModel {
            id: model.clone(),
            context_tokens,
        });
    }

    let model = fetch_llama_models(client, config)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| "llama.cpp has no available model".to_string())?;
    Ok(ExtractionModel {
        context_tokens: model_context_tokens(&model),
        id: model.id,
    })
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
            SELECT id, note_id, content_hash, attempts, max_attempts,
                   llm_provider, llm_model
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
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?;
    let Some((id, note_id, content_hash, attempts, max_attempts, provider, model)) = job else {
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
        selection: provider.map(|provider| llm::LlmSelection {
            provider: Some(if matches!(provider.as_str(), "remote" | "inception") {
                llm::LlmProvider::Remote
            } else {
                llm::LlmProvider::Local
            }),
            model,
        }),
    }))
}

fn has_available_extraction_job(app: &AppHandle) -> Result<bool, String> {
    let connection = open_database(app)?;
    connection
        .query_row(
            "
            SELECT EXISTS(
                SELECT 1 FROM extraction_jobs
                WHERE status = 'pending' AND available_at <= ?1
            )
            ",
            params![now_string()],
            |row| row.get(0),
        )
        .map_err(db_error)
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

fn resolve_or_create_entity(
    transaction: &rusqlite::Transaction<'_>,
    canonical_name: &str,
    normalized_name: &str,
    entity_type: &str,
    now: &str,
) -> Result<i64, String> {
    if let Some(alias_entity_id) = transaction
        .query_row(
            "
            SELECT entity_id
            FROM entity_aliases
            WHERE normalized_alias = ?1 AND entity_type = ?2
            ORDER BY entity_id ASC
            LIMIT 1
            ",
            params![normalized_name, entity_type],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(db_error)?
    {
        transaction
            .execute(
                "UPDATE entities SET updated_at = ?1 WHERE id = ?2",
                params![now, alias_entity_id],
            )
            .map_err(db_error)?;
        return Ok(alias_entity_id);
    }

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
            params![canonical_name, normalized_name, entity_type, now],
        )
        .map_err(db_error)?;
    let entity_id = transaction
        .query_row(
            "
            SELECT id FROM entities
            WHERE normalized_name = ?1 AND entity_type = ?2
            ",
            params![normalized_name, entity_type],
            |row| row.get::<_, i64>(0),
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "
            INSERT OR IGNORE INTO entity_aliases (
                entity_id, alias, normalized_alias, entity_type
            )
            VALUES (?1, ?2, ?3, ?4)
            ",
            params![entity_id, canonical_name, normalized_name, entity_type],
        )
        .map_err(db_error)?;
    Ok(entity_id)
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

    let content = read_note_content(app, &job.note_id)?;
    let now = now_string();
    let mut entity_ids = HashMap::<(String, String), i64>::new();
    let mut mention_keys = HashSet::new();
    let mut used_ranges = Vec::<(usize, usize)>::new();
    for (chunk_index, entity) in entities {
        let normalized_name = normalize_entity_name(&entity.name);
        let entity_key = (normalized_name.clone(), entity.entity_type.clone());
        let entity_id = if let Some(id) = entity_ids.get(&entity_key) {
            *id
        } else {
            let id = resolve_or_create_entity(
                &transaction,
                &entity.name,
                &normalized_name,
                &entity.entity_type,
                &now,
            )?;
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
                        entity_id, alias, normalized_alias, entity_type
                    )
                    VALUES (?1, ?2, ?3, ?4)
                    ",
                    params![entity_id, alias, normalized_alias, entity.entity_type],
                )
                .map_err(db_error)?;
        }

        let context = entity.context.as_deref().unwrap_or("");
        let location = find_mention_location(
            &content,
            &entity.surface_text,
            entity.context.as_deref(),
            &used_ranges,
        );
        if let (Some(start), Some(end)) = (location.start_offset, location.end_offset) {
            used_ranges.push((start as usize, end as usize));
        }
        let mention_key = format!(
            "{}|{}|{}|{}|{}",
            entity_key.0,
            entity_key.1,
            normalize_entity_name(&entity.surface_text),
            normalize_entity_name(context),
            location.start_offset.unwrap_or(-1)
        );
        if !mention_keys.insert(mention_key) {
            continue;
        }
        transaction
            .execute(
                "
                INSERT INTO entity_mentions (
                    note_id, entity_id, surface_text, context, chunk_index,
                    confidence, start_offset, end_offset, match_status, created_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ",
                params![
                    job.note_id,
                    entity_id,
                    entity.surface_text,
                    entity.context,
                    chunk_index as i64,
                    entity.confidence,
                    location.start_offset,
                    location.end_offset,
                    location.match_status,
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
    model: &ExtractionModel,
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

    let budget = extraction_budget(model.context_tokens, &model.id);
    let interests = load_enabled_entity_interests(app)?;
    let chunks = fit_chunks_to_context(
        client,
        config,
        &model.id,
        budget,
        &note.title,
        &content,
        &interests,
    )
    .await?;
    let mut all_entities = Vec::new();
    for (chunk_index, chunk) in chunks.iter().enumerate() {
        if !extraction_job_is_current(app, job)? {
            return Ok(());
        }
        let entities = extract_entities_adaptively(
            client,
            config,
            &model.id,
            budget,
            &note.title,
            chunk.clone(),
            &interests,
        )
        .await?;
        all_entities.extend(entities.into_iter().map(|entity| (chunk_index, entity)));
    }
    persist_extraction_results(app, job, all_entities)?;
    Ok(())
}

async fn extraction_worker(app: AppHandle) {
    loop {
        if !has_available_extraction_job(&app).unwrap_or(false) {
            tokio::time::sleep(Duration::from_secs(EXTRACTION_POLL_SECONDS)).await;
            continue;
        }
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
        let target = match llm::resolve_target(&app, job.selection.as_ref()) {
            Ok(target) => target,
            Err(error) => {
                let _ = fail_extraction_job(&app, &job, &error);
                continue;
            }
        };
        let mut config =
            match open_database(&app).and_then(|connection| load_llama_config(&connection)) {
                Ok(config) => config,
                Err(error) => {
                    let _ = fail_extraction_job(&app, &job, &error);
                    continue;
                }
            };
        config.default_provider = target.provider;
        config.base_url = target.base_url.clone();
        config.preferred_model = target.model.clone();
        let client = match target.client_builder().and_then(|builder| {
            builder
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(180))
                .build()
                .map_err(|error| error.to_string())
        }) {
            Ok(client) => client,
            Err(error) => {
                let _ = fail_extraction_job(&app, &job, &error);
                continue;
            }
        };
        if !llama_server_ready(&client, &config).await {
            let _ = fail_extraction_job(
                &app,
                &job,
                &format!("{} is unavailable", target.provider_name()),
            );
            continue;
        }
        let model = match resolve_extraction_model(&client, &config).await {
            Ok(mut model) => {
                model.context_tokens = target.context_tokens.or(model.context_tokens);
                model
            }
            Err(error) => {
                let _ = fail_extraction_job(&app, &job, &error);
                continue;
            }
        };

        if let Err(error) = process_extraction_job(&app, &client, &config, &model, &job).await {
            let _ = fail_extraction_job(&app, &job, &error);
        }
    }
}

pub(crate) fn read_note_content(app: &AppHandle, note_id: &str) -> Result<String, String> {
    let path = note_path(app, note_id)?;
    if !path.exists() {
        return Ok(String::new());
    }

    fs::read_to_string(path).map_err(|error| error.to_string())
}

pub(crate) fn write_note_content(
    app: &AppHandle,
    note_id: &str,
    content: &str,
) -> Result<(), String> {
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

fn normalize_directed_pair(source_id: &str, target_id: &str) -> Option<(String, String)> {
    if source_id == target_id {
        return None;
    }

    Some((source_id.to_string(), target_id.to_string()))
}

fn normalize_link_label(label: Option<String>) -> Option<String> {
    label
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_link_kind(link_kind: Option<String>) -> Result<String, String> {
    match link_kind.as_deref().unwrap_or("manual") {
        "manual" => Ok("manual".to_string()),
        "entity_sharing" => Ok("entity_sharing".to_string()),
        _ => Err("Unsupported note link type".to_string()),
    }
}

pub(crate) fn load_note_meta(connection: &Connection, id: &str) -> Result<NoteMeta, String> {
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

pub(crate) fn snapshot(app: &AppHandle, connection: &Connection) -> Result<BankSnapshot, String> {
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
            .prepare("SELECT id, name, created_at, system_key FROM folders ORDER BY created_at ASC")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(Folder {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                    system_key: row.get(3)?,
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
                SELECT source_id, target_id, created_at, label, link_kind
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
                    label: row.get(3)?,
                    link_kind: row.get(4)?,
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
fn set_always_obey_global_llm(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let connection = open_database(&app)?;
    connection
        .execute(
            "
            INSERT INTO app_meta (key, value)
            VALUES ('always_obey_global_llm', ?1)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            ",
            params![enabled.to_string()],
        )
        .map_err(db_error)?;
    Ok(enabled)
}

#[tauri::command]
fn save_llama_config(app: AppHandle, config: LlamaConfig) -> Result<LlamaConfig, String> {
    let base_url = validate_llama_base_url(&config.base_url)?;
    let remote_base_url = if config.remote_base_url.trim().is_empty() {
        if config.default_provider == llm::LlmProvider::Remote {
            return Err("Enter a remote OpenAI-compatible API URL".to_string());
        }
        String::new()
    } else {
        validate_remote_llm_base_url(&config.remote_base_url)?
    };
    let remote_model = config.remote_model.trim().to_string();
    if remote_model.is_empty() && config.default_provider == llm::LlmProvider::Remote {
        return Err("Enter a remote model name".to_string());
    }
    if !(1024..=2_000_000).contains(&config.remote_context_tokens) {
        return Err("Remote context size must be between 1024 and 2000000".to_string());
    }
    let preferred_model = if config.mode == LlamaMode::Managed {
        None
    } else {
        config
            .preferred_model
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let managed_model = config.managed_model.trim().to_string();
    if managed_model.is_empty() {
        return Err("Enter a Hugging Face model reference for managed llama.cpp".to_string());
    }
    if !(512..=262_144).contains(&config.context_size) {
        return Err("Context size must be between 512 and 262144".to_string());
    }
    if config.parallel == 0 || config.parallel > 32 {
        return Err("Parallel slots must be between 1 and 32".to_string());
    }
    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    let values = [
        (
            "llm_default_provider",
            match config.default_provider {
                llm::LlmProvider::Local => "local".to_string(),
                llm::LlmProvider::Remote => "remote".to_string(),
            },
        ),
        (
            "always_obey_global_llm",
            config.always_obey_global_llm.to_string(),
        ),
        (
            "llama_mode",
            match config.mode {
                LlamaMode::Managed => "managed".to_string(),
                LlamaMode::External => "external".to_string(),
            },
        ),
        ("llama_base_url", base_url.clone()),
        (
            "llama_preferred_model",
            preferred_model.clone().unwrap_or_default(),
        ),
        ("llama_managed_model", managed_model.clone()),
        ("llama_context_size", config.context_size.to_string()),
        ("llama_gpu_layers", config.gpu_layers.to_string()),
        ("llama_flash_attention", config.flash_attention.to_string()),
        ("llama_parallel", config.parallel.to_string()),
        ("llama_cache_ram_mb", config.cache_ram_mb.to_string()),
        (
            "llama_context_checkpoints",
            config.context_checkpoints.to_string(),
        ),
        ("llama_cache_type_k", config.cache_type_k.clone()),
        ("llama_cache_type_v", config.cache_type_v.clone()),
        ("llama_spec_type", config.spec_type.clone()),
        (
            "llama_spec_draft_n_max",
            config.spec_draft_n_max.to_string(),
        ),
        ("remote_base_url", remote_base_url.clone()),
        ("remote_model", remote_model.clone()),
        (
            "remote_context_tokens",
            config.remote_context_tokens.to_string(),
        ),
    ];
    for (key, value) in values {
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
    }
    if config.clear_remote_api_key {
        transaction
            .execute(
                "DELETE FROM app_meta WHERE key IN ('remote_api_key', 'inception_api_key')",
                [],
            )
            .map_err(db_error)?;
    } else if let Some(api_key) = config
        .remote_api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        transaction
            .execute(
                "INSERT INTO app_meta (key, value) VALUES ('remote_api_key', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![api_key],
            )
            .map_err(db_error)?;
    }
    transaction.commit().map_err(db_error)?;

    app.state::<llama_runtime::LlamaRuntimeState>().stop()?;

    Ok(LlamaConfig {
        default_provider: config.default_provider,
        always_obey_global_llm: config.always_obey_global_llm,
        mode: config.mode,
        base_url,
        preferred_model,
        managed_model,
        context_size: config.context_size,
        gpu_layers: config.gpu_layers,
        flash_attention: config.flash_attention,
        parallel: config.parallel,
        cache_ram_mb: config.cache_ram_mb,
        context_checkpoints: config.context_checkpoints,
        cache_type_k: config.cache_type_k,
        cache_type_v: config.cache_type_v,
        spec_type: config.spec_type,
        spec_draft_n_max: config.spec_draft_n_max,
        remote_base_url,
        remote_model,
        remote_api_key: None,
        clear_remote_api_key: false,
        remote_api_key_configured: !config.clear_remote_api_key
            && (config.remote_api_key_configured
                || config
                    .remote_api_key
                    .as_deref()
                    .map(str::trim)
                    .map(|value| !value.is_empty())
                    .unwrap_or(false)
                || std::env::var("OPENAI_API_KEY")
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false)
                || std::env::var("INCEPTION_API_KEY")
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false)),
        remote_context_tokens: config.remote_context_tokens,
    })
}

#[tauri::command]
async fn test_remote_llm_connection(app: AppHandle) -> Result<Vec<LlamaModel>, String> {
    let selection = llm::LlmSelection {
        provider: Some(llm::LlmProvider::Remote),
        model: None,
    };
    let target = llm::resolve_target(&app, Some(&selection))?;
    let client = target
        .client_builder()?
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(llama_endpoint(&target.base_url, "/v1/models"))
        .send()
        .await
        .map_err(|error| format!("Unable to reach remote API: {error}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Remote API returned HTTP {}: {}",
            status.as_u16(),
            truncate_chars(body.trim(), 300)
        ));
    }
    let models = response
        .json::<LlamaModelsResponse>()
        .await
        .map_err(|error| format!("Invalid OpenAI-compatible models response: {error}"))?;
    Ok(models
        .data
        .into_iter()
        .map(|model| {
            let context_size = model_context_tokens(&model).map(|value| value as u64);
            let parameter_count = model.meta.as_ref().and_then(|meta| meta.n_params);
            let size_bytes = model.meta.as_ref().and_then(|meta| meta.size);
            LlamaModel {
                id: model.id,
                owned_by: model.owned_by,
                context_size,
                parameter_count,
                size_bytes,
            }
        })
        .collect())
}

#[tauri::command]
async fn get_llama_status(app: AppHandle) -> Result<LlamaStatus, String> {
    let mut config = {
        let connection = open_database(&app)?;
        load_llama_config(&connection)?
    };
    let managed = if config.mode == LlamaMode::Managed {
        let snapshot = app
            .state::<llama_runtime::LlamaRuntimeState>()
            .snapshot(&app);
        if !snapshot.running {
            let message = snapshot
                .last_error
                .clone()
                .unwrap_or_else(|| "Managed llama.cpp is stopped".to_string());
            return Ok(llama_status(
                &config,
                LlamaConnectionState::Offline,
                message,
                None,
                Vec::new(),
                Some(snapshot),
            ));
        }
        config.base_url = snapshot
            .endpoint
            .clone()
            .ok_or_else(|| "Managed llama.cpp has no endpoint".to_string())?;
        Some(snapshot)
    } else {
        None
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
            let (state, message) = if managed.is_some() {
                (
                    LlamaConnectionState::Loading,
                    "llama.cpp is downloading or loading the model",
                )
            } else {
                (LlamaConnectionState::Offline, "llama.cpp is not reachable")
            };
            return Ok(llama_status(
                &config,
                state,
                message,
                None,
                Vec::new(),
                managed.clone(),
            ));
        }
        Err(error) => {
            return Ok(llama_status(
                &config,
                LlamaConnectionState::Error,
                error.to_string(),
                None,
                Vec::new(),
                managed.clone(),
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
            managed.clone(),
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
            managed.clone(),
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
        managed,
    ))
}

#[tauri::command]
async fn start_llama_server(app: AppHandle) -> Result<LlamaStatus, String> {
    let config = {
        let connection = open_database(&app)?;
        load_llama_config(&connection)?
    };
    if config.mode != LlamaMode::Managed {
        return Err("Switch to Managed mode before starting llama.cpp".to_string());
    }
    app.state::<llama_runtime::LlamaRuntimeState>()
        .ensure_running(&app, &managed_llama_launch_config(&config))?;
    get_llama_status(app).await
}

#[tauri::command]
fn stop_llama_server(app: AppHandle) -> Result<LlamaStatus, String> {
    let connection = open_database(&app)?;
    let config = load_llama_config(&connection)?;
    drop(connection);
    app.state::<llama_runtime::LlamaRuntimeState>().stop()?;
    Ok(llama_status(
        &config,
        LlamaConnectionState::Offline,
        "Managed llama.cpp is stopped",
        None,
        Vec::new(),
        Some(
            app.state::<llama_runtime::LlamaRuntimeState>()
                .snapshot(&app),
        ),
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
    let mut connection = open_database(&app)?;
    backfill_note_mention_offsets(&app, &mut connection, &id)?;
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
    let mentions = {
        let mut statement = connection
            .prepare(
                "
                SELECT id,
                       entity_id,
                       surface_text,
                       context,
                       start_offset,
                       end_offset,
                       match_status
                FROM entity_mentions
                WHERE note_id = ?1
                ORDER BY COALESCE(start_offset, 9223372036854775807), id
                ",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map(params![id], |row| {
                Ok(NoteEntityMention {
                    id: row.get(0)?,
                    entity_id: row.get(1)?,
                    surface_text: row.get(2)?,
                    context: row.get(3)?,
                    start_offset: row.get(4)?,
                    end_offset: row.get(5)?,
                    match_status: row.get(6)?,
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
        mentions,
    })
}

fn backfill_note_mention_offsets(
    app: &AppHandle,
    connection: &mut Connection,
    note_id: &str,
) -> Result<(), String> {
    let needs_backfill = connection
        .query_row(
            "
            SELECT EXISTS(
                SELECT 1
                FROM entity_mentions
                WHERE note_id = ?1
                  AND (match_status = 'unresolved' OR match_status IS NULL)
            )
            ",
            params![note_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(db_error)?;
    if !needs_backfill {
        return Ok(());
    }

    let content = read_note_content(app, note_id)?;
    let mentions = {
        let mut statement = connection
            .prepare(
                "
                SELECT id, surface_text, context
                FROM entity_mentions
                WHERE note_id = ?1
                  AND (match_status = 'unresolved' OR match_status IS NULL)
                ORDER BY chunk_index, id
                ",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map(params![note_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        rows
    };

    let transaction = connection.transaction().map_err(db_error)?;
    let mut used_ranges = Vec::<(usize, usize)>::new();
    for (mention_id, surface_text, context) in mentions {
        let location =
            find_mention_location(&content, &surface_text, context.as_deref(), &used_ranges);
        if let (Some(start), Some(end)) = (location.start_offset, location.end_offset) {
            used_ranges.push((start as usize, end as usize));
        }
        transaction
            .execute(
                "
                UPDATE entity_mentions
                SET start_offset = ?1,
                    end_offset = ?2,
                    match_status = ?3
                WHERE id = ?4
                ",
                params![
                    location.start_offset,
                    location.end_offset,
                    location.match_status,
                    mention_id
                ],
            )
            .map_err(db_error)?;
    }
    transaction.commit().map_err(db_error)
}

#[tauri::command]
fn rename_entity(app: AppHandle, entity_id: i64, canonical_name: String) -> Result<(), String> {
    let canonical_name = truncate_chars(canonical_name.trim(), 160);
    if canonical_name.is_empty() {
        return Err("Entity name is required".to_string());
    }
    let normalized_name = normalize_entity_name(&canonical_name);
    if normalized_name.is_empty() {
        return Err("Entity name must include letters or numbers".to_string());
    }

    let mut connection = open_database(&app)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let (old_name, old_normalized, entity_type) = transaction
        .query_row(
            "
            SELECT canonical_name, normalized_name, entity_type
            FROM entities
            WHERE id = ?1
            ",
            params![entity_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| "Entity not found".to_string())?;
    let now = now_string();
    let target_id = transaction
        .query_row(
            "
            SELECT id
            FROM entities
            WHERE normalized_name = ?1 AND entity_type = ?2 AND id != ?3
            ",
            params![normalized_name, entity_type, entity_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(db_error)?;

    if let Some(target_id) = target_id {
        transaction
            .execute(
                "UPDATE entity_mentions SET entity_id = ?1 WHERE entity_id = ?2",
                params![target_id, entity_id],
            )
            .map_err(db_error)?;
        for alias in [old_name.as_str(), canonical_name.as_str()] {
            let normalized_alias = normalize_entity_name(alias);
            if normalized_alias.is_empty() {
                continue;
            }
            transaction
                .execute(
                    "
                    INSERT OR IGNORE INTO entity_aliases (
                        entity_id, alias, normalized_alias, entity_type
                    )
                    VALUES (?1, ?2, ?3, ?4)
                    ",
                    params![target_id, alias, normalized_alias, entity_type],
                )
                .map_err(db_error)?;
        }
        transaction
            .execute(
                "
                UPDATE entity_aliases
                SET entity_id = ?1
                WHERE entity_id = ?2
                  AND NOT EXISTS (
                      SELECT 1
                      FROM entity_aliases AS existing
                      WHERE existing.entity_id = ?1
                        AND existing.normalized_alias = entity_aliases.normalized_alias
                        AND existing.entity_type = entity_aliases.entity_type
                  )
                ",
                params![target_id, entity_id],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "DELETE FROM entity_aliases WHERE entity_id = ?1",
                params![entity_id],
            )
            .map_err(db_error)?;
        transaction
            .execute("DELETE FROM entities WHERE id = ?1", params![entity_id])
            .map_err(db_error)?;
        transaction
            .execute(
                "UPDATE entities SET updated_at = ?1 WHERE id = ?2",
                params![now, target_id],
            )
            .map_err(db_error)?;
    } else {
        transaction
            .execute(
                "
                UPDATE entities
                SET canonical_name = ?1,
                    normalized_name = ?2,
                    updated_at = ?3
                WHERE id = ?4
                ",
                params![canonical_name, normalized_name, now, entity_id],
            )
            .map_err(db_error)?;
        for alias in [
            old_name.as_str(),
            old_normalized.as_str(),
            canonical_name.as_str(),
        ] {
            let normalized_alias = normalize_entity_name(alias);
            if normalized_alias.is_empty() {
                continue;
            }
            transaction
                .execute(
                    "
                    INSERT OR IGNORE INTO entity_aliases (
                        entity_id, alias, normalized_alias, entity_type
                    )
                    VALUES (?1, ?2, ?3, ?4)
                    ",
                    params![entity_id, alias, normalized_alias, entity_type],
                )
                .map_err(db_error)?;
        }
    }

    transaction.commit().map_err(db_error)
}

#[tauri::command]
fn get_entity_interests(app: AppHandle) -> Result<Vec<EntityInterestDefinition>, String> {
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "
            SELECT id, name, description, enabled, sort_order
            FROM entity_interest_definitions
            ORDER BY sort_order ASC, name COLLATE NOCASE ASC
            ",
        )
        .map_err(db_error)?;
    let interests = statement
        .query_map([], |row| {
            Ok(EntityInterestDefinition {
                id: Some(row.get(0)?),
                name: row.get(1)?,
                description: row.get(2)?,
                enabled: row.get::<_, i64>(3)? != 0,
                sort_order: row.get(4)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(interests)
}

#[tauri::command]
fn save_entity_interests(
    app: AppHandle,
    interests: Vec<EntityInterestDefinition>,
) -> Result<Vec<EntityInterestDefinition>, String> {
    let mut connection = open_database(&app)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let now = now_string();
    let mut keep_ids = Vec::<i64>::new();

    for (index, interest) in interests.into_iter().enumerate() {
        let name = truncate_chars(interest.name.trim(), 80);
        if name.is_empty() {
            continue;
        }
        let description = truncate_chars(interest.description.trim(), 500);
        let enabled = if interest.enabled { 1_i64 } else { 0_i64 };
        let sort_order = index as i64;
        let id = if let Some(id) = interest.id {
            transaction
                .execute(
                    "
                    UPDATE entity_interest_definitions
                    SET name = ?1,
                        description = ?2,
                        enabled = ?3,
                        sort_order = ?4,
                        updated_at = ?5
                    WHERE id = ?6
                    ",
                    params![name, description, enabled, sort_order, now, id],
                )
                .map_err(db_error)?;
            id
        } else {
            transaction
                .execute(
                    "
                    INSERT INTO entity_interest_definitions (
                        name, description, enabled, sort_order, created_at, updated_at
                    )
                    VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                    ON CONFLICT(name) DO UPDATE SET
                        description = excluded.description,
                        enabled = excluded.enabled,
                        sort_order = excluded.sort_order,
                        updated_at = excluded.updated_at
                    ",
                    params![name, description, enabled, sort_order, now],
                )
                .map_err(db_error)?;
            transaction
                .query_row(
                    "SELECT id FROM entity_interest_definitions WHERE name = ?1 COLLATE NOCASE",
                    params![name],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(db_error)?
        };
        keep_ids.push(id);
    }

    if keep_ids.is_empty() {
        transaction
            .execute("DELETE FROM entity_interest_definitions", [])
            .map_err(db_error)?;
    } else {
        let placeholders = keep_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql =
            format!("DELETE FROM entity_interest_definitions WHERE id NOT IN ({placeholders})");
        transaction
            .execute(&sql, rusqlite::params_from_iter(keep_ids))
            .map_err(db_error)?;
    }
    transaction.commit().map_err(db_error)?;
    get_entity_interests(app)
}

fn load_enabled_entity_interests(app: &AppHandle) -> Result<Vec<EntityInterestDefinition>, String> {
    get_entity_interests(app.clone()).map(|interests| {
        interests
            .into_iter()
            .filter(|interest| interest.enabled)
            .collect()
    })
}

#[tauri::command]
fn get_link_suggestions(
    app: AppHandle,
    note_id: String,
    limit: Option<u32>,
) -> Result<Vec<LinkSuggestion>, String> {
    get_link_suggestions_internal(app, note_id, limit)
}

pub(crate) fn get_link_suggestions_internal(
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
fn enqueue_note_extraction(
    app: AppHandle,
    id: String,
    selection: Option<llm::LlmSelection>,
) -> Result<ExtractionQueueStatus, String> {
    let mut connection = open_database(&app)?;
    let note = load_note_meta(&connection, &id)?;
    if note.deleted_at.is_some() {
        return Err("Trashed notes cannot be queued for extraction".to_string());
    }
    if note.extraction_status == "disabled" {
        return Err("Entity extraction is disabled for this note".to_string());
    }

    let content = read_note_content(&app, &id)?;
    let transaction = connection.transaction().map_err(db_error)?;
    if content.trim().is_empty() {
        clear_extraction_job(&transaction, &id)?;
    } else {
        force_enqueue_extraction_with_selection(
            &transaction,
            &id,
            &content_hash(&content),
            selection.as_ref(),
        )?;
    }
    transaction.commit().map_err(db_error)?;
    get_extraction_queue_status(app)
}

/// Enqueue extraction for a meeting note once the meeting has stopped.
/// Unlike `enqueue_note_extraction`, this intentionally clears the `disabled`
/// state (meeting notes skip live extraction while recording).
#[tauri::command]
fn finalize_meeting_extraction(
    app: AppHandle,
    id: String,
) -> Result<ExtractionQueueStatus, String> {
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
        .map(|id| {
            let note = load_note_meta(&connection, id)?;
            Ok((
                id.clone(),
                note.extraction_status,
                read_note_content(&app, id)?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;

    let transaction = connection.transaction().map_err(db_error)?;
    for (id, extraction_status, content) in contents {
        if extraction_status == "disabled" {
            continue;
        }
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
    create_standard_note(app, title, folder_id)
}

pub(crate) fn create_standard_note(
    app: AppHandle,
    title: Option<String>,
    folder_id: Option<String>,
) -> Result<NoteWithContent, String> {
    create_note_with_extraction_status(app, title, folder_id, default_extraction_status())
}

#[tauri::command]
fn create_meeting_note(
    app: AppHandle,
    title: Option<String>,
    folder_id: Option<String>,
) -> Result<NoteWithContent, String> {
    create_note_with_extraction_status(app, title, folder_id, disabled_extraction_status())
}

pub(crate) fn create_note_with_extraction_status(
    app: AppHandle,
    title: Option<String>,
    folder_id: Option<String>,
    extraction_status: String,
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
        extraction_status,
    };

    write_note_content(&app, &note.id, "")?;
    if let Err(error) = connection.execute(
        "
        INSERT INTO notes (id, title, folder_id, created_at, updated_at, deleted_at, extraction_status)
        VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)
        ",
        params![
            note.id,
            note.title,
            note.folder_id,
            note.created_at,
            note.updated_at,
            note.extraction_status
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
    save_note_internal(app, id, title, content, folder_id, false)
}

#[tauri::command]
fn save_meeting_note(
    app: AppHandle,
    id: String,
    title: String,
    content: String,
    folder_id: Option<String>,
) -> Result<NoteWithContent, String> {
    save_note_internal(app, id, title, content, folder_id, true)
}

pub(crate) fn save_note_internal(
    app: AppHandle,
    id: String,
    title: String,
    content: String,
    folder_id: Option<String>,
    force_disable_extraction: bool,
) -> Result<NoteWithContent, String> {
    let mut connection = open_database(&app)?;
    let note = load_note_meta(&connection, &id)?;

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
    let extraction_disabled = force_disable_extraction || note.extraction_status == "disabled";
    transaction
        .execute(
            "
            UPDATE notes
            SET title = ?1,
                folder_id = ?2,
                updated_at = ?3,
                extraction_status = CASE WHEN ?5 THEN 'disabled' ELSE extraction_status END,
                extraction_error = CASE WHEN ?5 THEN NULL ELSE extraction_error END
            WHERE id = ?4
            ",
            params![saved_title, folder_id, updated_at, id, extraction_disabled],
        )
        .map_err(db_error)?;
    if extraction_disabled {
        transaction
            .execute(
                "DELETE FROM extraction_jobs WHERE note_id = ?1",
                params![id],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "
                UPDATE notes
                SET content_hash = NULL,
                    extraction_status = 'disabled',
                    extraction_error = NULL
                WHERE id = ?1
                ",
                params![id],
            )
            .map_err(db_error)?;
    } else if content.trim().is_empty() {
        clear_extraction_job(&transaction, &id)?;
    } else {
        enqueue_extraction(&transaction, &id, &hash)?;
    }
    if content.trim().is_empty() {
        transaction
            .execute("DELETE FROM embedding_jobs WHERE note_id = ?1", params![id])
            .map_err(db_error)?;
        transaction
            .execute("DELETE FROM note_chunks WHERE note_id = ?1", params![id])
            .map_err(db_error)?;
    } else {
        semantic_search::enqueue(&transaction, &id, &hash)?;
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
    if clean_name.eq_ignore_ascii_case(imports::IMPORTED_FOLDER_NAME) {
        return Err("Imported is a system folder".to_string());
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
fn rename_folder(app: AppHandle, id: String, name: String) -> Result<BankSnapshot, String> {
    let clean_name = name.trim();
    if clean_name.is_empty() {
        return Err("Folder name is required".to_string());
    }
    let connection = open_database(&app)?;
    if imports::is_system_folder(&connection, &id)? {
        return Err("System folders cannot be renamed".to_string());
    }
    if clean_name.eq_ignore_ascii_case(imports::IMPORTED_FOLDER_NAME) {
        return Err("Imported is a reserved folder name".to_string());
    }
    connection
        .execute(
            "UPDATE folders SET name = ?2 WHERE id = ?1",
            params![id, clean_name],
        )
        .map_err(db_error)?;
    snapshot(&app, &connection)
}

#[tauri::command]
fn delete_folder(app: AppHandle, id: String) -> Result<BankSnapshot, String> {
    let mut connection = open_database(&app)?;
    if imports::is_system_folder(&connection, &id)? {
        return Err("System folders cannot be deleted".to_string());
    }
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
fn link_notes(
    app: AppHandle,
    ids: Vec<String>,
    label: Option<String>,
    link_kind: Option<String>,
) -> Result<BankSnapshot, String> {
    let mut connection = open_database(&app)?;
    let link_kind = normalize_link_kind(link_kind)?;
    let label = if link_kind == "entity_sharing" {
        Some("Entity Sharing".to_string())
    } else {
        normalize_link_label(label)
    };
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
                normalize_directed_pair(&selected_ids[first_index], &selected_ids[second_index])
            {
                let created_at = now_string();
                transaction
                    .execute(
                        "
                        INSERT OR IGNORE INTO note_links
                            (source_id, target_id, created_at, label, link_kind)
                        VALUES (?1, ?2, ?3, ?4, ?5)
                        ",
                        params![
                            &source_id,
                            &target_id,
                            &created_at,
                            label.as_deref(),
                            link_kind.as_str()
                        ],
                    )
                    .map_err(db_error)?;
                transaction
                    .execute(
                        "
                        INSERT OR IGNORE INTO note_links
                            (source_id, target_id, created_at, label, link_kind)
                        VALUES (?1, ?2, ?3, ?4, ?5)
                        ",
                        params![
                            &target_id,
                            &source_id,
                            &created_at,
                            label.as_deref(),
                            link_kind.as_str()
                        ],
                    )
                    .map_err(db_error)?;
            }
        }
    }
    transaction.commit().map_err(db_error)?;
    snapshot(&app, &connection)
}

#[tauri::command]
fn rename_note_link(
    app: AppHandle,
    source_id: String,
    target_id: String,
    label: Option<String>,
) -> Result<BankSnapshot, String> {
    let Some((source_id, target_id)) = normalize_directed_pair(&source_id, &target_id) else {
        return Err("Cannot rename a link to the same note".to_string());
    };

    let connection = open_database(&app)?;
    let link_kind = connection
        .query_row(
            "
            SELECT link_kind
            FROM note_links
            WHERE source_id = ?1 AND target_id = ?2
            ",
            params![&source_id, &target_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| "Note link not found".to_string())?;

    if link_kind == "entity_sharing" {
        return Err("Entity Sharing links cannot be renamed".to_string());
    }

    connection
        .execute(
            "
            UPDATE note_links
            SET label = ?3
            WHERE source_id = ?1 AND target_id = ?2
            ",
            params![&source_id, &target_id, normalize_link_label(label)],
        )
        .map_err(db_error)?;
    snapshot(&app, &connection)
}

#[tauri::command]
fn unlink_notes(
    app: AppHandle,
    source_id: String,
    target_id: String,
) -> Result<BankSnapshot, String> {
    let Some((source_id, target_id)) = normalize_directed_pair(&source_id, &target_id) else {
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

fn fallback_chat_link_label(prompt: &str) -> String {
    let prompt = prompt.to_lowercase();
    if prompt.contains("action") || prompt.contains("todo") || prompt.contains("follow up") {
        "Action Items".to_string()
    } else if prompt.contains("question") || prompt.contains("open") {
        "Open Questions".to_string()
    } else if prompt.contains("summary") || prompt.contains("summar") {
        "Summary".to_string()
    } else if prompt.contains("email") || prompt.contains("draft") {
        "Draft".to_string()
    } else if prompt.contains("decision") {
        "Decisions".to_string()
    } else {
        "Generated Note".to_string()
    }
}

fn clean_generated_link_label(value: &str, fallback: &str) -> String {
    for line in value.lines() {
        let label = line
            .trim()
            .trim_matches(['"', '\'', '`', '.', ':', '-'])
            .trim();
        let lower = label.to_lowercase();
        if label.is_empty()
            || label.contains('<')
            || label.contains('|')
            || label.contains('>')
            || lower.contains("channel")
            || lower.contains("thought")
            || lower.contains("analysis")
            || lower.contains("model")
            || lower.contains("assistant")
        {
            continue;
        }

        let filtered = label
            .chars()
            .filter(|character| {
                character.is_alphanumeric()
                    || character.is_whitespace()
                    || matches!(character, '/' | '&')
            })
            .collect::<String>()
            .split_whitespace()
            .take(4)
            .collect::<Vec<_>>()
            .join(" ");
        let filtered = truncate_chars(&filtered, 48);
        if !filtered.is_empty() {
            return filtered;
        }
    }
    fallback.to_string()
}

async fn suggest_chat_created_link_label(
    app: &AppHandle,
    prompt: &str,
    response: &str,
) -> Result<String, String> {
    let fallback = fallback_chat_link_label(prompt);
    let target = llm::resolve_target(app, None)?;
    let mut config = {
        let connection = open_database(app)?;
        load_llama_config(&connection)?
    };
    config.default_provider = target.provider;
    config.base_url = target.base_url.clone();
    config.preferred_model = target.model.clone();

    let client = target
        .client_builder()?
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let model = if let Some(model) = config.preferred_model.clone() {
        model
    } else {
        fetch_llama_models(&client, &config)
            .await?
            .into_iter()
            .next()
            .map(|model| model.id)
            .ok_or_else(|| "llama.cpp has no available model".to_string())?
    };

    let mut payload = json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": "Name the relationship from the source note to a note created from a chat answer. Return only a short 1-4 word label, no punctuation."
            },
            {
                "role": "user",
                "content": format!(
                    "User request:\n{}\n\nCreated note content:\n{}",
                    truncate_chars(prompt, 800),
                    truncate_chars(response, 1200)
                )
            }
        ],
        "temperature": 0.0,
        "max_tokens": 32,
        "stream": false
    });
    if llm::is_mercury_model(&model) {
        payload["reasoning_effort"] = json!("instant");
    } else {
        payload["reasoning_format"] = json!("none");
        payload["chat_template_kwargs"] = json!({ "enable_thinking": false });
    }
    let response = client
        .post(llama_endpoint(&config.base_url, "/v1/chat/completions"))
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("llama.cpp request failed: {error}"))?;
    if !response.status().is_success() {
        return Ok(fallback);
    }
    let body = response.text().await.unwrap_or_default();
    let parsed = serde_json::from_str::<ChatCompletionResponse>(&body)
        .map_err(|error| format!("Invalid llama.cpp response: {error}"))?;
    let label = parsed
        .choices
        .first()
        .and_then(|choice| choice.message.content.as_deref())
        .map(|content| clean_generated_link_label(content, &fallback))
        .unwrap_or(fallback);
    Ok(label)
}

async fn refine_chat_created_link_label(
    app: AppHandle,
    parent_id: String,
    child_id: String,
    source_prompt: String,
    response_content: String,
) {
    let Ok(label) = suggest_chat_created_link_label(&app, &source_prompt, &response_content).await
    else {
        return;
    };
    if label == fallback_chat_link_label(&source_prompt) {
        return;
    }

    let Ok(connection) = open_database(&app) else {
        return;
    };
    let updated = connection
        .execute(
            "
            UPDATE note_links
            SET label = ?3
            WHERE source_id = ?1
              AND target_id = ?2
              AND link_kind = 'manual'
            ",
            params![parent_id, child_id, label],
        )
        .unwrap_or(0);
    if updated > 0 {
        let _ = app.emit(
            "note-links-updated",
            json!({ "source_id": parent_id, "target_id": child_id }),
        );
    }
}

#[tauri::command]
fn link_chat_created_note(
    app: AppHandle,
    parent_id: String,
    child_id: String,
    source_prompt: String,
    response_content: String,
) -> Result<BankSnapshot, String> {
    let Some((parent_id, child_id)) = normalize_directed_pair(&parent_id, &child_id) else {
        return Err("Cannot link a note to itself".to_string());
    };

    let label = fallback_chat_link_label(&source_prompt);
    let created_at = now_string();
    let connection = open_database(&app)?;
    connection
        .execute(
            "
            INSERT INTO note_links (source_id, target_id, created_at, label, link_kind)
            VALUES (?1, ?2, ?3, ?4, 'manual')
            ON CONFLICT(source_id, target_id) DO UPDATE SET
                label = excluded.label,
                link_kind = excluded.link_kind
            ",
            params![&parent_id, &child_id, &created_at, label],
        )
        .map_err(db_error)?;
    connection
        .execute(
            "
            INSERT INTO note_links (source_id, target_id, created_at, label, link_kind)
            VALUES (?1, ?2, ?3, 'Parent Note', 'manual')
            ON CONFLICT(source_id, target_id) DO UPDATE SET
                label = excluded.label,
                link_kind = excluded.link_kind
            ",
            params![&child_id, &parent_id, &created_at],
        )
        .map_err(db_error)?;
    let bank = snapshot(&app, &connection)?;
    let refinement_app = app.clone();
    tauri::async_runtime::spawn(refine_chat_created_link_label(
        refinement_app,
        parent_id,
        child_id,
        source_prompt,
        response_content,
    ));
    Ok(bank)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_google_auth::init())
        .manage(AudioCaptureState::default())
        .manage(SystemAudioCaptureState::default())
        .manage(SttRuntime::default())
        .manage(llama_runtime::LlamaRuntimeState::default())
        .manage(diarization::DiarizationState::default())
        .manage(slack::SlackState::default())
        .manage(agents::AgentRuntime::new())
        .setup(|app| {
            // Frosted translucent window backdrop on macOS (NSVisualEffectView).
            // The webview is transparent where CSS allows, so this material
            // shows through the sidebar and the gaps around content panels.
            #[cfg(target_os = "macos")]
            {
                use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
                if let Some(window) = app.get_webview_window("main") {
                    let _ = apply_vibrancy(
                        &window,
                        NSVisualEffectMaterial::Sidebar,
                        Some(NSVisualEffectState::Active),
                        None,
                    );
                }
            }

            let connection = open_database(app.handle()).map_err(std::io::Error::other)?;
            recover_interrupted_extraction_jobs(&connection).map_err(std::io::Error::other)?;
            recover_interrupted_stt_jobs(&connection).map_err(std::io::Error::other)?;
            meeting_notes::recover_interrupted_jobs(&connection).map_err(std::io::Error::other)?;
            semantic_search::recover(&connection).map_err(std::io::Error::other)?;
            semantic_search::enqueue_missing(app.handle()).map_err(std::io::Error::other)?;
            imports::recover_interrupted_jobs(&connection).map_err(std::io::Error::other)?;
            agents::reminder_workflows::recover(&connection).map_err(std::io::Error::other)?;
            mcp::start(app.handle().clone()).map_err(std::io::Error::other)?;
            tauri::async_runtime::spawn(slack::worker(app.handle().clone()));
            tauri::async_runtime::spawn(extraction_worker(app.handle().clone()));
            tauri::async_runtime::spawn(stt::stt_worker(app.handle().clone()));
            tauri::async_runtime::spawn(meeting_notes::worker(app.handle().clone()));
            tauri::async_runtime::spawn(imports::worker(app.handle().clone()));
            tauri::async_runtime::spawn(reminders::worker(app.handle().clone()));
            tauri::async_runtime::spawn(agents::reminder_workflows::worker(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_llama_config,
            set_always_obey_global_llm,
            save_llama_config,
            get_llama_status,
            test_remote_llm_connection,
            start_llama_server,
            stop_llama_server,
            get_extraction_queue_status,
            get_note_extraction,
            rename_entity,
            get_entity_interests,
            save_entity_interests,
            get_link_suggestions,
            enqueue_note_extraction,
            finalize_meeting_extraction,
            enqueue_all_note_extractions,
            retry_failed_extractions,
            chat::get_chat_messages,
            chat::send_chat_message,
            chat::clear_chat,
            export_notes::export_note_markdown,
            export_notes::export_notes_markdown_zip,
            gmail::get_gmail_config,
            gmail::save_gmail_config,
            gmail::save_gmail_tokens,
            gmail::clear_gmail_auth,
            gmail::create_gmail_draft,
            calendar::get_calendar_config,
            calendar::save_calendar_tokens,
            calendar::clear_calendar_auth,
            calendar::list_upcoming_calendar_events,
            diarization::start_diarization_session,
            diarization::diarize_capture_file,
            diarization::stop_diarization_session,
            get_audio_capture_status,
            start_audio_capture,
            flush_audio_capture_chunk,
            stop_audio_capture,
            get_stt_config,
            save_stt_config,
            get_stt_status,
            get_stt_queue_status,
            enqueue_stt_job,
            transcribe_capture_file,
            transcribe_last_capture,
            check_system_audio_permission,
            list_meeting_visual_sources,
            capture_meeting_snapshot,
            get_system_audio_capture_status,
            start_system_audio_capture,
            stop_system_audio_capture,
            get_bank,
            create_note,
            get_note,
            save_note,
            create_meeting_note,
            save_meeting_note,
            meeting_notes::create_meeting_quick_note,
            meeting_notes::enqueue_meeting_note_completion,
            semantic_search::claim_embedding_job,
            semantic_search::complete_embedding_job,
            semantic_search::fail_embedding_job,
            semantic_search::semantic_search_notes,
            imports::enqueue_imports,
            imports::list_import_jobs,
            imports::retry_import_job,
            reminders::create_reminder,
            reminders::list_reminders,
            reminders::list_due_reminders,
            reminders::complete_reminder,
            reminders::dismiss_reminder,
            reminders::snooze_reminder,
            reminders::delete_reminder,
            agents::reminder_workflows::list_reminder_workflows,
            agents::reminder_workflows::set_reminder_workflow,
            agents::reminder_workflows::retry_reminder_workflow,
            agents::reminder_workflows::cancel_reminder_workflow,
            agents::reminder_workflows::approve_reminder_workflow_step,
            mcp::get_mcp_status,
            slack::get_slack_config,
            slack::save_slack_config,
            slack::clear_slack_config,
            slack::post_note_to_slack,
            mcp::set_mcp_bearer_token,
            create_folder,
            rename_folder,
            delete_folder,
            move_note,
            trash_note,
            restore_note,
            permanent_delete_note,
            link_notes,
            link_chat_created_note,
            rename_note_link,
            unlink_notes,
            agents::agent_execute_tool,
            agents::agent_list_tools,
            agents::agent_run,
            agents::follow_up::prepare_follow_up_email,
            agents::agent_list_runs,
            agents::agent_get_run_events,
            agents::agent_list_definitions,
            agents::agent_create_definition,
            agents::agent_update_definition,
            agents::agent_delete_definition
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            app_handle.state::<SttRuntime>().shutdown();
            app_handle
                .state::<llama_runtime::LlamaRuntimeState>()
                .shutdown();
        }
    });
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
                    last_error TEXT,
                    llm_provider TEXT,
                    llm_model TEXT
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
    fn extraction_budget_scales_for_8192_context() {
        let budget = extraction_budget(Some(8192), "prism-ml/Bonsai-27B-gguf:Q1_0");

        assert_eq!(budget.input_tokens, 6336);
        assert_eq!(budget.max_output_tokens, 1600);
        assert_eq!(budget.initial_chars, 5760);
        assert_eq!(budget.max_entities, 32);
    }

    #[test]
    fn detects_llama_grammar_sampler_errors() {
        let body = r#"common_sampler_init: error initializing grammar sampler
Generation prompt:
'<|im_start|>assistant
<think>

</think>

'"#;

        assert!(is_grammar_sampler_error(body));
        assert!(is_grammar_sampler_error(
            r#"{"error":{"message":"Failed to initialize samplers: std::exception"}}"#
        ));
        assert!(!is_grammar_sampler_error("ordinary HTTP failure"));
    }

    #[test]
    fn extraction_payload_uses_json_object_compatibility_mode() {
        let model = "prism-ml/Bonsai-27B-gguf:Q1_0";
        let budget = extraction_budget(Some(DEFAULT_EXTRACTION_CONTEXT_TOKENS), model);
        let payload = extraction_request_payload(
            model,
            false,
            budget,
            "Test note",
            "Curvo uses local models.",
            &[],
            ExtractionResponseMode::JsonObject,
        );

        assert_eq!(payload["response_format"], json!({ "type": "json_object" }));
        assert_eq!(payload["temperature"], 0.0);
        let system_prompt = payload["messages"][0]["content"]
            .as_str()
            .expect("system prompt");
        assert!(system_prompt.contains("Do not use a key named type"));
        assert!(system_prompt.contains("\"entity_type\""));
    }

    #[test]
    fn plain_extraction_payload_omits_response_format() {
        let model = "prism-ml/Bonsai-27B-gguf:Q1_0";
        let budget = extraction_budget(Some(DEFAULT_EXTRACTION_CONTEXT_TOKENS), model);
        let payload = extraction_request_payload(
            model,
            false,
            budget,
            "Test note",
            "Curvo uses local models.",
            &[],
            ExtractionResponseMode::PlainJson,
        );

        assert!(payload.get("response_format").is_none());
    }

    #[test]
    fn gemma_extraction_preserves_legacy_profile() {
        let model = "unsloth/gemma-4-12B-it-qat-GGUF:UD-Q4_K_XL";
        let budget = extraction_budget(Some(8192), model);
        let payload = extraction_request_payload(
            model,
            false,
            budget,
            "Test note",
            "Curvo uses local models.",
            &[],
            ExtractionResponseMode::StrictSchema,
        );
        let system_prompt = payload["messages"][0]["content"]
            .as_str()
            .expect("system prompt");
        let user_prompt = payload["messages"][1]["content"]
            .as_str()
            .expect("user prompt");

        assert_eq!(budget.initial_chars, 25_344);
        assert_eq!(budget.max_entities, 64);
        assert_eq!(payload["temperature"], 0.1);
        assert!(system_prompt.contains("Return only the schema-constrained JSON"));
        assert!(!user_prompt.contains("Return at most"));
        assert_eq!(
            next_extraction_response_mode(model, false, ExtractionResponseMode::StrictSchema),
            Some(ExtractionResponseMode::PlainJson)
        );
    }

    #[test]
    fn remote_extraction_uses_only_openai_compatible_fields() {
        let model = "mercury-2";
        let budget = extraction_budget(Some(128_000), model);
        let payload = extraction_request_payload(
            model,
            true,
            budget,
            "Test note",
            "Curvo uses local models.",
            &[],
            ExtractionResponseMode::StrictSchema,
        );

        assert_eq!(budget.max_entities, 32);
        assert_eq!(payload["response_format"]["type"], "json_schema");
        assert!(payload.get("reasoning_effort").is_none());
        assert!(payload.get("reasoning_format").is_none());
        assert!(payload.get("chat_template_kwargs").is_none());
        assert_eq!(
            next_extraction_response_mode(model, true, ExtractionResponseMode::StrictSchema),
            Some(ExtractionResponseMode::JsonObject)
        );
    }

    #[test]
    fn bonsai_extraction_uses_compatibility_fallback_sequence() {
        let model = "prism-ml/Bonsai-27B-gguf:Q1_0";

        assert_eq!(
            next_extraction_response_mode(model, false, ExtractionResponseMode::StrictSchema),
            Some(ExtractionResponseMode::JsonObject)
        );
        assert_eq!(
            next_extraction_response_mode(model, false, ExtractionResponseMode::JsonObject),
            Some(ExtractionResponseMode::PlainJson)
        );
    }

    #[test]
    fn paragraph_chunking_preserves_all_content() {
        let budget = extraction_budget(Some(DEFAULT_EXTRACTION_CONTEXT_TOKENS), "test-model");
        let first = "A".repeat(budget.initial_chars - 100);
        let second = "B".repeat(250);
        let content = format!("{first}\n\n{second}");
        let chunks = initial_note_chunks(&content, budget.initial_chars);

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], first);
        assert_eq!(chunks[1], second);
    }

    #[test]
    fn dense_large_paragraph_is_split_before_inference() {
        let budget = extraction_budget(Some(DEFAULT_EXTRACTION_CONTEXT_TOKENS), "test-model");
        let sentence = "Entity-rich sentence. ";
        let content = sentence.repeat((budget.initial_chars / sentence.len()) + 20);
        let chunks = initial_note_chunks(&content, budget.initial_chars);

        assert!(chunks.len() >= 2);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.chars().count() <= budget.initial_chars));
    }

    #[test]
    fn openai_endpoint_accepts_base_urls_with_or_without_v1() {
        assert_eq!(
            llama_endpoint("https://api.example.com", "/v1/models"),
            "https://api.example.com/v1/models"
        );
        assert_eq!(
            llama_endpoint("https://api.example.com/v1/", "/v1/chat/completions"),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            llama_endpoint("https://api.example.com/openai/v1", "/v1/models"),
            "https://api.example.com/openai/v1/models"
        );
    }

    #[test]
    fn remote_urls_require_tls_except_on_loopback() {
        assert_eq!(
            validate_remote_llm_base_url("http://127.0.0.1:1234/v1").unwrap(),
            "http://127.0.0.1:1234/v1"
        );
        assert!(validate_remote_llm_base_url("http://api.example.com/v1").is_err());
        assert!(validate_remote_llm_base_url("https://api.example.com/v1").is_ok());
    }
}
