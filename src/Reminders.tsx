import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import {
  Bell,
  Check,
  ChevronRight,
  Clock3,
  ExternalLink,
  RotateCcw,
  Trash2,
  X,
} from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ReminderWorkflowBuilder,
  ReminderWorkflowAssignment,
  ReminderWorkflowPanel,
  ReminderWorkflowToastStatus,
  type ReminderWorkflowRecord,
  type ReminderWorkflowStepDraft,
} from "./ReminderWorkflows";

export type ReminderRecord = {
  id: string;
  noteId: string;
  noteTitle: string;
  scheduledAt: number;
  comment: string | null;
  selectedText: string;
  startOffset: number;
  endOffset: number;
  contextBefore: string;
  contextAfter: string;
  status: "pending" | "completed" | "dismissed";
  lastNotifiedAt: number | null;
  createdAt: string;
  updatedAt: string;
};

export type ReminderSelection = {
  selectedText: string;
  startOffset: number;
  endOffset: number;
  contextBefore: string;
  contextAfter: string;
};

export type ReminderJumpTarget = ReminderRecord & { nonce: number };

const REMINDER_CHANGED_EVENT = "smooth-reminders-changed";

export function announceReminderChange() {
  window.dispatchEvent(new Event(REMINDER_CHANGED_EVENT));
}

function toLocalInputValue(timestamp: number) {
  const date = new Date(timestamp);
  const local = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
  return local.toISOString().slice(0, 16);
}

function formatTime(timestamp: number) {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(timestamp));
}

const RELATIVE_UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 24 * 60 * 60_000],
  ["month", 30 * 24 * 60 * 60_000],
  ["week", 7 * 24 * 60 * 60_000],
  ["day", 24 * 60 * 60_000],
  ["hour", 60 * 60_000],
  ["minute", 60_000],
];

const relativeFormatter = new Intl.RelativeTimeFormat(undefined, {
  numeric: "auto",
});

function formatRelative(timestamp: number, now: number) {
  const diff = timestamp - now;
  if (Math.abs(diff) < 60_000) {
    return diff >= 0 ? "in under a minute" : "just now";
  }
  for (const [unit, ms] of RELATIVE_UNITS) {
    if (Math.abs(diff) >= ms) {
      return relativeFormatter.format(Math.round(diff / ms), unit);
    }
  }
  return relativeFormatter.format(Math.round(diff / 60_000), "minute");
}

type ReminderState = "overdue" | "upcoming" | "completed" | "dismissed";

function reminderState(reminder: ReminderRecord, now: number): ReminderState {
  if (reminder.status === "completed") return "completed";
  if (reminder.status === "dismissed") return "dismissed";
  return reminder.scheduledAt <= now ? "overdue" : "upcoming";
}

function ReminderBadge({ state }: { state: ReminderState }) {
  if (state === "overdue") {
    return <span className="reminder-badge overdue">Overdue</span>;
  }
  if (state === "completed") {
    return (
      <span className="reminder-badge done">
        <Check size={11} /> Done
      </span>
    );
  }
  if (state === "dismissed") {
    return <span className="reminder-badge dismissed">Dismissed</span>;
  }
  return null;
}

function reminderBody(reminder: ReminderRecord) {
  return reminder.comment?.trim() || reminder.selectedText.trim();
}

async function ensureNotificationPermission() {
  if (await isPermissionGranted()) return true;
  return (await requestPermission()) === "granted";
}

async function completeReminder(id: string) {
  await invoke("complete_reminder", { id });
  announceReminderChange();
}

async function dismissReminder(id: string) {
  await invoke("dismiss_reminder", { id });
  announceReminderChange();
}

async function snoozeReminder(id: string, minutes: number) {
  await invoke("snooze_reminder", {
    input: { id, scheduledAt: Date.now() + minutes * 60_000 },
  });
  announceReminderChange();
}

export function ReminderCreateDialog({
  noteId,
  selection,
  onClose,
  onCreated,
}: {
  noteId: string;
  selection: ReminderSelection;
  onClose: () => void;
  onCreated: (reminder: ReminderRecord) => void;
}) {
  const [scheduledAt, setScheduledAt] = useState(() =>
    toLocalInputValue(Date.now() + 60 * 60_000),
  );
  const [comment, setComment] = useState("");
  const [workflowSteps, setWorkflowSteps] = useState<ReminderWorkflowStepDraft[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit() {
    const timestamp = new Date(scheduledAt).getTime();
    if (!Number.isFinite(timestamp)) {
      setError("Choose a valid reminder time");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const reminder = await invoke<ReminderRecord>("create_reminder", {
        input: {
          noteId,
          scheduledAt: timestamp,
          comment: comment.trim() || null,
          workflowSteps,
          ...selection,
        },
      });
      void ensureNotificationPermission();
      announceReminderChange();
      onCreated(reminder);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="reminder-dialog-backdrop" role="presentation" onMouseDown={onClose}>
      <section
        className="reminder-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="reminder-dialog-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header>
          <div>
            <span className="section-icon"><Bell size={17} /></span>
            <h2 id="reminder-dialog-title">Create reminder</h2>
          </div>
          <button className="icon-button" type="button" onClick={onClose} title="Close">
            <X size={17} />
          </button>
        </header>

        <blockquote>{selection.selectedText}</blockquote>

        <label>
          <span>When</span>
          <input
            type="datetime-local"
            value={scheduledAt}
            min={toLocalInputValue(Date.now())}
            onChange={(event) => setScheduledAt(event.currentTarget.value)}
            autoFocus
          />
        </label>
        <div className="reminder-presets" aria-label="Quick reminder times">
          <button type="button" onClick={() => setScheduledAt(toLocalInputValue(Date.now() + 15 * 60_000))}>15 min</button>
          <button type="button" onClick={() => setScheduledAt(toLocalInputValue(Date.now() + 60 * 60_000))}>1 hour</button>
          <button type="button" onClick={() => setScheduledAt(toLocalInputValue(Date.now() + 24 * 60 * 60_000))}>Tomorrow</button>
        </div>
        <label>
          <span>Comment <small>optional</small></span>
          <textarea
            rows={3}
            maxLength={2000}
            value={comment}
            onChange={(event) => setComment(event.currentTarget.value)}
            placeholder="Why should I come back to this?"
          />
        </label>
        <ReminderWorkflowBuilder steps={workflowSteps} onChange={setWorkflowSteps} />
        {error ? <p className="form-error">{error}</p> : null}
        <footer>
          <button className="secondary-action" type="button" onClick={onClose}>Cancel</button>
          <button className="primary-action" type="button" disabled={busy} onClick={() => void submit()}>
            <Bell size={16} />
            {busy ? "Creating" : "Create reminder"}
          </button>
        </footer>
      </section>
    </div>
  );
}

export function ReminderCenter({
  onOpenReminders,
}: {
  onOpenReminders: () => void;
}) {
  const [due, setDue] = useState<ReminderRecord[]>([]);
  const [workflows, setWorkflows] = useState<ReminderWorkflowRecord[]>([]);
  const onOpenRemindersRef = useRef(onOpenReminders);
  const dueRef = useRef(due);
  const awaitingNativeOpenRef = useRef<string | null>(null);
  onOpenRemindersRef.current = onOpenReminders;
  dueRef.current = due;

  const refresh = useCallback(async () => {
    try {
      const [nextDue, nextWorkflows] = await Promise.all([
        invoke<ReminderRecord[]>("list_due_reminders"),
        invoke<ReminderWorkflowRecord[]>("list_reminder_workflows"),
      ]);
      setDue(nextDue);
      setWorkflows(nextWorkflows);
    } catch {
      // The center retries on the next worker event or reminder mutation.
    }
  }, []);

  useEffect(() => {
    void refresh();
    const changed = () => void refresh();
    window.addEventListener(REMINDER_CHANGED_EVENT, changed);
    const unlistenPromise = listen<ReminderRecord>("reminder-due", async ({ payload }) => {
      setDue((current) => current.some(({ id }) => id === payload.id) ? current : [...current, payload]);
      void refresh();
      try {
        if (await ensureNotificationPermission()) {
          if (!document.hasFocus()) {
            awaitingNativeOpenRef.current = payload.id;
          }
          sendNotification({
            title: payload.noteTitle || "Smooth reminder",
            body: reminderBody(payload).slice(0, 240),
            group: "smooth-reminders",
            extra: { reminderId: payload.id },
            autoCancel: true,
          });
        }
      } catch {
        // The in-app reminder remains available when native notifications are unavailable.
      }
    });
    const focusPromise = getCurrentWindow().onFocusChanged(({ payload }) => {
      const reminderId = awaitingNativeOpenRef.current;
      if (!payload || !reminderId) return;
      const reminder = dueRef.current.find(({ id }) => id === reminderId);
      awaitingNativeOpenRef.current = null;
      if (reminder) onOpenRemindersRef.current();
    });
    const workflowUnlistenPromise = listen("reminder-workflow-changed", changed);
    return () => {
      window.removeEventListener(REMINDER_CHANGED_EVENT, changed);
      void unlistenPromise.then((unlisten) => unlisten());
      void focusPromise.then((unlisten) => unlisten());
      void workflowUnlistenPromise.then((unlisten) => unlisten());
    };
  }, [refresh]);

  if (!due.length) return null;
  const reminder = due[0];
  const workflow = workflows.find(({ reminderId }) => reminderId === reminder.id);

  return (
    <aside className="due-reminder" aria-live="assertive">
      <div className="due-reminder-icon"><Bell size={18} /></div>
      <button className="due-reminder-copy" type="button" onClick={onOpenReminders}>
        <strong>{reminder.noteTitle || "Untitled note"}</strong>
        <span>{reminderBody(reminder)}</span>
        {workflow ? <ReminderWorkflowToastStatus workflow={workflow} /> : null}
        {due.length > 1 ? <small>{due.length - 1} more overdue</small> : null}
      </button>
      <div className="due-reminder-actions">
        <button type="button" onClick={onOpenReminders} title="Open reminders">
          <ExternalLink size={16} />
        </button>
        <button type="button" onClick={() => void snoozeReminder(reminder.id, 10)} title="Snooze 10 minutes">
          <Clock3 size={16} />
        </button>
        <button type="button" onClick={() => void completeReminder(reminder.id)} title="Complete reminder">
          <Check size={16} />
        </button>
        <button type="button" onClick={() => void dismissReminder(reminder.id)} title="Dismiss reminder">
          <X size={16} />
        </button>
      </div>
    </aside>
  );
}

export function RemindersView({
  onClose,
  onOpen,
}: {
  onClose: () => void;
  onOpen: (reminder: ReminderRecord) => void;
}) {
  const [reminders, setReminders] = useState<ReminderRecord[]>([]);
  const [workflows, setWorkflows] = useState<ReminderWorkflowRecord[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  const load = useCallback(async () => {
    try {
      const [nextReminders, nextWorkflows] = await Promise.all([
        invoke<ReminderRecord[]>("list_reminders"),
        invoke<ReminderWorkflowRecord[]>("list_reminder_workflows"),
      ]);
      setReminders(nextReminders);
      setWorkflows(nextWorkflows);
      setError(null);
    } catch (reason) {
      setError(String(reason));
    }
  }, []);

  useEffect(() => {
    void load();
    const changed = () => void load();
    window.addEventListener(REMINDER_CHANGED_EVENT, changed);
    const unlistenPromise = listen("reminder-workflow-changed", changed);
    // Keep relative times fresh and let cards flip to "overdue" on their own.
    const tick = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => {
      window.removeEventListener(REMINDER_CHANGED_EVENT, changed);
      void unlistenPromise.then((unlisten) => unlisten());
      window.clearInterval(tick);
    };
  }, [load]);

  const pending = useMemo(() => reminders.filter(({ status }) => status === "pending"), [reminders]);
  const history = useMemo(() => reminders.filter(({ status }) => status !== "pending"), [reminders]);
  const overdueCount = useMemo(
    () => pending.filter(({ scheduledAt }) => scheduledAt <= now).length,
    [pending, now],
  );
  const workflowsByReminder = useMemo(
    () => new Map(workflows.map((workflow) => [workflow.reminderId, workflow])),
    [workflows],
  );

  const subtitle = pending.length
    ? `${pending.length} upcoming${overdueCount ? ` · ${overdueCount} overdue` : ""}`
    : reminders.length
      ? "All caught up"
      : "Nothing scheduled";

  async function remove(id: string) {
    await invoke("delete_reminder", { id });
    announceReminderChange();
  }

  async function clearReminders(ids: string[]) {
    if (!ids.length) return;
    await Promise.all(ids.map((id) => invoke("delete_reminder", { id })));
    announceReminderChange();
  }

  return (
    <section className="reminders-view">
      <header className="view-header">
        <div>
          <span className="section-icon"><Bell size={19} /></span>
          <div><h1>Reminders</h1><p>{subtitle}</p></div>
        </div>
        <button className="icon-button" type="button" onClick={onClose} title="Close reminders"><X size={18} /></button>
      </header>
      {error ? <p className="form-error">{error}</p> : null}
      {!reminders.length && !error ? (
        <div className="empty-state reminders-empty">
          <div className="empty-icon"><Bell size={26} /></div>
          <h2>No reminders yet</h2>
          <p>Select text in a note and use the bell button in the editor toolbar to set one.</p>
        </div>
      ) : null}
      {pending.length ? <ReminderList title="Upcoming" reminders={pending} workflows={workflowsByReminder} now={now} onOpen={onOpen} onDelete={remove} onWorkflowChanged={load} /> : null}
      {history.length ? <ReminderHistory reminders={history} workflows={workflowsByReminder} now={now} onOpen={onOpen} onDelete={remove} onClear={clearReminders} onWorkflowChanged={load} /> : null}
    </section>
  );
}

function ReminderCard({
  reminder,
  workflow,
  now,
  onOpen,
  onDelete,
  onWorkflowChanged,
}: {
  reminder: ReminderRecord;
  workflow: ReminderWorkflowRecord | undefined;
  now: number;
  onOpen: (reminder: ReminderRecord) => void;
  onDelete: (id: string) => Promise<void>;
  onWorkflowChanged: () => void | Promise<void>;
}) {
  const state = reminderState(reminder, now);
  const selected = reminder.selectedText.trim();
  const [restoreOpen, setRestoreOpen] = useState(false);
  const restoreRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!restoreOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      if (restoreRef.current && !restoreRef.current.contains(event.target as Node)) {
        setRestoreOpen(false);
      }
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [restoreOpen]);

  function restore(minutes: number) {
    setRestoreOpen(false);
    void snoozeReminder(reminder.id, minutes);
  }

  return (
    <article className={`reminder-card ${state}`}>
      <div className="reminder-card-body">
        <button className="reminder-main" type="button" onClick={() => onOpen(reminder)}>
          <span className="reminder-card-head">
            <span className="reminder-note-title">{reminder.noteTitle || "Untitled note"}</span>
            <ReminderBadge state={state} />
          </span>
          <span className="reminder-time">
            <Clock3 size={13} />
            <span className="reminder-time-relative">{formatRelative(reminder.scheduledAt, now)}</span>
            <span className="reminder-time-abs">{formatTime(reminder.scheduledAt)}</span>
          </span>
          {selected ? <q>{selected}</q> : null}
          {reminder.comment ? <p>{reminder.comment}</p> : null}
        </button>
        {workflow ? (
          <ReminderWorkflowPanel workflow={workflow} onChanged={onWorkflowChanged} />
        ) : reminder.status === "pending" ? (
          <ReminderWorkflowAssignment reminderId={reminder.id} onChanged={onWorkflowChanged} />
        ) : null}
      </div>
      <div className="reminder-row-actions">
        {reminder.status === "pending" ? (
          <>
            <button type="button" onClick={() => void snoozeReminder(reminder.id, 10)} title="Snooze 10 minutes"><RotateCcw size={15} /></button>
            <button type="button" onClick={() => void completeReminder(reminder.id)} title="Complete"><Check size={15} /></button>
            <button type="button" onClick={() => void onDelete(reminder.id)} title="Delete reminder"><Trash2 size={15} /></button>
          </>
        ) : restoreOpen ? (
          <div className="reminder-restore-inline" ref={restoreRef}>
            <span className="reminder-restore-label">Remind</span>
            <button type="button" onClick={() => restore(15)}>15m</button>
            <button type="button" onClick={() => restore(60)}>1h</button>
            <button type="button" onClick={() => restore(24 * 60)}>Tomorrow</button>
            <button type="button" className="reminder-restore-cancel" onClick={() => setRestoreOpen(false)} title="Cancel"><X size={14} /></button>
          </div>
        ) : (
          <>
            <button type="button" onClick={() => setRestoreOpen(true)} title="Restore to upcoming"><RotateCcw size={15} /></button>
            <button type="button" onClick={() => void onDelete(reminder.id)} title="Delete reminder"><Trash2 size={15} /></button>
          </>
        )}
      </div>
    </article>
  );
}

function ReminderList({
  title,
  reminders,
  workflows,
  now,
  onOpen,
  onDelete,
  onWorkflowChanged,
}: {
  title: string;
  reminders: ReminderRecord[];
  workflows: Map<string, ReminderWorkflowRecord>;
  now: number;
  onOpen: (reminder: ReminderRecord) => void;
  onDelete: (id: string) => Promise<void>;
  onWorkflowChanged: () => void | Promise<void>;
}) {
  return (
    <div className="reminder-list-section">
      <h2>{title}<span className="reminder-count">{reminders.length}</span></h2>
      <div className="reminder-list">
        {reminders.map((reminder) => (
          <ReminderCard
            key={reminder.id}
            reminder={reminder}
            workflow={workflows.get(reminder.id)}
            now={now}
            onOpen={onOpen}
            onDelete={onDelete}
            onWorkflowChanged={onWorkflowChanged}
          />
        ))}
      </div>
    </div>
  );
}

function startOfDay(timestamp: number) {
  const d = new Date(timestamp);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

function dayLabel(timestamp: number, now: number) {
  const diffDays = Math.round((startOfDay(now) - startOfDay(timestamp)) / 86_400_000);
  if (diffDays === 0) return "Today";
  if (diffDays === 1) return "Yesterday";
  const d = new Date(timestamp);
  return new Intl.DateTimeFormat(undefined, {
    month: "long",
    day: "numeric",
    year: d.getFullYear() === new Date(now).getFullYear() ? undefined : "numeric",
  }).format(d);
}

function historySummary(reminders: ReminderRecord[]) {
  const done = reminders.filter((r) => r.status === "completed").length;
  const dismissed = reminders.filter((r) => r.status === "dismissed").length;
  const parts: string[] = [];
  if (done) parts.push(`${done} done`);
  if (dismissed) parts.push(`${dismissed} dismissed`);
  return parts.join(" · ");
}

// History is grouped by calendar day, newest day first, and each day is a
// collapsible section so the whole archive isn't dumped on screen at once.
// The most recent day starts open; the user can toggle any day (overrides win).
function ReminderHistory({
  reminders,
  workflows,
  now,
  onOpen,
  onDelete,
  onClear,
  onWorkflowChanged,
}: {
  reminders: ReminderRecord[];
  workflows: Map<string, ReminderWorkflowRecord>;
  now: number;
  onOpen: (reminder: ReminderRecord) => void;
  onDelete: (id: string) => Promise<void>;
  onClear: (ids: string[]) => Promise<void>;
  onWorkflowChanged: () => void | Promise<void>;
}) {
  const [overrides, setOverrides] = useState<Record<string, boolean>>({});
  const [filter, setFilter] = useState<"all" | "completed" | "dismissed">("all");
  const [confirmingClear, setConfirmingClear] = useState(false);

  const doneCount = useMemo(
    () => reminders.filter((r) => r.status === "completed").length,
    [reminders],
  );
  const dismissedCount = useMemo(
    () => reminders.filter((r) => r.status === "dismissed").length,
    [reminders],
  );
  // Only worth showing the filter when history actually holds both kinds.
  const showFilter = doneCount > 0 && dismissedCount > 0;

  const groups = useMemo(() => {
    const source =
      filter === "all"
        ? reminders
        : reminders.filter((r) => r.status === filter);
    const sorted = [...source].sort((a, b) => b.scheduledAt - a.scheduledAt);
    const byDay = new Map<number, ReminderRecord[]>();
    for (const reminder of sorted) {
      const key = startOfDay(reminder.scheduledAt);
      const bucket = byDay.get(key);
      if (bucket) {
        bucket.push(reminder);
      } else {
        byDay.set(key, [reminder]);
      }
    }
    return [...byDay.entries()]
      .sort((a, b) => b[0] - a[0])
      .map(([key, items]) => ({ key, items, label: dayLabel(key, now) }));
  }, [reminders, now, filter]);

  // Ids currently shown (respects the active filter) — that's what Clear removes.
  const visibleIds = groups.flatMap((group) => group.items.map((r) => r.id));

  return (
    <div className="reminder-list-section">
      <div className="reminder-history-head">
        <h2>History<span className="reminder-count">{reminders.length}</span></h2>
        <div className="reminder-history-actions">
          {showFilter ? (
            <div className="reminder-filter" role="tablist" aria-label="Filter history">
              <button
                type="button"
                role="tab"
                aria-selected={filter === "all"}
                className={filter === "all" ? "active" : ""}
                onClick={() => setFilter("all")}
              >
                All
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={filter === "completed"}
                className={filter === "completed" ? "active" : ""}
                onClick={() => setFilter("completed")}
              >
                Done {doneCount}
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={filter === "dismissed"}
                className={filter === "dismissed" ? "active" : ""}
                onClick={() => setFilter("dismissed")}
              >
                Dismissed {dismissedCount}
              </button>
            </div>
          ) : null}
          {confirmingClear ? (
            <span className="reminder-clear-confirm">
              <span>
                {filter === "all"
                  ? `Clear all ${visibleIds.length}?`
                  : `Clear ${visibleIds.length} ${
                      filter === "completed" ? "done" : "dismissed"
                    }?`}
              </span>
              <button
                type="button"
                className="reminder-clear-cancel"
                onClick={() => setConfirmingClear(false)}
              >
                Cancel
              </button>
              <button
                type="button"
                className="reminder-clear-go"
                onClick={() => {
                  setConfirmingClear(false);
                  void onClear(visibleIds);
                }}
              >
                Clear
              </button>
            </span>
          ) : (
            <button
              type="button"
              className="reminder-clear-btn"
              title="Delete the reminders shown below"
              onClick={() => setConfirmingClear(true)}
            >
              <Trash2 size={13} /> Clear
            </button>
          )}
        </div>
      </div>
      <div className="reminder-history-groups">
        {groups.map((group, index) => {
          // A specific filter expands every matching day so the results are
          // all visible at once; "All" keeps the newest-day-open default.
          const open =
            filter !== "all" ? true : overrides[group.key] ?? index === 0;
          const summary = historySummary(group.items);
          return (
            <div className={open ? "reminder-day open" : "reminder-day"} key={group.key}>
              <button
                type="button"
                className="reminder-day-header"
                aria-expanded={open}
                onClick={() =>
                  setOverrides((current) => ({
                    ...current,
                    [group.key]: !(current[group.key] ?? index === 0),
                  }))
                }
              >
                <ChevronRight size={14} className="reminder-day-chevron" />
                <span className="reminder-day-label">{group.label}</span>
                <span className="reminder-count">{group.items.length}</span>
                {summary ? <span className="reminder-day-summary">{summary}</span> : null}
              </button>
              <div className="reminder-day-body">
                <div className="reminder-list">
                  {group.items.map((reminder) => (
                    <ReminderCard
                      key={reminder.id}
                      reminder={reminder}
                      workflow={workflows.get(reminder.id)}
                      now={now}
                      onOpen={onOpen}
                      onDelete={onDelete}
                      onWorkflowChanged={onWorkflowChanged}
                    />
                  ))}
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
