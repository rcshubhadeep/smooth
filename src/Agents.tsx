import { invoke } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  ArrowLeft,
  ChevronDown,
  FileText,
  Link2,
  Loader2,
  MessageSquare,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import SlackShareAgent from "./SlackShareAgent";
import { marked } from "marked";
import { useEffect, useMemo, useState } from "react";

// ---------------------------------------------------------------------------
// Domain types + built-in agents
//
// An "agent" in the current backend is a saved prompt preset: the runtime has
// no notion of a named agent, only `agent_run(prompt, max_steps)` over the
// shared tool registry. Keeping the shape here (not inline in App.tsx) means
// Phase 2 (user-defined agents) and Phase 3 (run history) can reuse the exact
// same `AgentDefinition` / `AgentRunResult` types and the `AgentRunResultView`
// renderer without rework.
// ---------------------------------------------------------------------------

export type AgentScope = "note" | "global";

/** Built-in presets ship with the app; user agents come from the DB (Phase 2). */
export type AgentSource = "builtin" | "user";

export type AgentDefinition = {
  id: string;
  name: string;
  description: string;
  /** "note" agents act on the open note; "global" agents roam the whole bank. */
  scope: AgentScope;
  /** The task handed to the model. May be composed with note context at run time. */
  instructions: string;
  icon: AgentIconName;
  /** Optional cap on tool-use iterations; backend clamps to a safe range. */
  maxSteps?: number;
  source: AgentSource;
};

export type AgentIconName = "summary" | "links" | "overview" | "slack";

// Mirrors `flow::AgentRunResult` / `flow::AgentRunStep` in the Rust backend.
export type AgentRunStep = {
  tool_name: string;
  input: unknown;
  output: unknown | null;
  error: string | null;
};

export type AgentRunResult = {
  run_id: string;
  model: string;
  answer: string;
  steps: AgentRunStep[];
  raw_model_output: string;
};

/** Minimal note shape the Agents UI needs — structurally satisfied by NoteListItem. */
export type AgentNoteRef = { id: string; title: string };

const AGENT_ICONS: Record<AgentIconName, typeof FileText> = {
  summary: FileText,
  links: Link2,
  overview: Sparkles,
  slack: MessageSquare,
};

export const BUILTIN_AGENTS: AgentDefinition[] = [
  {
    id: "share-note-slack",
    name: "Share to Slack",
    description: "Reviews this note and posts it to a Slack channel or thread.",
    scope: "note",
    icon: "slack",
    source: "builtin",
    instructions: "Prepare this note for a user-approved Slack post.",
  },
  {
    id: "summarize-note",
    name: "Summarize this note",
    description: "Reads the open note and distills it into a few sharp bullets.",
    scope: "note",
    icon: "summary",
    source: "builtin",
    instructions:
      "Read the current note and write a concise summary: 3–5 bullet points capturing the key ideas, decisions, and any action items. Stay faithful to the note — do not invent details.",
  },
  {
    id: "suggest-links",
    name: "Suggest links",
    description: "Finds related notes and explains why they connect.",
    scope: "note",
    icon: "links",
    source: "builtin",
    instructions:
      "For the current note, find the most related notes in the knowledge bank. Recommend up to 5 to link, and for each give one short sentence on why it is relevant (shared topics, entities, or themes).",
  },
  {
    id: "bank-overview",
    name: "Knowledge bank overview",
    description: "Surveys your notes and surfaces the main themes.",
    scope: "global",
    icon: "overview",
    source: "builtin",
    instructions:
      "Search across the knowledge bank and identify the main recurring themes. For each theme give a short name, one line describing it, and list 2–3 representative note titles.",
  },
];

/**
 * Build the prompt sent to `agent_run`. Note-scoped agents are handed the note
 * id + title and told to use the `read_note` tool, so the actual content is
 * fetched through the same vetted path every other tool uses (never inlined /
 * never raw SQL). Global agents run their instructions verbatim.
 */
export function composeAgentPrompt(
  agent: AgentDefinition,
  note?: AgentNoteRef | null,
): string {
  if (agent.scope === "note" && note) {
    return [
      "You are operating on one specific note in the user's knowledge bank.",
      `Note id: ${note.id}`,
      `Note title: ${note.title || "(untitled)"}`,
      "Call the read_note tool with this id to read the note content before answering.",
      "",
      `Task: ${agent.instructions}`,
    ].join("\n");
  }
  return agent.instructions;
}

export async function runAgent(
  agent: AgentDefinition,
  note?: AgentNoteRef | null,
): Promise<AgentRunResult> {
  const prompt = composeAgentPrompt(agent, note);
  return invoke<AgentRunResult>("agent_run", {
    prompt,
    maxSteps: agent.maxSteps ?? null,
  });
}

// ---------------------------------------------------------------------------
// User-defined agents (Phase 2) — thin wrappers over the Tauri commands.
// ---------------------------------------------------------------------------

/** Mirrors `persistence::AgentDefinitionRecord`. */
type AgentDefinitionRecord = {
  id: string;
  name: string;
  description: string;
  instructions: string;
  scope: string;
  icon: string;
  max_steps: number | null;
  created_at: string;
  updated_at: string;
};

/** Editable fields sent to create/update (matches `AgentDefinitionInput`). */
export type AgentDraft = {
  name: string;
  description: string;
  instructions: string;
  max_steps: number | null;
};

function normalizeIcon(icon: string): AgentIconName {
  return icon === "summary" || icon === "links" || icon === "slack" ? icon : "overview";
}

function recordToAgent(record: AgentDefinitionRecord): AgentDefinition {
  return {
    id: record.id,
    name: record.name,
    description: record.description,
    scope: record.scope === "note" ? "note" : "global",
    instructions: record.instructions,
    icon: normalizeIcon(record.icon),
    maxSteps: record.max_steps ?? undefined,
    source: "user",
  };
}

export async function listUserAgents(): Promise<AgentDefinition[]> {
  const rows = await invoke<AgentDefinitionRecord[]>("agent_list_definitions");
  return rows.map(recordToAgent);
}

export async function createUserAgent(draft: AgentDraft): Promise<AgentDefinition> {
  const record = await invoke<AgentDefinitionRecord>("agent_create_definition", {
    definition: draft,
  });
  return recordToAgent(record);
}

export async function updateUserAgent(
  id: string,
  draft: AgentDraft,
): Promise<AgentDefinition> {
  const record = await invoke<AgentDefinitionRecord>("agent_update_definition", {
    id,
    definition: draft,
  });
  return recordToAgent(record);
}

export async function deleteUserAgent(id: string): Promise<void> {
  await invoke("agent_delete_definition", { id });
}

// ---------------------------------------------------------------------------
// Run history (Phase 3) — reads the persisted runs/events the backend already
// records for every `agent_run`. Detail view reuses `AgentRunResultView` by
// reconstructing its step trace from the stored `tool_execution` events.
// ---------------------------------------------------------------------------

/** Mirrors `persistence::AgentRunRecord`. */
export type AgentRunRecord = {
  id: string;
  run_kind: string;
  status: string;
  prompt: string;
  model: string | null;
  max_steps: number;
  answer: string | null;
  error: string | null;
  started_at: string;
  completed_at: string | null;
  updated_at: string;
};

/** Mirrors `persistence::AgentEventRecord`. */
export type AgentEventRecord = {
  id: string;
  run_id: string;
  sequence: number;
  event_type: string;
  role: string | null;
  tool_name: string | null;
  content: string | null;
  input_json: unknown | null;
  output_json: unknown | null;
  error: string | null;
  created_at: string;
};

export async function listRuns(limit = 50): Promise<AgentRunRecord[]> {
  return invoke<AgentRunRecord[]>("agent_list_runs", { options: { limit } });
}

export async function getRunEvents(runId: string): Promise<AgentEventRecord[]> {
  return invoke<AgentEventRecord[]>("agent_get_run_events", { runId });
}

function formatTimestamp(value: string): string {
  const ms = Number(value);
  return Number.isFinite(ms) ? new Date(ms).toLocaleString() : value;
}

/** Rebuild the tool-step trace from stored events for `AgentRunResultView`. */
function eventsToSteps(events: AgentEventRecord[]): AgentRunStep[] {
  return events
    .filter((event) => event.event_type === "tool_execution")
    .map((event) => ({
      tool_name: event.tool_name ?? "tool",
      input: event.input_json ?? null,
      output: event.output_json ?? null,
      error: event.error,
    }));
}

function renderMarkdown(text: string) {
  return { __html: marked.parse(text, { async: false }) as string };
}

// ---------------------------------------------------------------------------
// Shared result renderer — reused by the Agents view, the note-panel Agents
// tab (Phase 1.3) and the run-history drill-in (Phase 3).
// ---------------------------------------------------------------------------

export function AgentRunResultView({ result }: { result: AgentRunResult }) {
  const [showSteps, setShowSteps] = useState(false);
  const stepCount = result.steps.length;

  return (
    <div className="agent-result">
      <div
        className="agent-answer"
        dangerouslySetInnerHTML={renderMarkdown(
          result.answer.trim() || "_The agent returned no answer._",
        )}
      />

      {stepCount > 0 ? (
        <div className="agent-steps">
          <button
            type="button"
            className="agent-steps-toggle"
            aria-expanded={showSteps}
            onClick={() => setShowSteps((value) => !value)}
          >
            <ChevronDown size={14} className={showSteps ? "chev open" : "chev"} />
            {stepCount} tool {stepCount === 1 ? "step" : "steps"}
          </button>

          {showSteps ? (
            <ol className="agent-step-list">
              {result.steps.map((step, index) => (
                <li
                  key={index}
                  className={step.error ? "agent-step is-error" : "agent-step"}
                >
                  <div className="agent-step-head">
                    <code>{step.tool_name}</code>
                  </div>
                  <pre className="agent-step-io">
                    {JSON.stringify(step.input, null, 2)}
                  </pre>
                  {step.error ? (
                    <pre className="agent-step-io err">{step.error}</pre>
                  ) : (
                    <pre className="agent-step-io">
                      {JSON.stringify(step.output ?? null, null, 2)}
                    </pre>
                  )}
                </li>
              ))}
            </ol>
          ) : null}
        </div>
      ) : null}

      {result.model ? (
        <div className="agent-result-meta">model · {result.model}</div>
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Top-level Agents view
// ---------------------------------------------------------------------------

type RunState =
  | { status: "idle" }
  | { status: "running"; agent: AgentDefinition; note: AgentNoteRef | null }
  | {
      status: "done";
      agent: AgentDefinition;
      note: AgentNoteRef | null;
      result: AgentRunResult;
    }
  | {
      status: "error";
      agent: AgentDefinition;
      note: AgentNoteRef | null;
      message: string;
    };

type EditorState =
  | { mode: "create" }
  | { mode: "edit"; agent: AgentDefinition }
  | null;

export function AgentsView({
  notes,
  currentNoteId,
  onClose,
}: {
  notes: AgentNoteRef[];
  currentNoteId?: string | null;
  onClose: () => void;
}) {
  const [tab, setTab] = useState<"agents" | "history">("agents");
  const [run, setRun] = useState<RunState>({ status: "idle" });
  const [userAgents, setUserAgents] = useState<AgentDefinition[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState>(null);
  // Inline confirm — native window.confirm() is blocked in the Tauri webview.
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [targetNoteId, setTargetNoteId] = useState<string>(
    () => currentNoteId ?? notes[0]?.id ?? "",
  );
  const [slackShareNote, setSlackShareNote] = useState<AgentNoteRef | null>(null);

  const targetNote = useMemo(
    () => notes.find((note) => note.id === targetNoteId) ?? null,
    [notes, targetNoteId],
  );

  async function refresh() {
    try {
      setUserAgents(await listUserAgents());
      setLoadError(null);
    } catch (error) {
      setLoadError(String(error));
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function start(agent: AgentDefinition) {
    const note = agent.scope === "note" ? targetNote : null;
    if (agent.scope === "note" && !note) {
      setRun({
        status: "error",
        agent,
        note: null,
        message: "Pick a note for this agent to run on.",
      });
      return;
    }
    if (agent.id === "share-note-slack" && note) {
      setSlackShareNote(note);
      return;
    }

    setRun({ status: "running", agent, note });
    try {
      const result = await runAgent(agent, note);
      setRun({ status: "done", agent, note, result });
    } catch (error) {
      setRun({ status: "error", agent, note, message: String(error) });
    }
  }

  async function remove(agent: AgentDefinition) {
    try {
      await deleteUserAgent(agent.id);
      setConfirmDeleteId(null);
      await refresh();
    } catch (error) {
      setConfirmDeleteId(null);
      setLoadError(String(error));
    }
  }

  // A live run takes over the whole view; the editor is next in priority.
  if (slackShareNote) {
    return <SlackShareAgent note={slackShareNote} onClose={() => setSlackShareNote(null)} />;
  }

  if (run.status !== "idle") {
    return (
      <AgentRunPanel
        run={run}
        onBack={() => setRun({ status: "idle" })}
        onRerun={() => void start(run.agent)}
      />
    );
  }

  if (editor) {
    return (
      <AgentEditor
        initial={editor.mode === "edit" ? editor.agent : null}
        onCancel={() => setEditor(null)}
        onSaved={async () => {
          setEditor(null);
          await refresh();
        }}
      />
    );
  }

  const hasNotes = notes.length > 0;

  const renderCard = (agent: AgentDefinition) => {
    const Icon = AGENT_ICONS[agent.icon];
    const disabled = agent.scope === "note" && !targetNote;
    return (
      <article key={agent.id} className="agent-card">
        <div className="agent-card-top">
          <div className="agent-card-icon">
            <Icon size={18} />
          </div>
          {agent.source === "user" ? (
            <div className="agent-card-actions">
              <button
                type="button"
                className="ghost-icon"
                title="Edit agent"
                onClick={() => setEditor({ mode: "edit", agent })}
              >
                <Pencil size={14} />
              </button>
              <button
                type="button"
                className="ghost-icon danger"
                title="Delete agent"
                onClick={() => setConfirmDeleteId(agent.id)}
              >
                <Trash2 size={14} />
              </button>
            </div>
          ) : null}
        </div>
        <div className="agent-card-body">
          <h2>{agent.name}</h2>
          <p>{agent.description || "No description."}</p>
        </div>
        <div className="agent-card-foot">
          {confirmDeleteId === agent.id ? (
            <div className="agent-confirm">
              <span>Delete this agent?</span>
              <button
                type="button"
                className="agent-back sm"
                onClick={() => setConfirmDeleteId(null)}
              >
                Cancel
              </button>
              <button
                type="button"
                className="agent-run-btn sm danger"
                onClick={() => void remove(agent)}
              >
                Delete
              </button>
            </div>
          ) : (
            <>
              <span
                className={
                  agent.scope === "note"
                    ? "scope-badge note"
                    : "scope-badge global"
                }
              >
                {agent.scope === "note" ? "This note" : "Whole bank"}
              </span>
              <button
                type="button"
                className="agent-run-btn"
                disabled={disabled}
                title={
                  disabled ? "Choose a note above first" : `Run ${agent.name}`
                }
                onClick={() => void start(agent)}
              >
                <Play size={14} />
                Run
              </button>
            </>
          )}
        </div>
      </article>
    );
  };

  return (
    <div className="agents-view">
      <header className="agents-header">
        <div>
          <h1>Agents</h1>
          <p>Run an assistant over a note or across your whole knowledge bank.</p>
        </div>
        <div className="agents-header-actions">
          {tab === "agents" ? (
            <button
              type="button"
              className="agent-run-btn"
              onClick={() => setEditor({ mode: "create" })}
            >
              <Plus size={15} />
              New agent
            </button>
          ) : null}
          <button
            type="button"
            className="agent-back"
            onClick={onClose}
            title="Back to notes"
          >
            <ArrowLeft size={16} />
            Notes
          </button>
        </div>
      </header>

      <div className="agents-tabs">
        <div className="segmented" role="tablist">
          <button
            type="button"
            role="tab"
            aria-selected={tab === "agents"}
            className={tab === "agents" ? "active" : ""}
            onClick={() => setTab("agents")}
          >
            Agents
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={tab === "history"}
            className={tab === "history" ? "active" : ""}
            onClick={() => setTab("history")}
          >
            History
          </button>
        </div>
      </div>

      {tab === "history" ? <RunHistory /> : null}

      <div className="agent-target" hidden={tab !== "agents"}>
        <label htmlFor="agent-target-note">Note for note-scoped agents</label>
        <select
          id="agent-target-note"
          value={targetNoteId}
          disabled={!hasNotes}
          onChange={(event) => setTargetNoteId(event.target.value)}
        >
          {hasNotes ? (
            notes.map((note) => (
              <option key={note.id} value={note.id}>
                {note.title || "(untitled)"}
              </option>
            ))
          ) : (
            <option value="">No notes yet</option>
          )}
        </select>
      </div>

      {tab === "agents" ? (
        <>
          {loadError ? (
            <div className="agent-run-state error sm">
              <AlertTriangle size={15} />
              <span>{loadError}</span>
            </div>
          ) : null}

          <h3 className="agent-section-label">Built-in</h3>
          <div className="agent-grid">{BUILTIN_AGENTS.map(renderCard)}</div>

          <h3 className="agent-section-label">Your agents</h3>
          {userAgents.length > 0 ? (
            <div className="agent-grid">{userAgents.map(renderCard)}</div>
          ) : (
            <p className="agent-empty">
              No custom agents yet. Use <strong>New agent</strong> to create one
              from your own instructions.
            </p>
          )}
        </>
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Create / edit form for user-defined agents.
// ---------------------------------------------------------------------------

function AgentEditor({
  initial,
  onCancel,
  onSaved,
}: {
  initial: AgentDefinition | null;
  onCancel: () => void;
  onSaved: () => void | Promise<void>;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [description, setDescription] = useState(initial?.description ?? "");
  const [instructions, setInstructions] = useState(initial?.instructions ?? "");
  const [maxSteps, setMaxSteps] = useState<string>(
    initial?.maxSteps ? String(initial.maxSteps) : "",
  );
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const canSave = name.trim().length > 0 && instructions.trim().length > 0;

  async function save() {
    if (!canSave) {
      setError("Name and instructions are required.");
      return;
    }
    const parsedSteps = maxSteps.trim() ? Number(maxSteps) : null;
    const draft: AgentDraft = {
      name: name.trim(),
      description: description.trim(),
      instructions: instructions.trim(),
      max_steps:
        parsedSteps !== null && Number.isFinite(parsedSteps)
          ? parsedSteps
          : null,
    };

    setSaving(true);
    setError(null);
    try {
      if (initial) {
        await updateUserAgent(initial.id, draft);
      } else {
        await createUserAgent(draft);
      }
      await onSaved();
    } catch (saveError) {
      setError(String(saveError));
      setSaving(false);
    }
  }

  return (
    <div className="agents-view">
      <header className="agents-header run">
        <button type="button" className="agent-back" onClick={onCancel}>
          <ArrowLeft size={16} />
          All agents
        </button>
      </header>

      <div className="agent-editor">
        <h2>{initial ? "Edit agent" : "New agent"}</h2>
        <p className="agent-editor-note">
          Custom agents run across your whole knowledge bank using the available
          tools.
        </p>

        <label className="agent-field">
          <span>Name</span>
          <input
            type="text"
            value={name}
            placeholder="e.g. Weekly digest"
            onChange={(event) => setName(event.target.value)}
          />
        </label>

        <label className="agent-field">
          <span>Description</span>
          <input
            type="text"
            value={description}
            placeholder="Short summary shown on the card"
            onChange={(event) => setDescription(event.target.value)}
          />
        </label>

        <label className="agent-field">
          <span>Instructions</span>
          <textarea
            value={instructions}
            rows={7}
            placeholder="Tell the agent what to do. It can read, search and link your notes."
            onChange={(event) => setInstructions(event.target.value)}
          />
        </label>

        <label className="agent-field narrow">
          <span>Max steps (optional, 1–6)</span>
          <input
            type="number"
            min={1}
            max={6}
            value={maxSteps}
            placeholder="3"
            onChange={(event) => setMaxSteps(event.target.value)}
          />
        </label>

        {error ? (
          <div className="agent-run-state error sm">
            <AlertTriangle size={15} />
            <span>{error}</span>
          </div>
        ) : null}

        <div className="agent-editor-actions">
          <button type="button" className="agent-back" onClick={onCancel}>
            Cancel
          </button>
          <button
            type="button"
            className="agent-run-btn"
            disabled={!canSave || saving}
            onClick={() => void save()}
          >
            {saving ? <Loader2 size={14} className="spin" /> : null}
            {initial ? "Save changes" : "Create agent"}
          </button>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Run history tab
// ---------------------------------------------------------------------------

function RunHistory() {
  const [runs, setRuns] = useState<AgentRunRecord[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<AgentRunRecord | null>(null);
  const [events, setEvents] = useState<AgentEventRecord[] | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);

  async function refresh() {
    setRuns(null);
    setError(null);
    try {
      setRuns(await listRuns());
    } catch (loadError) {
      setError(String(loadError));
      setRuns([]);
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function open(run: AgentRunRecord) {
    setSelected(run);
    setEvents(null);
    setDetailError(null);
    try {
      setEvents(await getRunEvents(run.id));
    } catch (loadError) {
      setDetailError(String(loadError));
      setEvents([]);
    }
  }

  if (selected) {
    const result: AgentRunResult = {
      run_id: selected.id,
      model: selected.model ?? "",
      answer: selected.answer ?? "",
      steps: events ? eventsToSteps(events) : [],
      raw_model_output: "",
    };
    return (
      <div className="run-detail">
        <button
          type="button"
          className="agent-back"
          onClick={() => setSelected(null)}
        >
          <ArrowLeft size={16} />
          All runs
        </button>

        <div className="run-detail-head">
          <span className={`run-status ${selected.status}`}>
            {selected.status}
          </span>
          <span className="run-detail-time">
            {formatTimestamp(selected.started_at)}
          </span>
          {selected.model ? (
            <span className="run-detail-model">{selected.model}</span>
          ) : null}
        </div>

        <div className="run-prompt">{selected.prompt}</div>

        {selected.error ? (
          <div className="agent-run-state error sm">
            <AlertTriangle size={15} />
            <span>{selected.error}</span>
          </div>
        ) : null}

        {events === null ? (
          <div className="agent-run-state busy sm">
            <Loader2 size={15} className="spin" />
            <span>Loading trace…</span>
          </div>
        ) : detailError ? (
          <div className="agent-run-state error sm">
            <AlertTriangle size={15} />
            <span>{detailError}</span>
          </div>
        ) : (
          <AgentRunResultView result={result} />
        )}
      </div>
    );
  }

  return (
    <div className="run-history">
      <div className="run-history-bar">
        <button
          type="button"
          className="agent-back"
          onClick={() => void refresh()}
        >
          <RefreshCw size={14} />
          Refresh
        </button>
      </div>

      {error ? (
        <div className="agent-run-state error sm">
          <AlertTriangle size={15} />
          <span>{error}</span>
        </div>
      ) : null}

      {runs === null ? (
        <div className="agent-run-state busy sm">
          <Loader2 size={15} className="spin" />
          <span>Loading runs…</span>
        </div>
      ) : runs.length === 0 ? (
        <p className="agent-empty">
          No runs yet. Run an agent and it will show up here.
        </p>
      ) : (
        <ul className="run-list">
          {runs.map((run) => (
            <li key={run.id}>
              <button
                type="button"
                className="run-row"
                onClick={() => void open(run)}
              >
                <span className={`run-status-dot ${run.status}`} />
                <span className="run-row-prompt">{run.prompt}</span>
                <span className="run-row-meta">
                  {formatTimestamp(run.started_at)}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Note-scoped agents, embedded in the right context panel. Shows only agents
// whose scope is "note" and always runs them against the open note. Mount with
// `key={note.id}` so switching notes resets any in-flight result.
// ---------------------------------------------------------------------------

type NoteRunState =
  | { status: "idle" }
  | { status: "running"; agent: AgentDefinition }
  | { status: "done"; agent: AgentDefinition; result: AgentRunResult }
  | { status: "error"; agent: AgentDefinition; message: string };

export function NoteAgentsPanel({ note }: { note: AgentNoteRef }) {
  const agents = useMemo(
    () => BUILTIN_AGENTS.filter((agent) => agent.scope === "note"),
    [],
  );
  const [run, setRun] = useState<NoteRunState>({ status: "idle" });
  const [slackShareOpen, setSlackShareOpen] = useState(false);
  const busy = run.status === "running";

  async function start(agent: AgentDefinition) {
    if (agent.id === "share-note-slack") {
      setSlackShareOpen(true);
      return;
    }
    setRun({ status: "running", agent });
    try {
      const result = await runAgent(agent, note);
      setRun({ status: "done", agent, result });
    } catch (error) {
      setRun({ status: "error", agent, message: String(error) });
    }
  }

  return (
    <div className="note-agents">
      <div className="context-heading">
        <span>Agents</span>
      </div>

      <ul className="note-agent-list">
        {agents.map((agent) => {
          const Icon = AGENT_ICONS[agent.icon];
          const isRunning = busy && run.agent.id === agent.id;
          return (
            <li key={agent.id} className="note-agent-row">
              <div className="agent-card-icon sm">
                <Icon size={15} />
              </div>
              <div className="note-agent-meta">
                <strong>{agent.name}</strong>
                <span>{agent.description}</span>
              </div>
              <button
                type="button"
                className="agent-run-btn sm"
                disabled={busy}
                onClick={() => void start(agent)}
              >
                {isRunning ? (
                  <Loader2 size={13} className="spin" />
                ) : (
                  <Play size={13} />
                )}
                Run
              </button>
            </li>
          );
        })}
      </ul>

      {run.status === "running" ? (
        <div className="agent-run-state busy sm">
          <Loader2 size={15} className="spin" />
          <span>Working on “{run.agent.name}”…</span>
        </div>
      ) : null}

      {run.status === "error" ? (
        <div className="agent-run-state error sm">
          <AlertTriangle size={15} />
          <span>{run.message}</span>
        </div>
      ) : null}

      {run.status === "done" ? (
        <div className="note-agent-result">
          <div className="note-agent-result-head">
            <span>{run.agent.name}</span>
            <button
              type="button"
              className="ghost-icon"
              title="Clear result"
              onClick={() => setRun({ status: "idle" })}
            >
              <X size={14} />
            </button>
          </div>
          <AgentRunResultView result={run.result} />
        </div>
      ) : null}

      {slackShareOpen ? (
        <SlackShareAgent note={note} onClose={() => setSlackShareOpen(false)} />
      ) : null}
    </div>
  );
}

function AgentRunPanel({
  run,
  onBack,
  onRerun,
}: {
  run: Exclude<RunState, { status: "idle" }>;
  onBack: () => void;
  onRerun: () => void;
}) {
  const Icon = AGENT_ICONS[run.agent.icon];
  const running = run.status === "running";

  return (
    <div className="agents-view">
      <header className="agents-header run">
        <button type="button" className="agent-back" onClick={onBack}>
          <ArrowLeft size={16} />
          All agents
        </button>
      </header>

      <div className="agent-run-panel">
        <div className="agent-run-title">
          <div className="agent-card-icon">
            <Icon size={18} />
          </div>
          <div>
            <h2>{run.agent.name}</h2>
            {run.note ? (
              <p className="agent-run-target">on “{run.note.title || "untitled"}”</p>
            ) : (
              <p className="agent-run-target">across your knowledge bank</p>
            )}
          </div>
          {!running ? (
            <button type="button" className="agent-run-btn ghost" onClick={onRerun}>
              <Play size={14} />
              Run again
            </button>
          ) : null}
        </div>

        {run.status === "running" ? (
          <div className="agent-run-state busy">
            <Loader2 size={18} className="spin" />
            <span>Working… the agent is reading and reasoning over your notes.</span>
          </div>
        ) : null}

        {run.status === "error" ? (
          <div className="agent-run-state error">
            <AlertTriangle size={18} />
            <span>{run.message}</span>
          </div>
        ) : null}

        {run.status === "done" ? <AgentRunResultView result={run.result} /> : null}
      </div>
    </div>
  );
}
