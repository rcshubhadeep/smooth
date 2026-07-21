import { invoke } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  Check,
  ChevronRight,
  CircleStop,
  FileText,
  Link2,
  ListChecks,
  Loader2,
  Mail,
  MessageSquare,
  Play,
  RotateCcw,
  Send,
  Sparkles,
  Trash2,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { listTasks, type AgentDefinition } from "./Agents";
import "./ReminderWorkflows.css";

export type ReminderWorkflowStepDraft = {
  agentId: string;
};

export type ReminderWorkflowStep = {
  id: string;
  position: number;
  agentId: string;
  agentName: string;
  stepKind: "transform" | "external_slack" | "external_gmail";
  status:
    | "pending"
    | "running"
    | "awaiting_approval"
    | "succeeded"
    | "failed"
    | "cancelled";
  outputText: string | null;
  destination: string | null;
  subject: string | null;
  agentRunId: string | null;
  error: string | null;
};

export type ReminderWorkflowRecord = {
  id: string;
  reminderId: string;
  status:
    | "scheduled"
    | "running"
    | "awaiting_approval"
    | "succeeded"
    | "failed"
    | "cancelled";
  error: string | null;
  createdAt: string;
  updatedAt: string;
  resultNoteId: string | null;
  steps: ReminderWorkflowStep[];
};

type ReminderAgentOption = {
  id: string;
  name: string;
  description: string;
  external: boolean;
  icon: typeof Sparkles;
};

function reminderIcon(agent: AgentDefinition) {
  switch (agent.icon) {
    case "summary": return FileText;
    case "links": return Link2;
    case "slack": return MessageSquare;
    case "email": return Mail;
    case "todo": return ListChecks;
    default: return Sparkles;
  }
}

export function ReminderWorkflowBuilder({
  steps,
  onChange,
}: {
  steps: ReminderWorkflowStepDraft[];
  onChange: (steps: ReminderWorkflowStepDraft[]) => void;
}) {
  const [options, setOptions] = useState<ReminderAgentOption[]>([]);
  useEffect(() => {
    let active = true;
    const refreshTasks = () => {
      void listTasks()
        .then((tasks) => {
          if (!active) return;
          setOptions(
            tasks
              .filter(({ scope }) => scope === "note")
              .map((task) => ({
                id: task.id,
                name: task.name,
                description: task.description,
                external: task.resultKind.startsWith("external_"),
                icon: reminderIcon(task),
              })),
          );
        })
        .catch(() => {
          if (active) setOptions([]);
        });
    };
    refreshTasks();
    window.addEventListener("smooth-agent-definitions-changed", refreshTasks);
    return () => {
      active = false;
      window.removeEventListener("smooth-agent-definitions-changed", refreshTasks);
    };
  }, []);
  const optionById = useMemo(
    () => new Map(options.map((agent) => [agent.id, agent])),
    [options],
  );
  const hasExternal = steps.some(({ agentId }) => optionById.get(agentId)?.external);
  const selectedAgentIds = new Set(steps.map(({ agentId }) => agentId));
  const available = options.filter(
    (agent) => !selectedAgentIds.has(agent.id) && (!agent.external || !hasExternal),
  );

  function add(agentId: string) {
    if (!agentId || selectedAgentIds.has(agentId)) return;
    onChange([...steps, { agentId }]);
  }

  return (
    <section className="reminder-workflow-builder">
      <div className="reminder-workflow-heading">
        <div>
          <span>Task workflow <small>optional</small></span>
          <p>Runs in order when the reminder is due.</p>
        </div>
        {steps.length ? <span className="reminder-workflow-count">{steps.length}</span> : null}
      </div>

      {steps.length ? (
        <ol className="reminder-workflow-draft">
          {steps.map((step, index) => {
            const agent = optionById.get(step.agentId);
            if (!agent) return null;
            const Icon = agent.icon;
            return (
              <li key={`${step.agentId}-${index}`}>
                <span className="workflow-step-icon"><Icon size={15} /></span>
                <span className="workflow-step-copy">
                  <strong>{agent.name}</strong>
                  <small>{agent.external ? "Approval required" : "Runs automatically"}</small>
                </span>
                {index + 1 < steps.length ? <ChevronRight className="workflow-connector" size={14} /> : null}
                <button
                  type="button"
                  title={`Remove ${agent.name}`}
                  onClick={() => onChange(steps.filter((_, stepIndex) => stepIndex !== index))}
                >
                  <Trash2 size={14} />
                </button>
              </li>
            );
          })}
        </ol>
      ) : null}

      {!hasExternal ? (
        <div className="reminder-workflow-add">
          <select
            value=""
            onChange={(event) => add(event.currentTarget.value)}
            aria-label="Task to add"
          >
            <option value="">Add a task...</option>
            {available.map((agent) => (
              <option key={agent.id} value={agent.id}>{agent.name}</option>
            ))}
          </select>
        </div>
      ) : (
        <p className="reminder-workflow-boundary">
          External actions finish the workflow and wait for your approval.
        </p>
      )}

      {steps.length && !hasExternal ? (
        <p className="reminder-workflow-generated-only">
          The final text result will be saved as a note linked to the parent note.
        </p>
      ) : null}
    </section>
  );
}

export function ReminderWorkflowToastStatus({
  workflow,
}: {
  workflow: ReminderWorkflowRecord;
}) {
  const completed = workflow.steps.filter(({ status }) => status === "succeeded").length;
  const activeStep = workflow.steps.find(({ status }) => status === "running");
  const hasExternal = workflow.steps.some(({ stepKind }) => stepKind.startsWith("external_"));
  const labels: Record<ReminderWorkflowRecord["status"], string> = {
    scheduled: "Task workflow queued",
    running: activeStep ? `${activeStep.agentName} is working` : "Tasks are running",
    awaiting_approval: "Draft ready - approval required",
    succeeded: hasExternal ? "External action completed" : "Task result ready",
    failed: "Task workflow failed - open to retry",
    cancelled: "Task workflow cancelled",
  };
  const busy = workflow.status === "scheduled" || workflow.status === "running";

  return (
    <span className={`due-reminder-workflow ${workflow.status}`}>
      {busy ? <Loader2 size={13} className="spin" /> : null}
      {workflow.status === "awaiting_approval" ? <Send size={13} /> : null}
      {workflow.status === "succeeded" ? <Check size={13} /> : null}
      {workflow.status === "failed" ? <AlertTriangle size={13} /> : null}
      <span>{labels[workflow.status]}</span>
      <small>{completed}/{workflow.steps.length}</small>
    </span>
  );
}

export function ReminderWorkflowPanel({
  workflow,
  onChanged,
  onOpenResultNote,
}: {
  workflow: ReminderWorkflowRecord;
  onChanged: () => void | Promise<void>;
  onOpenResultNote: (noteId: string) => void;
}) {
  const [cancelling, setCancelling] = useState(false);
  const [cancelError, setCancelError] = useState<string | null>(null);
  const approvalStep = workflow.steps.find(
    ({ status }) => status === "awaiting_approval",
  );
  const finalOutput = useMemo(
    () => [...workflow.steps].reverse().find(({ outputText }) => outputText?.trim())?.outputText,
    [workflow.steps],
  );

  async function retry() {
    await invoke("retry_reminder_workflow", { workflowId: workflow.id });
    await onChanged();
  }

  async function cancel() {
    setCancelling(true);
    setCancelError(null);
    try {
      await invoke("cancel_reminder_workflow", { workflowId: workflow.id });
      await onChanged();
    } catch (reason) {
      setCancelError(String(reason));
    } finally {
      setCancelling(false);
    }
  }

  const canCancel = ["scheduled", "running", "awaiting_approval"].includes(workflow.status);

  return (
    <section className={`reminder-workflow-panel ${workflow.status}`}>
      <header>
        <div>
          <Sparkles size={14} />
          <strong>Task workflow</strong>
        </div>
        <div className="reminder-workflow-header-actions">
          <WorkflowStatus status={workflow.status} />
          {canCancel ? (
            <button
              type="button"
              className="cancel-reminder-workflow"
              disabled={cancelling}
              onClick={() => void cancel()}
            >
              {cancelling ? <Loader2 size={13} className="spin" /> : <CircleStop size={13} />}
              {cancelling ? "Cancelling" : "Cancel run"}
            </button>
          ) : null}
        </div>
      </header>

      <ol className="reminder-workflow-progress">
        {workflow.steps.map((step) => (
          <li key={step.id} className={step.status}>
            <StepStatus status={step.status} />
            <span>{step.agentName}</span>
            {step.stepKind.startsWith("external_") ? <small>External</small> : null}
          </li>
        ))}
      </ol>

      {workflow.status === "failed" ? (
        <div className="reminder-workflow-error">
          <AlertTriangle size={15} />
          <span>{workflow.error || "The workflow could not finish."}</span>
          <button type="button" onClick={() => void retry()}>
            <RotateCcw size={14} /> Retry
          </button>
        </div>
      ) : null}

      {cancelError ? <p className="form-error reminder-workflow-cancel-error">{cancelError}</p> : null}

      {approvalStep?.stepKind === "external_slack" ? (
        <SlackWorkflowApproval
          key={approvalStep.id}
          step={approvalStep}
          onApproved={onChanged}
        />
      ) : null}

      {approvalStep?.stepKind === "external_gmail" ? (
        <GmailWorkflowApproval
          key={approvalStep.id}
          step={approvalStep}
          onApproved={onChanged}
        />
      ) : null}

      {workflow.status === "succeeded" && finalOutput ? (
        <div className="reminder-workflow-output">
          <span>Result</span>
          <p>{finalOutput}</p>
          {workflow.resultNoteId ? (
            <button type="button" onClick={() => onOpenResultNote(workflow.resultNoteId!)}>
              <FileText size={14} /> Open result note
            </button>
          ) : null}
        </div>
      ) : null}

      {workflow.status === "succeeded" && workflow.resultNoteId ? (
        <p className="reminder-workflow-generated-only finished">
          The final result was saved as a note linked to the parent note.
        </p>
      ) : null}
    </section>
  );
}

export function ReminderWorkflowAssignment({
  reminderId,
  onChanged,
}: {
  reminderId: string;
  onChanged: () => void | Promise<void>;
}) {
  const [expanded, setExpanded] = useState(false);
  const [steps, setSteps] = useState<ReminderWorkflowStepDraft[]>([]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [provider, setProvider] = useState<"default" | "local" | "remote">("default");

  async function save() {
    if (!steps.length) return;
    setSaving(true);
    setError(null);
    try {
      await invoke("set_reminder_workflow", {
        input: {
          reminderId,
          steps,
          selection: provider === "default" ? null : { provider, model: null },
        },
      });
      await onChanged();
      setExpanded(false);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setSaving(false);
    }
  }

  if (!expanded) {
    return (
      <button
        type="button"
        className="assign-reminder-workflow"
        onClick={() => setExpanded(true)}
      >
        <Sparkles size={14} />
        Assign tasks
      </button>
    );
  }

  return (
    <section className="reminder-workflow-assignment">
      <ReminderWorkflowBuilder steps={steps} onChange={setSteps} />
      <label className="settings-field reminder-workflow-provider">
        <span>LLM provider</span>
        <select
          value={provider}
          onChange={(event) =>
            setProvider(event.currentTarget.value as "default" | "local" | "remote")
          }
        >
          <option value="default">Use default</option>
          <option value="local">Local</option>
          <option value="remote">Remote</option>
        </select>
      </label>
      {error ? <p className="form-error">{error}</p> : null}
      <footer>
        <button type="button" onClick={() => setExpanded(false)}>Cancel</button>
        <button type="button" disabled={saving || !steps.length} onClick={() => void save()}>
          {saving ? <Loader2 size={14} className="spin" /> : <Check size={14} />}
          {saving ? "Assigning" : "Assign workflow"}
        </button>
      </footer>
    </section>
  );
}

function SlackWorkflowApproval({
  step,
  onApproved,
}: {
  step: ReminderWorkflowStep;
  onApproved: () => void | Promise<void>;
}) {
  const [destination, setDestination] = useState(
    () => step.destination || localStorage.getItem("smooth-slack-destination") || "",
  );
  const [text, setText] = useState(step.outputText || "");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => setText(step.outputText || ""), [step.outputText]);

  async function approve() {
    setSending(true);
    setError(null);
    try {
      await invoke("approve_reminder_workflow_step", {
        input: {
          stepId: step.id,
          destination: destination.trim(),
          subject: null,
          text: text.trim(),
        },
      });
      localStorage.setItem("smooth-slack-destination", destination.trim());
      await onApproved();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setSending(false);
    }
  }

  return (
    <div className="workflow-approval">
      <div className="workflow-approval-title">
        <MessageSquare size={16} />
        <div>
          <strong>Slack draft ready</strong>
          <span>Review and approve before anything is posted.</span>
        </div>
      </div>
      <label>
        <span>Destination</span>
        <input
          value={destination}
          onChange={(event) => setDestination(event.currentTarget.value)}
          placeholder="Channel ID or Slack message URL"
        />
      </label>
      <label>
        <span>Message</span>
        <textarea rows={6} value={text} onChange={(event) => setText(event.currentTarget.value)} />
      </label>
      {error ? <p className="form-error">{error}</p> : null}
      <footer>
        <button
          type="button"
          className="workflow-approve-button"
          disabled={sending || !destination.trim() || !text.trim()}
          onClick={() => void approve()}
        >
          {sending ? <Loader2 size={14} className="spin" /> : <Send size={14} />}
          {sending ? "Posting" : "Approve and post"}
        </button>
      </footer>
    </div>
  );
}

function GmailWorkflowApproval({
  step,
  onApproved,
}: {
  step: ReminderWorkflowStep;
  onApproved: () => void | Promise<void>;
}) {
  const [to, setTo] = useState(step.destination || "");
  const [subject, setSubject] = useState(step.subject || "");
  const [body, setBody] = useState(step.outputText || "");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setSubject(step.subject || "");
    setBody(step.outputText || "");
  }, [step.outputText, step.subject]);

  async function approve() {
    setCreating(true);
    setError(null);
    try {
      await invoke("approve_reminder_workflow_step", {
        input: {
          stepId: step.id,
          destination: to.trim(),
          subject: subject.trim(),
          text: body.trim(),
        },
      });
      await onApproved();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setCreating(false);
    }
  }

  return (
    <div className="workflow-approval">
      <div className="workflow-approval-title">
        <Mail size={16} />
        <div>
          <strong>Gmail draft ready</strong>
          <span>Review and approve before the draft is created in Gmail.</span>
        </div>
      </div>
      <label>
        <span>To <small>optional</small></span>
        <input
          value={to}
          onChange={(event) => setTo(event.currentTarget.value)}
          placeholder="name@example.com"
        />
      </label>
      <label>
        <span>Subject</span>
        <input
          value={subject}
          onChange={(event) => setSubject(event.currentTarget.value)}
          placeholder="Email subject"
        />
      </label>
      <label>
        <span>Body</span>
        <textarea rows={8} value={body} onChange={(event) => setBody(event.currentTarget.value)} />
      </label>
      {error ? <p className="form-error">{error}</p> : null}
      <footer>
        <button
          type="button"
          className="workflow-approve-button"
          disabled={creating || !subject.trim() || !body.trim()}
          onClick={() => void approve()}
        >
          {creating ? <Loader2 size={14} className="spin" /> : <Mail size={14} />}
          {creating ? "Creating draft" : "Approve and create draft"}
        </button>
      </footer>
    </div>
  );
}

function WorkflowStatus({ status }: { status: ReminderWorkflowRecord["status"] }) {
  const labels: Record<ReminderWorkflowRecord["status"], string> = {
    scheduled: "Scheduled",
    running: "Running",
    awaiting_approval: "Needs approval",
    succeeded: "Finished",
    failed: "Failed",
    cancelled: "Cancelled",
  };
  return <span className={`workflow-status ${status}`}>{labels[status]}</span>;
}

function StepStatus({ status }: { status: ReminderWorkflowStep["status"] }) {
  if (status === "running") return <Loader2 size={13} className="spin" />;
  if (status === "succeeded") return <Check size={13} />;
  if (status === "failed") return <AlertTriangle size={13} />;
  if (status === "awaiting_approval") return <Play size={13} />;
  return <span className="workflow-step-dot" />;
}
