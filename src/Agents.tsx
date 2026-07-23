import { invoke } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  ArrowLeft,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  FilePlus,
  FileText,
  Link2,
  ListChecks,
  Loader2,
  Mail,
  MessageSquare,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Search,
  Sparkles,
  Trash2,
  Wrench,
  X,
} from "lucide-react";
import SlackShareAgent from "./SlackShareAgent";
import FollowUpEmailAgent from "./FollowUpEmailAgent";
import { marked } from "marked";
import { useEffect, useMemo, useState } from "react";
import LlmRunChoiceDialog from "./LlmRunChoice";
import {
  loadLlmPreferences,
  type LlmPreferences,
  type LlmProvider,
} from "./llmPreferences";

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
  resultKind: "text" | "external_slack" | "external_gmail";
};

export type AgentIconName = "summary" | "links" | "overview" | "slack" | "email" | "todo";

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
  email: Mail,
  todo: ListChecks,
};

type TaskDefinitionRecord = {
  id: string;
  name: string;
  description: string;
  instructions: string;
  scope: string;
  icon: string;
  maxSteps: number | null;
  source: string;
  resultKind: string;
};

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
  provider: LlmProvider | null = null,
): Promise<AgentRunResult> {
  const prompt = composeAgentPrompt(agent, note);
  return invoke<AgentRunResult>("agent_run", {
    agentId: agent.id,
    prompt,
    maxSteps: agent.maxSteps ?? null,
    selection: provider ? { provider, model: null } : null,
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
  scope?: AgentScope;
  icon?: AgentIconName;
};

function normalizeIcon(icon: string): AgentIconName {
  return icon === "summary" || icon === "links" || icon === "slack" || icon === "email" || icon === "todo"
    ? icon
    : "overview";
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
    resultKind: "text",
  };
}

function taskRecordToAgent(record: TaskDefinitionRecord): AgentDefinition {
  return {
    id: record.id,
    name: record.name,
    description: record.description,
    scope: record.scope === "note" ? "note" : "global",
    instructions: record.instructions,
    icon: normalizeIcon(record.icon),
    maxSteps: record.maxSteps ?? undefined,
    source: record.source === "user" ? "user" : "builtin",
    resultKind:
      record.resultKind === "external_slack" || record.resultKind === "external_gmail"
        ? record.resultKind
        : "text",
  };
}

export async function listTasks(): Promise<AgentDefinition[]> {
  const rows = await invoke<TaskDefinitionRecord[]>("agent_list_tasks");
  return rows.map(taskRecordToAgent);
}

export async function listUserAgents(): Promise<AgentDefinition[]> {
  const rows = await invoke<AgentDefinitionRecord[]>("agent_list_definitions");
  return rows.map(recordToAgent);
}

export async function createUserAgent(draft: AgentDraft): Promise<AgentDefinition> {
  const record = await invoke<AgentDefinitionRecord>("agent_create_definition", {
    definition: draft,
  });
  window.dispatchEvent(new Event("smooth-agent-definitions-changed"));
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
  window.dispatchEvent(new Event("smooth-agent-definitions-changed"));
  return recordToAgent(record);
}

export async function deleteUserAgent(id: string): Promise<void> {
  await invoke("agent_delete_definition", { id });
  window.dispatchEvent(new Event("smooth-agent-definitions-changed"));
}

// ---------------------------------------------------------------------------
// Tool registry — every agent runs over this same shared set of tools. Mirrors
// `registry::ToolDescriptor`.
// ---------------------------------------------------------------------------

export type AgentToolInfo = {
  name: string;
  description: string;
  input_schema: unknown;
};

export async function listAgentTools(): Promise<AgentToolInfo[]> {
  return invoke<AgentToolInfo[]>("agent_list_tools");
}

// ---------------------------------------------------------------------------
// Run history (Phase 3) — reads the persisted runs/events the backend already
// records for every `agent_run`. Detail view reuses `AgentRunResultView` by
// reconstructing its step trace from the stored `tool_execution` events.
// ---------------------------------------------------------------------------

/** Mirrors `persistence::AgentRunRecord`. */
export type AgentRunRecord = {
  id: string;
  agent_id: string | null;
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

export async function listRuns(
  options: { limit?: number; agentId?: string } = {},
): Promise<AgentRunRecord[]> {
  return invoke<AgentRunRecord[]>("agent_list_runs", {
    options: {
      limit: options.limit ?? 50,
      agent_id: options.agentId ?? null,
    },
  });
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

// Custom (user-defined) agents are temporarily hidden while the creation flow
// is being rethought. Flip to `true` to restore the "New agent" button, the
// editor, and the "Your agents" section — all the code below stays intact.
const CUSTOM_AGENTS_ENABLED = true;

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
  const [inspect, setInspect] = useState<AgentDefinition | null>(null);
  const [availableAgents, setAvailableAgents] = useState<AgentDefinition[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState>(null);
  // Inline confirm — native window.confirm() is blocked in the Tauri webview.
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [slackShare, setSlackShare] = useState<{ note: AgentNoteRef; provider: LlmProvider | null } | null>(null);
  const [followUp, setFollowUp] = useState<{ note: AgentNoteRef; provider: LlmProvider | null } | null>(null);
  const [llmPreferences, setLlmPreferences] = useState<LlmPreferences | null>(null);
  const [pendingAgent, setPendingAgent] = useState<AgentDefinition | null>(null);

  // Note-scoped agents run against the currently-open note (falling back to the
  // first note). The Agents page no longer shows a note picker — those agents
  // are launched from a note's own Agents panel.
  const targetNote = useMemo(
    () => notes.find((note) => note.id === currentNoteId) ?? notes[0] ?? null,
    [notes, currentNoteId],
  );

  async function refresh() {
    try {
      setAvailableAgents(await listTasks());
      setLoadError(null);
    } catch (error) {
      setLoadError(String(error));
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  const builtinAgents = availableAgents.filter(({ source }) => source === "builtin");
  const userAgents = availableAgents.filter(({ source }) => source === "user");

  useEffect(() => {
    let active = true;
    setPendingAgent(null);
    loadLlmPreferences()
      .then((preferences) => {
        if (active) setLlmPreferences(preferences);
      })
      .catch(() => {
        if (active) setLlmPreferences({ defaultProvider: "local", alwaysObeyGlobal: false });
      });
    return () => {
      active = false;
    };
  }, [currentNoteId]);

  async function executeStart(agent: AgentDefinition, provider: LlmProvider | null) {
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
      setSlackShare({ note, provider });
      return;
    }
    if (agent.id === "create-gmail-draft" && note) {
      setFollowUp({ note, provider });
      return;
    }

    setRun({ status: "running", agent, note });
    try {
      const result = await runAgent(agent, note, provider);
      setRun({ status: "done", agent, note, result });
    } catch (error) {
      setRun({ status: "error", agent, note, message: String(error) });
    }
  }

  async function start(agent: AgentDefinition) {
    if (agent.scope === "note" && !targetNote) {
      await executeStart(agent, null);
      return;
    }

    // Reload fresh each run so toggling "always obey" in Settings applies now.
    let preferences: LlmPreferences;
    try {
      preferences = await loadLlmPreferences();
      setLlmPreferences(preferences);
    } catch {
      preferences = { defaultProvider: "local", alwaysObeyGlobal: false };
    }

    if (preferences.alwaysObeyGlobal) {
      await executeStart(agent, null);
    } else {
      setPendingAgent(agent);
    }
  }

  const choiceDialog = pendingAgent && llmPreferences ? (
    <LlmRunChoiceDialog
      defaultProvider={llmPreferences.defaultProvider}
      actionLabel={`Run “${pendingAgent.name}” with:`}
      onCancel={() => setPendingAgent(null)}
      onChoose={(provider, alwaysObey) => {
        const agent = pendingAgent;
        setPendingAgent(null);
        if (alwaysObey) {
          void invoke("set_always_obey_global_llm", { enabled: true });
          setLlmPreferences((prev) =>
            prev ? { ...prev, alwaysObeyGlobal: true } : prev,
          );
        }
        if (agent) void executeStart(agent, provider);
      }}
    />
  ) : null;

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
  if (slackShare) {
    return <SlackShareAgent note={slackShare.note} provider={slackShare.provider} onClose={() => setSlackShare(null)} />;
  }

  if (followUp) {
    return <FollowUpEmailAgent note={followUp.note} provider={followUp.provider} onClose={() => setFollowUp(null)} />;
  }

  if (run.status !== "idle") {
    return (
      <>
      <AgentRunPanel
        run={run}
        onBack={() => setRun({ status: "idle" })}
        onRerun={() => void start(run.agent)}
      />
      {choiceDialog}
      </>
    );
  }

  if (inspect) {
    return (
      <>
      <AgentInspect
        agent={inspect}
        note={inspect.scope === "note" ? targetNote : null}
        onRun={() => void start(inspect)}
        onBack={() => setInspect(null)}
      />
      {choiceDialog}
      </>
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

  const renderCard = (agent: AgentDefinition) => {
    const Icon = AGENT_ICONS[agent.icon];
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
                className="agent-run-btn ghost"
                title={`Inspect ${agent.name}`}
                onClick={() => setInspect(agent)}
              >
                <Search size={14} />
                Inspect
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
          {tab === "agents" && CUSTOM_AGENTS_ENABLED ? (
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

      {tab === "agents" ? (
        <>
          {loadError ? (
            <div className="agent-run-state error sm">
              <AlertTriangle size={15} />
              <span>{loadError}</span>
            </div>
          ) : null}

          {CUSTOM_AGENTS_ENABLED ? (
            <h3 className="agent-section-label">Built-in</h3>
          ) : null}
          <div className="agent-grid">{builtinAgents.map(renderCard)}</div>

          {CUSTOM_AGENTS_ENABLED ? (
            <>
              <h3 className="agent-section-label">Your agents</h3>
              {userAgents.length > 0 ? (
                <div className="agent-grid">{userAgents.map(renderCard)}</div>
              ) : (
                <p className="agent-empty">
                  No custom agents yet. Use <strong>New agent</strong> to create
                  one from your own instructions.
                </p>
              )}
            </>
          ) : null}
        </>
      ) : null}
      {choiceDialog}
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
  const [scope, setScope] = useState<AgentScope>(initial?.scope ?? "note");
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
      scope,
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
          Custom tasks use the same runner everywhere: from notes, live calls,
          and scheduled workflows.
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
          <span>Works on</span>
          <select
            value={scope}
            onChange={(event) => setScope(event.currentTarget.value as AgentScope)}
          >
            <option value="note">A note or live call</option>
            <option value="global">The whole knowledge bank</option>
          </select>
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

function RunRow({ run, onOpen }: { run: AgentRunRecord; onOpen: () => void }) {
  return (
    <li>
      <button type="button" className="run-row" onClick={onOpen}>
        <span className={`run-status-dot ${run.status}`} />
        <span className="run-row-prompt">{run.prompt}</span>
        <span className="run-row-meta">{formatTimestamp(run.started_at)}</span>
      </button>
    </li>
  );
}

function RunDetail({
  run,
  onBack,
  backLabel = "All runs",
}: {
  run: AgentRunRecord;
  onBack: () => void;
  backLabel?: string;
}) {
  const [events, setEvents] = useState<AgentEventRecord[] | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setEvents(null);
    setDetailError(null);
    getRunEvents(run.id)
      .then((rows) => {
        if (active) setEvents(rows);
      })
      .catch((loadError) => {
        if (!active) return;
        setDetailError(String(loadError));
        setEvents([]);
      });
    return () => {
      active = false;
    };
  }, [run.id]);

  const result: AgentRunResult = {
    run_id: run.id,
    model: run.model ?? "",
    answer: run.answer ?? "",
    steps: events ? eventsToSteps(events) : [],
    raw_model_output: "",
  };

  return (
    <div className="run-detail">
      <button type="button" className="agent-back" onClick={onBack}>
        <ArrowLeft size={16} />
        {backLabel}
      </button>

      <div className="run-detail-head">
        <span className={`run-status ${run.status}`}>{run.status}</span>
        <span className="run-detail-time">{formatTimestamp(run.started_at)}</span>
        {run.model ? <span className="run-detail-model">{run.model}</span> : null}
      </div>

      <div className="run-prompt">{run.prompt}</div>

      {run.error ? (
        <div className="agent-run-state error sm">
          <AlertTriangle size={15} />
          <span>{run.error}</span>
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

function RunHistory() {
  const [runs, setRuns] = useState<AgentRunRecord[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<AgentRunRecord | null>(null);

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

  if (selected) {
    return <RunDetail run={selected} onBack={() => setSelected(null)} />;
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
            <RunRow key={run.id} run={run} onOpen={() => setSelected(run)} />
          ))}
        </ul>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Inspect view — a read-only look at one agent: the prompt it sends, the tools
// it can call, and its own run history (drilling into any run reuses the same
// RunDetail trace the History tab uses). Running is still available here so
// global agents, which have no note-panel entry point, stay reachable.
// ---------------------------------------------------------------------------

function AgentInspect({
  agent,
  note,
  onRun,
  onBack,
}: {
  agent: AgentDefinition;
  note: AgentNoteRef | null;
  onRun: () => void;
  onBack: () => void;
}) {
  const [tools, setTools] = useState<AgentToolInfo[] | null>(null);
  const [toolsError, setToolsError] = useState<string | null>(null);
  const [runs, setRuns] = useState<AgentRunRecord[] | null>(null);
  const [runsError, setRunsError] = useState<string | null>(null);
  const [selectedRun, setSelectedRun] = useState<AgentRunRecord | null>(null);

  const Icon = AGENT_ICONS[agent.icon];
  const noteScoped = agent.scope === "note";
  const canRun = !noteScoped || !!note;

  // For note-scoped agents show the prompt with a placeholder note, so the
  // template is legible even when no note is currently selected.
  const promptNote: AgentNoteRef | null = noteScoped
    ? note ?? { id: "<note-id>", title: "<note title>" }
    : null;
  const prompt = composeAgentPrompt(agent, promptNote);

  useEffect(() => {
    let active = true;
    listAgentTools()
      .then((rows) => {
        if (active) setTools(rows);
      })
      .catch((error) => {
        if (!active) return;
        setTools([]);
        setToolsError(String(error));
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    listRuns({ agentId: agent.id, limit: 200 })
      .then((rows) => {
        if (active) setRuns(rows);
      })
      .catch((error) => {
        if (!active) return;
        setRuns([]);
        setRunsError(String(error));
      });
    return () => {
      active = false;
    };
  }, [agent.id]);

  if (selectedRun) {
    return (
      <RunDetail
        run={selectedRun}
        backLabel={`${agent.name} runs`}
        onBack={() => setSelectedRun(null)}
      />
    );
  }

  return (
    <div className="agents-view">
      <header className="agents-header run">
        <button type="button" className="agent-back" onClick={onBack}>
          <ArrowLeft size={16} />
          All agents
        </button>
        <button
          type="button"
          className="agent-run-btn"
          disabled={!canRun}
          title={canRun ? `Run ${agent.name}` : "Pick a note in the agent list first"}
          onClick={onRun}
        >
          <Play size={14} />
          Run
        </button>
      </header>

      <div className="agent-inspect">
        <div className="agent-run-title">
          <div className="agent-card-icon">
            <Icon size={18} />
          </div>
          <div>
            <h2>{agent.name}</h2>
            <p className="agent-run-target">
              {agent.description || "No description."}
            </p>
          </div>
          <span className={noteScoped ? "scope-badge note" : "scope-badge global"}>
            {noteScoped ? "This note" : "Whole bank"}
          </span>
        </div>

        <section className="inspect-section">
          <h3>Prompt</h3>
          <p className="inspect-hint">
            {noteScoped
              ? "Sent to the model with the open note’s id and title filled in; the agent reads the note through the read_note tool."
              : "Sent to the model as-is."}
          </p>
          <pre className="inspect-prompt">{prompt}</pre>
        </section>

        <section className="inspect-section">
          <h3>
            Tools
            {tools ? <span className="inspect-count">{tools.length}</span> : null}
          </h3>
          <p className="inspect-hint">
            Agents can call any of these tools over your local notes.
          </p>
          {toolsError ? (
            <div className="agent-run-state error sm">
              <AlertTriangle size={15} />
              <span>{toolsError}</span>
            </div>
          ) : tools === null ? (
            <div className="agent-run-state busy sm">
              <Loader2 size={15} className="spin" />
              <span>Loading tools…</span>
            </div>
          ) : (
            <ul className="inspect-tools">
              {tools.map((tool) => (
                <li key={tool.name} className="inspect-tool">
                  <div className="inspect-tool-name">
                    <Wrench size={13} />
                    <code>{tool.name}</code>
                  </div>
                  <span>{tool.description}</span>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="inspect-section">
          <h3>
            Runs
            {runs ? <span className="inspect-count">{runs.length}</span> : null}
          </h3>
          <p className="inspect-hint">
            Past runs of this agent. The History tab lists runs from every agent.
          </p>
          {runsError ? (
            <div className="agent-run-state error sm">
              <AlertTriangle size={15} />
              <span>{runsError}</span>
            </div>
          ) : runs === null ? (
            <div className="agent-run-state busy sm">
              <Loader2 size={15} className="spin" />
              <span>Loading runs…</span>
            </div>
          ) : runs.length === 0 ? (
            <p className="agent-empty">No runs yet.</p>
          ) : (
            <ul className="run-list">
              {runs.map((run) => (
                <RunRow key={run.id} run={run} onOpen={() => setSelectedRun(run)} />
              ))}
            </ul>
          )}
        </section>
      </div>
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

const DEFAULT_TASKS_OPEN_KEY = "smooth-default-tasks-open";
const USER_TASKS_OPEN_KEY = "smooth-user-tasks-open";

function taskGroupInitiallyOpen(key: string) {
  try {
    return window.localStorage.getItem(key) !== "false";
  } catch {
    return true;
  }
}

function rememberTaskGroup(key: string, open: boolean) {
  try {
    window.localStorage.setItem(key, String(open));
  } catch {
    // Storage may be unavailable; the in-memory fold state still works.
  }
}

export function NoteAgentsPanel({
  note,
  onCreateNote,
}: {
  note: AgentNoteRef;
  onCreateNote: (
    content: string,
    sourcePrompt: string | null,
    title?: string | null,
  ) => void;
}) {
  const [agents, setAgents] = useState<AgentDefinition[]>([]);
  const [run, setRun] = useState<NoteRunState>({ status: "idle" });
  const [slackProvider, setSlackProvider] = useState<LlmProvider | null | undefined>(undefined);
  const [followUpProvider, setFollowUpProvider] = useState<LlmProvider | null | undefined>(undefined);
  const [llmPreferences, setLlmPreferences] = useState<LlmPreferences | null>(null);
  const [pendingAgent, setPendingAgent] = useState<AgentDefinition | null>(null);
  const [copied, setCopied] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [defaultTasksOpen, setDefaultTasksOpen] = useState(() =>
    taskGroupInitiallyOpen(DEFAULT_TASKS_OPEN_KEY),
  );
  const [userTasksOpen, setUserTasksOpen] = useState(() =>
    taskGroupInitiallyOpen(USER_TASKS_OPEN_KEY),
  );
  const busy = run.status === "running";
  const defaultTasks = agents.filter(({ source }) => source === "builtin");
  const userTasks = agents.filter(({ source }) => source === "user");

  useEffect(() => {
    let active = true;
    const refreshTasks = () => {
      void listTasks()
        .then((tasks) => {
          if (active) setAgents(tasks.filter((agent) => agent.scope === "note"));
        })
        .catch(() => {
          if (active) setAgents([]);
        });
    };
    refreshTasks();
    window.addEventListener("smooth-agent-definitions-changed", refreshTasks);
    return () => {
      active = false;
      window.removeEventListener("smooth-agent-definitions-changed", refreshTasks);
    };
  }, []);

  async function copyResult(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable — ignore */
    }
  }

  async function removeTask(agent: AgentDefinition) {
    if (agent.source !== "user") return;
    setDeleteError(null);
    try {
      await deleteUserAgent(agent.id);
      setAgents((current) => current.filter(({ id }) => id !== agent.id));
      setConfirmDeleteId(null);
      setRun((current) =>
        current.status !== "idle" && current.agent.id === agent.id
          ? { status: "idle" }
          : current,
      );
    } catch (reason) {
      setDeleteError(String(reason));
    }
  }

  useEffect(() => {
    let active = true;
    setPendingAgent(null);
    loadLlmPreferences()
      .then((preferences) => {
        if (active) setLlmPreferences(preferences);
      })
      .catch(() => {
        if (active) setLlmPreferences({ defaultProvider: "local", alwaysObeyGlobal: false });
      });
    return () => {
      active = false;
    };
  }, [note.id]);

  async function executeStart(agent: AgentDefinition, provider: LlmProvider | null) {
    if (agent.id === "share-note-slack") {
      setSlackProvider(provider);
      return;
    }
    if (agent.id === "create-gmail-draft") {
      setFollowUpProvider(provider);
      return;
    }
    setRun({ status: "running", agent });
    try {
      const result = await runAgent(agent, note, provider);
      setRun({ status: "done", agent, result });
    } catch (error) {
      setRun({ status: "error", agent, message: String(error) });
    }
  }

  async function start(agent: AgentDefinition) {
    // Reload fresh each run so toggling "always obey" in Settings applies now.
    let preferences: LlmPreferences;
    try {
      preferences = await loadLlmPreferences();
      setLlmPreferences(preferences);
    } catch {
      preferences = { defaultProvider: "local", alwaysObeyGlobal: false };
    }

    if (preferences.alwaysObeyGlobal) {
      await executeStart(agent, null);
    } else {
      setPendingAgent(agent);
    }
  }

  function renderTask(agent: AgentDefinition) {
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
        {confirmDeleteId === agent.id ? (
          <div className="note-agent-delete-confirm">
            <span>Delete?</span>
            <button type="button" onClick={() => setConfirmDeleteId(null)}>
              No
            </button>
            <button
              type="button"
              className="danger"
              onClick={() => void removeTask(agent)}
            >
              Yes
            </button>
          </div>
        ) : (
          <div className="note-agent-actions">
            {agent.source === "user" ? (
              <button
                type="button"
                className="ghost-icon danger"
                title={`Delete task “${agent.name}”`}
                aria-label={`Delete task ${agent.name}`}
                onClick={() => setConfirmDeleteId(agent.id)}
              >
                <Trash2 size={14} />
              </button>
            ) : null}
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
          </div>
        )}
      </li>
    );
  }

  return (
    <div className="note-agents">
      <section className="note-agent-group">
        <button
          type="button"
          className="note-agent-group-toggle"
          aria-expanded={defaultTasksOpen}
          aria-controls="default-note-tasks"
          onClick={() =>
            setDefaultTasksOpen((open) => {
              rememberTaskGroup(DEFAULT_TASKS_OPEN_KEY, !open);
              return !open;
            })
          }
        >
          {defaultTasksOpen ? (
            <ChevronDown size={14} />
          ) : (
            <ChevronRight size={14} />
          )}
          <span>Default Tasks</span>
          <small>{defaultTasks.length}</small>
        </button>
        {defaultTasksOpen ? (
          <ul className="note-agent-list" id="default-note-tasks">
            {defaultTasks.map(renderTask)}
          </ul>
        ) : null}
      </section>

      <section className="note-agent-group">
        <button
          type="button"
          className="note-agent-group-toggle"
          aria-expanded={userTasksOpen}
          aria-controls="user-note-tasks"
          onClick={() =>
            setUserTasksOpen((open) => {
              rememberTaskGroup(USER_TASKS_OPEN_KEY, !open);
              return !open;
            })
          }
        >
          {userTasksOpen ? (
            <ChevronDown size={14} />
          ) : (
            <ChevronRight size={14} />
          )}
          <span>User Tasks</span>
          <small>{userTasks.length}</small>
        </button>
        {userTasksOpen ? (
          userTasks.length > 0 ? (
            <ul className="note-agent-list" id="user-note-tasks">
              {userTasks.map(renderTask)}
            </ul>
          ) : (
            <p className="note-agent-group-empty" id="user-note-tasks">
              No user-created tasks yet.
            </p>
          )
        ) : null}
      </section>

      {deleteError ? (
        <div className="agent-run-state error sm">
          <AlertTriangle size={15} />
          <span>{deleteError}</span>
        </div>
      ) : null}

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
            <div className="note-agent-result-actions">
              <button
                type="button"
                className="ghost-icon"
                title={copied ? "Copied" : "Copy to clipboard"}
                onClick={() => void copyResult(run.result.answer)}
              >
                {copied ? (
                  <Check size={14} className="copy-tick" />
                ) : (
                  <Copy size={14} />
                )}
              </button>
              <button
                type="button"
                className="ghost-icon"
                title="Create a note from this"
                onClick={() =>
                  onCreateNote(
                    run.result.answer,
                    null,
                    run.agent.id === "create-todo"
                      ? `TO-DO — ${note.title}`
                      : `${run.agent.name} — ${note.title}`,
                  )
                }
              >
                <FilePlus size={14} />
              </button>
              <button
                type="button"
                className="ghost-icon"
                title="Clear result"
                onClick={() => setRun({ status: "idle" })}
              >
                <X size={14} />
              </button>
            </div>
          </div>
          <AgentRunResultView result={run.result} />
        </div>
      ) : null}

      {slackProvider !== undefined ? (
        <SlackShareAgent note={note} provider={slackProvider ?? null} onClose={() => setSlackProvider(undefined)} />
      ) : null}
      {followUpProvider !== undefined ? (
        <FollowUpEmailAgent note={note} provider={followUpProvider ?? null} onClose={() => setFollowUpProvider(undefined)} />
      ) : null}
      {pendingAgent && llmPreferences ? (
        <LlmRunChoiceDialog
          defaultProvider={llmPreferences.defaultProvider}
          actionLabel={`Run “${pendingAgent.name}” with:`}
          onCancel={() => setPendingAgent(null)}
          onChoose={(provider, alwaysObey) => {
            const agent = pendingAgent;
            setPendingAgent(null);
            if (alwaysObey) {
              void invoke("set_always_obey_global_llm", { enabled: true });
              setLlmPreferences((prev) =>
                prev ? { ...prev, alwaysObeyGlobal: true } : prev,
              );
            }
            if (agent) void executeStart(agent, provider);
          }}
        />
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
