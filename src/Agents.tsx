import { invoke } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  ArrowLeft,
  ChevronDown,
  FileText,
  Link2,
  Loader2,
  Play,
  Sparkles,
} from "lucide-react";
import { marked } from "marked";
import { useMemo, useState } from "react";

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
};

export type AgentIconName = "summary" | "links" | "overview";

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
};

export const BUILTIN_AGENTS: AgentDefinition[] = [
  {
    id: "summarize-note",
    name: "Summarize this note",
    description: "Reads the open note and distills it into a few sharp bullets.",
    scope: "note",
    icon: "summary",
    instructions:
      "Read the current note and write a concise summary: 3–5 bullet points capturing the key ideas, decisions, and any action items. Stay faithful to the note — do not invent details.",
  },
  {
    id: "suggest-links",
    name: "Suggest links",
    description: "Finds related notes and explains why they connect.",
    scope: "note",
    icon: "links",
    instructions:
      "For the current note, find the most related notes in the knowledge bank. Recommend up to 5 to link, and for each give one short sentence on why it is relevant (shared topics, entities, or themes).",
  },
  {
    id: "bank-overview",
    name: "Knowledge bank overview",
    description: "Surveys your notes and surfaces the main themes.",
    scope: "global",
    icon: "overview",
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

export function AgentsView({
  notes,
  currentNoteId,
}: {
  notes: AgentNoteRef[];
  currentNoteId?: string | null;
}) {
  const [run, setRun] = useState<RunState>({ status: "idle" });
  const [targetNoteId, setTargetNoteId] = useState<string>(
    () => currentNoteId ?? notes[0]?.id ?? "",
  );

  const targetNote = useMemo(
    () => notes.find((note) => note.id === targetNoteId) ?? null,
    [notes, targetNoteId],
  );

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

    setRun({ status: "running", agent, note });
    try {
      const result = await runAgent(agent, note);
      setRun({ status: "done", agent, note, result });
    } catch (error) {
      setRun({ status: "error", agent, note, message: String(error) });
    }
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

  const hasNotes = notes.length > 0;

  return (
    <div className="agents-view">
      <header className="agents-header">
        <div>
          <h1>Agents</h1>
          <p>Run an assistant over a note or across your whole knowledge bank.</p>
        </div>
      </header>

      <div className="agent-target">
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

      <div className="agent-grid">
        {BUILTIN_AGENTS.map((agent) => {
          const Icon = AGENT_ICONS[agent.icon];
          const disabled = agent.scope === "note" && !targetNote;
          return (
            <article key={agent.id} className="agent-card">
              <div className="agent-card-icon">
                <Icon size={18} />
              </div>
              <div className="agent-card-body">
                <h2>{agent.name}</h2>
                <p>{agent.description}</p>
              </div>
              <div className="agent-card-foot">
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
              </div>
            </article>
          );
        })}
      </div>
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
