import { invoke } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  Check,
  ChevronRight,
  FileText,
  Link2,
  Loader2,
  MessageSquare,
  Play,
  RotateCcw,
  Send,
  Sparkles,
  Trash2,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import "./ReminderWorkflows.css";

export type ReminderWorkflowStepDraft = {
  agentId: string;
};

export type ReminderWorkflowStep = {
  id: string;
  position: number;
  agentId: string;
  agentName: string;
  stepKind: "transform" | "external_slack";
  status:
    | "pending"
    | "running"
    | "awaiting_approval"
    | "succeeded"
    | "failed"
    | "cancelled";
  outputText: string | null;
  destination: string | null;
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
  steps: ReminderWorkflowStep[];
};

type ReminderAgentOption = {
  id: string;
  name: string;
  description: string;
  external: boolean;
  icon: typeof Sparkles;
};

const REMINDER_AGENT_OPTIONS: ReminderAgentOption[] = [
  {
    id: "summarize-note",
    name: "Summarize",
    description: "Distill the selected passage into key points and actions.",
    external: false,
    icon: FileText,
  },
  {
    id: "suggest-links",
    name: "Suggest links",
    description: "Find related notes using the reminder as context.",
    external: false,
    icon: Link2,
  },
  {
    id: "share-note-slack",
    name: "Prepare Slack message",
    description: "Create an editable draft and wait for approval before posting.",
    external: true,
    icon: MessageSquare,
  },
];

const OPTION_BY_ID = new Map(REMINDER_AGENT_OPTIONS.map((agent) => [agent.id, agent]));

export function ReminderWorkflowBuilder({
  steps,
  onChange,
}: {
  steps: ReminderWorkflowStepDraft[];
  onChange: (steps: ReminderWorkflowStepDraft[]) => void;
}) {
  const hasExternal = steps.some(({ agentId }) => OPTION_BY_ID.get(agentId)?.external);
  const selectedAgentIds = new Set(steps.map(({ agentId }) => agentId));
  const available = REMINDER_AGENT_OPTIONS.filter(
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
          <span>Agent workflow <small>optional</small></span>
          <p>Runs in order when the reminder is due.</p>
        </div>
        {steps.length ? <span className="reminder-workflow-count">{steps.length}</span> : null}
      </div>

      {steps.length ? (
        <ol className="reminder-workflow-draft">
          {steps.map((step, index) => {
            const agent = OPTION_BY_ID.get(step.agentId);
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
            aria-label="Agent to add"
          >
            <option value="">Add an agent...</option>
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
          This workflow only generates a result. Add Prepare Slack message to review and post it.
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
    scheduled: "Agent workflow queued",
    running: activeStep ? `${activeStep.agentName} is working` : "Agents are working",
    awaiting_approval: "Draft ready - approval required",
    succeeded: hasExternal ? "External action completed" : "Agent result ready",
    failed: "Agent workflow failed - open to retry",
    cancelled: "Agent workflow cancelled",
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
}: {
  workflow: ReminderWorkflowRecord;
  onChanged: () => void | Promise<void>;
}) {
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

  return (
    <section className={`reminder-workflow-panel ${workflow.status}`}>
      <header>
        <div>
          <Sparkles size={14} />
          <strong>Agent workflow</strong>
        </div>
        <WorkflowStatus status={workflow.status} />
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

      {approvalStep?.stepKind === "external_slack" ? (
        <SlackWorkflowApproval
          key={approvalStep.id}
          step={approvalStep}
          onApproved={onChanged}
        />
      ) : null}

      {workflow.status === "succeeded" && finalOutput ? (
        <div className="reminder-workflow-output">
          <span>Result</span>
          <p>{finalOutput}</p>
        </div>
      ) : null}

      {workflow.status === "succeeded" && !workflow.steps.some(({ stepKind }) => stepKind.startsWith("external_")) ? (
        <p className="reminder-workflow-generated-only finished">
          No external action was assigned. This workflow only generated the result above.
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

  async function save() {
    if (!steps.length) return;
    setSaving(true);
    setError(null);
    try {
      await invoke("set_reminder_workflow", {
        input: { reminderId, steps },
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
        Assign agents
      </button>
    );
  }

  return (
    <section className="reminder-workflow-assignment">
      <ReminderWorkflowBuilder steps={steps} onChange={setSteps} />
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
