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
  Clock3,
  ExternalLink,
  RotateCcw,
  Trash2,
  X,
} from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

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
  onOpen,
}: {
  onOpen: (reminder: ReminderRecord) => void;
}) {
  const [due, setDue] = useState<ReminderRecord[]>([]);
  const onOpenRef = useRef(onOpen);
  const dueRef = useRef(due);
  const awaitingNativeOpenRef = useRef<string | null>(null);
  onOpenRef.current = onOpen;
  dueRef.current = due;

  const refresh = useCallback(async () => {
    try {
      setDue(await invoke<ReminderRecord[]>("list_due_reminders"));
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
      if (reminder) onOpenRef.current(reminder);
    });
    return () => {
      window.removeEventListener(REMINDER_CHANGED_EVENT, changed);
      void unlistenPromise.then((unlisten) => unlisten());
      void focusPromise.then((unlisten) => unlisten());
    };
  }, [refresh]);

  if (!due.length) return null;
  const reminder = due[0];

  return (
    <aside className="due-reminder" aria-live="assertive">
      <div className="due-reminder-icon"><Bell size={18} /></div>
      <div className="due-reminder-copy">
        <strong>{reminder.noteTitle || "Untitled note"}</strong>
        <span>{reminderBody(reminder)}</span>
        {due.length > 1 ? <small>{due.length - 1} more overdue</small> : null}
      </div>
      <div className="due-reminder-actions">
        <button type="button" onClick={() => onOpen(reminder)} title="Open reminder">
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
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setReminders(await invoke<ReminderRecord[]>("list_reminders"));
      setError(null);
    } catch (reason) {
      setError(String(reason));
    }
  }, []);

  useEffect(() => {
    void load();
    const changed = () => void load();
    window.addEventListener(REMINDER_CHANGED_EVENT, changed);
    return () => window.removeEventListener(REMINDER_CHANGED_EVENT, changed);
  }, [load]);

  const pending = useMemo(() => reminders.filter(({ status }) => status === "pending"), [reminders]);
  const history = useMemo(() => reminders.filter(({ status }) => status !== "pending"), [reminders]);

  async function remove(id: string) {
    await invoke("delete_reminder", { id });
    announceReminderChange();
  }

  return (
    <section className="reminders-view">
      <header className="view-header">
        <div>
          <span className="section-icon"><Bell size={19} /></span>
          <div><h1>Reminders</h1><p>{pending.length} upcoming or overdue</p></div>
        </div>
        <button className="icon-button" type="button" onClick={onClose} title="Close reminders"><X size={18} /></button>
      </header>
      {error ? <p className="form-error">{error}</p> : null}
      {!reminders.length && !error ? (
        <div className="empty-state reminders-empty">
          <div className="empty-icon"><Bell size={26} /></div>
          <h2>No reminders</h2>
          <p>Select text in a note and use the bell button in the editor toolbar.</p>
        </div>
      ) : null}
      {pending.length ? <ReminderList title="Upcoming" reminders={pending} now={Date.now()} onOpen={onOpen} onDelete={remove} /> : null}
      {history.length ? <ReminderList title="History" reminders={history} now={Date.now()} onOpen={onOpen} onDelete={remove} /> : null}
    </section>
  );
}

function ReminderList({
  title,
  reminders,
  now,
  onOpen,
  onDelete,
}: {
  title: string;
  reminders: ReminderRecord[];
  now: number;
  onOpen: (reminder: ReminderRecord) => void;
  onDelete: (id: string) => Promise<void>;
}) {
  return (
    <div className="reminder-list-section">
      <h2>{title}</h2>
      <div className="reminder-list">
        {reminders.map((reminder) => (
          <article className={reminder.status === "pending" && reminder.scheduledAt <= now ? "overdue" : ""} key={reminder.id}>
            <button className="reminder-main" type="button" onClick={() => onOpen(reminder)}>
              <span className="reminder-note-title">{reminder.noteTitle || "Untitled note"}</span>
              <span className="reminder-time"><Clock3 size={14} />{formatTime(reminder.scheduledAt)}</span>
              <q>{reminder.selectedText}</q>
              {reminder.comment ? <p>{reminder.comment}</p> : null}
            </button>
            <div className="reminder-row-actions">
              {reminder.status === "pending" ? (
                <>
                  <button type="button" onClick={() => void snoozeReminder(reminder.id, 10)} title="Snooze 10 minutes"><RotateCcw size={15} /></button>
                  <button type="button" onClick={() => void completeReminder(reminder.id)} title="Complete"><Check size={15} /></button>
                </>
              ) : null}
              <button type="button" onClick={() => void onDelete(reminder.id)} title="Delete reminder"><Trash2 size={15} /></button>
            </div>
          </article>
        ))}
      </div>
    </div>
  );
}
