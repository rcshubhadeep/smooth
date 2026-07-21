//! Canonical metadata for the built-in tasks.
//!
//! Foreground runs and reminder workflows both resolve tasks here so adding a
//! task never creates a second execution path or a duplicate UI-only catalog.

use serde::Serialize;
use tauri::AppHandle;

use super::persistence;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskDefinition {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) instructions: String,
    pub(crate) scope: String,
    pub(crate) icon: String,
    pub(crate) max_steps: Option<i64>,
    pub(crate) source: String,
    pub(crate) result_kind: String,
}

struct BuiltinTask {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    instructions: &'static str,
    scope: &'static str,
    icon: &'static str,
    max_steps: Option<i64>,
    result_kind: &'static str,
}

const BUILTIN_TASKS: &[BuiltinTask] = &[
    BuiltinTask {
        id: "create-gmail-draft",
        name: "Write follow-up email",
        description: "Turns a meeting transcript into a short, action-oriented Gmail draft.",
        instructions: "Prepare a concise email draft from the input. Preserve facts, decisions, owners, and next steps. Return exactly two sections: 'Subject: <subject>' followed by 'Body:' and the editable email body. Do not create or send anything.",
        scope: "note",
        icon: "email",
        max_steps: Some(3),
        result_kind: "external_gmail",
    },
    BuiltinTask {
        id: "share-note-slack",
        name: "Share to Slack",
        description: "Reviews this note and prepares it for a user-approved Slack post.",
        instructions: "Prepare a concise Slack message. Preserve facts, decisions, owners, and next steps. Return only the editable message draft and do not send anything.",
        scope: "note",
        icon: "slack",
        max_steps: Some(3),
        result_kind: "external_slack",
    },
    BuiltinTask {
        id: "create-todo",
        name: "Create TO-DO",
        description: "Turns the open note into a concise, actionable checklist.",
        instructions: "Read the current note and produce a concise Markdown task list. Use '- [ ]' checklist items. Include owners and dates only when explicitly present. Preserve the context needed to perform each task. Do not invent tasks, owners, deadlines, or decisions. Return only the checklist.",
        scope: "note",
        icon: "todo",
        max_steps: Some(3),
        result_kind: "text",
    },
    BuiltinTask {
        id: "summarize-note",
        name: "Summarize this note",
        description: "Reads the open note and distills it into a few sharp bullets.",
        instructions: "Read the current note and write a concise summary: 3-5 bullet points capturing the key ideas, decisions, and action items. Stay faithful to the source and return only the summary.",
        scope: "note",
        icon: "summary",
        max_steps: Some(3),
        result_kind: "text",
    },
    BuiltinTask {
        id: "suggest-links",
        name: "Suggest links",
        description: "Finds related notes and explains why they connect.",
        instructions: "Find up to five related notes and briefly explain each connection. Return only the recommendations.",
        scope: "note",
        icon: "links",
        max_steps: Some(4),
        result_kind: "text",
    },
    BuiltinTask {
        id: "bank-overview",
        name: "Knowledge bank overview",
        description: "Surveys your notes and surfaces the main themes.",
        instructions: "Search across the knowledge bank and identify the main recurring themes. For each theme give a short name, one line describing it, and list 2-3 representative note titles.",
        scope: "global",
        icon: "overview",
        max_steps: Some(4),
        result_kind: "text",
    },
];

fn from_builtin(task: &BuiltinTask) -> TaskDefinition {
    TaskDefinition {
        id: task.id.to_string(),
        name: task.name.to_string(),
        description: task.description.to_string(),
        instructions: task.instructions.to_string(),
        scope: task.scope.to_string(),
        icon: task.icon.to_string(),
        max_steps: task.max_steps,
        source: "builtin".to_string(),
        result_kind: task.result_kind.to_string(),
    }
}

pub(crate) fn resolve_builtin(id: &str) -> Option<TaskDefinition> {
    BUILTIN_TASKS
        .iter()
        .find(|task| task.id == id)
        .map(from_builtin)
}

pub(crate) fn list_tasks(app: AppHandle) -> Result<Vec<TaskDefinition>, String> {
    let mut tasks = BUILTIN_TASKS.iter().map(from_builtin).collect::<Vec<_>>();
    tasks.extend(
        persistence::list_definitions(app)?
            .into_iter()
            .map(|definition| TaskDefinition {
                id: definition.id,
                name: definition.name,
                description: definition.description,
                instructions: definition.instructions,
                scope: definition.scope,
                icon: definition.icon,
                max_steps: definition.max_steps,
                source: "user".to_string(),
                result_kind: "text".to_string(),
            }),
    );
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_todo_is_a_normal_note_text_task() {
        let task = resolve_builtin("create-todo").expect("Create TO-DO task");
        assert_eq!(task.scope, "note");
        assert_eq!(task.result_kind, "text");
        assert!(task.instructions.contains("- [ ]"));
    }
}
