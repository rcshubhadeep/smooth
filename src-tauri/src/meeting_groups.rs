use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    hash::{Hash, Hasher},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::{db_error, new_id, now_string, open_database};

const PARTICIPANT_OVERLAP_THRESHOLD: f64 = 0.70;
const MIN_GROUP_MEETINGS: usize = 3;
const RESUGGEST_AFTER_MEETINGS: usize = 3;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MeetingObservationInput {
    pub(crate) started_at_ms: i64,
    pub(crate) calendar_id: Option<String>,
    pub(crate) calendar_event_id: Option<String>,
    pub(crate) recurring_event_id: Option<String>,
    pub(crate) calendar_title: Option<String>,
    #[serde(default)]
    pub(crate) calendar_participants: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MeetingGroupSuggestion {
    pub(crate) candidate_key: String,
    pub(crate) suggested_name: String,
    pub(crate) cadence_label: String,
    pub(crate) meeting_count: usize,
    pub(crate) participants: Vec<String>,
    pub(crate) meetings: Vec<MeetingGroupSuggestionMeeting>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MeetingGroupSuggestionMeeting {
    pub(crate) note_id: String,
    pub(crate) title: String,
    pub(crate) started_at_ms: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MeetingGroupAcceptance {
    pub(crate) folder_id: String,
    pub(crate) folder_name: String,
    pub(crate) meeting_count: usize,
}

#[derive(Clone, Debug)]
struct Observation {
    note_id: String,
    note_title: String,
    started_at_ms: i64,
    recurring_event_id: Option<String>,
    calendar_id: Option<String>,
    calendar_title: Option<String>,
    calendar_participants: Vec<String>,
    transcript_participants: Vec<String>,
}

#[derive(Clone, Debug)]
struct Candidate {
    key: String,
    suggested_name: String,
    series_key: Option<String>,
    participant_source: String,
    participant_keys: Vec<String>,
    cadence_seconds: i64,
    cadence_tolerance_seconds: i64,
    observations: Vec<Observation>,
}

#[derive(Debug)]
struct GroupRule {
    id: String,
    folder_id: String,
    series_key: Option<String>,
    participant_source: String,
    participant_keys: Vec<String>,
    cadence_seconds: i64,
    cadence_tolerance_seconds: i64,
}

pub(crate) fn init_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "
            CREATE TABLE IF NOT EXISTS meeting_groups (
                id TEXT PRIMARY KEY NOT NULL,
                folder_id TEXT NOT NULL UNIQUE
                    REFERENCES folders(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                candidate_key TEXT NOT NULL UNIQUE,
                series_key TEXT,
                participant_source TEXT NOT NULL
                    CHECK (participant_source IN ('calendar', 'transcript')),
                participant_keys_json TEXT NOT NULL,
                cadence_seconds INTEGER NOT NULL,
                cadence_tolerance_seconds INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS meeting_occurrences (
                note_id TEXT PRIMARY KEY NOT NULL
                    REFERENCES notes(id) ON DELETE CASCADE,
                group_id TEXT REFERENCES meeting_groups(id) ON DELETE SET NULL,
                started_at_ms INTEGER NOT NULL,
                completed_at_ms INTEGER,
                calendar_id TEXT,
                calendar_event_id TEXT,
                recurring_event_id TEXT,
                calendar_title TEXT,
                calendar_participants_json TEXT NOT NULL DEFAULT '[]',
                transcript_participants_json TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS meeting_occurrences_completed_idx
                ON meeting_occurrences(completed_at_ms, started_at_ms);
            CREATE INDEX IF NOT EXISTS meeting_occurrences_group_idx
                ON meeting_occurrences(group_id, started_at_ms);

            CREATE TABLE IF NOT EXISTS meeting_group_suppressions (
                candidate_key TEXT PRIMARY KEY NOT NULL,
                meeting_count INTEGER NOT NULL,
                dismissed_at TEXT NOT NULL
            );
            ",
        )
        .map_err(db_error)
}

pub(crate) fn resolve_folder_for_new_meeting(
    connection: &Connection,
    observation: &MeetingObservationInput,
) -> Result<Option<(String, String)>, String> {
    let rules = load_active_rules(connection)?;
    let calendar_participants = normalize_participants(&observation.calendar_participants);
    let series_key = observation_series_key(
        observation.calendar_id.as_deref(),
        observation.recurring_event_id.as_deref(),
        observation.calendar_title.as_deref(),
    );
    for rule in rules {
        if rule.participant_source != "calendar" {
            continue;
        }
        if rule_matches(
            connection,
            &rule,
            series_key.as_deref(),
            &calendar_participants,
            observation.started_at_ms,
        )? {
            return Ok(Some((rule.folder_id, rule.id)));
        }
    }
    Ok(None)
}

pub(crate) fn record_started_meeting(
    connection: &Connection,
    note_id: &str,
    observation: &MeetingObservationInput,
    group_id: Option<&str>,
) -> Result<(), String> {
    let now = now_string();
    let calendar_participants =
        serde_json::to_string(&normalize_participants(&observation.calendar_participants))
            .map_err(|error| error.to_string())?;
    connection
        .execute(
            "
            INSERT INTO meeting_occurrences (
                note_id, group_id, started_at_ms, calendar_id,
                calendar_event_id, recurring_event_id, calendar_title,
                calendar_participants_json, transcript_participants_json,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '[]', ?9, ?9)
            ON CONFLICT(note_id) DO UPDATE SET
                group_id = excluded.group_id,
                started_at_ms = excluded.started_at_ms,
                calendar_id = excluded.calendar_id,
                calendar_event_id = excluded.calendar_event_id,
                recurring_event_id = excluded.recurring_event_id,
                calendar_title = excluded.calendar_title,
                calendar_participants_json = excluded.calendar_participants_json,
                updated_at = excluded.updated_at
            ",
            params![
                note_id,
                group_id,
                observation.started_at_ms,
                clean_optional(&observation.calendar_id),
                clean_optional(&observation.calendar_event_id),
                clean_optional(&observation.recurring_event_id),
                clean_optional(&observation.calendar_title),
                calendar_participants,
                now,
            ],
        )
        .map_err(db_error)?;
    Ok(())
}

#[tauri::command]
pub(crate) fn finalize_meeting_group_observation(
    app: AppHandle,
    note_id: String,
    transcript_participants: Vec<String>,
) -> Result<Vec<MeetingGroupSuggestion>, String> {
    let mut connection = open_database(&app)?;
    let normalized = normalize_participants(&transcript_participants);
    let participant_json = serde_json::to_string(&normalized).map_err(|error| error.to_string())?;
    connection
        .execute(
            "
            UPDATE meeting_occurrences
            SET completed_at_ms = ?2, transcript_participants_json = ?3,
                updated_at = ?4
            WHERE note_id = ?1
            ",
            params![note_id, now_ms(), participant_json, now_string()],
        )
        .map_err(db_error)?;

    route_completed_meeting(&mut connection, &note_id)?;
    suggestions(&connection)
}

#[tauri::command]
pub(crate) fn list_meeting_group_suggestions(
    app: AppHandle,
) -> Result<Vec<MeetingGroupSuggestion>, String> {
    let connection = open_database(&app)?;
    suggestions(&connection)
}

#[tauri::command]
pub(crate) fn dismiss_meeting_group_suggestion(
    app: AppHandle,
    candidate_key: String,
) -> Result<(), String> {
    let connection = open_database(&app)?;
    let candidate = candidates(&connection)?
        .into_iter()
        .find(|candidate| candidate.key == candidate_key)
        .ok_or_else(|| "This meeting group suggestion is no longer available".to_string())?;
    connection
        .execute(
            "
            INSERT INTO meeting_group_suppressions (
                candidate_key, meeting_count, dismissed_at
            ) VALUES (?1, ?2, ?3)
            ON CONFLICT(candidate_key) DO UPDATE SET
                meeting_count = excluded.meeting_count,
                dismissed_at = excluded.dismissed_at
            ",
            params![
                candidate.key,
                candidate.observations.len() as i64,
                now_string()
            ],
        )
        .map_err(db_error)?;
    Ok(())
}

#[tauri::command]
pub(crate) fn accept_meeting_group_suggestion(
    app: AppHandle,
    candidate_key: String,
) -> Result<MeetingGroupAcceptance, String> {
    let mut connection = open_database(&app)?;
    let candidate = candidates(&connection)?
        .into_iter()
        .find(|candidate| candidate.key == candidate_key)
        .ok_or_else(|| "This meeting group suggestion is no longer available".to_string())?;
    let folder_name = unique_folder_name(&connection, &candidate.suggested_name)?;
    let folder_id = new_id("folder");
    let group_id = new_id("meeting-group");
    let participant_json =
        serde_json::to_string(&candidate.participant_keys).map_err(|error| error.to_string())?;
    let now = now_string();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    transaction
        .execute(
            "INSERT INTO folders (id, name, created_at)
             VALUES (?1, ?2, ?3)",
            params![folder_id, folder_name, now],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "
            INSERT INTO meeting_groups (
                id, folder_id, name, candidate_key, series_key,
                participant_source, participant_keys_json,
                cadence_seconds, cadence_tolerance_seconds,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
            ",
            params![
                group_id,
                folder_id,
                folder_name,
                candidate.key,
                candidate.series_key,
                candidate.participant_source,
                participant_json,
                candidate.cadence_seconds,
                candidate.cadence_tolerance_seconds,
                now,
            ],
        )
        .map_err(db_error)?;
    for observation in &candidate.observations {
        transaction
            .execute(
                "UPDATE notes SET folder_id = ?2, updated_at = ?3
                 WHERE deleted_at IS NULL AND (
                    id = ?1 OR id IN (
                        SELECT target_id FROM note_links
                        WHERE source_id = ?1 AND label = 'Meeting Note'
                    )
                 )",
                params![observation.note_id, folder_id, now],
            )
            .map_err(db_error)?;
        transaction
            .execute(
                "UPDATE meeting_occurrences SET group_id = ?2, updated_at = ?3
                 WHERE note_id = ?1",
                params![observation.note_id, group_id, now],
            )
            .map_err(db_error)?;
    }
    transaction
        .execute(
            "DELETE FROM meeting_group_suppressions WHERE candidate_key = ?1",
            params![candidate.key],
        )
        .map_err(db_error)?;
    transaction.commit().map_err(db_error)?;
    Ok(MeetingGroupAcceptance {
        folder_id,
        folder_name,
        meeting_count: candidate.observations.len(),
    })
}

fn route_completed_meeting(connection: &mut Connection, note_id: &str) -> Result<(), String> {
    let observation = load_observation(connection, note_id)?
        .ok_or_else(|| "Meeting observation was not found".to_string())?;
    let current_folder = connection
        .query_row(
            "SELECT folder_id FROM notes WHERE id = ?1",
            params![note_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(db_error)?;
    if current_folder.is_some() {
        return Ok(());
    }

    let series_key = observation_series_key(
        observation.calendar_id.as_deref(),
        observation.recurring_event_id.as_deref(),
        observation.calendar_title.as_deref(),
    );
    for rule in load_active_rules(connection)? {
        let participants = if rule.participant_source == "calendar" {
            &observation.calendar_participants
        } else {
            &observation.transcript_participants
        };
        if rule_matches(
            connection,
            &rule,
            series_key.as_deref(),
            participants,
            observation.started_at_ms,
        )? {
            let now = now_string();
            let transaction = connection.transaction().map_err(db_error)?;
            transaction
                .execute(
                    "UPDATE notes SET folder_id = ?2, updated_at = ?3
                     WHERE folder_id IS NULL AND deleted_at IS NULL AND (
                        id = ?1 OR id IN (
                            SELECT target_id FROM note_links
                            WHERE source_id = ?1 AND label = 'Meeting Note'
                        )
                     )",
                    params![note_id, rule.folder_id, now],
                )
                .map_err(db_error)?;
            transaction
                .execute(
                    "UPDATE meeting_occurrences SET group_id = ?2, updated_at = ?3
                     WHERE note_id = ?1",
                    params![note_id, rule.id, now],
                )
                .map_err(db_error)?;
            transaction.commit().map_err(db_error)?;
            break;
        }
    }
    Ok(())
}

fn load_active_rules(connection: &Connection) -> Result<Vec<GroupRule>, String> {
    let mut statement = connection
        .prepare(
            "
            SELECT g.id, g.folder_id, g.series_key, g.participant_source,
                   g.participant_keys_json, g.cadence_seconds,
                   g.cadence_tolerance_seconds
            FROM meeting_groups g
            WHERE (
                SELECT COUNT(*)
                FROM meeting_occurrences o
                JOIN notes n ON n.id = o.note_id
                WHERE n.folder_id = g.folder_id AND n.deleted_at IS NULL
            ) >= ?1
            ",
        )
        .map_err(db_error)?;
    let rules = statement
        .query_map(params![MIN_GROUP_MEETINGS as i64], |row| {
            let participant_json = row.get::<_, String>(4)?;
            Ok(GroupRule {
                id: row.get(0)?,
                folder_id: row.get(1)?,
                series_key: row.get(2)?,
                participant_source: row.get(3)?,
                participant_keys: serde_json::from_str(&participant_json).unwrap_or_default(),
                cadence_seconds: row.get(5)?,
                cadence_tolerance_seconds: row.get(6)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(rules)
}

fn rule_matches(
    connection: &Connection,
    rule: &GroupRule,
    series_key: Option<&str>,
    participants: &[String],
    started_at_ms: i64,
) -> Result<bool, String> {
    if let Some(expected) = rule.series_key.as_deref() {
        if Some(expected) != series_key {
            return Ok(false);
        }
    }
    if participant_overlap(&rule.participant_keys, participants) < PARTICIPANT_OVERLAP_THRESHOLD {
        return Ok(false);
    }
    let latest = connection
        .query_row(
            "
            SELECT MAX(o.started_at_ms)
            FROM meeting_occurrences o
            JOIN notes n ON n.id = o.note_id
            WHERE o.group_id = ?1 AND n.folder_id = ?2 AND n.deleted_at IS NULL
            ",
            params![rule.id, rule.folder_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(db_error)?;
    let Some(latest) = latest else {
        return Ok(false);
    };
    let interval = ((started_at_ms - latest).abs() / 1000).max(0);
    let expected = rule.cadence_seconds.max(1);
    let nearest_multiple = ((interval as f64 / expected as f64).round() as i64).max(1);
    let deviation = (interval - expected * nearest_multiple).abs();
    Ok(deviation <= rule.cadence_tolerance_seconds)
}

fn suggestions(connection: &Connection) -> Result<Vec<MeetingGroupSuggestion>, String> {
    let suppressions = load_suppressions(connection)?;
    let active_keys = connection
        .prepare("SELECT candidate_key FROM meeting_groups")
        .map_err(db_error)?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error)?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(db_error)?;
    Ok(candidates(connection)?
        .into_iter()
        .filter(|candidate| !active_keys.contains(&candidate.key))
        .filter(|candidate| match suppressions.get(&candidate.key) {
            None => true,
            Some(dismissed_count) => {
                candidate.observations.len()
                    >= dismissed_count.saturating_add(RESUGGEST_AFTER_MEETINGS)
            }
        })
        .map(|candidate| MeetingGroupSuggestion {
            candidate_key: candidate.key,
            suggested_name: candidate.suggested_name,
            cadence_label: cadence_label(candidate.cadence_seconds),
            meeting_count: candidate.observations.len(),
            participants: candidate.participant_keys,
            meetings: candidate
                .observations
                .into_iter()
                .map(|observation| MeetingGroupSuggestionMeeting {
                    note_id: observation.note_id,
                    title: observation.note_title,
                    started_at_ms: observation.started_at_ms,
                })
                .collect(),
        })
        .collect())
}

fn candidates(connection: &Connection) -> Result<Vec<Candidate>, String> {
    let observations = load_ungrouped_observations(connection)?;
    let mut series_buckets = HashMap::<String, Vec<Observation>>::new();
    let mut participant_only = Vec::<Observation>::new();
    for observation in observations {
        if let Some(series_key) = observation_series_key(
            observation.calendar_id.as_deref(),
            observation.recurring_event_id.as_deref(),
            observation.calendar_title.as_deref(),
        ) {
            series_buckets
                .entry(series_key)
                .or_default()
                .push(observation);
        } else {
            participant_only.push(observation);
        }
    }

    let mut clusters = series_buckets
        .into_iter()
        .flat_map(|(series_key, values)| {
            split_by_participant_overlap(values)
                .into_iter()
                .map(move |values| (Some(series_key.clone()), values))
        })
        .collect::<Vec<_>>();
    clusters.extend(
        split_by_participant_overlap(participant_only)
            .into_iter()
            .map(|values| (None, values)),
    );

    let mut result = Vec::new();
    for (series_key, mut observations) in clusters {
        observations.sort_by_key(|observation| observation.started_at_ms);
        if observations.len() < MIN_GROUP_MEETINGS {
            continue;
        }
        let evidence = observations
            .iter()
            .rev()
            .take(6)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();
        let participant_source = if evidence
            .iter()
            .all(|observation| !observation.calendar_participants.is_empty())
        {
            "calendar"
        } else {
            "transcript"
        };
        let participant_sets = evidence
            .iter()
            .map(|observation| {
                if participant_source == "calendar" {
                    observation.calendar_participants.clone()
                } else {
                    observation.transcript_participants.clone()
                }
            })
            .collect::<Vec<_>>();
        if participant_sets
            .iter()
            .any(|participants| participants.len() < 2)
        {
            continue;
        }
        if !last_three_overlap(&participant_sets) {
            continue;
        }
        let Some((cadence_seconds, cadence_tolerance_seconds)) = detect_cadence(&observations)
        else {
            continue;
        };
        let participant_keys = common_participants(&participant_sets);
        if participant_keys.len() < 2 {
            continue;
        }
        let candidate_key = candidate_key(series_key.as_deref(), &participant_keys);
        let suggested_name = suggested_group_name(&observations, &participant_keys);
        result.push(Candidate {
            key: candidate_key,
            suggested_name,
            series_key,
            participant_source: participant_source.to_string(),
            participant_keys,
            cadence_seconds,
            cadence_tolerance_seconds,
            observations,
        });
    }
    result.sort_by(|first, second| {
        second
            .observations
            .len()
            .cmp(&first.observations.len())
            .then_with(|| first.suggested_name.cmp(&second.suggested_name))
    });
    Ok(result)
}

fn split_by_participant_overlap(mut observations: Vec<Observation>) -> Vec<Vec<Observation>> {
    observations.sort_by_key(|observation| observation.started_at_ms);
    let mut clusters = Vec::<Vec<Observation>>::new();
    for observation in observations {
        let participants = observation_participants(&observation);
        if participants.len() < 2 {
            continue;
        }
        let matching = clusters.iter().position(|cluster| {
            cluster
                .last()
                .map(observation_participants)
                .is_some_and(|existing| {
                    participant_overlap(existing, participants) >= PARTICIPANT_OVERLAP_THRESHOLD
                })
        });
        if let Some(index) = matching {
            clusters[index].push(observation);
        } else {
            clusters.push(vec![observation]);
        }
    }
    clusters
}

fn load_ungrouped_observations(connection: &Connection) -> Result<Vec<Observation>, String> {
    let mut statement = connection
        .prepare(
            "
            SELECT o.note_id, n.title, o.started_at_ms, o.recurring_event_id,
                   o.calendar_id, o.calendar_title,
                   o.calendar_participants_json, o.transcript_participants_json
            FROM meeting_occurrences o
            JOIN notes n ON n.id = o.note_id
            WHERE o.completed_at_ms IS NOT NULL
              AND o.group_id IS NULL
              AND n.deleted_at IS NULL
            ORDER BY o.started_at_ms
            ",
        )
        .map_err(db_error)?;
    let observations = statement
        .query_map([], read_observation)
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(observations)
}

fn load_observation(connection: &Connection, note_id: &str) -> Result<Option<Observation>, String> {
    connection
        .query_row(
            "
            SELECT o.note_id, n.title, o.started_at_ms, o.recurring_event_id,
                   o.calendar_id, o.calendar_title,
                   o.calendar_participants_json, o.transcript_participants_json
            FROM meeting_occurrences o
            JOIN notes n ON n.id = o.note_id
            WHERE o.note_id = ?1
            ",
            params![note_id],
            read_observation,
        )
        .optional()
        .map_err(db_error)
}

fn read_observation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Observation> {
    let calendar_json = row.get::<_, String>(6)?;
    let transcript_json = row.get::<_, String>(7)?;
    Ok(Observation {
        note_id: row.get(0)?,
        note_title: row.get(1)?,
        started_at_ms: row.get(2)?,
        recurring_event_id: row.get(3)?,
        calendar_id: row.get(4)?,
        calendar_title: row.get(5)?,
        calendar_participants: serde_json::from_str(&calendar_json).unwrap_or_default(),
        transcript_participants: serde_json::from_str(&transcript_json).unwrap_or_default(),
    })
}

fn load_suppressions(connection: &Connection) -> Result<HashMap<String, usize>, String> {
    let mut statement = connection
        .prepare("SELECT candidate_key, meeting_count FROM meeting_group_suppressions")
        .map_err(db_error)?;
    let suppressions = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as usize,
            ))
        })
        .map_err(db_error)?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(db_error)?;
    Ok(suppressions)
}

fn observation_series_key(
    calendar_id: Option<&str>,
    recurring_event_id: Option<&str>,
    calendar_title: Option<&str>,
) -> Option<String> {
    if let Some(recurring) = recurring_event_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(format!(
            "calendar:{}:{}",
            calendar_id.unwrap_or_default().trim().to_lowercase(),
            recurring.to_lowercase()
        ));
    }
    let title = calendar_title.map(normalize_identity)?;
    (!title.is_empty()).then(|| format!("title:{title}"))
}

fn observation_participants(observation: &Observation) -> &[String] {
    if !observation.calendar_participants.is_empty() {
        &observation.calendar_participants
    } else {
        &observation.transcript_participants
    }
}

fn normalize_participants(values: &[String]) -> Vec<String> {
    let mut normalized = values
        .iter()
        .map(|value| normalize_identity(value))
        .filter(|value| {
            !value.is_empty()
                && value != "you"
                && value != "participants"
                && !value.starts_with("speaker ")
        })
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn normalize_identity(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn participant_overlap(first: &[String], second: &[String]) -> f64 {
    if first.is_empty() || second.is_empty() {
        return 0.0;
    }
    let first = first.iter().collect::<HashSet<_>>();
    let second = second.iter().collect::<HashSet<_>>();
    let common = first.intersection(&second).count();
    common as f64 / first.len().max(second.len()) as f64
}

fn last_three_overlap(sets: &[Vec<String>]) -> bool {
    let recent = sets.iter().rev().take(3).collect::<Vec<_>>();
    recent.len() >= MIN_GROUP_MEETINGS
        && recent.iter().enumerate().all(|(index, first)| {
            recent
                .iter()
                .skip(index + 1)
                .all(|second| participant_overlap(first, second) >= PARTICIPANT_OVERLAP_THRESHOLD)
        })
}

fn common_participants(sets: &[Vec<String>]) -> Vec<String> {
    let recent = sets.iter().rev().take(3).collect::<Vec<_>>();
    let Some(first) = recent.first() else {
        return Vec::new();
    };
    let mut common = first.iter().cloned().collect::<HashSet<_>>();
    for participants in recent.into_iter().skip(1) {
        let current = participants.iter().cloned().collect::<HashSet<_>>();
        common.retain(|participant| current.contains(participant));
    }
    let mut result = common.into_iter().collect::<Vec<_>>();
    result.sort();
    result
}

fn detect_cadence(observations: &[Observation]) -> Option<(i64, i64)> {
    if observations.len() < MIN_GROUP_MEETINGS {
        return None;
    }
    let mut intervals = observations
        .windows(2)
        .map(|pair| ((pair[1].started_at_ms - pair[0].started_at_ms).abs() / 1000).max(1))
        .collect::<Vec<_>>();
    intervals.sort();
    let median = intervals[intervals.len() / 2];
    let tolerance = (median / 4).max(4 * 60 * 60);
    let consistent = intervals.iter().all(|interval| {
        let nearest_multiple = ((*interval as f64 / median as f64).round() as i64).max(1);
        (*interval - median * nearest_multiple).abs() <= tolerance
    });
    consistent.then_some((median, tolerance))
}

fn candidate_key(series_key: Option<&str>, participant_keys: &[String]) -> String {
    let mut hasher = DefaultHasher::new();
    series_key.unwrap_or("participants").hash(&mut hasher);
    participant_keys.hash(&mut hasher);
    format!("meeting-candidate-{:016x}", hasher.finish())
}

fn suggested_group_name(observations: &[Observation], participants: &[String]) -> String {
    let mut titles = HashMap::<String, usize>::new();
    for observation in observations {
        if let Some(title) = observation
            .calendar_title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            *titles.entry(title.to_string()).or_default() += 1;
        }
    }
    if let Some((title, _)) = titles.into_iter().max_by_key(|(_, count)| *count) {
        return title;
    }
    let names = participants.iter().take(3).cloned().collect::<Vec<_>>();
    format!("Meetings with {}", names.join(", "))
}

fn cadence_label(seconds: i64) -> String {
    let hours = seconds as f64 / 3600.0;
    if (20.0..=30.0).contains(&hours) {
        "Daily".to_string()
    } else if (40.0..=56.0).contains(&hours) {
        "Every other day".to_string()
    } else if (6.0 * 24.0..=8.0 * 24.0).contains(&hours) {
        "Weekly".to_string()
    } else if hours < 24.0 {
        format!("About every {} hours", hours.round() as i64)
    } else {
        format!("About every {} days", (hours / 24.0).round() as i64)
    }
}

fn unique_folder_name(connection: &Connection, requested: &str) -> Result<String, String> {
    let base = requested.trim();
    let base = if base.is_empty() {
        "Meeting group"
    } else {
        base
    };
    for suffix in 0..100 {
        let candidate = if suffix == 0 {
            base.to_string()
        } else {
            format!("{base} {}", suffix + 1)
        };
        let exists = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM folders WHERE name = ?1 COLLATE NOCASE)",
                params![candidate],
                |row| row.get::<_, bool>(0),
            )
            .map_err(db_error)?;
        if !exists {
            return Ok(candidate);
        }
    }
    Err("Could not choose a unique meeting group folder name".to_string())
}

fn clean_optional(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_connection() -> Connection {
        let connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(
                "
                PRAGMA foreign_keys = ON;
                CREATE TABLE folders (
                    id TEXT PRIMARY KEY NOT NULL,
                    name TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );
                CREATE TABLE notes (
                    id TEXT PRIMARY KEY NOT NULL,
                    title TEXT NOT NULL,
                    folder_id TEXT REFERENCES folders(id) ON DELETE SET NULL,
                    deleted_at TEXT,
                    updated_at TEXT NOT NULL
                );
                CREATE TABLE note_links (
                    source_id TEXT NOT NULL,
                    target_id TEXT NOT NULL,
                    label TEXT
                );
                ",
            )
            .expect("create base schema");
        init_schema(&connection).expect("create meeting group schema");
        connection
    }

    fn insert_completed_meeting(connection: &Connection, index: usize) {
        let note_id = format!("note-{index}");
        let started_at_ms = index as i64 * 24 * 60 * 60 * 1000;
        connection
            .execute(
                "INSERT INTO notes (id, title, folder_id, deleted_at, updated_at)
                 VALUES (?1, ?2, NULL, NULL, 'now')",
                params![note_id, format!("Daily sync {index}")],
            )
            .expect("insert note");
        connection
            .execute(
                "
                INSERT INTO meeting_occurrences (
                    note_id, started_at_ms, completed_at_ms, calendar_id,
                    recurring_event_id, calendar_title,
                    calendar_participants_json, transcript_participants_json,
                    created_at, updated_at
                ) VALUES (?1, ?2, ?2, 'primary', 'daily-sync', 'Daily sync',
                          '[\"alex@example.com\",\"sam@example.com\"]', '[]',
                          'now', 'now')
                ",
                params![note_id, started_at_ms],
            )
            .expect("insert occurrence");
    }

    fn observation(at_days: i64, participants: &[&str]) -> Observation {
        Observation {
            note_id: format!("note-{at_days}"),
            note_title: format!("Meeting {at_days}"),
            started_at_ms: at_days * 24 * 60 * 60 * 1000,
            recurring_event_id: None,
            calendar_id: None,
            calendar_title: None,
            calendar_participants: participants.iter().map(|value| value.to_string()).collect(),
            transcript_participants: Vec::new(),
        }
    }

    #[test]
    fn participant_overlap_uses_the_larger_meeting_as_denominator() {
        let first = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let second = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        assert_eq!(participant_overlap(&first, &second), 0.75);
    }

    #[test]
    fn detects_daily_and_weekly_cadence() {
        let daily = vec![
            observation(1, &["a", "b"]),
            observation(2, &["a", "b"]),
            observation(3, &["a", "b"]),
        ];
        assert_eq!(cadence_label(detect_cadence(&daily).unwrap().0), "Daily");

        let weekly = vec![
            observation(1, &["a", "b"]),
            observation(8, &["a", "b"]),
            observation(15, &["a", "b"]),
        ];
        assert_eq!(cadence_label(detect_cadence(&weekly).unwrap().0), "Weekly");
    }

    #[test]
    fn requires_three_recent_meetings_with_seventy_percent_overlap() {
        let sets = vec![
            vec!["a".into(), "b".into(), "c".into(), "d".into()],
            vec!["a".into(), "b".into(), "c".into()],
            vec!["a".into(), "b".into(), "c".into(), "e".into()],
        ];
        assert!(last_three_overlap(&sets));
        let weak = vec![
            vec!["a".into(), "b".into()],
            vec!["a".into(), "c".into()],
            vec!["a".into(), "d".into()],
        ];
        assert!(!last_three_overlap(&weak));
    }

    #[test]
    fn suggests_only_after_three_completed_matches() {
        let connection = test_connection();
        insert_completed_meeting(&connection, 1);
        insert_completed_meeting(&connection, 2);
        assert!(suggestions(&connection).expect("suggestions").is_empty());

        insert_completed_meeting(&connection, 3);
        let suggestions = suggestions(&connection).expect("suggestions");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].meeting_count, 3);
        assert_eq!(suggestions[0].suggested_name, "Daily sync");
    }

    #[test]
    fn dismissed_candidate_waits_for_three_additional_matches() {
        let connection = test_connection();
        for index in 1..=3 {
            insert_completed_meeting(&connection, index);
        }
        let candidate = candidates(&connection)
            .expect("candidates")
            .pop()
            .expect("candidate");
        connection
            .execute(
                "INSERT INTO meeting_group_suppressions (
                    candidate_key, meeting_count, dismissed_at
                 ) VALUES (?1, 3, 'now')",
                params![candidate.key],
            )
            .expect("suppress candidate");

        for index in 4..=5 {
            insert_completed_meeting(&connection, index);
        }
        assert!(suggestions(&connection).expect("suggestions").is_empty());

        insert_completed_meeting(&connection, 6);
        let suggestions = suggestions(&connection).expect("suggestions");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].meeting_count, 6);
    }

    #[test]
    fn grouping_rule_stops_when_fewer_than_three_meetings_remain() {
        let connection = test_connection();
        connection
            .execute(
                "INSERT INTO folders (id, name, created_at)
                 VALUES ('folder-1', 'Daily sync', 'now')",
                [],
            )
            .expect("insert folder");
        connection
            .execute(
                "
                INSERT INTO meeting_groups (
                    id, folder_id, name, candidate_key, series_key,
                    participant_source, participant_keys_json,
                    cadence_seconds, cadence_tolerance_seconds,
                    created_at, updated_at
                ) VALUES (
                    'group-1', 'folder-1', 'Daily sync', 'candidate-1',
                    'calendar:primary:daily-sync', 'calendar',
                    '[\"alex@example.com\",\"sam@example.com\"]',
                    86400, 21600, 'now', 'now'
                )
                ",
                [],
            )
            .expect("insert group");
        for index in 1..=3 {
            insert_completed_meeting(&connection, index);
            connection
                .execute(
                    "UPDATE notes SET folder_id = 'folder-1' WHERE id = ?1",
                    params![format!("note-{index}")],
                )
                .expect("move note");
            connection
                .execute(
                    "UPDATE meeting_occurrences SET group_id = 'group-1'
                     WHERE note_id = ?1",
                    params![format!("note-{index}")],
                )
                .expect("assign occurrence");
        }
        assert_eq!(load_active_rules(&connection).expect("rules").len(), 1);

        connection
            .execute("UPDATE notes SET folder_id = NULL WHERE id = 'note-3'", [])
            .expect("move meeting out");
        assert!(load_active_rules(&connection).expect("rules").is_empty());
    }
}
