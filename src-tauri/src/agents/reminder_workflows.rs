use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::{db_error, gmail, new_id, now_string, open_database, reminders, slack};

use super::{flow, AgentRuntime};

const WORKER_POLL_SECONDS: u64 = 2;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowStepInput {
    pub(crate) agent_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReminderWorkflowRecord {
    pub(crate) id: String,
    pub(crate) reminder_id: String,
    pub(crate) status: String,
    pub(crate) error: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) steps: Vec<ReminderWorkflowStepRecord>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReminderWorkflowStepRecord {
    pub(crate) id: String,
    pub(crate) position: i64,
    pub(crate) agent_id: String,
    pub(crate) agent_name: String,
    pub(crate) step_kind: String,
    pub(crate) status: String,
    pub(crate) output_text: Option<String>,
    pub(crate) destination: Option<String>,
    pub(crate) subject: Option<String>,
    pub(crate) agent_run_id: Option<String>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApproveWorkflowStepInput {
    step_id: String,
    destination: String,
    subject: Option<String>,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetReminderWorkflowInput {
    reminder_id: String,
    steps: Vec<WorkflowStepInput>,
}

struct AgentSpec {
    name: String,
    instructions: String,
    max_steps: u8,
    step_kind: &'static str,
}

#[derive(Clone)]
struct ClaimedWorkflow {
    id: String,
    reminder_id: String,
}

struct PendingStep {
    id: String,
    agent_id: String,
    agent_name: String,
    instructions: String,
    max_steps: u8,
    step_kind: String,
}

pub(crate) fn init_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "
            CREATE TABLE IF NOT EXISTS reminder_workflows (
                id TEXT PRIMARY KEY,
                reminder_id TEXT NOT NULL UNIQUE
                    REFERENCES reminders(id) ON DELETE CASCADE,
                status TEXT NOT NULL DEFAULT 'scheduled'
                    CHECK (status IN (
                        'scheduled', 'running', 'awaiting_approval',
                        'succeeded', 'failed', 'cancelled'
                    )),
                error TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS reminder_workflow_steps (
                id TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL
                    REFERENCES reminder_workflows(id) ON DELETE CASCADE,
                position INTEGER NOT NULL,
                agent_id TEXT NOT NULL,
                agent_name TEXT NOT NULL,
                instructions TEXT NOT NULL,
                max_steps INTEGER NOT NULL DEFAULT 3,
                step_kind TEXT NOT NULL
                    CHECK (step_kind IN ('transform', 'external_slack', 'external_gmail')),
                status TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN (
                        'pending', 'running', 'awaiting_approval',
                        'succeeded', 'failed', 'cancelled'
                    )),
                output_text TEXT,
                destination TEXT,
                subject TEXT,
                agent_run_id TEXT,
                error TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(workflow_id, position)
            );

            CREATE INDEX IF NOT EXISTS reminder_workflows_status_idx
                ON reminder_workflows(status, updated_at);
            CREATE INDEX IF NOT EXISTS reminder_workflow_steps_workflow_idx
                ON reminder_workflow_steps(workflow_id, position);
            ",
        )
        .map_err(db_error)?;
    migrate_external_gmail_step_kind(connection)
}

fn migrate_external_gmail_step_kind(connection: &Connection) -> Result<(), String> {
    let table_sql = connection
        .query_row(
            "SELECT sql FROM sqlite_master
             WHERE type = 'table' AND name = 'reminder_workflow_steps'",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(db_error)?;
    if table_sql.contains("external_gmail") && table_sql.contains("subject TEXT") {
        return Ok(());
    }

    connection
        .execute_batch(
            "
            BEGIN IMMEDIATE;
            DROP INDEX IF EXISTS reminder_workflow_steps_workflow_idx;
            ALTER TABLE reminder_workflow_steps RENAME TO reminder_workflow_steps_legacy;
            CREATE TABLE reminder_workflow_steps (
                id TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL
                    REFERENCES reminder_workflows(id) ON DELETE CASCADE,
                position INTEGER NOT NULL,
                agent_id TEXT NOT NULL,
                agent_name TEXT NOT NULL,
                instructions TEXT NOT NULL,
                max_steps INTEGER NOT NULL DEFAULT 3,
                step_kind TEXT NOT NULL
                    CHECK (step_kind IN ('transform', 'external_slack', 'external_gmail')),
                status TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN (
                        'pending', 'running', 'awaiting_approval',
                        'succeeded', 'failed', 'cancelled'
                    )),
                output_text TEXT,
                destination TEXT,
                subject TEXT,
                agent_run_id TEXT,
                error TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(workflow_id, position)
            );
            INSERT INTO reminder_workflow_steps (
                id, workflow_id, position, agent_id, agent_name, instructions,
                max_steps, step_kind, status, output_text, destination, subject,
                agent_run_id, error, created_at, updated_at
            )
            SELECT id, workflow_id, position, agent_id, agent_name, instructions,
                   max_steps, step_kind, status, output_text, destination, NULL,
                   agent_run_id, error, created_at, updated_at
            FROM reminder_workflow_steps_legacy;
            DROP TABLE reminder_workflow_steps_legacy;
            CREATE INDEX reminder_workflow_steps_workflow_idx
                ON reminder_workflow_steps(workflow_id, position);
            COMMIT;
            ",
        )
        .map_err(db_error)
}

pub(crate) fn recover(connection: &Connection) -> Result<(), String> {
    let now = now_string();
    connection
        .execute(
            "UPDATE reminder_workflow_steps
             SET status = 'pending', error = NULL, updated_at = ?1
             WHERE status = 'running'",
            params![now],
        )
        .map_err(db_error)?;
    connection
        .execute(
            "UPDATE reminder_workflows
             SET status = 'scheduled', error = NULL, updated_at = ?1
             WHERE status = 'running'",
            params![now],
        )
        .map_err(db_error)?;
    Ok(())
}

pub(crate) fn insert_workflow(
    connection: &Connection,
    reminder_id: &str,
    steps: &[WorkflowStepInput],
) -> Result<Option<String>, String> {
    if steps.is_empty() {
        return Ok(None);
    }
    if steps.len() > 8 {
        return Err("A reminder workflow can contain at most 8 agents".to_string());
    }

    let specs = steps
        .iter()
        .map(|step| resolve_agent(connection, &step.agent_id))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(position) = specs
        .iter()
        .position(|spec| spec.step_kind.starts_with("external_"))
    {
        if position + 1 != specs.len() {
            return Err("An external action must be the final workflow step".to_string());
        }
    }

    let workflow_id = new_id("reminder-workflow");
    let now = now_string();
    connection
        .execute(
            "INSERT INTO reminder_workflows
                (id, reminder_id, status, created_at, updated_at)
             VALUES (?1, ?2, 'scheduled', ?3, ?3)",
            params![workflow_id, reminder_id, now],
        )
        .map_err(db_error)?;

    for (position, (step, spec)) in steps.iter().zip(specs).enumerate() {
        connection
            .execute(
                "INSERT INTO reminder_workflow_steps (
                    id, workflow_id, position, agent_id, agent_name,
                    instructions, max_steps, step_kind, status,
                    created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?9)",
                params![
                    format!("{workflow_id}-step-{position}"),
                    workflow_id,
                    position as i64,
                    step.agent_id,
                    spec.name,
                    spec.instructions,
                    spec.max_steps,
                    spec.step_kind,
                    now,
                ],
            )
            .map_err(db_error)?;
    }
    Ok(Some(workflow_id))
}

fn resolve_agent(connection: &Connection, agent_id: &str) -> Result<AgentSpec, String> {
    let builtin = match agent_id {
        "summarize-note" => Some(AgentSpec {
            name: "Summarize this note".to_string(),
            instructions: "Write a concise summary in 3-5 bullets. Capture key ideas, decisions, and action items. Stay faithful to the source and return only the summary.".to_string(),
            max_steps: 3,
            step_kind: "transform",
        }),
        "suggest-links" => Some(AgentSpec {
            name: "Suggest links".to_string(),
            instructions: "Find up to five related notes and briefly explain each connection. Return only the recommendations.".to_string(),
            max_steps: 4,
            step_kind: "transform",
        }),
        "share-note-slack" => Some(AgentSpec {
            name: "Share to Slack".to_string(),
            instructions: "Prepare a concise Slack message. Preserve facts, decisions, owners, and next steps. Return only the editable message draft and do not send anything.".to_string(),
            max_steps: 3,
            step_kind: "external_slack",
        }),
        "create-gmail-draft" => Some(AgentSpec {
            name: "Prepare Gmail draft".to_string(),
            instructions: "Prepare a concise email draft from the input. Preserve facts, decisions, owners, and next steps. Return exactly two sections: 'Subject: <subject>' followed by 'Body:' and the editable email body. Do not create or send anything.".to_string(),
            max_steps: 3,
            step_kind: "external_gmail",
        }),
        _ => None,
    };
    if let Some(spec) = builtin {
        return Ok(spec);
    }

    connection
        .query_row(
            "SELECT name, instructions, COALESCE(max_steps, 3)
             FROM agent_definitions WHERE id = ?1",
            params![agent_id],
            |row| {
                Ok(AgentSpec {
                    name: row.get(0)?,
                    instructions: row.get(1)?,
                    max_steps: row.get::<_, i64>(2)?.clamp(1, 6) as u8,
                    step_kind: "transform",
                })
            },
        )
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| format!("Agent '{agent_id}' is not available for reminders"))
}

#[tauri::command]
pub(crate) fn list_reminder_workflows(
    app: AppHandle,
) -> Result<Vec<ReminderWorkflowRecord>, String> {
    let connection = open_database(&app)?;
    load_workflows(&connection)
}

#[tauri::command]
pub(crate) fn set_reminder_workflow(
    app: AppHandle,
    input: SetReminderWorkflowInput,
) -> Result<(), String> {
    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    let reminder_status = transaction
        .query_row(
            "SELECT status FROM reminders WHERE id = ?1",
            params![input.reminder_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| "Reminder not found".to_string())?;
    if reminder_status != "pending" {
        return Err("Agents can only be assigned to a pending reminder".to_string());
    }

    let active_status = transaction
        .query_row(
            "SELECT status FROM reminder_workflows WHERE reminder_id = ?1",
            params![input.reminder_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?;
    if active_status
        .as_deref()
        .is_some_and(|status| matches!(status, "running" | "awaiting_approval"))
    {
        return Err("This reminder workflow has already started".to_string());
    }

    transaction
        .execute(
            "DELETE FROM reminder_workflows WHERE reminder_id = ?1",
            params![input.reminder_id],
        )
        .map_err(db_error)?;
    let workflow_id =
        insert_workflow(&transaction, &input.reminder_id, &input.steps)?.unwrap_or_default();
    transaction.commit().map_err(db_error)?;
    emit_changed(&app, &workflow_id, Some(&input.reminder_id));
    Ok(())
}

fn load_workflows(connection: &Connection) -> Result<Vec<ReminderWorkflowRecord>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, reminder_id, status, error, created_at, updated_at
             FROM reminder_workflows ORDER BY CAST(created_at AS INTEGER) ASC",
        )
        .map_err(db_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;

    rows.into_iter()
        .map(|(id, reminder_id, status, error, created_at, updated_at)| {
            Ok(ReminderWorkflowRecord {
                steps: load_steps(connection, &id)?,
                id,
                reminder_id,
                status,
                error,
                created_at,
                updated_at,
            })
        })
        .collect()
}

fn load_steps(
    connection: &Connection,
    workflow_id: &str,
) -> Result<Vec<ReminderWorkflowStepRecord>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, position, agent_id, agent_name, step_kind, status,
                    output_text, destination, subject, agent_run_id, error
             FROM reminder_workflow_steps
             WHERE workflow_id = ?1 ORDER BY position ASC",
        )
        .map_err(db_error)?;
    let steps = statement
        .query_map(params![workflow_id], |row| {
            Ok(ReminderWorkflowStepRecord {
                id: row.get(0)?,
                position: row.get(1)?,
                agent_id: row.get(2)?,
                agent_name: row.get(3)?,
                step_kind: row.get(4)?,
                status: row.get(5)?,
                output_text: row.get(6)?,
                destination: row.get(7)?,
                subject: row.get(8)?,
                agent_run_id: row.get(9)?,
                error: row.get(10)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(steps)
}

#[tauri::command]
pub(crate) fn retry_reminder_workflow(app: AppHandle, workflow_id: String) -> Result<(), String> {
    let mut connection = open_database(&app)?;
    let transaction = connection.transaction().map_err(db_error)?;
    let changed = transaction
        .execute(
            "UPDATE reminder_workflow_steps
             SET status = 'pending', error = NULL, updated_at = ?2
             WHERE workflow_id = ?1 AND status = 'failed'",
            params![workflow_id, now_string()],
        )
        .map_err(db_error)?;
    if changed == 0 {
        return Err("This workflow has no failed step to retry".to_string());
    }
    transaction
        .execute(
            "UPDATE reminder_workflows
             SET status = 'scheduled', error = NULL, updated_at = ?2
             WHERE id = ?1",
            params![workflow_id, now_string()],
        )
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)?;
    emit_changed(&app, &workflow_id, None);
    Ok(())
}

#[tauri::command]
pub(crate) async fn approve_reminder_workflow_step(
    app: AppHandle,
    input: ApproveWorkflowStepInput,
) -> Result<ReminderWorkflowRecord, String> {
    let (workflow_id, reminder_id, step_kind) = {
        let connection = open_database(&app)?;
        connection
            .query_row(
                "SELECT s.workflow_id, w.reminder_id, s.step_kind
                 FROM reminder_workflow_steps s
                 JOIN reminder_workflows w ON w.id = s.workflow_id
                 WHERE s.id = ?1 AND s.status = 'awaiting_approval'",
                params![input.step_id],
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
            .ok_or_else(|| "This workflow step is not awaiting approval".to_string())?
    };
    let destination = input.destination.trim();
    let subject = input.subject.as_deref().map(str::trim).unwrap_or_default();
    let text = input.text.trim();
    match step_kind.as_str() {
        "external_slack" => {
            if destination.is_empty() || text.is_empty() {
                return Err("Slack destination and message are required".to_string());
            }
            slack::post_message(app.clone(), destination.to_string(), text.to_string()).await?;
        }
        "external_gmail" => {
            if subject.is_empty() || text.is_empty() {
                return Err("Email subject and body are required".to_string());
            }
            gmail::create_gmail_draft(
                app.clone(),
                gmail::GmailDraftInput {
                    to: (!destination.is_empty()).then(|| destination.to_string()),
                    subject: subject.to_string(),
                    body: text.to_string(),
                },
            )
            .await?;
        }
        _ => return Err("This external action is not supported yet".to_string()),
    }

    let connection = open_database(&app)?;
    let now = now_string();
    connection
        .execute(
            "UPDATE reminder_workflow_steps
             SET status = 'succeeded', output_text = ?2, destination = ?3,
                 subject = ?4, error = NULL, updated_at = ?5 WHERE id = ?1",
            params![
                input.step_id,
                text,
                destination,
                (!subject.is_empty()).then_some(subject),
                now
            ],
        )
        .map_err(db_error)?;
    connection
        .execute(
            "UPDATE reminder_workflows
             SET status = 'succeeded', error = NULL, updated_at = ?2 WHERE id = ?1",
            params![workflow_id, now],
        )
        .map_err(db_error)?;
    emit_changed(&app, &workflow_id, Some(&reminder_id));
    load_workflows(&connection)?
        .into_iter()
        .find(|workflow| workflow.id == workflow_id)
        .ok_or_else(|| "Workflow not found after approval".to_string())
}

fn claim_due(connection: &mut Connection) -> Result<Option<ClaimedWorkflow>, String> {
    let transaction = connection.transaction().map_err(db_error)?;
    let claimed = transaction
        .query_row(
            "SELECT w.id, w.reminder_id
             FROM reminder_workflows w
             JOIN reminders r ON r.id = w.reminder_id
             WHERE w.status = 'scheduled'
               AND r.status = 'pending'
               AND r.scheduled_at <= ?1
             ORDER BY r.scheduled_at ASC LIMIT 1",
            params![chrono::Utc::now().timestamp_millis()],
            |row| {
                Ok(ClaimedWorkflow {
                    id: row.get(0)?,
                    reminder_id: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(db_error)?;
    if let Some(workflow) = &claimed {
        transaction
            .execute(
                "UPDATE reminder_workflows
                 SET status = 'running', error = NULL, updated_at = ?2
                 WHERE id = ?1 AND status = 'scheduled'",
                params![workflow.id, now_string()],
            )
            .map_err(db_error)?;
    }
    transaction.commit().map_err(db_error)?;
    Ok(claimed)
}

fn load_pending_step(
    connection: &Connection,
    workflow_id: &str,
) -> Result<Option<PendingStep>, String> {
    connection
        .query_row(
            "SELECT id, agent_id, agent_name, instructions,
                    max_steps, step_kind
             FROM reminder_workflow_steps
             WHERE workflow_id = ?1 AND status = 'pending'
             ORDER BY position ASC LIMIT 1",
            params![workflow_id],
            |row| {
                Ok(PendingStep {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    agent_name: row.get(2)?,
                    instructions: row.get(3)?,
                    max_steps: row.get::<_, i64>(4)?.clamp(1, 6) as u8,
                    step_kind: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(db_error)
}

fn workflow_input(
    connection: &Connection,
    workflow_id: &str,
) -> Result<(reminders::ReminderRecord, Option<String>), String> {
    let reminder_id: String = connection
        .query_row(
            "SELECT reminder_id FROM reminder_workflows WHERE id = ?1",
            params![workflow_id],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    let reminder = reminders::load_by_id(connection, &reminder_id)?;
    let previous = connection
        .query_row(
            "SELECT output_text FROM reminder_workflow_steps
             WHERE workflow_id = ?1 AND status = 'succeeded'
             ORDER BY position DESC LIMIT 1",
            params![workflow_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(db_error)?
        .flatten();
    Ok((reminder, previous))
}

fn compose_prompt(
    step: &PendingStep,
    reminder: &reminders::ReminderRecord,
    previous: Option<&str>,
) -> String {
    let source = previous.unwrap_or(&reminder.selected_text);
    [
        "You are executing one step in a scheduled reminder workflow.",
        "Work primarily from the supplied passage or previous step output.",
        "You may call read_note with the note id only when the surrounding note is necessary.",
        &format!("Note id: {}", reminder.note_id),
        &format!("Note title: {}", reminder.note_title),
        &format!(
            "Reminder comment: {}",
            reminder.comment.as_deref().unwrap_or("(none)")
        ),
        &format!("Workflow step: {}", step.agent_name),
        &format!("Task: {}", step.instructions),
        "",
        "Input:",
        source,
    ]
    .join("\n")
}

fn parse_gmail_draft_output(raw: &str, note_title: &str) -> (String, String) {
    let trimmed = raw.trim();
    let mut subject = None;
    let mut body_start = None;
    let mut offset = 0;
    for line in trimmed.split_inclusive('\n') {
        let line_without_break = line.trim_end_matches(['\r', '\n']);
        if subject.is_none() {
            if let Some(value) = line_without_break.strip_prefix("Subject:") {
                let value = value.trim();
                if !value.is_empty() {
                    subject = Some(value.to_string());
                }
            }
        } else if line_without_break.trim().eq_ignore_ascii_case("Body:") {
            body_start = Some(offset + line.len());
            break;
        }
        offset += line.len();
    }
    let fallback_subject = format!("Follow up: {}", note_title.trim());
    let subject = subject.unwrap_or(fallback_subject);
    let body = body_start
        .and_then(|start| trimmed.get(start..))
        .map(str::trim)
        .filter(|body| !body.is_empty())
        .unwrap_or(trimmed)
        .to_string();
    (subject, body)
}

async fn execute_workflow(app: &AppHandle, workflow: ClaimedWorkflow) -> Result<(), String> {
    loop {
        let (step, reminder, previous) = {
            let connection = open_database(app)?;
            let status: String = connection
                .query_row(
                    "SELECT status FROM reminder_workflows WHERE id = ?1",
                    params![workflow.id],
                    |row| row.get(0),
                )
                .map_err(db_error)?;
            if status != "running" {
                return Ok(());
            }
            let step = load_pending_step(&connection, &workflow.id)?;
            if step.is_none() {
                connection
                    .execute(
                        "UPDATE reminder_workflows SET status = 'succeeded', updated_at = ?2
                         WHERE id = ?1",
                        params![workflow.id, now_string()],
                    )
                    .map_err(db_error)?;
                emit_changed(app, &workflow.id, Some(&workflow.reminder_id));
                return Ok(());
            }
            let (reminder, previous) = workflow_input(&connection, &workflow.id)?;
            (step.expect("checked above"), reminder, previous)
        };

        {
            let connection = open_database(app)?;
            connection
                .execute(
                    "UPDATE reminder_workflow_steps SET status = 'running', updated_at = ?2
                     WHERE id = ?1 AND status = 'pending'",
                    params![step.id, now_string()],
                )
                .map_err(db_error)?;
        }
        emit_changed(app, &workflow.id, Some(&workflow.reminder_id));

        let prompt = compose_prompt(&step, &reminder, previous.as_deref());
        let runtime = app.state::<AgentRuntime>();
        let result = flow::run_agent_once_with_kind(
            app.clone(),
            &runtime,
            Some(&step.agent_id),
            prompt,
            Some(step.max_steps),
            "reminder",
        )
        .await;

        match result {
            Ok(result) if !result.answer.trim().is_empty() => {
                let connection = open_database(app)?;
                let status: String = connection
                    .query_row(
                        "SELECT status FROM reminder_workflows WHERE id = ?1",
                        params![workflow.id],
                        |row| row.get(0),
                    )
                    .map_err(db_error)?;
                if status != "running" {
                    return Ok(());
                }
                let step_status = if step.step_kind.starts_with("external_") {
                    "awaiting_approval"
                } else {
                    "succeeded"
                };
                let (subject, output_text) = if step.step_kind == "external_gmail" {
                    let (subject, body) =
                        parse_gmail_draft_output(&result.answer, &reminder.note_title);
                    (Some(subject), body)
                } else {
                    (None, result.answer.trim().to_string())
                };
                connection
                    .execute(
                        "UPDATE reminder_workflow_steps
                         SET status = ?2, output_text = ?3, subject = ?4,
                             agent_run_id = ?5, error = NULL, updated_at = ?6 WHERE id = ?1",
                        params![
                            step.id,
                            step_status,
                            output_text,
                            subject,
                            result.run_id,
                            now_string()
                        ],
                    )
                    .map_err(db_error)?;
                if step_status == "awaiting_approval" {
                    connection
                        .execute(
                            "UPDATE reminder_workflows
                             SET status = 'awaiting_approval', updated_at = ?2 WHERE id = ?1",
                            params![workflow.id, now_string()],
                        )
                        .map_err(db_error)?;
                    emit_changed(app, &workflow.id, Some(&workflow.reminder_id));
                    return Ok(());
                }
            }
            Ok(_) => {
                fail_step(app, &workflow, &step, "The agent returned an empty result")?;
                return Ok(());
            }
            Err(error) => {
                fail_step(app, &workflow, &step, &error)?;
                return Ok(());
            }
        }
    }
}

fn fail_step(
    app: &AppHandle,
    workflow: &ClaimedWorkflow,
    step: &PendingStep,
    error: &str,
) -> Result<(), String> {
    let connection = open_database(app)?;
    connection
        .execute(
            "UPDATE reminder_workflow_steps
             SET status = 'failed', error = ?2, updated_at = ?3 WHERE id = ?1",
            params![step.id, error, now_string()],
        )
        .map_err(db_error)?;
    connection
        .execute(
            "UPDATE reminder_workflows
             SET status = 'failed', error = ?2, updated_at = ?3 WHERE id = ?1",
            params![workflow.id, error, now_string()],
        )
        .map_err(db_error)?;
    emit_changed(app, &workflow.id, Some(&workflow.reminder_id));
    Ok(())
}

fn emit_changed(app: &AppHandle, workflow_id: &str, reminder_id: Option<&str>) {
    let _ = app.emit(
        "reminder-workflow-changed",
        serde_json::json!({
            "workflowId": workflow_id,
            "reminderId": reminder_id,
        }),
    );
}

pub(crate) async fn worker(app: AppHandle) {
    loop {
        let claim = open_database(&app).and_then(|mut connection| claim_due(&mut connection));
        match claim {
            Ok(Some(workflow)) => {
                if let Err(error) = execute_workflow(&app, workflow.clone()).await {
                    eprintln!("[smooth:reminder-agent-worker] {error}");
                    let connection = open_database(&app);
                    if let Ok(connection) = connection {
                        let _ = connection.execute(
                            "UPDATE reminder_workflows
                             SET status = 'failed', error = ?2, updated_at = ?3
                             WHERE id = ?1 AND status = 'running'",
                            params![workflow.id, error, now_string()],
                        );
                        emit_changed(&app, &workflow.id, Some(&workflow.reminder_id));
                    }
                }
            }
            Ok(None) => {
                tokio::time::sleep(std::time::Duration::from_secs(WORKER_POLL_SECONDS)).await;
            }
            Err(error) => {
                eprintln!("[smooth:reminder-agent-worker] {error}");
                tokio::time::sleep(std::time::Duration::from_secs(WORKER_POLL_SECONDS)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(
                "
                PRAGMA foreign_keys = ON;
                CREATE TABLE notes (id TEXT PRIMARY KEY, title TEXT NOT NULL);
                CREATE TABLE reminders (
                    id TEXT PRIMARY KEY, note_id TEXT NOT NULL REFERENCES notes(id),
                    scheduled_at INTEGER NOT NULL, comment TEXT, selected_text TEXT NOT NULL,
                    start_offset INTEGER NOT NULL, end_offset INTEGER NOT NULL,
                    context_before TEXT NOT NULL, context_after TEXT NOT NULL,
                    status TEXT NOT NULL, last_notified_at INTEGER,
                    created_at TEXT NOT NULL, updated_at TEXT NOT NULL
                );
                CREATE TABLE agent_definitions (
                    id TEXT PRIMARY KEY, name TEXT NOT NULL, instructions TEXT NOT NULL,
                    max_steps INTEGER
                );
                INSERT INTO notes VALUES ('note-1', 'Note');
                INSERT INTO reminders VALUES
                    ('reminder-1', 'note-1', 1, NULL, 'Selected', 1, 9, '', '',
                     'pending', NULL, '1', '1');
                ",
            )
            .expect("base schema");
        init_schema(&connection).expect("workflow schema");
        connection
    }

    #[test]
    fn external_action_must_be_last() {
        let connection = database();
        let error = insert_workflow(
            &connection,
            "reminder-1",
            &[
                WorkflowStepInput {
                    agent_id: "share-note-slack".to_string(),
                },
                WorkflowStepInput {
                    agent_id: "summarize-note".to_string(),
                },
            ],
        )
        .expect_err("invalid order");
        assert!(error.contains("final"));
    }

    #[test]
    fn stores_ordered_agent_ids() {
        let connection = database();
        insert_workflow(
            &connection,
            "reminder-1",
            &[
                WorkflowStepInput {
                    agent_id: "summarize-note".to_string(),
                },
                WorkflowStepInput {
                    agent_id: "share-note-slack".to_string(),
                },
            ],
        )
        .expect("insert workflow");
        let workflows = load_workflows(&connection).expect("load workflows");
        assert_eq!(workflows[0].steps[0].agent_id, "summarize-note");
        assert_eq!(workflows[0].steps[1].step_kind, "external_slack");
    }

    #[test]
    fn stores_gmail_as_an_external_final_step() {
        let connection = database();
        insert_workflow(
            &connection,
            "reminder-1",
            &[
                WorkflowStepInput {
                    agent_id: "summarize-note".to_string(),
                },
                WorkflowStepInput {
                    agent_id: "create-gmail-draft".to_string(),
                },
            ],
        )
        .expect("insert Gmail workflow");
        let workflows = load_workflows(&connection).expect("load workflows");
        assert_eq!(workflows[0].steps[1].agent_id, "create-gmail-draft");
        assert_eq!(workflows[0].steps[1].step_kind, "external_gmail");
        assert_eq!(workflows[0].steps[1].subject, None);
    }

    #[test]
    fn parses_structured_and_fallback_gmail_drafts() {
        let (subject, body) = parse_gmail_draft_output(
            "Subject: Project next steps\nBody:\nHi Anuj,\n\nHere is the summary.",
            "Planning",
        );
        assert_eq!(subject, "Project next steps");
        assert_eq!(body, "Hi Anuj,\n\nHere is the summary.");

        let (subject, body) =
            parse_gmail_draft_output("Hi Anuj,\n\nHere is the summary.", "Planning");
        assert_eq!(subject, "Follow up: Planning");
        assert_eq!(body, "Hi Anuj,\n\nHere is the summary.");
    }

    #[test]
    fn migrates_existing_step_table_for_gmail() {
        let connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(
                "
                PRAGMA foreign_keys = ON;
                CREATE TABLE reminders (id TEXT PRIMARY KEY);
                CREATE TABLE reminder_workflows (
                    id TEXT PRIMARY KEY,
                    reminder_id TEXT NOT NULL UNIQUE REFERENCES reminders(id) ON DELETE CASCADE,
                    status TEXT NOT NULL, error TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
                );
                CREATE TABLE reminder_workflow_steps (
                    id TEXT PRIMARY KEY,
                    workflow_id TEXT NOT NULL REFERENCES reminder_workflows(id) ON DELETE CASCADE,
                    position INTEGER NOT NULL,
                    agent_id TEXT NOT NULL,
                    agent_name TEXT NOT NULL,
                    instructions TEXT NOT NULL,
                    max_steps INTEGER NOT NULL DEFAULT 3,
                    step_kind TEXT NOT NULL CHECK (step_kind IN ('transform', 'external_slack')),
                    status TEXT NOT NULL,
                    output_text TEXT,
                    destination TEXT,
                    agent_run_id TEXT,
                    error TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    UNIQUE(workflow_id, position)
                );
                INSERT INTO reminders VALUES ('reminder-1');
                INSERT INTO reminder_workflows VALUES
                    ('workflow-1', 'reminder-1', 'succeeded', NULL, '1', '1');
                INSERT INTO reminder_workflow_steps VALUES
                    ('step-1', 'workflow-1', 0, 'summarize-note', 'Summarize', '', 3,
                     'transform', 'succeeded', 'Result', NULL, NULL, NULL, '1', '1');
                ",
            )
            .expect("legacy schema");

        init_schema(&connection).expect("migrate workflow schema");
        let preserved: String = connection
            .query_row(
                "SELECT output_text FROM reminder_workflow_steps WHERE id = 'step-1'",
                [],
                |row| row.get(0),
            )
            .expect("preserved step");
        let table_sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'reminder_workflow_steps'",
                [],
                |row| row.get(0),
            )
            .expect("table SQL");
        assert_eq!(preserved, "Result");
        assert!(table_sql.contains("external_gmail"));
        assert!(table_sql.contains("subject TEXT"));
        assert_eq!(
            connection
                .query_row("PRAGMA foreign_key_check", [], |_| Ok(1_i64))
                .optional()
                .expect("foreign key check"),
            None
        );
    }
}
