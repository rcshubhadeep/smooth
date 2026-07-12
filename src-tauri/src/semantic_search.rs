use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tauri::AppHandle;

use crate::{db_error, load_note_meta, now_string, open_database, read_note_content};

const MODEL_ID: &str = "ternlight-base-0.1.0";
const DIMENSIONS: usize = 384;

#[derive(Serialize)]
pub(crate) struct EmbeddingChunk {
    id: i64,
    text: String,
}

#[derive(Serialize)]
pub(crate) struct EmbeddingJob {
    id: i64,
    note_id: String,
    chunks: Vec<EmbeddingChunk>,
}

#[derive(Deserialize)]
pub(crate) struct CompletedEmbedding {
    chunk_id: i64,
    vector: Vec<f32>,
}

#[derive(Serialize)]
pub(crate) struct SemanticSearchResult {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) heading: Option<String>,
    pub(crate) excerpt: String,
    pub(crate) score: f32,
    pub(crate) start_offset: usize,
    pub(crate) end_offset: usize,
}

pub(crate) fn init_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "
        CREATE TABLE IF NOT EXISTS embedding_jobs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            note_id TEXT NOT NULL UNIQUE REFERENCES notes(id) ON DELETE CASCADE,
            content_hash TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending'
                CHECK (status IN ('pending', 'processing', 'failed')),
            attempts INTEGER NOT NULL DEFAULT 0,
            available_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_error TEXT
        );
        CREATE INDEX IF NOT EXISTS embedding_jobs_claim_idx
            ON embedding_jobs(status, available_at, created_at);

        CREATE TABLE IF NOT EXISTS note_chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id INTEGER REFERENCES embedding_jobs(id) ON DELETE CASCADE,
            note_id TEXT NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
            model_id TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            ordinal INTEGER NOT NULL,
            heading TEXT,
            content TEXT NOT NULL,
            embedding_text TEXT NOT NULL,
            start_offset INTEGER NOT NULL,
            end_offset INTEGER NOT NULL,
            active INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS note_chunks_active_idx
            ON note_chunks(note_id, active, model_id);

        CREATE TABLE IF NOT EXISTS chunk_embeddings (
            chunk_id INTEGER PRIMARY KEY REFERENCES note_chunks(id) ON DELETE CASCADE,
            dimensions INTEGER NOT NULL,
            vector BLOB NOT NULL
        );
        ",
        )
        .map_err(db_error)
}

pub(crate) fn enqueue(
    transaction: &Transaction<'_>,
    note_id: &str,
    hash: &str,
) -> Result<(), String> {
    let now = now_string();
    transaction
        .execute(
            "INSERT INTO embedding_jobs
         (note_id, content_hash, status, attempts, available_at, created_at, updated_at, last_error)
         VALUES (?1, ?2, 'pending', 0, ?3, ?3, ?3, NULL)
         ON CONFLICT(note_id) DO UPDATE SET content_hash=excluded.content_hash,
           status='pending', attempts=0, available_at=excluded.available_at,
           updated_at=excluded.updated_at, last_error=NULL",
            params![note_id, hash, now],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "DELETE FROM note_chunks WHERE note_id=?1 AND active=0",
            params![note_id],
        )
        .map_err(db_error)?;
    Ok(())
}

pub(crate) fn recover(connection: &Connection) -> Result<(), String> {
    connection
        .execute("DELETE FROM note_chunks WHERE active=0", [])
        .map_err(db_error)?;
    connection
        .execute(
            "UPDATE embedding_jobs SET status='pending', available_at=?1, updated_at=?1,
         last_error='Interrupted by application shutdown' WHERE status='processing'",
            params![now_string()],
        )
        .map_err(db_error)?;
    Ok(())
}

pub(crate) fn enqueue_missing(app: &AppHandle) -> Result<(), String> {
    let mut connection = open_database(app)?;
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

    for note_id in note_ids {
        let content = read_note_content(app, &note_id)?;
        if content.trim().is_empty() {
            continue;
        }
        let hash = text_hash(&content);
        let already_indexed = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM note_chunks
                 WHERE note_id=?1 AND content_hash=?2 AND model_id=?3 AND active=1)",
                params![note_id, hash, MODEL_ID],
                |row| row.get::<_, bool>(0),
            )
            .map_err(db_error)?;
        if !already_indexed {
            let transaction = connection.transaction().map_err(db_error)?;
            enqueue(&transaction, &note_id, &hash)?;
            transaction.commit().map_err(db_error)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn claim_embedding_job(app: AppHandle) -> Result<Option<EmbeddingJob>, String> {
    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    let row = transaction
        .query_row(
            "SELECT id, note_id, content_hash FROM embedding_jobs
         WHERE status='pending' AND available_at <= ?1 ORDER BY created_at LIMIT 1",
            params![now_string()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?;
    let Some((job_id, note_id, hash)) = row else {
        return Ok(None);
    };
    transaction.execute(
        "UPDATE embedding_jobs SET status='processing', attempts=attempts+1, updated_at=?1 WHERE id=?2",
        params![now_string(), job_id],
    ).map_err(db_error)?;
    transaction.commit().map_err(db_error)?;

    let note = load_note_meta(&connection, &note_id)?;
    let content = read_note_content(&app, &note_id)?;
    if text_hash(&content) != hash {
        let tx = connection.transaction().map_err(db_error)?;
        enqueue(&tx, &note_id, &text_hash(&content))?;
        tx.commit().map_err(db_error)?;
        return Ok(None);
    }
    let chunks = chunk_markdown(&note.title, &content);
    let tx = connection.transaction().map_err(db_error)?;
    let mut output = Vec::with_capacity(chunks.len());
    for (ordinal, chunk) in chunks.into_iter().enumerate() {
        tx.execute(
            "INSERT INTO note_chunks
             (job_id,note_id,model_id,content_hash,ordinal,heading,content,embedding_text,start_offset,end_offset,active)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,0)",
            params![job_id, note_id, MODEL_ID, hash, ordinal as i64, chunk.heading,
                chunk.content, chunk.embedding_text, chunk.start as i64, chunk.end as i64],
        ).map_err(db_error)?;
        output.push(EmbeddingChunk {
            id: tx.last_insert_rowid(),
            text: chunk.embedding_text,
        });
    }
    tx.commit().map_err(db_error)?;
    Ok(Some(EmbeddingJob {
        id: job_id,
        note_id,
        chunks: output,
    }))
}

#[tauri::command]
pub(crate) fn complete_embedding_job(
    app: AppHandle,
    job_id: i64,
    embeddings: Vec<CompletedEmbedding>,
) -> Result<(), String> {
    if embeddings
        .iter()
        .any(|item| item.vector.len() != DIMENSIONS || item.vector.iter().any(|v| !v.is_finite()))
    {
        return Err(format!(
            "Every Ternlight vector must contain {DIMENSIONS} finite values"
        ));
    }
    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    let note_id: String = transaction
        .query_row(
            "SELECT note_id FROM embedding_jobs WHERE id=?1 AND status='processing'",
            params![job_id],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    let expected: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM note_chunks WHERE job_id=?1",
            params![job_id],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    if expected != embeddings.len() as i64 {
        return Err("Embedding batch is incomplete".into());
    }
    for item in embeddings {
        transaction
            .execute(
                "INSERT INTO chunk_embeddings (chunk_id,dimensions,vector) VALUES (?1,?2,?3)",
                params![
                    item.chunk_id,
                    DIMENSIONS as i64,
                    vector_to_blob(&item.vector)
                ],
            )
            .map_err(db_error)?;
    }
    transaction
        .execute(
            "DELETE FROM note_chunks WHERE note_id=?1 AND active=1",
            params![note_id],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "UPDATE note_chunks SET active=1, job_id=NULL WHERE job_id=?1",
            params![job_id],
        )
        .map_err(db_error)?;
    transaction
        .execute("DELETE FROM embedding_jobs WHERE id=?1", params![job_id])
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)
}

#[tauri::command]
pub(crate) fn fail_embedding_job(app: AppHandle, job_id: i64, error: String) -> Result<(), String> {
    let connection = open_database(&app)?;
    connection
        .execute("DELETE FROM note_chunks WHERE job_id=?1", params![job_id])
        .map_err(db_error)?;
    connection.execute(
        "UPDATE embedding_jobs SET status=CASE WHEN attempts>=3 THEN 'failed' ELSE 'pending' END,
         available_at=?1, updated_at=?1, last_error=?2 WHERE id=?3",
        params![now_string(), error.chars().take(500).collect::<String>(), job_id],
    ).map_err(db_error)?;
    Ok(())
}

#[tauri::command]
pub(crate) fn semantic_search_notes(
    app: AppHandle,
    query: String,
    query_embedding: Vec<f32>,
    limit: Option<u32>,
) -> Result<Vec<SemanticSearchResult>, String> {
    if query_embedding.len() != DIMENSIONS {
        return Err(format!("Expected a {DIMENSIONS}-dimensional query vector"));
    }
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT c.note_id,n.title,c.heading,c.content,c.start_offset,c.end_offset,e.vector
         FROM note_chunks c JOIN chunk_embeddings e ON e.chunk_id=c.id
         JOIN notes n ON n.id=c.note_id
         WHERE c.active=1 AND c.model_id=?1 AND n.deleted_at IS NULL",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map(params![MODEL_ID], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)? as usize,
                row.get::<_, i64>(5)? as usize,
                row.get::<_, Vec<u8>>(6)?,
            ))
        })
        .map_err(db_error)?;
    let needle = query.trim().to_lowercase();
    let mut best: HashMap<String, SemanticSearchResult> = HashMap::new();
    for row in rows {
        let (id, title, heading, content, start, end, blob) = row.map_err(db_error)?;
        let Some(vector) = blob_to_vector(&blob) else {
            continue;
        };
        let mut score = dot(&query_embedding, &vector);
        if title.to_lowercase().contains(&needle) {
            score += 0.18;
        }
        if content.to_lowercase().contains(&needle) {
            score += 0.08;
        }
        let candidate = SemanticSearchResult {
            id: id.clone(),
            title,
            heading,
            excerpt: excerpt(&content),
            score,
            start_offset: start,
            end_offset: end,
        };
        if best
            .get(&id)
            .map_or(true, |current| candidate.score > current.score)
        {
            best.insert(id, candidate);
        }
    }
    let mut results: Vec<_> = best.into_values().collect();
    results.sort_by(|a, b| b.score.total_cmp(&a.score));
    results.truncate(limit.unwrap_or(30).clamp(1, 100) as usize);
    Ok(results)
}

struct Chunk {
    heading: Option<String>,
    content: String,
    embedding_text: String,
    start: usize,
    end: usize,
}

fn chunk_markdown(title: &str, content: &str) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut heading: Option<String> = None;
    let mut start = 0;
    let mut buffer = String::new();
    let mut buffer_start = 0;
    let flush = |chunks: &mut Vec<Chunk>,
                 buffer: &mut String,
                 buffer_start: usize,
                 heading: &Option<String>| {
        let text = buffer.trim().to_string();
        if !text.is_empty() {
            let prefix = heading
                .as_ref()
                .map(|h| format!("Note: {title}\nSection: {h}\n\n"))
                .unwrap_or_else(|| format!("Note: {title}\n\n"));
            chunks.push(Chunk {
                heading: heading.clone(),
                content: text.clone(),
                embedding_text: format!("{prefix}{text}"),
                start: buffer_start,
                end: buffer_start + buffer.len(),
            });
        }
        buffer.clear();
    };
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('#') && trimmed.trim_start_matches('#').starts_with(' ') {
            flush(&mut chunks, &mut buffer, buffer_start, &heading);
            heading = Some(trimmed.trim_start_matches('#').trim().to_string());
            start += line.len();
            buffer_start = start;
            continue;
        }
        if buffer.is_empty() {
            buffer_start = start;
        }
        if buffer.split_whitespace().count() + line.split_whitespace().count() > 80 {
            flush(&mut chunks, &mut buffer, buffer_start, &heading);
            buffer_start = start;
        }
        buffer.push_str(line);
        start += line.len();
    }
    flush(&mut chunks, &mut buffer, buffer_start, &heading);
    chunks
}

fn text_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn vector_to_blob(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn blob_to_vector(blob: &[u8]) -> Option<Vec<f32>> {
    if blob.len() != DIMENSIONS * 4 {
        return None;
    }
    Some(
        blob.chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect(),
    )
}
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
fn excerpt(text: &str) -> String {
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= 180 {
        clean
    } else {
        format!("{}...", clean.chars().take(177).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunks_markdown_and_preserves_heading() {
        let chunks = chunk_markdown(
            "Plan",
            "# Goals\nBuild semantic search.\n\n# Risks\nKeep vectors local.",
        );
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].heading.as_deref(), Some("Goals"));
        assert!(chunks[1].embedding_text.contains("Section: Risks"));
    }
    #[test]
    fn vector_blob_round_trip() {
        let vector = vec![0.25; DIMENSIONS];
        assert_eq!(blob_to_vector(&vector_to_blob(&vector)).unwrap(), vector);
    }
}
