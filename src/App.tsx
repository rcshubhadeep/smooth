import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { TokenResponse } from "@choochmeque/tauri-plugin-google-auth-api";
import Image from "@tiptap/extension-image";
import Placeholder from "@tiptap/extension-placeholder";
import { EditorContent, useEditor } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import {
  ArchiveRestore,
  ArrowLeft,
  ArrowDownUp,
  Bell,
  Bold,
  BookOpen,
  CalendarDays,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  CircleAlert,
  Database,
  Download,
  FileText,
  Folder,
  FolderInput,
  FolderPlus,
  FolderOpen,
  Heading2,
  Inbox,
  Info,
  Italic,
  Link2,
  List,
  Mail,
  Mic,
  Monitor,
  Moon,
  NotebookPen,
  PanelRight,
  Pause,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Search,
  Settings,
  Sparkles,
  Square,
  Strikethrough,
  Sun,
  Trash2,
  Unlink,
  Wrench,
  X,
} from "lucide-react";
import { Bot } from "lucide-react";
import { AgentsView, NoteAgentsPanel, type AgentRunResult } from "./Agents";
import { marked } from "marked";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type DragEvent,
} from "react";
import { flushSync } from "react-dom";
import TurndownService from "turndown";
import NoteChat from "./Chat";
import CommandPalette from "./CommandPalette";
import ImportDocuments from "./ImportDocuments";
import LlamaSettings from "./LlamaSettings";
import McpSettings from "./McpSettings";
import SlackSettings from "./SlackSettings";
import {
  announceReminderChange,
  ReminderCenter,
  ReminderCreateDialog,
  RemindersView,
  type ReminderJumpTarget,
  type ReminderRecord,
  type ReminderSelection,
} from "./Reminders";
import { startSemanticIndexer } from "./semantic";
import "./App.css";

type ThemeMode = "light" | "dark" | "system";
type ViewMode = "notes" | "settings" | "agents" | "reminders";
type SaveState = "idle" | "saving" | "saved" | "error";
type SortMode = "updated-desc" | "updated-asc" | "created-desc" | "created-asc";
type DictationState = "idle" | "recording" | "transcribing";
type MeetingState = "idle" | "starting" | "recording" | "paused" | "stopping";

type Folder = {
  id: string;
  name: string;
  created_at: string;
  system_key: string | null;
};

type NoteListItem = {
  id: string;
  title: string;
  folder_id: string | null;
  created_at: string;
  updated_at: string;
  deleted_at: string | null;
  excerpt: string;
  extraction_status: string;
};

type NoteWithContent = {
  id: string;
  title: string;
  folder_id: string | null;
  created_at: string;
  updated_at: string;
  deleted_at: string | null;
  content: string;
  extraction_status: string;
};

type NoteLink = {
  source_id: string;
  target_id: string;
  created_at: string;
  label: string | null;
  link_kind: "manual" | "entity_sharing" | string;
};

type BankSnapshot = {
  notes: NoteListItem[];
  folders: Folder[];
  links: NoteLink[];
};

type ExportResult = {
  path: string;
  count: number;
};

type GmailConfig = {
  client_id: string;
  client_secret: string;
  has_access_token: boolean;
  has_refresh_token: boolean;
  access_token_expires_at: number | null;
};

type GmailTokenPayload = {
  access_token: string;
  refresh_token: string | null;
  expires_at: number | null;
};

type GmailDraftInput = {
  to: string | null;
  subject: string;
  body: string;
};

type GmailDraftResult = {
  id: string;
  message_id: string | null;
};

type CalendarConfig = {
  client_id: string;
  client_secret: string;
  has_access_token: boolean;
  has_refresh_token: boolean;
  access_token_expires_at: number | null;
};

type CalendarTokenPayload = {
  access_token: string;
  refresh_token: string | null;
  expires_at: number | null;
};

type CalendarEvent = {
  id: string;
  calendar_id: string;
  calendar_name: string;
  title: string;
  starts_at: string;
  ends_at: string | null;
  is_all_day: boolean;
  location: string | null;
  html_link: string | null;
  video_link: string | null;
  attendee_count: number;
};

type ExtractionQueueStatus = {
  pending: number;
  processing: number;
  failed: number;
  indexed: number;
  not_indexed: number;
};

type AgentToolDescriptor = {
  name: string;
  description: string;
  input_schema: unknown;
};

// AgentRunResult / AgentRunStep are defined in ./Agents and imported above so
// the Settings dev tester and the Agents view share one shape.

type AudioCapturePreview = {
  path: string;
  duration_ms: number;
  sample_rate: number;
  channels: number;
  samples: number;
};

type AudioCaptureStatus = {
  is_recording: boolean;
  device_name: string | null;
  sample_rate: number | null;
  channels: number | null;
  captured_samples: number;
  dropped_samples: number;
  elapsed_ms: number | null;
  started_at_ms: number | null;
  last_preview: AudioCapturePreview | null;
};

type SystemAudioPermissionStatus = {
  granted: boolean;
  message: string;
  displays: number;
  error: string | null;
};

type SystemAudioCapturePreview = {
  path: string;
  duration_ms: number;
  sample_rate: number;
  channels: number;
  samples: number;
};

type SystemAudioCaptureStatus = {
  is_recording: boolean;
  output_path: string | null;
  elapsed_ms: number | null;
  started_at_ms: number | null;
  last_preview: SystemAudioCapturePreview | null;
  last_error: string | null;
};

type MeetingVisualSource = {
  id: string;
  kind: "display" | "window" | string;
  name: string;
  display_id: number | null;
  window_id: number | null;
  app_name: string | null;
  width: number | null;
  height: number | null;
};

type MeetingSnapshot = {
  path: string;
  source_id: string;
  width: number | null;
  height: number | null;
  captured_at_ms: number;
};

type DiarizationTurn = {
  speaker_id: string;
  start_ms: number;
  end_ms: number;
};

type DiarizationResult = {
  turns: DiarizationTurn[];
  engine: string;
  session_id: string | null;
  chunk_start_ms: number;
  chunk_end_ms: number;
};

type DiarizationSessionStarted = {
  session_id: string;
};

type DiarizationSpeakerPrompt = {
  speakerId: string;
  fallbackName: string;
};

type MeetingNoteCompletionStatus = {
  status: "queued" | "not_needed" | string;
  user_note_id: string;
  empty_headings: number;
};

type MeetingNoteCompletedEvent = {
  note_id: string;
  status: string;
  error: string | null;
};

type SttConfig = {
  model_path: string;
  language: string | null;
  threads: number;
};

type SttStatus = {
  state: "not_configured" | "ready" | "error";
  message: string;
  model_path: string;
  language: string | null;
  threads: number;
  acceleration: string[];
};

type SttQueueStatus = {
  pending_mic: number;
  pending_system: number;
  processing: number;
  failed: number;
  oldest_pending_ms: number;
  recent_average_real_time_factor: number | null;
  last_real_time_factor: number | null;
  last_inference_ms: number | null;
  last_model_load_ms: number | null;
};

type SttSegment = {
  text: string;
  start_ms: number;
  end_ms: number;
};

type SttTranscription = {
  text: string;
  segments: SttSegment[];
  raw_segment_count: number;
  language_id: number;
  language: string | null;
  audio: {
    source_sample_rate: number;
    source_channels: number;
    sample_rate: number;
    channels: number;
    duration_ms: number;
    samples: number;
    rms_db: number | null;
    peak_db: number | null;
  };
  elapsed_ms: number;
  preprocessing_ms: number;
  model_load_ms: number;
  inference_ms: number;
  real_time_factor: number;
  model_reloaded: boolean;
  model_path: string;
};

type SttJob = {
  id: number;
  note_id: string;
  source: "mic" | "system" | string;
  chunk_path: string;
  sequence: number;
  chunk_started_at_ms: number | null;
  duration_ms: number | null;
  status: string;
  attempts: number;
  last_error: string | null;
};

type SttJobEvent = {
  job_id: number;
  jobId?: number;
  note_id: string;
  noteId?: string;
  source: "mic" | "system" | string;
  path: string;
  sequence: number;
  chunk_started_at_ms: number | null;
  chunkStartedAtMs?: number | null;
  duration_ms: number | null;
  durationMs?: number | null;
  transcription: SttTranscription | null;
  error: string | null;
};

type NoteEntity = {
  id: number;
  name: string;
  entity_type: string;
  mention_count: number;
};

type NoteEntityMention = {
  id: number;
  entity_id: number;
  surface_text: string;
  context: string | null;
  start_offset: number | null;
  end_offset: number | null;
  match_status: string;
};

type NoteExtractionView = {
  status: string;
  error: string | null;
  entities: NoteEntity[];
  mentions: NoteEntityMention[];
};

type EntityInterestDefinition = {
  id: number | null;
  name: string;
  description: string;
  enabled: boolean;
  sort_order: number;
};

type LinkSuggestion = {
  note: NoteListItem;
  shared_entities: NoteEntity[];
  shared_entity_count: number;
  shared_mention_count: number;
};

type LinkedNote = {
  note: NoteListItem;
  link: NoteLink;
};

type DropTarget = {
  folderId: string | null;
  index: number;
};

const emptySnapshot: BankSnapshot = {
  notes: [],
  folders: [],
  links: [],
};

const ENTITY_PREVIEW_LIMIT = 12;
const DICTATION_CHUNK_MS = 5_000;
const MEETING_CHUNK_MS = 10_000;
const MEETING_SNAPSHOT_MS = 30_000;
const CALENDAR_CUE_WINDOW_MS = 10 * 60 * 1000;
const GMAIL_DRAFT_SCOPE = "https://www.googleapis.com/auth/gmail.drafts.create";
const CALENDAR_READONLY_SCOPE =
  "https://www.googleapis.com/auth/calendar.readonly";

function markdownToHtml(markdown: string) {
  return marked.parse(markdown || "", { async: false }) as string;
}

function markdownToExcerpt(markdown: string) {
  return markdown
    .replace(/[#>*_`~\-[\]()]/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 180);
}

function formatTime(value: string) {
  const date = new Date(Number(value));
  if (Number.isNaN(date.getTime())) {
    return "";
  }

  return date.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

function noteWordCount(content: string) {
  const text = content.replace(/[#>*_`~\-[\]()]/g, " ").trim();
  return text ? text.split(/\s+/).length : 0;
}

function dateValue(value: string) {
  const parsed = Number(value);
  return Number.isNaN(parsed) ? 0 : parsed;
}

function formatDuration(ms: number | null | undefined) {
  if (ms === null || ms === undefined) {
    return "0:00";
  }

  const totalSeconds = Math.max(0, Math.round(ms / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}:${seconds.toString().padStart(2, "0")}`;
}

function formatCalendarEventTime(event: CalendarEvent) {
  const start = new Date(event.starts_at);
  if (Number.isNaN(start.getTime())) {
    return event.is_all_day ? "All day" : "";
  }

  if (event.is_all_day) {
    return start.toLocaleDateString(undefined, {
      month: "short",
      day: "numeric",
    });
  }

  const end = event.ends_at ? new Date(event.ends_at) : null;
  const startsToday = start.toDateString() === new Date().toDateString();
  const datePart = startsToday
    ? "Today"
    : start.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  const timePart = start.toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
  const endPart =
    end && !Number.isNaN(end.getTime())
      ? `-${end.toLocaleTimeString(undefined, {
          hour: "numeric",
          minute: "2-digit",
        })}`
      : "";
  return `${datePart} ${timePart}${endPart}`;
}

function isCalendarEventActionable(event: CalendarEvent, nowMs: number) {
  if (event.is_all_day) {
    return false;
  }
  const startMs = new Date(event.starts_at).getTime();
  const endMs = event.ends_at ? new Date(event.ends_at).getTime() : startMs;
  if (!Number.isFinite(startMs)) {
    return false;
  }
  return startMs <= nowMs + CALENDAR_CUE_WINDOW_MS && endMs >= nowMs;
}

function formatDb(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return "silent";
  }
  return `${value.toFixed(1)} dB`;
}

function defaultAgentToolInput(toolName: string) {
  switch (toolName) {
    case "read_note":
      return JSON.stringify({ note_id: "" }, null, 2);
    case "create_note":
      return JSON.stringify({ title: "Agent test note", folder_id: null }, null, 2);
    case "write_note":
      return JSON.stringify({ note_id: "", content: "" }, null, 2);
    case "search_notes":
      return JSON.stringify({ query: "", limit: 10 }, null, 2);
    case "get_link_suggestions":
      return JSON.stringify({ note_id: "", limit: 10 }, null, 2);
    case "ping":
      return JSON.stringify({ message: "hello" }, null, 2);
    default:
      return "{}";
  }
}

function meetingTitleNow() {
  const date = new Date();
  const parts = new Intl.DateTimeFormat(undefined, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  }).formatToParts(date);
  const value = (type: string) =>
    parts.find((part) => part.type === type)?.value ?? "00";
  return `Meeting ${value("year")}-${value("month")}-${value("day")} ${value("hour")}:${value("minute")}`;
}

function meetingInitialContent(title: string) {
  return `# ${title}\n\nStarted: ${new Date().toLocaleString()}\n\n## Transcript\n`;
}

function meetingTranscriptLine(label: string, text: string) {
  const speaker = label.trim() || "Speaker";
  return `- ${new Date().toLocaleTimeString()} · **${speaker}:** ${text.trim()}`;
}

// Matches the structural speaker slot of a transcript line:
// `- <time> · **<speaker>:** <text>` → [full prefix, "- <time> · **", speaker, ":** "]
const TRANSCRIPT_SPEAKER = /^(- .+? · \*\*)(.+?)(:\*\* )/;
const TRANSCRIPT_LINE = /^(- .+? · \*\*)(.+?)(:\*\* )(.*)$/;

function appendMeetingTranscript(
  content: string,
  label: string,
  text: string,
): string {
  const speaker = label.trim() || "Speaker";
  const cleanText = text.trim();
  if (!cleanText) {
    return content;
  }

  const trimmed = content.trimEnd();
  const lines = trimmed.split("\n");
  const lastIndex = lines.length - 1;
  const lastLine = lines[lastIndex] ?? "";
  const match = lastLine.match(TRANSCRIPT_LINE);
  if (match && match[2] === speaker) {
    const currentText = match[4].trimEnd();
    lines[lastIndex] = `${match[1]}${match[2]}${match[3]}${currentText} ${cleanText}`;
    return `${lines.join("\n")}\n`;
  }

  return `${trimmed}\n${meetingTranscriptLine(speaker, cleanText)}\n`;
}

function appendMeetingSnapshot(content: string, snapshotLine: string) {
  const trimmed = content.trimEnd();
  return `${trimmed}\n${snapshotLine}\n`;
}

function diarizedSpeakerForSegment(
  segment: SttSegment,
  turns: DiarizationTurn[],
): string | null {
  if (turns.length === 0) {
    return null;
  }
  if (turns.length === 1) {
    return turns[0].speaker_id;
  }

  let bestTurn: DiarizationTurn | null = null;
  let bestOverlap = 0;
  for (const turn of turns) {
    const overlap =
      Math.min(segment.end_ms, turn.end_ms) -
      Math.max(segment.start_ms, turn.start_ms);
    if (overlap > bestOverlap) {
      bestOverlap = overlap;
      bestTurn = turn;
    }
  }
  if (bestTurn && bestOverlap > 0) {
    return bestTurn.speaker_id;
  }

  const midpoint = (segment.start_ms + segment.end_ms) / 2;
  let nearestTurn = turns[0];
  let nearestDistance = Number.POSITIVE_INFINITY;
  for (const turn of turns) {
    const distance =
      midpoint < turn.start_ms
        ? turn.start_ms - midpoint
        : midpoint > turn.end_ms
          ? midpoint - turn.end_ms
          : 0;
    if (distance < nearestDistance) {
      nearestDistance = distance;
      nearestTurn = turn;
    }
  }
  return nearestTurn.speaker_id;
}

/** Distinct speaker names, in first-seen order, found in transcript lines. */
function parseSpeakers(content: string): string[] {
  const seen: string[] = [];
  for (const line of content.split("\n")) {
    const match = line.match(TRANSCRIPT_SPEAKER);
    if (match && !seen.includes(match[2])) {
      seen.push(match[2]);
    }
  }
  return seen;
}

/** Rewrite ONLY the speaker slot of lines whose speaker == oldName. */
function renameSpeakerInContent(
  content: string,
  oldName: string,
  newName: string,
): string {
  return content
    .split("\n")
    .map((line) => {
      const match = line.match(TRANSCRIPT_SPEAKER);
      if (match && match[2] === oldName) {
        return match[1] + newName + match[3] + line.slice(match[0].length);
      }
      return line;
    })
    .join("\n");
}

function meetingSnapshotLine(snapshot: MeetingSnapshot) {
  const label = new Date(Number(snapshot.captured_at_ms)).toLocaleTimeString();
  return `![Meeting snapshot ${label}](${convertFileSrc(snapshot.path)})`;
}

function wait(ms: number) {
  return new Promise<void>((resolve) => {
    window.setTimeout(resolve, ms);
  });
}

function sortNotes(notes: NoteListItem[], mode: SortMode) {
  return [...notes].sort((first, second) => {
    const [field, direction] = mode.split("-") as [
      "updated" | "created",
      "asc" | "desc",
    ];
    const firstValue = dateValue(
      field === "updated" ? first.updated_at : first.created_at,
    );
    const secondValue = dateValue(
      field === "updated" ? second.updated_at : second.created_at,
    );
    const result = firstValue - secondValue;
    return direction === "asc" ? result : -result;
  });
}

type ToastKind = "error" | "success" | "info";
type ToastItem = { id: number; kind: ToastKind; message: string };

const toastStore = (() => {
  let items: ToastItem[] = [];
  let nextId = 1;
  const listeners = new Set<() => void>();
  const emit = () => listeners.forEach((listener) => listener());

  function dismiss(id: number) {
    items = items.filter((item) => item.id !== id);
    emit();
  }

  function push(kind: ToastKind, message: string) {
    const id = nextId++;
    items = [...items, { id, kind, message }];
    emit();
    window.setTimeout(() => dismiss(id), 4500);
    return id;
  }

  return {
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    snapshot: () => items,
    push,
    dismiss,
  };
})();

function cleanErrorMessage(value: unknown) {
  return String(value)
    .replace(/^Error:\s*/i, "")
    .trim();
}

const toast = {
  error: (message: unknown) =>
    toastStore.push("error", cleanErrorMessage(message)),
  success: (message: string) => toastStore.push("success", message),
  info: (message: string) => toastStore.push("info", message),
};

function ToastViewport() {
  const items = useSyncExternalStore(toastStore.subscribe, toastStore.snapshot);

  if (items.length === 0) {
    return null;
  }

  return (
    <div className="toast-viewport">
      {items.map((item) => {
        const Icon = item.kind === "error" ? CircleAlert : CheckCircle2;
        return (
          <div className={`toast ${item.kind}`} key={item.id} role="status">
            <Icon size={17} />
            <span>{item.message}</span>
            <button
              type="button"
              onClick={() => toastStore.dismiss(item.id)}
              aria-label="Dismiss"
            >
              <X size={14} />
            </button>
          </div>
        );
      })}
    </div>
  );
}

function NoteInfoPopover({ note }: { note: NoteWithContent | null }) {
  if (!note) {
    return null;
  }

  return (
    <span className="note-info-popover">
      <button className="note-info-button" type="button" aria-label="Note info">
        <Info size={13} />
      </button>
      <span className="note-info-card" role="tooltip">
        <span>
          <b>Created</b>
          <small>{formatTime(note.created_at)}</small>
        </span>
        <span>
          <b>Updated</b>
          <small>{formatTime(note.updated_at)}</small>
        </span>
        <span>
          <b>Words</b>
          <small>{noteWordCount(note.content).toLocaleString()}</small>
        </span>
      </span>
    </span>
  );
}

function ResizeHandle({
  side,
  ariaLabel,
  style,
  onResize,
}: {
  side: "left" | "right";
  ariaLabel: string;
  style: React.CSSProperties;
  onResize: (clientX: number) => void;
}) {
  const [active, setActive] = useState(false);
  const onResizeRef = useRef(onResize);
  onResizeRef.current = onResize;
  const draggingRef = useRef(false);

  useEffect(() => {
    function onMove(event: PointerEvent) {
      if (draggingRef.current) {
        onResizeRef.current(event.clientX);
      }
    }
    function onUp() {
      if (draggingRef.current) {
        draggingRef.current = false;
        setActive(false);
        document.body.classList.remove("resizing");
      }
    }
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
  }, []);

  return (
    <div
      className={`resize-handle resize-handle-${side}${active ? " active" : ""}`}
      style={style}
      role="separator"
      aria-orientation="vertical"
      aria-label={ariaLabel}
      onDoubleClick={() => onResizeRef.current(-1)}
      onPointerDown={(event) => {
        event.preventDefault();
        draggingRef.current = true;
        setActive(true);
        document.body.classList.add("resizing");
      }}
    />
  );
}

function App() {
  const [snapshot, setSnapshot] = useState<BankSnapshot>(emptySnapshot);
  const [activeNote, setActiveNote] = useState<NoteWithContent | null>(null);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [collapsedSections, setCollapsedSections] = useState<string[]>([]);
  const [sortMode, setSortMode] = useState<SortMode>(() => {
    return (
      (localStorage.getItem("smooth-note-sort") as SortMode | null) ??
      "updated-desc"
    );
  });
  const [theme, setTheme] = useState<ThemeMode>(() => {
    return (
      (localStorage.getItem("smooth-theme") as ThemeMode | null) ?? "system"
    );
  });
  const [view, setView] = useState<ViewMode>("notes");
  const setError = (message: string | null) => {
    if (message) {
      toast.error(message);
    }
  };
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [panelOpen, setPanelOpen] = useState(true);
  const [editorReloadKey, setEditorReloadKey] = useState(0);
  const [createMenuOpen, setCreateMenuOpen] = useState(false);
  const [moveMenuOpen, setMoveMenuOpen] = useState(false);
  const [renamingFolderId, setRenamingFolderId] = useState<string | null>(null);

  useEffect(() => {
    if (!createMenuOpen) {
      return;
    }
    const close = () => setCreateMenuOpen(false);
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [createMenuOpen]);

  useEffect(() => {
    if (!moveMenuOpen) {
      return;
    }
    const close = () => setMoveMenuOpen(false);
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [moveMenuOpen]);
  const [linkSuggestions, setLinkSuggestions] = useState<LinkSuggestion[]>([]);
  const [menuTarget, setMenuTarget] = useState<{
    note: NoteListItem;
    x: number;
    y: number;
  } | null>(null);
  const [draggedNoteId, setDraggedNoteId] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<DropTarget | null>(null);
  const [sidebarWidth, setSidebarWidth] = useState(
    () => Number(localStorage.getItem("smooth-sidebar-width")) || 340,
  );
  const [panelWidth, setPanelWidth] = useState(
    () => Number(localStorage.getItem("smooth-panel-width")) || 308,
  );
  const [meetingState, setMeetingState] = useState<MeetingState>("idle");
  const [meetingNoteTitle, setMeetingNoteTitle] = useState("");
  const [meetingDetail, setMeetingDetail] = useState("Ready");
  const [meetingContentRevision, setMeetingContentRevision] = useState(0);
  const [meetingNoteId, setMeetingNoteId] = useState<string | null>(null);
  const [meetingNotesOpen, setMeetingNotesOpen] = useState(false);
  const [meetingQuickNotes, setMeetingQuickNotes] = useState("");
  const [meetingQuickNoteId, setMeetingQuickNoteId] = useState<string | null>(
    null,
  );
  const [meetingQuickNoteSaveState, setMeetingQuickNoteSaveState] =
    useState<SaveState>("idle");
  const [entityJumpTarget, setEntityJumpTarget] = useState<{
    nonce: number;
    surfaceText: string;
  } | null>(null);
  const [reminderJumpTarget, setReminderJumpTarget] =
    useState<ReminderJumpTarget | null>(null);
  const [meetingVisualSources, setMeetingVisualSources] = useState<
    MeetingVisualSource[]
  >([]);
  const [meetingSourcePickerOpen, setMeetingSourcePickerOpen] = useState(false);
  const [meetingVisualSourceName, setMeetingVisualSourceName] = useState<
    string | null
  >(null);
  const [calendarConfig, setCalendarConfig] = useState<CalendarConfig | null>(
    null,
  );
  const [calendarEvents, setCalendarEvents] = useState<CalendarEvent[]>([]);
  const [calendarError, setCalendarError] = useState<string | null>(null);
  const [isNotesRefreshing, setIsNotesRefreshing] = useState(false);
  const [isCalendarRefreshing, setIsCalendarRefreshing] = useState(false);
  const [calendarNow, setCalendarNow] = useState(() => Date.now());
  const [meetingMicLabel, setMeetingMicLabel] = useState(
    () => localStorage.getItem("smooth-meeting-you") || "You",
  );
  const [meetingSystemLabel, setMeetingSystemLabel] = useState(
    () => localStorage.getItem("smooth-meeting-others") || "Participants",
  );
  const [diarizationPrompt, setDiarizationPrompt] =
    useState<DiarizationSpeakerPrompt | null>(null);
  const meetingMicLabelRef = useRef(meetingMicLabel);
  meetingMicLabelRef.current = meetingMicLabel;
  const meetingSystemLabelRef = useRef(meetingSystemLabel);
  meetingSystemLabelRef.current = meetingSystemLabel;
  const meetingLoopActiveRef = useRef(false);
  const meetingNoteIdRef = useRef<string | null>(null);
  const meetingTitleRef = useRef("");
  const meetingFolderIdRef = useRef<string | null>(null);
  const meetingContentRef = useRef("");
  const meetingQuickNotesRef = useRef("");
  const meetingQuickNoteIdRef = useRef<string | null>(null);
  const meetingQuickNoteFolderIdRef = useRef<string | null>(null);
  const meetingQuickNoteCreateRef = useRef<Promise<NoteWithContent> | null>(
    null,
  );
  const meetingQuickNoteSaveTimerRef = useRef<number | null>(null);
  const meetingLoopRef = useRef<Promise<void> | null>(null);
  const meetingChunkRef = useRef<Promise<void> | null>(null);
  const meetingAppendQueueRef = useRef<Promise<void>>(Promise.resolve());
  const meetingSttSequenceRef = useRef(0);
  const meetingPendingSttJobsRef = useRef(0);
  const meetingPendingSttResolversRef = useRef<Array<() => void>>([]);
  const lastSystemCapturePathRef = useRef<string | null>(null);
  const meetingVisualSourceIdRef = useRef<string | null>(null);
  const lastMeetingSnapshotAtRef = useRef(0);
  const diarizationSessionIdRef = useRef<string | null>(null);
  const diarizationSpeakerNamesRef = useRef<Record<string, string>>({});
  const diarizationSpeakerOrderRef = useRef<string[]>([]);
  const diarizationPromptedSpeakersRef = useRef<Set<string>>(new Set());
  const diarizationPromptQueueRef = useRef<DiarizationSpeakerPrompt[]>([]);
  const diarizationPromptRef = useRef<DiarizationSpeakerPrompt | null>(null);
  const diarizationUnavailableRef = useRef(false);
  const shouldShowContextPanel = Boolean(
    activeNote && !activeNote.deleted_at && panelOpen,
  );

  const MIN_EDITOR = 360;

  function resizeSidebar(clientX: number) {
    if (clientX < 0) {
      setSidebarWidth(340);
      return;
    }
    const reserved = shouldShowContextPanel ? panelWidth : 0;
    const max = Math.max(240, window.innerWidth - reserved - MIN_EDITOR);
    setSidebarWidth(Math.min(Math.max(clientX, 240), max));
  }

  function resizePanel(clientX: number) {
    if (clientX < 0) {
      setPanelWidth(308);
      return;
    }
    const next = window.innerWidth - clientX;
    const max = Math.max(240, window.innerWidth - sidebarWidth - MIN_EDITOR);
    setPanelWidth(Math.min(Math.max(next, 240), max));
  }

  const activeNotes = snapshot.notes.filter((note) => !note.deleted_at);
  const trashedNotes = useMemo(
    () =>
      sortNotes(
        snapshot.notes.filter((note) => note.deleted_at),
        sortMode,
      ),
    [snapshot.notes, sortMode],
  );

  const filteredNotes = useMemo(() => {
    return sortNotes(activeNotes, sortMode);
  }, [activeNotes, sortMode]);

  useEffect(() => {
    startSemanticIndexer();
  }, []);

  const folderGroups = useMemo(() => {
    const groups = snapshot.folders.map((folder) => ({
      folder,
      notes: filteredNotes.filter((note) => note.folder_id === folder.id),
    }));
    groups.sort((first, second) => {
      if (first.folder.system_key === "imported") return -1;
      if (second.folder.system_key === "imported") return 1;
      return 0;
    });
    // While renaming a (usually just-created) folder, hoist it to the top so the
    // inline edit box is immediately visible.
    if (renamingFolderId) {
      groups.sort((first, second) => {
        if (first.folder.id === renamingFolderId) return -1;
        if (second.folder.id === renamingFolderId) return 1;
        return 0;
      });
    }
    return groups;
  }, [filteredNotes, snapshot.folders, renamingFolderId]);

  const inboxNotes = filteredNotes.filter((note) => !note.folder_id);
  const actionableCalendarEvent =
    calendarEvents.find((event) =>
      isCalendarEventActionable(event, calendarNow),
    ) ?? null;

  const paletteNotes = useMemo(
    () =>
      sortNotes(
        snapshot.notes.filter((note) => !note.deleted_at),
        "updated-desc",
      ),
    [snapshot.notes],
  );

  const createNoteRef = useRef<() => void>(() => {});
  createNoteRef.current = () => void createNote();

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const meta = event.metaKey || event.ctrlKey;
      if (meta && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setPaletteOpen((open) => !open);
      } else if (meta && event.key.toLowerCase() === "n") {
        event.preventDefault();
        createNoteRef.current();
      } else if (meta && event.key.toLowerCase() === "f") {
        event.preventDefault();
        setPaletteOpen(true);
      } else if (meta && event.key === "\\") {
        event.preventDefault();
        setPanelOpen((open) => !open);
      }
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  function isSectionOpen(id: string) {
    const isFolder = snapshot.folders.some((folder) => folder.id === id);
    if (isFolder) {
      return (
        collapsedSections.includes(`open:${id}`) || renamingFolderId === id
      );
    }
    return !collapsedSections.includes(id);
  }

  function toggleSection(id: string) {
    const isFolder = snapshot.folders.some((folder) => folder.id === id);
    const key = isFolder ? `open:${id}` : id;
    setCollapsedSections((current) =>
      current.includes(key)
        ? current.filter((sectionId) => sectionId !== key)
        : [...current, key],
    );
  }

  const linkedNotes = useMemo(() => {
    const sourceIds =
      activeNote?.id !== undefined
        ? [activeNote.id]
        : selectedIds.length > 0
          ? selectedIds
          : [];

    const linkByNoteId = new Map<string, NoteLink>();
    for (const link of snapshot.links) {
      if (sourceIds.length === 0) {
        linkByNoteId.set(link.source_id, link);
        linkByNoteId.set(link.target_id, link);
      } else if (sourceIds.includes(link.source_id)) {
        linkByNoteId.set(link.target_id, link);
      }
    }

    return snapshot.notes
      .filter((note) => linkByNoteId.has(note.id) && !note.deleted_at)
      .map((note) => ({ note, link: linkByNoteId.get(note.id)! }));
  }, [activeNote?.id, selectedIds, snapshot.links, snapshot.notes]);

  const loadBank = useCallback(async () => {
    const bank = await invoke<BankSnapshot>("get_bank");
    setSnapshot(bank);
    return bank;
  }, []);

  async function refreshNotes() {
    setIsNotesRefreshing(true);
    try {
      await loadBank();
    } catch (refreshError) {
      setError(String(refreshError));
    } finally {
      setIsNotesRefreshing(false);
    }
  }

  function revealNoteInTree(note: Pick<NoteWithContent, "id" | "folder_id" | "deleted_at">) {
    setCollapsedSections((current) => {
      if (note.deleted_at) {
        return current.filter((sectionId) => sectionId !== "trash");
      }
      if (note.folder_id) {
        const key = `open:${note.folder_id}`;
        return current.includes(key) ? current : [...current, key];
      }
      return current.filter((sectionId) => sectionId !== "inbox");
    });

    window.requestAnimationFrame(() => {
      window.requestAnimationFrame(() => {
        const row = Array.from(
          document.querySelectorAll<HTMLElement>(".notes-pane [data-note-id]"),
        ).find((element) => element.dataset.noteId === note.id);
        row?.scrollIntoView({ block: "nearest" });
      });
    });
  }

  async function refreshCalendarEvents({ silent = false } = {}) {
    setIsCalendarRefreshing(true);
    try {
      const config = await invoke<CalendarConfig>("get_calendar_config");
      setCalendarConfig(config);
      if (!config.has_access_token && !config.has_refresh_token) {
        setCalendarEvents([]);
        setCalendarError(null);
        return [];
      }

      const events = await invoke<CalendarEvent[]>(
        "list_upcoming_calendar_events",
      );
      setCalendarEvents(events);
      setCalendarError(null);
      return events;
    } catch (calendarRefreshError) {
      const message = String(calendarRefreshError);
      setCalendarError(message);
      if (!silent) {
        toast.error(message);
      }
      return [];
    } finally {
      setIsCalendarRefreshing(false);
    }
  }

  useEffect(() => {
    loadBank().catch((loadError: unknown) => setError(String(loadError)));
    void refreshCalendarEvents({ silent: true });
  }, [loadBank]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen("note-links-updated", () => {
      void loadBank().catch((loadError: unknown) => {
        if (!disposed) {
          setError(String(loadError));
        }
      });
    })
      .then((nextUnlisten) => {
        if (disposed) {
          nextUnlisten();
        } else {
          unlisten = nextUnlisten;
        }
      })
      .catch((listenError) => setError(String(listenError)));

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [loadBank]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<SttJobEvent>("stt-job-completed", (event) => {
      if (disposed || event.payload.note_id !== meetingNoteIdRef.current) {
        return;
      }
      handleMeetingSttEvent(event.payload);
    })
      .then((nextUnlisten) => {
        if (disposed) {
          nextUnlisten();
        } else {
          unlisten = nextUnlisten;
        }
      })
      .catch((listenError) => setError(String(listenError)));

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<MeetingNoteCompletedEvent>("meeting-note-completed", (event) => {
      if (disposed) {
        return;
      }
      void loadBank();
      if (event.payload.error) {
        toast.error(`Meeting note completion failed: ${event.payload.error}`);
        return;
      }
      if (activeNote?.id === event.payload.note_id) {
        void invoke<NoteWithContent>("get_note", {
          id: event.payload.note_id,
        }).then((note) => {
          if (!disposed) {
            setActiveNote(note);
            setEditorReloadKey((key) => key + 1);
          }
        });
      }
      toast.success("Meeting note completed");
    })
      .then((nextUnlisten) => {
        if (disposed) {
          nextUnlisten();
        } else {
          unlisten = nextUnlisten;
        }
      })
      .catch((listenError) => setError(String(listenError)));

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [activeNote?.id, loadBank]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    void listen<string>("slack-note-created", () => {
      void loadBank();
    }).then((stopListening) => {
      unlisten = stopListening;
    });
    return () => unlisten?.();
  }, [loadBank]);

  useEffect(() => {
    const interval = window.setInterval(() => setCalendarNow(Date.now()), 30_000);
    return () => window.clearInterval(interval);
  }, []);

  useEffect(() => {
    localStorage.setItem("smooth-theme", theme);
    if (theme === "system") {
      document.documentElement.removeAttribute("data-theme");
    } else {
      document.documentElement.dataset.theme = theme;
    }
  }, [theme]);

  useEffect(() => {
    localStorage.setItem("smooth-note-sort", sortMode);
  }, [sortMode]);

  useEffect(() => {
    localStorage.setItem("smooth-sidebar-width", String(sidebarWidth));
  }, [sidebarWidth]);

  useEffect(() => {
    localStorage.setItem("smooth-panel-width", String(panelWidth));
  }, [panelWidth]);

  useEffect(() => {
    localStorage.setItem("smooth-meeting-you", meetingMicLabel);
  }, [meetingMicLabel]);

  useEffect(() => {
    localStorage.setItem("smooth-meeting-others", meetingSystemLabel);
  }, [meetingSystemLabel]);

  useEffect(() => {
    let cancelled = false;

    if (!activeNote || activeNote.deleted_at) {
      setLinkSuggestions([]);
      return;
    }

    invoke<LinkSuggestion[]>("get_link_suggestions", {
      noteId: activeNote.id,
      limit: 6,
    })
      .then((suggestions) => {
        if (!cancelled) {
          setLinkSuggestions(suggestions);
        }
      })
      .catch((suggestionError: unknown) => {
        if (!cancelled) {
          setLinkSuggestions([]);
          setError(String(suggestionError));
        }
      });

    return () => {
      cancelled = true;
    };
  }, [activeNote?.deleted_at, activeNote?.id, snapshot.links]);

  async function openNote(id: string) {
    try {
      setError(null);
      const note = await invoke<NoteWithContent>("get_note", { id });
      setActiveNote(note);
      revealNoteInTree(note);
      setView("notes");
    } catch (openError) {
      setError(String(openError));
    }
  }

  async function openReminder(reminder: ReminderRecord) {
    setReminderJumpTarget({ ...reminder, nonce: Date.now() });
    await openNote(reminder.noteId);
  }

  async function createNote() {
    try {
      setError(null);
      const note = await invoke<NoteWithContent>("create_note", {
        title: null,
        folderId: null,
      });
      await loadBank();
      setActiveNote(note);
      setSelectedIds([note.id]);
      setView("notes");
    } catch (createError) {
      setError(String(createError));
    }
  }

  // Create a normal note seeded with content (e.g. a chat summary). Saving as a
  // regular note enqueues entity extraction automatically.
  async function createNoteFromContent(
    content: string,
    sourcePrompt: string | null = null,
  ) {
    const parentId = activeNote?.id ?? null;
    try {
      setError(null);
      const note = await invoke<NoteWithContent>("create_note", {
        title: null,
        folderId: null,
      });
      const saved = await invoke<NoteWithContent>("save_note", {
        id: note.id,
        title: "",
        content,
        folderId: null,
      });
      if (parentId) {
        const bank = await invoke<BankSnapshot>("link_chat_created_note", {
          parentId,
          childId: saved.id,
          sourcePrompt: sourcePrompt ?? "",
          responseContent: content,
        });
        setSnapshot(bank);
      } else {
        await loadBank();
      }
      setActiveNote(saved);
      setSelectedIds([saved.id]);
      setView("notes");
      toast.success(
        parentId
          ? "Note created and linked — extracting entities"
          : "Note created — extracting entities",
      );
    } catch (createError) {
      toast.error(createError);
    }
  }

  // Rename a meeting speaker everywhere by rewriting only the transcript speaker
  // slots (never free text). Reloads the editor so the change isn't clobbered.
  async function renameSpeaker(oldName: string, newName: string) {
    if (!activeNote) {
      return;
    }
    const trimmed = newName.trim();
    if (!trimmed || trimmed === oldName) {
      return;
    }
    const nextContent = renameSpeakerInContent(
      activeNote.content,
      oldName,
      trimmed,
    );
    if (nextContent === activeNote.content) {
      return;
    }
    try {
      await saveNote(
        activeNote.id,
        activeNote.title,
        nextContent,
        activeNote.folder_id,
      );
      setEditorReloadKey((key) => key + 1);
      toast.success(`Renamed “${oldName}” to “${trimmed}”`);
    } catch (renameError) {
      toast.error(renameError);
    }
  }

  async function createMeetingNote(title: string) {
    setError(null);
    const note = await invoke<NoteWithContent>("create_meeting_note", {
      title,
      folderId: null,
    });
    await loadBank();
    setActiveNote(note);
    setSelectedIds([note.id]);
    setView("notes");
    return note;
  }

  const applySavedNote = useCallback((saved: NoteWithContent) => {
    setActiveNote((current) => (current?.id === saved.id ? saved : current));
    setSnapshot((current) => ({
      ...current,
      notes: current.notes.map((note) =>
        note.id === saved.id
          ? {
              ...note,
              title: saved.title,
              folder_id: saved.folder_id,
              updated_at: saved.updated_at,
              excerpt: markdownToExcerpt(saved.content),
              extraction_status: saved.extraction_status,
            }
          : note,
      ),
    }));
  }, []);

  const saveNote = useCallback(
    async (
      id: string,
      title: string,
      content: string,
      folderId: string | null,
    ) => {
      const saved = await invoke<NoteWithContent>("save_note", {
        id,
        title,
        content,
        folderId,
      });
      applySavedNote(saved);
      return saved;
    },
    [applySavedNote],
  );

  const saveMeetingNote = useCallback(
    async (
      id: string,
      title: string,
      content: string,
      folderId: string | null,
    ) => {
      const saved = await invoke<NoteWithContent>("save_meeting_note", {
        id,
        title,
        content,
        folderId,
      });
      applySavedNote(saved);
      setMeetingContentRevision((revision) => revision + 1);
      return saved;
    },
    [applySavedNote],
  );

  function meetingQuickNoteTitle() {
    const base = meetingTitleRef.current.trim() || "Meeting";
    return `${base.replace(/\s+Notes$/i, "")} Notes`;
  }

  async function saveMeetingQuickNoteNow() {
    const noteId = meetingQuickNoteIdRef.current;
    if (!noteId) {
      return null;
    }
    setMeetingQuickNoteSaveState("saving");
    try {
      const saved = await saveMeetingNote(
        noteId,
        meetingQuickNoteTitle(),
        meetingQuickNotesRef.current,
        meetingQuickNoteFolderIdRef.current,
      );
      meetingQuickNoteFolderIdRef.current = saved.folder_id;
      setMeetingQuickNoteSaveState("saved");
      return saved;
    } catch (saveError) {
      setMeetingQuickNoteSaveState("error");
      throw saveError;
    }
  }

  function scheduleMeetingQuickNoteSave() {
    if (meetingQuickNoteSaveTimerRef.current !== null) {
      window.clearTimeout(meetingQuickNoteSaveTimerRef.current);
    }
    meetingQuickNoteSaveTimerRef.current = window.setTimeout(() => {
      meetingQuickNoteSaveTimerRef.current = null;
      void saveMeetingQuickNoteNow().catch((saveError) =>
        toast.error(saveError),
      );
    }, 500);
  }

  async function ensureMeetingQuickNote() {
    if (meetingQuickNoteIdRef.current) {
      return meetingQuickNoteIdRef.current;
    }
    if (!meetingQuickNotesRef.current.trim()) {
      return null;
    }
    if (meetingQuickNoteCreateRef.current) {
      return (await meetingQuickNoteCreateRef.current).id;
    }
    const transcriptNoteId = meetingNoteIdRef.current;
    if (!transcriptNoteId) {
      return null;
    }

    setMeetingQuickNoteSaveState("saving");
    const initialContent = meetingQuickNotesRef.current;
    const createPromise = invoke<NoteWithContent>("create_meeting_quick_note", {
      transcriptNoteId,
      transcriptTitle: meetingTitleRef.current,
      content: initialContent,
    });
    meetingQuickNoteCreateRef.current = createPromise;
    try {
      const created = await createPromise;
      meetingQuickNoteIdRef.current = created.id;
      meetingQuickNoteFolderIdRef.current = created.folder_id;
      setMeetingQuickNoteId(created.id);
      applySavedNote(created);
      await loadBank();
      if (meetingQuickNotesRef.current !== initialContent) {
        await saveMeetingQuickNoteNow();
      } else {
        setMeetingQuickNoteSaveState("saved");
      }
      return created.id;
    } catch (createError) {
      setMeetingQuickNoteSaveState("error");
      throw createError;
    } finally {
      meetingQuickNoteCreateRef.current = null;
    }
  }

  function updateMeetingQuickNotes(value: string) {
    setMeetingQuickNotes(value);
    meetingQuickNotesRef.current = value;
    if (!meetingQuickNoteIdRef.current) {
      if (value.trim()) {
        void ensureMeetingQuickNote().catch((createError) =>
          toast.error(createError),
        );
      }
      return;
    }
    setMeetingQuickNoteSaveState("saving");
    scheduleMeetingQuickNoteSave();
  }

  async function flushMeetingQuickNote() {
    if (meetingQuickNoteSaveTimerRef.current !== null) {
      window.clearTimeout(meetingQuickNoteSaveTimerRef.current);
      meetingQuickNoteSaveTimerRef.current = null;
    }
    const noteId = await ensureMeetingQuickNote();
    if (!noteId) {
      return null;
    }
    await saveMeetingQuickNoteNow();
    return noteId;
  }

  function resetMeetingDiarization() {
    diarizationSpeakerNamesRef.current = {};
    diarizationSpeakerOrderRef.current = [];
    diarizationPromptedSpeakersRef.current = new Set();
    diarizationPromptQueueRef.current = [];
    diarizationPromptRef.current = null;
    diarizationUnavailableRef.current = false;
    diarizationSessionIdRef.current = null;
    setDiarizationPrompt(null);
  }

  function showNextDiarizationPrompt() {
    if (diarizationPromptRef.current) {
      return;
    }
    const next = diarizationPromptQueueRef.current.shift() ?? null;
    diarizationPromptRef.current = next;
    setDiarizationPrompt(next);
  }

  function enqueueDiarizationPrompt(prompt: DiarizationSpeakerPrompt) {
    if (diarizationPromptedSpeakersRef.current.has(prompt.speakerId)) {
      return;
    }
    diarizationPromptedSpeakersRef.current.add(prompt.speakerId);
    diarizationPromptQueueRef.current.push(prompt);
    showNextDiarizationPrompt();
  }

  function dismissDiarizationPrompt() {
    diarizationPromptRef.current = null;
    setDiarizationPrompt(null);
    window.setTimeout(showNextDiarizationPrompt, 0);
  }

  function resolveDiarizedSpeakerLabel(speakerId: string | null) {
    if (!speakerId) {
      return meetingSystemLabelRef.current;
    }

    let speakerIndex = diarizationSpeakerOrderRef.current.indexOf(speakerId);
    if (speakerIndex === -1) {
      diarizationSpeakerOrderRef.current.push(speakerId);
      speakerIndex = diarizationSpeakerOrderRef.current.length - 1;
    }

    const fallbackName = `Speaker ${speakerIndex + 1}`;
    if (!diarizationSpeakerNamesRef.current[speakerId]) {
      diarizationSpeakerNamesRef.current[speakerId] = fallbackName;
      enqueueDiarizationPrompt({ speakerId, fallbackName });
    }

    return diarizationSpeakerNamesRef.current[speakerId] || fallbackName;
  }

  function existingDiarizedSpeakerNames(currentSpeakerId: string) {
    return Array.from(
      new Set(
        diarizationSpeakerOrderRef.current
          .filter((speakerId) => speakerId !== currentSpeakerId)
          .map((speakerId) => diarizationSpeakerNamesRef.current[speakerId])
          .filter((name): name is string => Boolean(name?.trim())),
      ),
    );
  }

  async function renameDiarizedSpeaker(speakerId: string, requestedName: string) {
    const nextName = requestedName.trim();
    if (!nextName) {
      dismissDiarizationPrompt();
      return;
    }

    const previousName =
      diarizationSpeakerNamesRef.current[speakerId] ||
      diarizationPromptRef.current?.fallbackName ||
      "Speaker";
    diarizationSpeakerNamesRef.current[speakerId] = nextName;
    dismissDiarizationPrompt();

    if (previousName === nextName) {
      return;
    }

    const noteId = meetingNoteIdRef.current;
    if (!noteId) {
      return;
    }

    const nextContent = renameSpeakerInContent(
      meetingContentRef.current,
      previousName,
      nextName,
    );
    if (nextContent === meetingContentRef.current) {
      return;
    }

    try {
      meetingContentRef.current = nextContent;
      const saved = await saveMeetingNote(
        noteId,
        meetingTitleRef.current,
        nextContent,
        meetingFolderIdRef.current,
      );
      meetingFolderIdRef.current = saved.folder_id;
    } catch (renameError) {
      toast.error(renameError);
    }
  }

  async function diarizeSystemCapture(path: string) {
    if (diarizationUnavailableRef.current) {
      return null;
    }

    setMeetingDetail("Identifying speakers");
    return await invoke<DiarizationResult>("diarize_capture_file", {
      input: {
        path,
        sessionId: diarizationSessionIdRef.current,
      },
    }).catch((diarizationError) => {
      const message = String(diarizationError);
      if (message.includes("Diarization helper binary was not found")) {
        diarizationUnavailableRef.current = true;
        toast.info("Diarization helper is unavailable; using the participant label");
      } else {
        setMeetingDetail(`Diarization skipped: ${message}`);
      }
      return null;
    });
  }

  function notifyMeetingSttProgress() {
    if (meetingPendingSttJobsRef.current === 0) {
      const resolvers = meetingPendingSttResolversRef.current.splice(0);
      resolvers.forEach((resolve) => resolve());
    }
  }

  function waitForMeetingSttJobs() {
    if (meetingPendingSttJobsRef.current === 0) {
      return Promise.resolve();
    }
    return new Promise<void>((resolve) => {
      meetingPendingSttResolversRef.current.push(resolve);
    });
  }

  async function enqueueMeetingSttJob(
    source: "mic" | "system",
    preview: AudioCapturePreview | SystemAudioCapturePreview,
  ) {
    const noteId = meetingNoteIdRef.current;
    if (!noteId || preview.samples <= 0) {
      return null;
    }

    meetingSttSequenceRef.current += 1;
    meetingPendingSttJobsRef.current += 1;
    setMeetingDetail("Queued transcription");
    try {
      return await invoke<SttJob>("enqueue_stt_job", {
        input: {
          noteId,
          source,
          path: preview.path,
          sequence: meetingSttSequenceRef.current,
          chunkStartedAtMs: Date.now() - Number(preview.duration_ms),
          durationMs: Number(preview.duration_ms),
        },
      });
    } catch (enqueueError) {
      meetingPendingSttJobsRef.current = Math.max(
        0,
        meetingPendingSttJobsRef.current - 1,
      );
      notifyMeetingSttProgress();
      setMeetingDetail(`Transcription queue skipped: ${String(enqueueError)}`);
      return null;
    }
  }

  function handleMeetingSttEvent(event: SttJobEvent) {
    const normalizedEvent: SttJobEvent = {
      ...event,
      job_id: event.job_id ?? event.jobId ?? 0,
      note_id: event.note_id ?? event.noteId ?? "",
      chunk_started_at_ms:
        event.chunk_started_at_ms ?? event.chunkStartedAtMs ?? null,
      duration_ms: event.duration_ms ?? event.durationMs ?? null,
    };
    meetingAppendQueueRef.current = meetingAppendQueueRef.current
      .catch(() => undefined)
      .then(async () => {
        try {
          await appendMeetingSttEvent(normalizedEvent);
        } finally {
          meetingPendingSttJobsRef.current = Math.max(
            0,
            meetingPendingSttJobsRef.current - 1,
          );
          if (!meetingLoopActiveRef.current) {
            const remaining = meetingPendingSttJobsRef.current;
            setMeetingDetail(
              remaining > 0
                ? `Finishing meeting processing (${remaining} remaining)`
                : "Meeting processing finished",
            );
          }
          notifyMeetingSttProgress();
        }
      });
  }

  async function appendMeetingSttEvent(event: SttJobEvent) {
    if (event.error) {
      setMeetingDetail(`Transcription skipped: ${event.error}`);
      return;
    }
    const text = event.transcription?.text.trim();
    if (!text) {
      setMeetingDetail(meetingLoopActiveRef.current ? "Listening" : "Stopped");
      return;
    }

    const noteId = meetingNoteIdRef.current;
    if (!noteId || event.note_id !== noteId) {
      return;
    }

    let nextContent = meetingContentRef.current;
    if (event.source === "mic") {
      nextContent = appendMeetingTranscript(
        nextContent,
        meetingMicLabelRef.current,
        text,
      );
    } else {
      const diarization = await diarizeSystemCapture(event.path);
      if (diarization?.turns.length && event.transcription?.segments.length) {
        for (const segment of event.transcription.segments) {
          const sessionSegment = {
            ...segment,
            start_ms: segment.start_ms + diarization.chunk_start_ms,
            end_ms: segment.end_ms + diarization.chunk_start_ms,
          };
          const speakerId = diarizedSpeakerForSegment(
            sessionSegment,
            diarization.turns,
          );
          nextContent = appendMeetingTranscript(
            nextContent,
            resolveDiarizedSpeakerLabel(speakerId),
            segment.text,
          );
        }
      } else {
        nextContent = appendMeetingTranscript(
          nextContent,
          meetingSystemLabelRef.current,
          text,
        );
      }
    }

    if (nextContent === meetingContentRef.current) {
      return;
    }
    meetingContentRef.current = nextContent;
    const saved = await saveMeetingNote(
      noteId,
      meetingTitleRef.current,
      nextContent,
      meetingFolderIdRef.current,
    );
    meetingFolderIdRef.current = saved.folder_id;
    setMeetingDetail(meetingLoopActiveRef.current ? "Listening" : "Stopped");
  }

  async function startMeetingMode() {
    if (meetingState !== "idle") {
      return;
    }

    setMeetingState("starting");
    setMeetingDetail("Finding screens");
    try {
      const sources = await invoke<MeetingVisualSource[]>(
        "list_meeting_visual_sources",
      );
      setMeetingVisualSources(sources);
      setMeetingSourcePickerOpen(true);
      setMeetingState("idle");
      setMeetingDetail(
        sources.length > 0 ? "Choose capture target" : "No visual sources",
      );
    } catch (sourceError) {
      toast.error(`Visual source list failed: ${String(sourceError)}`);
      await beginMeetingMode(null, null);
    }
  }

  async function beginMeetingMode(
    visualSourceId: string | null,
    visualSourceName: string | null,
  ) {
    if (meetingState !== "idle" && meetingState !== "starting") {
      return;
    }

    setMeetingSourcePickerOpen(false);
    setMeetingState("starting");
    setMeetingDetail("Creating note");
    try {
      const title = meetingTitleNow();
      const note = await createMeetingNote(title);
      const content = meetingInitialContent(title);

      meetingNoteIdRef.current = note.id;
      setMeetingNoteId(note.id);
      meetingTitleRef.current = title;
      meetingFolderIdRef.current = note.folder_id;
      meetingContentRef.current = content;
      meetingQuickNotesRef.current = "";
      meetingQuickNoteIdRef.current = null;
      meetingQuickNoteFolderIdRef.current = null;
      meetingQuickNoteCreateRef.current = null;
      if (meetingQuickNoteSaveTimerRef.current !== null) {
        window.clearTimeout(meetingQuickNoteSaveTimerRef.current);
        meetingQuickNoteSaveTimerRef.current = null;
      }
      setMeetingQuickNotes("");
      setMeetingQuickNoteId(null);
      setMeetingQuickNoteSaveState("idle");
      setMeetingNotesOpen(false);
      meetingAppendQueueRef.current = Promise.resolve();
      meetingSttSequenceRef.current = 0;
      meetingPendingSttJobsRef.current = 0;
      meetingPendingSttResolversRef.current = [];
      lastSystemCapturePathRef.current = null;
      meetingVisualSourceIdRef.current = visualSourceId;
      lastMeetingSnapshotAtRef.current = 0;
      resetMeetingDiarization();
      const diarizationSession = await invoke<DiarizationSessionStarted>(
        "start_diarization_session",
      ).catch((sessionError) => {
        setMeetingDetail(`Diarization session skipped: ${String(sessionError)}`);
        return null;
      });
      diarizationSessionIdRef.current =
        diarizationSession?.session_id ?? null;
      setMeetingVisualSourceName(visualSourceName);
      setMeetingNoteTitle(title);
      await saveMeetingNote(note.id, title, content, note.folder_id);

      setMeetingDetail("Starting audio");
      await startMeetingSources();
      meetingLoopActiveRef.current = true;
      setMeetingState("recording");
      setMeetingDetail("Listening");
      meetingLoopRef.current = runMeetingLoop();
    } catch (meetingError) {
      meetingLoopActiveRef.current = false;
      await stopDiarizationSession();
      await stopMeetingSources();
      meetingVisualSourceIdRef.current = null;
      setMeetingVisualSourceName(null);
      setMeetingState("idle");
      setMeetingDetail("Ready");
      toast.error(meetingError);
    }
  }

  function cancelMeetingSourcePicker() {
    setMeetingSourcePickerOpen(false);
    setMeetingState("idle");
    setMeetingDetail("Ready");
  }

  async function pauseMeetingMode() {
    if (meetingState !== "recording") {
      return;
    }

    setMeetingState("stopping");
    setMeetingDetail("Pausing");
    meetingLoopActiveRef.current = false;
    await meetingLoopRef.current;
    await processMeetingChunk(250, false);
    await stopMeetingSources();
    setMeetingState("paused");
    setMeetingDetail("Paused");
  }

  async function resumeMeetingMode() {
    if (meetingState !== "paused") {
      return;
    }

    try {
      setMeetingState("starting");
      setMeetingDetail("Resuming");
      await startMeetingSources();
      meetingLoopActiveRef.current = true;
      setMeetingState("recording");
      setMeetingDetail("Listening");
      meetingLoopRef.current = runMeetingLoop();
    } catch (meetingError) {
      meetingLoopActiveRef.current = false;
      await stopMeetingSources();
      setMeetingState("paused");
      setMeetingDetail("Paused");
      toast.error(meetingError);
    }
  }

  async function stopMeetingMode() {
    if (meetingState === "idle") {
      return;
    }

    const meetingNoteToExtract = meetingNoteIdRef.current;

    setMeetingState("stopping");
    setMeetingDetail("Stopping");
    meetingLoopActiveRef.current = false;
    await meetingLoopRef.current;
    await processMeetingChunk(250, false);
    await stopMeetingSources();
    const quickNoteToComplete = await flushMeetingQuickNote().catch(
      (quickNoteError) => {
        toast.error(quickNoteError);
        return null;
      },
    );
    setMeetingDetail("Finishing meeting processing");
    await waitForMeetingSttJobs();
    await meetingAppendQueueRef.current.catch(() => undefined);
    await stopDiarizationSession();

    // Meeting notes skip live extraction while recording. Now that the
    // transcript is finalized, kick off entity extraction for the note.
    if (meetingNoteToExtract) {
      try {
        await invoke("finalize_meeting_extraction", {
          id: meetingNoteToExtract,
        });
        const refreshed = await invoke<NoteWithContent>("get_note", {
          id: meetingNoteToExtract,
        });
        applySavedNote(refreshed);
        toast.info("Extracting entities from meeting");
      } catch (extractionError) {
        toast.error(extractionError);
      }
    }

    if (meetingNoteToExtract && quickNoteToComplete) {
      try {
        const completion = await invoke<MeetingNoteCompletionStatus>(
          "enqueue_meeting_note_completion",
          {
            transcriptNoteId: meetingNoteToExtract,
            userNoteId: quickNoteToComplete,
          },
        );
        if (completion.status === "queued") {
          toast.info(
            `Completing ${completion.empty_headings} meeting note section${completion.empty_headings === 1 ? "" : "s"}`,
          );
        }
      } catch (completionError) {
        toast.error(completionError);
      }
    }

    meetingLoopRef.current = null;
    meetingChunkRef.current = null;
    meetingNoteIdRef.current = null;
    meetingQuickNotesRef.current = "";
    meetingQuickNoteIdRef.current = null;
    meetingQuickNoteFolderIdRef.current = null;
    meetingVisualSourceIdRef.current = null;
    resetMeetingDiarization();
    setMeetingNoteId(null);
    setMeetingNotesOpen(false);
    setMeetingQuickNotes("");
    setMeetingQuickNoteId(null);
    setMeetingQuickNoteSaveState("idle");
    setMeetingVisualSourceName(null);
    setMeetingState("idle");
    setMeetingDetail("Ready");
  }

  async function stopDiarizationSession() {
    const sessionId = diarizationSessionIdRef.current;
    diarizationSessionIdRef.current = null;
    if (!sessionId) {
      return;
    }
    await invoke("stop_diarization_session", {
      sessionId,
    }).catch(() => undefined);
  }

  async function startMeetingSources() {
    await invoke<AudioCaptureStatus>("start_audio_capture");
    try {
      await invoke<SystemAudioCaptureStatus>("start_system_audio_capture");
    } catch (systemAudioError) {
      await invoke<AudioCaptureStatus>("stop_audio_capture").catch(
        () => undefined,
      );
      throw systemAudioError;
    }
  }

  async function stopMeetingSources() {
    await Promise.allSettled([
      invoke<AudioCaptureStatus>("stop_audio_capture"),
      invoke<SystemAudioCaptureStatus>("stop_system_audio_capture"),
    ]);
  }

  async function runMeetingLoop() {
    while (meetingLoopActiveRef.current) {
      const shouldContinue = await waitForMeetingChunk();
      if (!shouldContinue || !meetingLoopActiveRef.current) {
        break;
      }
      await processMeetingChunk(MEETING_CHUNK_MS - 750, true);
    }
  }

  async function waitForMeetingChunk() {
    let elapsed = 0;
    while (elapsed < MEETING_CHUNK_MS) {
      if (!meetingLoopActiveRef.current) {
        return false;
      }
      await wait(250);
      elapsed += 250;
    }
    return true;
  }

  async function processMeetingChunk(
    minDurationMs: number,
    restartSystemCapture: boolean,
  ) {
    if (meetingChunkRef.current) {
      await meetingChunkRef.current;
      return;
    }

    const chunkPromise = processMeetingChunkInner(
      minDurationMs,
      restartSystemCapture,
    );
    meetingChunkRef.current = chunkPromise;
    try {
      await chunkPromise;
    } finally {
      if (meetingChunkRef.current === chunkPromise) {
        meetingChunkRef.current = null;
      }
    }
  }

  async function processMeetingChunkInner(
    minDurationMs: number,
    restartSystemCapture: boolean,
  ) {
    const noteId = meetingNoteIdRef.current;
    if (!noteId) {
      return;
    }

    setMeetingDetail("Preparing audio chunks");
    const previews: Array<{
      source: "Mic" | "System";
      preview: AudioCapturePreview | SystemAudioCapturePreview;
    }> = [];

    const micPreview = await invoke<AudioCapturePreview | null>(
      "flush_audio_capture_chunk",
      {
        minDurationMs,
      },
    ).catch(() => null);
    if (micPreview) {
      previews.push({ source: "Mic", preview: micPreview });
    }

    const systemStatus = await invoke<SystemAudioCaptureStatus>(
      "stop_system_audio_capture",
    ).catch(() => null);
    const systemPreview = systemStatus?.last_preview ?? null;
    if (
      systemPreview &&
      systemPreview.path !== lastSystemCapturePathRef.current &&
      systemPreview.samples > 0
    ) {
      lastSystemCapturePathRef.current = systemPreview.path;
      previews.push({ source: "System", preview: systemPreview });
    }

    if (restartSystemCapture && meetingLoopActiveRef.current) {
      await invoke<SystemAudioCaptureStatus>(
        "start_system_audio_capture",
      ).catch((error) => {
        setMeetingDetail(`System audio paused: ${String(error)}`);
      });
    }

    for (const item of previews) {
      await enqueueMeetingSttJob(
        item.source === "Mic" ? "mic" : "system",
        item.preview,
      );
    }

    let nextContent = meetingContentRef.current;
    let hasNoteChanges = false;
    const snapshotLine = await maybeCaptureMeetingSnapshot();
    if (snapshotLine) {
      nextContent = appendMeetingSnapshot(nextContent, snapshotLine);
      hasNoteChanges = true;
    }

    if (!hasNoteChanges) {
      setMeetingDetail(meetingLoopActiveRef.current ? "Listening" : "Stopped");
      return;
    }

    meetingContentRef.current = nextContent;
    const saved = await saveMeetingNote(
      noteId,
      meetingTitleRef.current,
      nextContent,
      meetingFolderIdRef.current,
    );
    meetingFolderIdRef.current = saved.folder_id;
    setMeetingDetail(meetingLoopActiveRef.current ? "Listening" : "Stopped");
  }

  async function maybeCaptureMeetingSnapshot() {
    const sourceId = meetingVisualSourceIdRef.current;
    if (!sourceId) {
      return null;
    }

    const now = Date.now();
    if (now - lastMeetingSnapshotAtRef.current < MEETING_SNAPSHOT_MS) {
      return null;
    }

    lastMeetingSnapshotAtRef.current = now;
    const snapshot = await invoke<MeetingSnapshot>("capture_meeting_snapshot", {
      sourceId,
    }).catch((snapshotError) => {
      setMeetingDetail(`Snapshot skipped: ${String(snapshotError)}`);
      return null;
    });

    return snapshot ? meetingSnapshotLine(snapshot) : null;
  }

  // Create a folder with a placeholder name, then drop straight into inline rename.
  async function createFolderInline() {
    try {
      setError(null);
      const before = new Set(snapshot.folders.map((folder) => folder.id));
      const bank = await invoke<BankSnapshot>("create_folder", {
        name: "Untitled Folder",
      });
      setSnapshot(bank);
      const created = bank.folders.find((folder) => !before.has(folder.id));
      if (created) {
        setCollapsedSections((current) => [
          ...current.filter((id) => id !== created.id && id !== `open:${created.id}`),
          `open:${created.id}`,
        ]);
        setRenamingFolderId(created.id);
      }
    } catch (folderError) {
      toast.error(folderError);
    }
  }

  async function renameFolder(id: string, name: string) {
    setRenamingFolderId(null);
    const trimmed = name.trim();
    const current = snapshot.folders.find((folder) => folder.id === id);
    if (!trimmed || (current && current.name === trimmed)) {
      return;
    }
    try {
      const bank = await invoke<BankSnapshot>("rename_folder", {
        id,
        name: trimmed,
      });
      setSnapshot(bank);
    } catch (renameError) {
      toast.error(renameError);
    }
  }

  async function moveNote(id: string, folderId: string | null) {
    const note = snapshot.notes.find((item) => item.id === id);
    if (note && !note.deleted_at && note.folder_id === folderId) {
      return;
    }

    const bank = await invoke<BankSnapshot>("move_note", { id, folderId });
    setSnapshot(bank);
    setActiveNote((current) =>
      current?.id === id ? { ...current, folder_id: folderId } : current,
    );
  }

  async function moveNoteToFolder(id: string, folderId: string | null) {
    try {
      await moveNote(id, folderId);
      setCollapsedSections((current) => {
        if (!folderId) {
          return current.filter((sectionId) => sectionId !== "inbox");
        }
        const key = `open:${folderId}`;
        return current.includes(key) ? current : [...current, key];
      });
      const name = folderId
        ? (snapshot.folders.find((folder) => folder.id === folderId)?.name ??
          "folder")
        : "Inbox";
      toast.success(`Moved to ${name}`);
    } catch (moveError) {
      toast.error(moveError);
    }
  }

  function beginNoteDrag(id: string) {
    setDraggedNoteId(id);
    setDropTarget(null);
  }

  function endNoteDrag() {
    setDraggedNoteId(null);
    setDropTarget(null);
  }

  function updateDropTarget(target: DropTarget | null) {
    if (
      dropTarget?.folderId === target?.folderId &&
      dropTarget?.index === target?.index
    ) {
      return;
    }

    animateNoteTreeLayout(() => setDropTarget(target));
  }

  function isDropTarget(folderId: string | null) {
    return dropTarget?.folderId === folderId;
  }

  function renderNoteRows(
    notes: NoteListItem[],
    folderId: string | null,
    options: { indent?: boolean } = {},
  ) {
    const rows = notes.map((note) => (
      <NoteRow
        key={note.id}
        activeId={activeNote?.id ?? null}
        dragging={draggedNoteId === note.id}
        indent={options.indent}
        note={note}
        onDragEnd={endNoteDrag}
        onDragStart={beginNoteDrag}
        onOpen={openNote}
        onToggleSelected={toggleSelected}
        onContextMenu={(target, x, y) => setMenuTarget({ note: target, x, y })}
        draggable
        selectedIds={selectedIds}
        action={
          <button
            className="ghost-icon"
            type="button"
            onClick={() => void trashNote(note.id)}
            title="Move to trash"
          >
            <Trash2 size={15} />
          </button>
        }
      />
    ));

    if (!draggedNoteId || !isDropTarget(folderId)) {
      return rows;
    }

    const index = Math.max(
      0,
      Math.min(dropTarget?.index ?? rows.length, rows.length),
    );
    rows.splice(index, 0, <DropPlaceholder key="drop-placeholder" />);
    return rows;
  }

  async function moveSelected(folderId: string | null) {
    try {
      setError(null);
      for (const id of selectedIds) {
        await moveNote(id, folderId);
      }
    } catch (moveError) {
      setError(String(moveError));
    }
  }

  async function exportNote(
    id: string,
    title: string,
    content: string,
    folderId: string | null,
  ) {
    try {
      setError(null);
      await saveNote(id, title, content, folderId);
      const result = await invoke<ExportResult>("export_note_markdown", { id });
      toast.success(`Exported Markdown to ${result.path}`);
    } catch (exportError) {
      setError(String(exportError));
      toast.error(exportError);
    }
  }

  async function exportSelectedNotes() {
    try {
      setError(null);
      const result = await invoke<ExportResult>("export_notes_markdown_zip", {
        ids: selectedIds,
      });
      toast.success(`Exported ${result.count} notes to ${result.path}`);
    } catch (exportError) {
      setError(String(exportError));
      toast.error(exportError);
    }
  }

  async function createGmailDraft(input: GmailDraftInput) {
    try {
      setError(null);
      const result = await invoke<GmailDraftResult>("create_gmail_draft", {
        draft: input,
      });
      toast.success(`Gmail draft created: ${result.id}`);
    } catch (draftError) {
      setError(String(draftError));
      toast.error(draftError);
      throw draftError;
    }
  }

  async function trashNote(id: string) {
    try {
      setError(null);
      const bank = await invoke<BankSnapshot>("trash_note", { id });
      setSnapshot(bank);
      setSelectedIds((current) =>
        current.filter((selectedId) => selectedId !== id),
      );
      setActiveNote((current) =>
        current?.id === id
          ? {
              ...current,
              deleted_at: Date.now().toString(),
            }
          : current,
      );
    } catch (trashError) {
      setError(String(trashError));
    }
  }

  async function restoreNote(id: string) {
    try {
      setError(null);
      const bank = await invoke<BankSnapshot>("restore_note", { id });
      setSnapshot(bank);
      setActiveNote((current) =>
        current?.id === id ? { ...current, deleted_at: null } : current,
      );
    } catch (restoreError) {
      setError(String(restoreError));
    }
  }

  async function permanentDeleteNote(id: string) {
    try {
      setError(null);
      const bank = await invoke<BankSnapshot>("permanent_delete_note", { id });
      setSnapshot(bank);
      setSelectedIds((current) =>
        current.filter((selectedId) => selectedId !== id),
      );
      setActiveNote((current) => (current?.id === id ? null : current));
    } catch (deleteError) {
      setError(String(deleteError));
    }
  }

  async function linkSelectedNotes() {
    const count = selectedIds.length;
    try {
      setError(null);
      const bank = await invoke<BankSnapshot>("link_notes", {
        ids: selectedIds,
        label: null,
        linkKind: "manual",
      });
      setSnapshot(bank);
      toast.success(`Linked ${count} notes`);
    } catch (linkError) {
      setError(String(linkError));
    }
  }

  async function linkNote(targetId: string, label?: string | null) {
    if (!activeNote) {
      return;
    }

    try {
      setError(null);
      const bank = await invoke<BankSnapshot>("link_notes", {
        ids: [activeNote.id, targetId],
        label: label ?? null,
        linkKind: "manual",
      });
      setSnapshot(bank);
      toast.success("Linked note");
    } catch (linkError) {
      setError(String(linkError));
    }
  }

  async function linkSuggestedNote(targetId: string) {
    if (!activeNote) {
      return;
    }

    try {
      setError(null);
      const bank = await invoke<BankSnapshot>("link_notes", {
        ids: [activeNote.id, targetId],
        label: "Entity Sharing",
        linkKind: "entity_sharing",
      });
      setSnapshot(bank);
      setLinkSuggestions((current) =>
        current.filter((suggestion) => suggestion.note.id !== targetId),
      );
      toast.success("Linked note");
    } catch (linkError) {
      setError(String(linkError));
    }
  }

  async function renameNoteLink(
    sourceId: string,
    targetId: string,
    label: string | null,
  ) {
    try {
      setError(null);
      const bank = await invoke<BankSnapshot>("rename_note_link", {
        sourceId,
        targetId,
        label,
      });
      setSnapshot(bank);
      toast.success(label?.trim() ? "Link renamed" : "Link name cleared");
    } catch (renameError) {
      setError(String(renameError));
    }
  }

  async function unlinkNotes(sourceId: string, targetId: string) {
    try {
      setError(null);
      const bank = await invoke<BankSnapshot>("unlink_notes", {
        sourceId,
        targetId,
      });
      setSnapshot(bank);
      toast.success("Note unlinked");
    } catch (unlinkError) {
      setError(String(unlinkError));
    }
  }

  function toggleSelected(id: string) {
    setSelectedIds((current) =>
      current.includes(id)
        ? current.filter((selectedId) => selectedId !== id)
        : [...current, id],
    );
  }

  const updateNoteExtractionStatus = useCallback(
    (id: string, status: string) => {
      setActiveNote((current) =>
        current?.id === id && current.extraction_status !== status
          ? { ...current, extraction_status: status }
          : current,
      );
      setSnapshot((current) => ({
        ...current,
        notes: current.notes.map((note) =>
          note.id === id && note.extraction_status !== status
            ? { ...note, extraction_status: status }
            : note,
        ),
      }));
    },
    [],
  );

  function cycleTheme() {
    setTheme((current) =>
      current === "system" ? "light" : current === "light" ? "dark" : "system",
    );
  }

  const ThemeIcon =
    theme === "system" ? Monitor : theme === "light" ? Sun : Moon;

  return (
    <div className="app-shell">
      <header className="titlebar" data-tauri-drag-region>
        <span className="brand">
          <span className="brand-mark" aria-hidden="true">
            S
          </span>
          <span className="wordmark">Smooth</span>
        </span>
        <button
          className="palette-trigger"
          type="button"
          onClick={() => setPaletteOpen(true)}
          title="Search & commands (⌘K)"
        >
          <Search size={14} />
          <span>Search…</span>
          <kbd>⌘K</kbd>
        </button>
      </header>
      <div
        className="app-body"
        style={{ gridTemplateColumns: `${sidebarWidth}px minmax(0, 1fr)` }}
      >
        <aside className="sidebar">
          <div className="sidebar-header">
            <h1>Let&rsquo;s dive in</h1>
            <div className="sidebar-header-actions">
              <ImportDocuments
                onBankChanged={loadBank}
                onOpenNote={openNote}
                onError={toast.error}
                onSuccess={toast.success}
              />
              <button
                className="icon-button"
                type="button"
                onClick={() => void refreshNotes()}
                disabled={isNotesRefreshing}
                title="Refresh notes"
                aria-label="Refresh notes"
              >
                <RefreshCw
                  className={isNotesRefreshing ? "spin" : ""}
                  size={16}
                />
              </button>
              <div
                className="pop-wrap"
                onPointerDown={(event) => event.stopPropagation()}
              >
                <button
                  className="icon-button primary"
                  type="button"
                  onClick={() => setCreateMenuOpen((open) => !open)}
                  title="Create new"
                  aria-haspopup="menu"
                  aria-expanded={createMenuOpen}
                >
                  <Plus size={18} />
                </button>
                {createMenuOpen ? (
                  <div className="pop-menu" role="menu">
                    <button
                      type="button"
                      onClick={() => {
                        setCreateMenuOpen(false);
                        void createNote();
                      }}
                    >
                      <FileText size={15} />
                      New note
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        setCreateMenuOpen(false);
                        void createFolderInline();
                      }}
                    >
                      <FolderPlus size={15} />
                      New folder
                    </button>
                  </div>
                ) : null}
              </div>
            </div>
          </div>

          <div className="sidebar-controls">
            <div className="controls-row">
              <label className="sort-control">
                <ArrowDownUp size={15} />
                <select
                  value={sortMode}
                  onChange={(event) =>
                    setSortMode(event.currentTarget.value as SortMode)
                  }
                  aria-label="Sort notes"
                >
                  <option value="updated-desc">Updated newest</option>
                  <option value="updated-asc">Updated oldest</option>
                  <option value="created-desc">Created newest</option>
                  <option value="created-asc">Created oldest</option>
                </select>
              </label>
            </div>

            {selectedIds.length > 0 ? (
              <div className="selection-bar">
                <span>{selectedIds.length} selected</span>
                <button
                  type="button"
                  disabled={selectedIds.length < 2}
                  onClick={linkSelectedNotes}
                  title="Link selected notes"
                >
                  <Link2 size={15} />
                  <span>Link</span>
                </button>
                <button
                  type="button"
                  onClick={() => void exportSelectedNotes()}
                  title="Export selected notes as ZIP"
                >
                  <Download size={15} />
                  <span>Export</span>
                </button>
                <div
                  className="pop-wrap"
                  onPointerDown={(event) => event.stopPropagation()}
                >
                  <button
                    type="button"
                    className={moveMenuOpen ? "active" : ""}
                    onClick={() => setMoveMenuOpen((open) => !open)}
                    title="Move to folder"
                    aria-haspopup="menu"
                    aria-expanded={moveMenuOpen}
                  >
                    <FolderInput size={15} />
                    <span>Move</span>
                  </button>
                  {moveMenuOpen ? (
                    <div className="pop-menu align-right" role="menu">
                      <button
                        type="button"
                        onClick={() => {
                          setMoveMenuOpen(false);
                          void moveSelected(null);
                        }}
                      >
                        <Inbox size={15} />
                        Inbox
                      </button>
                      {snapshot.folders.map((folder) => (
                        <button
                          key={folder.id}
                          type="button"
                          onClick={() => {
                            setMoveMenuOpen(false);
                            void moveSelected(folder.id);
                          }}
                        >
                          <Folder size={15} />
                          {folder.name}
                        </button>
                      ))}
                    </div>
                  ) : null}
                </div>
              </div>
            ) : null}
          </div>

          <div className="notes-pane">
            <TreeSection
              count={inboxNotes.length}
              draggedNoteId={draggedNoteId}
              dropTarget={dropTarget}
              folderId={null}
              icon={<Inbox size={16} />}
              isOpen={isSectionOpen("inbox")}
              onToggle={() => toggleSection("inbox")}
              onDropTargetChange={updateDropTarget}
              title="Inbox"
              droppable
              onDropNote={(id) => void moveNoteToFolder(id, null)}
            >
              {renderNoteRows(inboxNotes, null)}
            </TreeSection>

            {folderGroups.map(({ folder, notes }) => (
              <TreeSection
                key={folder.id}
                count={notes.length}
                draggedNoteId={draggedNoteId}
                dropTarget={dropTarget}
                folderId={folder.id}
                icon={
                  isSectionOpen(folder.id) ? (
                    <FolderOpen size={16} />
                  ) : (
                    <Folder size={16} />
                  )
                }
                isOpen={isSectionOpen(folder.id)}
                onToggle={() => toggleSection(folder.id)}
                onDropTargetChange={updateDropTarget}
                title={folder.name}
                droppable
                onDropNote={(id) => void moveNoteToFolder(id, folder.id)}
                renamable={!folder.system_key}
                startRenaming={renamingFolderId === folder.id}
                onRename={(name) => void renameFolder(folder.id, name)}
              >
                {renderNoteRows(notes, folder.id, { indent: true })}
              </TreeSection>
            ))}

            {trashedNotes.length > 0 ? (
              <TreeSection
                count={trashedNotes.length}
                icon={<Trash2 size={16} />}
                isOpen={isSectionOpen("trash")}
                onToggle={() => toggleSection("trash")}
                title="Trash"
              >
                {trashedNotes.map((note) => (
                  <NoteRow
                    key={note.id}
                    activeId={activeNote?.id ?? null}
                    muted
                    note={note}
                    onOpen={openNote}
                    onContextMenu={(target, x, y) =>
                      setMenuTarget({ note: target, x, y })
                    }
                    selectable={false}
                    action={
                      <div className="row-actions">
                        <button
                          className="ghost-icon"
                          type="button"
                          onClick={() => void restoreNote(note.id)}
                          title="Restore to Inbox"
                        >
                          <ArchiveRestore size={15} />
                        </button>
                        <button
                          className="ghost-icon danger"
                          type="button"
                          onClick={() => void permanentDeleteNote(note.id)}
                          title="Delete forever"
                        >
                          <X size={15} />
                        </button>
                      </div>
                    }
                  />
                ))}
              </TreeSection>
            ) : null}

            <div className="sidebar-divider" role="separator" />

            <section className="meetings-panel" aria-label="Upcoming meetings">
              <div className="meetings-header">
                <div>
                  <CalendarDays size={15} />
                  <span>Meetings</span>
                </div>
                <button
                  type="button"
                  onClick={() => void refreshCalendarEvents()}
                  disabled={isCalendarRefreshing}
                  title="Refresh meetings"
                  aria-label="Refresh meetings"
                >
                  <RefreshCw
                    className={isCalendarRefreshing ? "spin" : ""}
                    size={14}
                  />
                </button>
              </div>

              {calendarConfig?.has_access_token ||
              calendarConfig?.has_refresh_token ? (
                calendarEvents.length > 0 ? (
                  <div className="meetings-list">
                    {calendarEvents.map((event) => {
                      const actionable = isCalendarEventActionable(
                        event,
                        calendarNow,
                      );
                      return (
                        <div
                          key={`${event.calendar_id}:${event.id}`}
                          className={
                            actionable ? "meeting-row actionable" : "meeting-row"
                          }
                        >
                          <div>
                            <strong>{event.title}</strong>
                            <span>{formatCalendarEventTime(event)}</span>
                          </div>
                          <small>{event.calendar_name}</small>
                        </div>
                      );
                    })}
                  </div>
                ) : (
                  <p className="meetings-empty">
                    {isCalendarRefreshing
                      ? "Loading meetings"
                      : "No upcoming meetings"}
                  </p>
                )
              ) : (
                <p className="meetings-empty">Connect Calendar in Settings</p>
              )}

              {calendarError ? (
                <p className="meetings-error">{calendarError}</p>
              ) : null}
            </section>
          </div>

          <div className="sidebar-footer">
            <button
              className={
                view === "reminders" ? "icon-button active" : "icon-button"
              }
              type="button"
              onClick={() =>
                setView((current) =>
                  current === "reminders" ? "notes" : "reminders",
                )
              }
              title={view === "reminders" ? "Back to notes" : "Reminders"}
            >
              <Bell size={18} />
            </button>
            <button
              className={
                view === "agents" ? "icon-button active" : "icon-button"
              }
              type="button"
              onClick={() =>
                setView((current) => (current === "agents" ? "notes" : "agents"))
              }
              title={view === "agents" ? "Back to notes" : "Agents"}
            >
              <Bot size={18} />
            </button>
            <button
              className={
                view === "settings" ? "icon-button active" : "icon-button"
              }
              type="button"
              onClick={() => setView("settings")}
              title="Settings"
            >
              <Settings size={18} />
            </button>
            <button
              className="icon-button"
              type="button"
              onClick={cycleTheme}
              title={`Theme: ${theme}`}
            >
              <ThemeIcon size={18} />
            </button>
          </div>
        </aside>

        <section className="workspace">
          {view === "settings" ? (
            <SettingsView
              onCalendarChanged={() => void refreshCalendarEvents()}
              onClose={() => setView("notes")}
            />
          ) : view === "agents" ? (
            <AgentsView
              notes={snapshot.notes}
              currentNoteId={activeNote?.id ?? null}
              onClose={() => setView("notes")}
            />
          ) : view === "reminders" ? (
            <RemindersView
              onClose={() => setView("notes")}
              onOpen={(reminder) => void openReminder(reminder)}
            />
          ) : (
            <div
              className={
                shouldShowContextPanel
                  ? "notes-workspace with-panel"
                  : "notes-workspace"
              }
              style={
                shouldShowContextPanel
                  ? { gridTemplateColumns: `minmax(0, 1fr) ${panelWidth}px` }
                  : undefined
              }
            >
              <NoteEditor
                key={`${activeNote?.id ?? "none"}:${editorReloadKey}`}
                note={activeNote}
                folders={snapshot.folders}
                panelOpen={shouldShowContextPanel}
                externalRevision={meetingContentRevision}
                externalNoteId={meetingNoteId}
                entityJumpTarget={entityJumpTarget}
                reminderJumpTarget={reminderJumpTarget}
                onDismissReminderTarget={() => setReminderJumpTarget(null)}
                onTogglePanel={() => setPanelOpen((open) => !open)}
                onCreate={createNote}
                onSave={saveNote}
                onTrash={trashNote}
                onRestore={restoreNote}
                onPermanentDelete={permanentDeleteNote}
                onMove={moveNote}
                onExport={exportNote}
                onCreateGmailDraft={createGmailDraft}
              />
              {activeNote && shouldShowContextPanel ? (
                <>
                  <ResizeHandle
                    side="right"
                    ariaLabel="Resize details panel"
                    style={{ right: panelWidth }}
                    onResize={resizePanel}
                  />
                  <ContextPanel
                    note={activeNote}
                    notes={snapshot.notes}
                    linkedNotes={linkedNotes}
                    linkSuggestions={linkSuggestions}
                    onLinkNote={linkNote}
                    onOpenNote={openNote}
                    onLinkSuggestion={linkSuggestedNote}
                    onExtractionStatusChange={updateNoteExtractionStatus}
                    onCreateNoteFromContent={createNoteFromContent}
                    onRenameSpeaker={(oldName, newName) =>
                      void renameSpeaker(oldName, newName)
                    }
                    onJumpToEntityMention={(surfaceText) =>
                      setEntityJumpTarget({
                        nonce: Date.now(),
                        surfaceText,
                      })
                    }
                    onRenameLink={renameNoteLink}
                    onUnlink={unlinkNotes}
                  />
                </>
              ) : null}
            </div>
          )}
        </section>
        <ResizeHandle
          side="left"
          ariaLabel="Resize sidebar"
          style={{ left: sidebarWidth }}
          onResize={resizeSidebar}
        />
      </div>

      {paletteOpen ? (
        <CommandPalette
          notes={paletteNotes}
          onClose={() => setPaletteOpen(false)}
          onOpenNote={(id) => void openNote(id)}
          onCreateNote={() => void createNote()}
          onOpenSettings={() => setView("settings")}
          onToggleTheme={cycleTheme}
        />
      ) : null}

      {menuTarget ? (
        <NoteContextMenu
          target={menuTarget}
          folders={snapshot.folders}
          canLink={selectedIds.length >= 2}
          onClose={() => setMenuTarget(null)}
          onOpen={(id) => void openNote(id)}
          onMove={(id, folderId) => void moveNoteToFolder(id, folderId)}
          onLink={() => void linkSelectedNotes()}
          onTrash={(id) => void trashNote(id)}
          onRestore={(id) => void restoreNote(id)}
          onDelete={(id) => void permanentDeleteNote(id)}
        />
      ) : null}

      {meetingNotesOpen && meetingState !== "idle" ? (
        <MeetingQuickNotesPanel
          noteCreated={Boolean(meetingQuickNoteId)}
          saveState={meetingQuickNoteSaveState}
          value={meetingQuickNotes}
          onChange={updateMeetingQuickNotes}
          onClose={() => setMeetingNotesOpen(false)}
        />
      ) : null}

      <ReminderCenter onOpenReminders={() => setView("reminders")} />

      <MeetingCapsule
        detail={meetingDetail}
        noteTitle={meetingNoteTitle}
        visualSourceName={meetingVisualSourceName}
        state={meetingState}
        calendarCueTitle={actionableCalendarEvent?.title ?? null}
        micLabel={meetingMicLabel}
        systemLabel={meetingSystemLabel}
        notesOpen={meetingNotesOpen}
        onMicLabelChange={setMeetingMicLabel}
        onSystemLabelChange={setMeetingSystemLabel}
        onToggleNotes={() => setMeetingNotesOpen((open) => !open)}
        onPause={() => void pauseMeetingMode()}
        onResume={() => void resumeMeetingMode()}
        onStart={() => void startMeetingMode()}
        onStop={() => void stopMeetingMode()}
      />

      {diarizationPrompt ? (
        <DiarizationSpeakerPromptBox
          existingNames={existingDiarizedSpeakerNames(
            diarizationPrompt.speakerId,
          )}
          prompt={diarizationPrompt}
          onDismiss={dismissDiarizationPrompt}
          onRename={(speakerId, name) =>
            void renameDiarizedSpeaker(speakerId, name)
          }
        />
      ) : null}

      {meetingSourcePickerOpen ? (
        <MeetingSourcePicker
          sources={meetingVisualSources}
          onCancel={cancelMeetingSourcePicker}
          onSelect={(source) => void beginMeetingMode(source.id, source.name)}
          onTranscriptOnly={() => void beginMeetingMode(null, null)}
        />
      ) : null}

      <ToastViewport />
    </div>
  );
}

function MeetingQuickNotesPanel({
  noteCreated,
  saveState,
  value,
  onChange,
  onClose,
}: {
  noteCreated: boolean;
  saveState: SaveState;
  value: string;
  onChange: (value: string) => void;
  onClose: () => void;
}) {
  const status = !noteCreated
    ? "Draft"
    : saveState === "saving"
      ? "Saving"
      : saveState === "error"
        ? "Save failed"
        : "Saved";

  return (
    <section className="meeting-quick-notes" aria-label="Meeting notes">
      <header>
        <div>
          <NotebookPen size={15} />
          <strong>Meeting notes</strong>
        </div>
        <span className={saveState === "error" ? "error" : ""}>{status}</span>
        <button type="button" onClick={onClose} title="Close meeting notes">
          <X size={15} />
        </button>
      </header>
      <textarea
        value={value}
        onChange={(event) => onChange(event.currentTarget.value)}
        placeholder={"# To do\n\n# Agreed next steps"}
        autoFocus
      />
    </section>
  );
}

type MeetingCapsuleProps = {
  detail: string;
  noteTitle: string;
  visualSourceName: string | null;
  state: MeetingState;
  calendarCueTitle: string | null;
  micLabel: string;
  systemLabel: string;
  notesOpen: boolean;
  onMicLabelChange: (value: string) => void;
  onSystemLabelChange: (value: string) => void;
  onToggleNotes: () => void;
  onPause: () => void;
  onResume: () => void;
  onStart: () => void;
  onStop: () => void;
};

function MeetingCapsule({
  detail,
  noteTitle,
  visualSourceName,
  state,
  calendarCueTitle,
  micLabel,
  systemLabel,
  notesOpen,
  onMicLabelChange,
  onSystemLabelChange,
  onToggleNotes,
  onPause,
  onResume,
  onStart,
  onStop,
}: MeetingCapsuleProps) {
  const isBusy = state === "starting" || state === "stopping";
  const isActive = state === "recording";
  const isPaused = state === "paused";
  const isCalendarReady = state === "idle" && Boolean(calendarCueTitle);

  return (
    <div
      className={`meeting-capsule ${state}${isCalendarReady ? " calendar-ready" : ""}`}
    >
      <div className="meeting-capsule-status">
        <span className="meeting-dot" aria-hidden="true" />
        <div>
          <strong>
            {isActive || isPaused || isBusy
              ? noteTitle || "Meeting"
              : calendarCueTitle || "Meeting mode"}
          </strong>
          <small>
            {isCalendarReady ? "Ready to start" : isBusy ? state : detail}
            {visualSourceName ? ` · ${visualSourceName}` : ""}
          </small>
        </div>
      </div>

      {state === "idle" ? (
        <div className="meeting-capsule-labels">
          <input
            value={micLabel}
            onChange={(event) => onMicLabelChange(event.currentTarget.value)}
            placeholder="You"
            title="Your name (labels your microphone)"
            aria-label="Your name"
          />
          <input
            value={systemLabel}
            onChange={(event) => onSystemLabelChange(event.currentTarget.value)}
            placeholder="Participants"
            title="Other participants (labels system audio)"
            aria-label="Other participants"
          />
        </div>
      ) : null}
      <div className="meeting-capsule-actions">
        {state !== "idle" ? (
          <button
            className={notesOpen ? "active" : ""}
            type="button"
            onClick={onToggleNotes}
            title="Meeting notes"
          >
            <NotebookPen size={15} />
            Notes
          </button>
        ) : null}
        {state === "idle" ? (
          <button type="button" onClick={onStart} title="Start meeting mode">
            <Play size={15} />
            Start
          </button>
        ) : isPaused ? (
          <button
            type="button"
            onClick={onResume}
            disabled={isBusy}
            title="Resume meeting mode"
          >
            <Play size={15} />
            Resume
          </button>
        ) : (
          <button
            type="button"
            onClick={onPause}
            disabled={!isActive}
            title="Pause meeting mode"
          >
            <Pause size={15} />
            Pause
          </button>
        )}
        {state !== "idle" ? (
          <button
            className="danger"
            type="button"
            onClick={onStop}
            disabled={isBusy}
            title="Stop meeting mode"
          >
            <Square size={14} />
            Stop
          </button>
        ) : null}
      </div>
    </div>
  );
}

function DiarizationSpeakerPromptBox({
  existingNames,
  prompt,
  onDismiss,
  onRename,
}: {
  existingNames: string[];
  prompt: DiarizationSpeakerPrompt;
  onDismiss: () => void;
  onRename: (speakerId: string, name: string) => void;
}) {
  const [value, setValue] = useState(prompt.fallbackName);

  useEffect(() => {
    setValue(prompt.fallbackName);
  }, [prompt]);

  function commit() {
    const next = value.trim();
    if (next && next !== prompt.fallbackName) {
      onRename(prompt.speakerId, next);
    } else {
      onDismiss();
    }
  }

  return (
    <section
      className="diarization-prompt"
      aria-label="Name newly detected speaker"
    >
      <div className="diarization-prompt-copy">
        <strong>New speaker detected</strong>
        <small>{prompt.fallbackName}</small>
      </div>
      <input
        value={value}
        onChange={(event) => setValue(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            commit();
          } else if (event.key === "Escape") {
            event.preventDefault();
            onDismiss();
          }
        }}
        aria-label={`Name ${prompt.fallbackName}`}
        autoFocus
      />
      {existingNames.length > 0 ? (
        <div className="diarization-existing-speakers">
          <small>Existing</small>
          <div>
            {existingNames.map((name) => (
              <button
                key={name}
                type="button"
                onClick={() => onRename(prompt.speakerId, name)}
                title={`Use existing speaker ${name}`}
              >
                {name}
              </button>
            ))}
          </div>
        </div>
      ) : null}
      <button type="button" onClick={commit}>
        Save
      </button>
      <button type="button" className="ghost" onClick={onDismiss}>
        Later
      </button>
    </section>
  );
}

type MeetingSourcePickerProps = {
  sources: MeetingVisualSource[];
  onCancel: () => void;
  onSelect: (source: MeetingVisualSource) => void;
  onTranscriptOnly: () => void;
};

function MeetingSourcePicker({
  sources,
  onCancel,
  onSelect,
  onTranscriptOnly,
}: MeetingSourcePickerProps) {
  return (
    <div
      className="meeting-source-backdrop"
      role="presentation"
      onMouseDown={onCancel}
    >
      <section
        className="meeting-source-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="meeting-source-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header>
          <div>
            <h2 id="meeting-source-title">Meeting capture</h2>
            <p>Choose what should be captured as periodic visual snapshots.</p>
          </div>
          <button
            type="button"
            className="icon-button"
            onClick={onCancel}
            title="Close"
          >
            <X size={18} />
          </button>
        </header>

        <div className="meeting-source-list">
          {sources.length === 0 ? (
            <p className="meeting-source-empty">
              No visual sources are available right now.
            </p>
          ) : (
            sources.map((source) => (
              <button
                type="button"
                key={source.id}
                onClick={() => onSelect(source)}
              >
                <span className="meeting-source-icon">
                  <Monitor size={18} />
                </span>
                <span>
                  <strong>{source.name}</strong>
                  <small>
                    {source.kind === "display"
                      ? "Display"
                      : (source.app_name ?? "Window")}
                    {source.width && source.height
                      ? ` · ${source.width}x${source.height}`
                      : ""}
                  </small>
                </span>
              </button>
            ))
          )}
        </div>

        <footer>
          <button type="button" onClick={onTranscriptOnly}>
            Transcript only
          </button>
          <button type="button" onClick={onCancel}>
            Cancel
          </button>
        </footer>
      </section>
    </div>
  );
}

// Each sub-section is its own rail item so the pane shows exactly one section
// at a time — minimal scrolling, direct navigation (macOS System-Settings style).
type SettingsSection =
  | "ai-local"
  | "ai-remote"
  | "ai-settings"
  | "audio-capture"
  | "system-audio"
  | "speech-to-text"
  | "entity-interests"
  | "extraction-queue"
  | "mcp"
  | "slack"
  | "google"
  | "agent-tools";

const SETTINGS_NAV: {
  group: string;
  icon: typeof Mic;
  items: { id: SettingsSection; label: string }[];
}[] = [
  {
    group: "AI",
    icon: Bot,
    items: [
      { id: "ai-local", label: "Local AI" },
      { id: "ai-remote", label: "Remote AI" },
      { id: "ai-settings", label: "Settings" },
    ],
  },
  {
    group: "Audio & voice",
    icon: Mic,
    items: [
      { id: "audio-capture", label: "Audio capture" },
      { id: "system-audio", label: "System audio" },
      { id: "speech-to-text", label: "Speech to text" },
    ],
  },
  {
    group: "Knowledge",
    icon: Database,
    items: [
      { id: "entity-interests", label: "Entity interests" },
      { id: "extraction-queue", label: "Extraction queue" },
    ],
  },
  {
    group: "Integrations",
    icon: CalendarDays,
    items: [
      { id: "mcp", label: "MCP servers" },
      { id: "slack", label: "Slack" },
      { id: "google", label: "Google" },
    ],
  },
  {
    group: "Developer",
    icon: Wrench,
    items: [{ id: "agent-tools", label: "Agent tools" }],
  },
];

type SettingsViewProps = {
  onCalendarChanged: () => void;
  onClose: () => void;
};

function SettingsView({ onCalendarChanged, onClose }: SettingsViewProps) {
  const [queueStatus, setQueueStatus] = useState<ExtractionQueueStatus | null>(
    null,
  );
  const [audioStatus, setAudioStatus] = useState<AudioCaptureStatus | null>(
    null,
  );
  const [systemAudioStatus, setSystemAudioStatus] =
    useState<SystemAudioPermissionStatus | null>(null);
  const [systemCaptureStatus, setSystemCaptureStatus] =
    useState<SystemAudioCaptureStatus | null>(null);
  const [sttConfig, setSttConfig] = useState<SttConfig>({
    model_path: "",
    language: "en",
    threads: 4,
  });
  const [gmailConfig, setGmailConfig] = useState<GmailConfig>({
    client_id: "",
    client_secret: "",
    has_access_token: false,
    has_refresh_token: false,
    access_token_expires_at: null,
  });
  const [calendarConfig, setCalendarConfig] = useState<CalendarConfig>({
    client_id: "",
    client_secret: "",
    has_access_token: false,
    has_refresh_token: false,
    access_token_expires_at: null,
  });
  const [agentTools, setAgentTools] = useState<AgentToolDescriptor[]>([]);
  const [entityInterests, setEntityInterests] = useState<
    EntityInterestDefinition[]
  >([]);
  const [selectedAgentTool, setSelectedAgentTool] = useState("ping");
  const [agentToolInput, setAgentToolInput] = useState(
    defaultAgentToolInput("ping"),
  );
  const [agentToolOutput, setAgentToolOutput] = useState<string | null>(null);
  const [agentPrompt, setAgentPrompt] = useState(
    "Search for notes about Curvo and summarize what you find.",
  );
  const [agentRunResult, setAgentRunResult] = useState<AgentRunResult | null>(
    null,
  );
  const [sttStatus, setSttStatus] = useState<SttStatus | null>(null);
  const [sttQueueStatus, setSttQueueStatus] =
    useState<SttQueueStatus | null>(null);
  const [transcription, setTranscription] = useState<SttTranscription | null>(
    null,
  );
  const [isLoading, setIsLoading] = useState(true);
  const [isQueueBusy, setIsQueueBusy] = useState(false);
  const [isAudioBusy, setIsAudioBusy] = useState(false);
  const [isSystemAudioBusy, setIsSystemAudioBusy] = useState(false);
  const [isSystemCaptureBusy, setIsSystemCaptureBusy] = useState(false);
  const [isSttBusy, setIsSttBusy] = useState(false);
  const [isGmailBusy, setIsGmailBusy] = useState(false);
  const [isCalendarBusy, setIsCalendarBusy] = useState(false);
  const [isEntityInterestBusy, setIsEntityInterestBusy] = useState(false);
  const [isAgentToolBusy, setIsAgentToolBusy] = useState(false);
  const [isAgentRunBusy, setIsAgentRunBusy] = useState(false);
  const [settingsSection, setSettingsSection] =
    useState<SettingsSection>("ai-local");
  const setSettingsError = useCallback((message: string | null) => {
    if (message) {
      toast.error(message);
    }
  }, []);

  const selectedAgentToolDescriptor = useMemo(
    () => agentTools.find((tool) => tool.name === selectedAgentTool) ?? null,
    [agentTools, selectedAgentTool],
  );

  const refreshAudioStatus = useCallback(async () => {
    const nextAudioStatus = await invoke<AudioCaptureStatus>(
      "get_audio_capture_status",
    );
    setAudioStatus(nextAudioStatus);
    return nextAudioStatus;
  }, []);

  const refreshSystemCaptureStatus = useCallback(async () => {
    const nextStatus = await invoke<SystemAudioCaptureStatus>(
      "get_system_audio_capture_status",
    );
    setSystemCaptureStatus(nextStatus);
    return nextStatus;
  }, []);

  const refreshSttStatus = useCallback(async () => {
    const nextStatus = await invoke<SttStatus>("get_stt_status");
    setSttStatus(nextStatus);
    return nextStatus;
  }, []);

  const refreshSttQueueStatus = useCallback(async () => {
    const nextStatus = await invoke<SttQueueStatus>("get_stt_queue_status");
    setSttQueueStatus(nextStatus);
    return nextStatus;
  }, []);

  const refreshQueueStatus = useCallback(async () => {
    const nextQueueStatus = await invoke<ExtractionQueueStatus>(
      "get_extraction_queue_status",
    );
    setQueueStatus(nextQueueStatus);
    return nextQueueStatus;
  }, []);

  useEffect(() => {
    Promise.all([
      refreshQueueStatus(),
      refreshAudioStatus(),
      refreshSystemCaptureStatus(),
      invoke<SttConfig>("get_stt_config").then((savedConfig) => {
        setSttConfig(savedConfig);
        return refreshSttStatus();
      }),
      refreshSttQueueStatus(),
      invoke<GmailConfig>("get_gmail_config").then(setGmailConfig),
      invoke<CalendarConfig>("get_calendar_config").then(setCalendarConfig),
      invoke<EntityInterestDefinition[]>("get_entity_interests").then(
        setEntityInterests,
      ),
      invoke<AgentToolDescriptor[]>("agent_list_tools").then((tools) => {
        const sortedTools = [...tools].sort((left, right) =>
          left.name.localeCompare(right.name),
        );
        setAgentTools(sortedTools);
        setSelectedAgentTool((current) => {
          if (current && sortedTools.some((tool) => tool.name === current)) {
            return current;
          }
          const nextTool = sortedTools[0]?.name ?? "";
          setAgentToolInput(defaultAgentToolInput(nextTool));
          return nextTool;
        });
      }),
    ])
      .catch((loadError) => setSettingsError(String(loadError)))
      .finally(() => setIsLoading(false));
  }, [
    refreshAudioStatus,
    refreshQueueStatus,
    refreshSttQueueStatus,
    refreshSttStatus,
    refreshSystemCaptureStatus,
  ]);

  useEffect(() => {
    const interval = window.setInterval(() => {
      void refreshQueueStatus();
      void refreshSttQueueStatus();
    }, 3000);
    return () => window.clearInterval(interval);
  }, [refreshQueueStatus, refreshSttQueueStatus]);

  useEffect(() => {
    if (!audioStatus?.is_recording) {
      return undefined;
    }

    const interval = window.setInterval(() => {
      void refreshAudioStatus();
    }, 500);
    return () => window.clearInterval(interval);
  }, [audioStatus?.is_recording, refreshAudioStatus]);

  useEffect(() => {
    if (!systemCaptureStatus?.is_recording) {
      return undefined;
    }

    const interval = window.setInterval(() => {
      void refreshSystemCaptureStatus();
    }, 500);
    return () => window.clearInterval(interval);
  }, [refreshSystemCaptureStatus, systemCaptureStatus?.is_recording]);

  async function queueAllNotes() {
    setIsQueueBusy(true);
    setSettingsError(null);
    try {
      const nextQueueStatus = await invoke<ExtractionQueueStatus>(
        "enqueue_all_note_extractions",
      );
      setQueueStatus(nextQueueStatus);
    } catch (queueError) {
      setSettingsError(String(queueError));
    } finally {
      setIsQueueBusy(false);
    }
  }

  async function retryFailedJobs() {
    setIsQueueBusy(true);
    setSettingsError(null);
    try {
      const nextQueueStatus = await invoke<ExtractionQueueStatus>(
        "retry_failed_extractions",
      );
      setQueueStatus(nextQueueStatus);
    } catch (queueError) {
      setSettingsError(String(queueError));
    } finally {
      setIsQueueBusy(false);
    }
  }

  async function startAudioCapture() {
    setIsAudioBusy(true);
    setSettingsError(null);
    try {
      const nextAudioStatus = await invoke<AudioCaptureStatus>(
        "start_audio_capture",
      );
      setAudioStatus(nextAudioStatus);
    } catch (audioError) {
      setSettingsError(String(audioError));
    } finally {
      setIsAudioBusy(false);
    }
  }

  async function stopAudioCapture() {
    setIsAudioBusy(true);
    setSettingsError(null);
    try {
      const nextAudioStatus =
        await invoke<AudioCaptureStatus>("stop_audio_capture");
      setAudioStatus(nextAudioStatus);
    } catch (audioError) {
      setSettingsError(String(audioError));
    } finally {
      setIsAudioBusy(false);
    }
  }

  async function checkSystemAudioPermission() {
    setIsSystemAudioBusy(true);
    setSettingsError(null);
    try {
      const nextStatus = await invoke<SystemAudioPermissionStatus>(
        "check_system_audio_permission",
      );
      setSystemAudioStatus(nextStatus);
    } catch (systemAudioError) {
      setSettingsError(String(systemAudioError));
    } finally {
      setIsSystemAudioBusy(false);
    }
  }

  async function startSystemAudioCapture() {
    setIsSystemCaptureBusy(true);
    setSettingsError(null);
    try {
      const nextStatus = await invoke<SystemAudioCaptureStatus>(
        "start_system_audio_capture",
      );
      setSystemCaptureStatus(nextStatus);
    } catch (systemAudioError) {
      setSettingsError(String(systemAudioError));
      await refreshSystemCaptureStatus();
    } finally {
      setIsSystemCaptureBusy(false);
    }
  }

  async function stopSystemAudioCapture() {
    setIsSystemCaptureBusy(true);
    setSettingsError(null);
    try {
      const nextStatus = await invoke<SystemAudioCaptureStatus>(
        "stop_system_audio_capture",
      );
      setSystemCaptureStatus(nextStatus);
    } catch (systemAudioError) {
      setSettingsError(String(systemAudioError));
      await refreshSystemCaptureStatus();
    } finally {
      setIsSystemCaptureBusy(false);
    }
  }

  async function saveAndCheckStt() {
    setIsSttBusy(true);
    setSettingsError(null);
    try {
      const savedConfig = await invoke<SttConfig>("save_stt_config", {
        config: sttConfig,
      });
      setSttConfig(savedConfig);
      const nextStatus = await invoke<SttStatus>("get_stt_status");
      setSttStatus(nextStatus);
    } catch (sttError) {
      setSettingsError(String(sttError));
    } finally {
      setIsSttBusy(false);
    }
  }

  async function transcribeLastCapture() {
    setIsSttBusy(true);
    setSettingsError(null);
    try {
      const savedConfig = await invoke<SttConfig>("save_stt_config", {
        config: sttConfig,
      });
      setSttConfig(savedConfig);
      const result = await invoke<SttTranscription>("transcribe_last_capture");
      setTranscription(result);
      await refreshSttStatus();
    } catch (sttError) {
      setSettingsError(String(sttError));
    } finally {
      setIsSttBusy(false);
    }
  }

  async function saveGmailConfig() {
    setIsGmailBusy(true);
    setSettingsError(null);
    try {
      const savedConfig = await invoke<GmailConfig>("save_gmail_config", {
        config: gmailConfig,
      });
      setGmailConfig(savedConfig);
      const nextCalendarConfig =
        await invoke<CalendarConfig>("get_calendar_config");
      setCalendarConfig(nextCalendarConfig);
      toast.success("Gmail OAuth settings saved");
    } catch (gmailError) {
      setSettingsError(String(gmailError));
    } finally {
      setIsGmailBusy(false);
    }
  }

  async function connectGmail() {
    setIsGmailBusy(true);
    setSettingsError(null);
    try {
      const savedConfig = await invoke<GmailConfig>("save_gmail_config", {
        config: gmailConfig,
      });
      setGmailConfig(savedConfig);
      const { signIn } =
        await import("@choochmeque/tauri-plugin-google-auth-api");
      const tokens = await signIn({
        clientId: savedConfig.client_id,
        clientSecret: savedConfig.client_secret,
        scopes: [GMAIL_DRAFT_SCOPE],
        successHtmlResponse:
          "<h1>Gmail connected</h1><p>You can close this window and return to Smooth.</p>",
      });
      const nextConfig = await saveGmailTokens(tokens);
      setGmailConfig(nextConfig);
      toast.success("Gmail connected for draft creation");
    } catch (gmailError) {
      setSettingsError(String(gmailError));
    } finally {
      setIsGmailBusy(false);
    }
  }

  async function saveGmailTokens(tokens: TokenResponse) {
    return invoke<GmailConfig>("save_gmail_tokens", {
      tokens: {
        access_token: tokens.accessToken,
        refresh_token: tokens.refreshToken ?? null,
        expires_at: tokens.expiresAt ?? null,
      } satisfies GmailTokenPayload,
    });
  }

  async function disconnectGmail() {
    setIsGmailBusy(true);
    setSettingsError(null);
    try {
      if (gmailConfig.has_access_token) {
        const { signOut } =
          await import("@choochmeque/tauri-plugin-google-auth-api");
        await signOut().catch(() => undefined);
      }
      const nextConfig = await invoke<GmailConfig>("clear_gmail_auth");
      setGmailConfig(nextConfig);
      toast.success("Gmail disconnected");
    } catch (gmailError) {
      setSettingsError(String(gmailError));
    } finally {
      setIsGmailBusy(false);
    }
  }

  async function connectCalendar() {
    setIsCalendarBusy(true);
    setSettingsError(null);
    try {
      const savedConfig = await invoke<GmailConfig>("save_gmail_config", {
        config: gmailConfig,
      });
      setGmailConfig(savedConfig);
      const { signIn } =
        await import("@choochmeque/tauri-plugin-google-auth-api");
      const tokens = await signIn({
        clientId: savedConfig.client_id,
        clientSecret: savedConfig.client_secret,
        scopes: [CALENDAR_READONLY_SCOPE],
        successHtmlResponse:
          "<h1>Calendar connected</h1><p>You can close this window and return to Smooth.</p>",
      });
      const nextConfig = await saveCalendarTokens(tokens);
      setCalendarConfig(nextConfig);
      onCalendarChanged();
      toast.success("Calendar connected");
    } catch (calendarError) {
      setSettingsError(String(calendarError));
    } finally {
      setIsCalendarBusy(false);
    }
  }

  async function saveCalendarTokens(tokens: TokenResponse) {
    return invoke<CalendarConfig>("save_calendar_tokens", {
      tokens: {
        access_token: tokens.accessToken,
        refresh_token: tokens.refreshToken ?? null,
        expires_at: tokens.expiresAt ?? null,
      } satisfies CalendarTokenPayload,
    });
  }

  async function disconnectCalendar() {
    setIsCalendarBusy(true);
    setSettingsError(null);
    try {
      const nextConfig = await invoke<CalendarConfig>("clear_calendar_auth");
      setCalendarConfig(nextConfig);
      onCalendarChanged();
      toast.success("Calendar disconnected");
    } catch (calendarError) {
      setSettingsError(String(calendarError));
    } finally {
      setIsCalendarBusy(false);
    }
  }

  function updateEntityInterest(
    index: number,
    patch: Partial<EntityInterestDefinition>,
  ) {
    setEntityInterests((current) =>
      current.map((interest, currentIndex) =>
        currentIndex === index ? { ...interest, ...patch } : interest,
      ),
    );
  }

  function addEntityInterest() {
    setEntityInterests((current) => [
      ...current,
      {
        id: null,
        name: "New interest",
        description: "",
        enabled: true,
        sort_order: current.length,
      },
    ]);
  }

  function removeEntityInterest(index: number) {
    setEntityInterests((current) =>
      current.filter((_, currentIndex) => currentIndex !== index),
    );
  }

  async function saveEntityInterests() {
    setIsEntityInterestBusy(true);
    setSettingsError(null);
    try {
      const savedInterests = await invoke<EntityInterestDefinition[]>(
        "save_entity_interests",
        {
          interests: entityInterests.map((interest, index) => ({
            ...interest,
            sort_order: index,
          })),
        },
      );
      setEntityInterests(savedInterests);
      toast.success("Entity interests saved");
    } catch (interestError) {
      setSettingsError(String(interestError));
    } finally {
      setIsEntityInterestBusy(false);
    }
  }

  async function runAgentTool() {
    if (!selectedAgentTool) {
      setSettingsError("Choose an agent tool first");
      return;
    }

    setIsAgentToolBusy(true);
    setSettingsError(null);
    try {
      const input = agentToolInput.trim() ? JSON.parse(agentToolInput) : {};
      const result = await invoke<unknown>("agent_execute_tool", {
        tool: selectedAgentTool,
        input,
      });
      setAgentToolOutput(JSON.stringify(result, null, 2));
    } catch (agentToolError) {
      const message =
        agentToolError instanceof SyntaxError
          ? `Invalid JSON input: ${agentToolError.message}`
          : String(agentToolError);
      setAgentToolOutput(message);
      setSettingsError(message);
    } finally {
      setIsAgentToolBusy(false);
    }
  }

  async function runAgentFlow() {
    setIsAgentRunBusy(true);
    setSettingsError(null);
    try {
      const result = await invoke<AgentRunResult>("agent_run", {
        prompt: agentPrompt,
        maxSteps: 5,
        selection: null,
      });
      setAgentRunResult(result);
    } catch (agentError) {
      const message = String(agentError);
      setAgentRunResult({
        model: "",
        run_id: "",
        answer: message,
        steps: [],
        raw_model_output: "",
      });
      setSettingsError(message);
    } finally {
      setIsAgentRunBusy(false);
    }
  }

  const SttStatusIcon =
    sttStatus?.state === "ready" ? CheckCircle2 : CircleAlert;
  const audioPreviewUrl = audioStatus?.last_preview
    ? convertFileSrc(audioStatus.last_preview.path)
    : null;
  const systemAudioPreviewUrl = systemCaptureStatus?.last_preview
    ? convertFileSrc(systemCaptureStatus.last_preview.path)
    : null;

  return (
    <div className="settings-view">
      <header className="settings-header" data-tauri-drag-region>
        <div>
          <p className="eyebrow">Settings</p>
          <h2>Settings</h2>
        </div>
        <div className="settings-header-actions">
          <button
            className="icon-button"
            type="button"
            onClick={onClose}
            title="Back to notes"
          >
            <ArrowLeft size={17} />
          </button>
          <button
            className="icon-button"
            type="button"
            onClick={() => {
              void refreshQueueStatus();
              void refreshAudioStatus();
              void refreshSystemCaptureStatus();
              void refreshSttStatus();
            }}
            disabled={isLoading}
            title="Refresh connection status"
          >
            <RefreshCw size={17} />
          </button>
        </div>
      </header>

      <div className="settings-body">
        <nav className="settings-rail" aria-label="Settings sections">
          {SETTINGS_NAV.map((group) => {
            const GroupIcon = group.icon;
            return (
              <div className="settings-rail-group" key={group.group}>
                <p className="settings-rail-label">
                  <GroupIcon size={13} />
                  {group.group}
                </p>
                {group.items.map((item) => (
                  <button
                    key={item.id}
                    type="button"
                    className={settingsSection === item.id ? "active" : ""}
                    onClick={() => setSettingsSection(item.id)}
                  >
                    {item.label}
                  </button>
                ))}
              </div>
            );
          })}
        </nav>

        <div className="settings-panes">
          {settingsSection === "mcp" ? <McpSettings /> : null}

          {settingsSection === "slack" ? <SlackSettings /> : null}

      <section
        className="settings-section"
        hidden={settingsSection !== "audio-capture"}
      >
        <div className="section-heading">
          <Mic size={18} />
          <span>Audio capture</span>
          <small>{audioStatus?.is_recording ? "Recording" : "Idle"}</small>
        </div>

        <div
          className={
            audioStatus?.is_recording
              ? "audio-capture-card recording"
              : "audio-capture-card"
          }
        >
          <div className="audio-capture-meter" aria-hidden="true">
            <Mic size={19} />
          </div>
          <div className="audio-capture-copy">
            <strong>{audioStatus?.device_name ?? "Default microphone"}</strong>
            <span>
              {audioStatus?.is_recording
                ? `${formatDuration(audioStatus.elapsed_ms)} · ${audioStatus.captured_samples.toLocaleString()} samples`
                : audioStatus?.last_preview
                  ? `${formatDuration(audioStatus.last_preview.duration_ms)} · ${audioStatus.last_preview.sample_rate.toLocaleString()} Hz`
                  : "Ready"}
            </span>
          </div>
          <div className="audio-capture-actions">
            <button
              type="button"
              onClick={() => void startAudioCapture()}
              disabled={isAudioBusy || audioStatus?.is_recording}
            >
              <Mic size={15} />
              Start
            </button>
            <button
              type="button"
              onClick={() => void stopAudioCapture()}
              disabled={isAudioBusy || !audioStatus?.is_recording}
            >
              <Square size={14} />
              Stop
            </button>
          </div>
        </div>

        {audioPreviewUrl && audioStatus?.last_preview ? (
          <div className="audio-preview">
            <div>
              <Play size={15} />
              <span>Last capture</span>
              <small>
                {formatDuration(audioStatus.last_preview.duration_ms)}
              </small>
            </div>
            <audio controls src={audioPreviewUrl} />
          </div>
        ) : null}
      </section>

      <section
        className="settings-section"
        hidden={settingsSection !== "system-audio"}
      >
        <div className="section-heading">
          <Monitor size={18} />
          <span>System audio</span>
          <small>
            {systemCaptureStatus?.is_recording
              ? "Recording"
              : systemAudioStatus
                ? systemAudioStatus.granted
                  ? "Available"
                  : "Permission needed"
                : "ScreenCaptureKit"}
          </small>
        </div>

        <div
          className={`connection-status ${systemAudioStatus?.granted ? "ready" : "offline"}`}
        >
          {systemAudioStatus?.granted ? (
            <CheckCircle2 size={19} />
          ) : (
            <CircleAlert size={19} />
          )}
          <div>
            <strong>
              {systemAudioStatus
                ? systemAudioStatus.granted
                  ? "ready"
                  : "not allowed"
                : "not checked"}
            </strong>
            <span>
              {systemAudioStatus?.message ??
                "Check macOS Screen & System Audio Recording access before capture."}
            </span>
          </div>
          <small>{systemAudioStatus?.displays ?? 0} displays</small>
        </div>

        {systemAudioStatus && !systemAudioStatus.granted ? (
          <p className="settings-help">
            Enable Smooth in System Settings, Privacy & Security, Screen &
            System Audio Recording, then restart the app.
          </p>
        ) : null}

        <div className="settings-actions system-audio-actions">
          <button
            type="button"
            onClick={() => void checkSystemAudioPermission()}
            disabled={isSystemAudioBusy}
          >
            {isSystemAudioBusy ? "Checking" : "Check Permission"}
          </button>
        </div>

        <div
          className={
            systemCaptureStatus?.is_recording
              ? "audio-capture-card recording"
              : "audio-capture-card"
          }
        >
          <div className="audio-capture-meter" aria-hidden="true">
            <Monitor size={19} />
          </div>
          <div className="audio-capture-copy">
            <strong>ScreenCaptureKit audio</strong>
            <span>
              {systemCaptureStatus?.is_recording
                ? `${formatDuration(systemCaptureStatus.elapsed_ms)} · capturing desktop audio`
                : systemCaptureStatus?.last_preview
                  ? `${formatDuration(systemCaptureStatus.last_preview.duration_ms)} · ${systemCaptureStatus.last_preview.sample_rate.toLocaleString()} Hz`
                  : "Ready after permission check"}
            </span>
          </div>
          <div className="audio-capture-actions">
            <button
              type="button"
              onClick={() => void startSystemAudioCapture()}
              disabled={
                isSystemCaptureBusy ||
                systemCaptureStatus?.is_recording ||
                !systemAudioStatus?.granted
              }
            >
              <Monitor size={15} />
              Start
            </button>
            <button
              type="button"
              onClick={() => void stopSystemAudioCapture()}
              disabled={
                isSystemCaptureBusy || !systemCaptureStatus?.is_recording
              }
            >
              <Square size={14} />
              Stop
            </button>
          </div>
        </div>

        {systemCaptureStatus?.last_error ? (
          <p className="settings-help">{systemCaptureStatus.last_error}</p>
        ) : null}

        {systemAudioPreviewUrl && systemCaptureStatus?.last_preview ? (
          <div className="audio-preview">
            <div>
              <Play size={15} />
              <span>Last system capture</span>
              <small>
                {formatDuration(systemCaptureStatus.last_preview.duration_ms)}
              </small>
            </div>
            <audio controls src={systemAudioPreviewUrl} />
          </div>
        ) : null}
      </section>

      <section
        className="settings-section"
        hidden={settingsSection !== "speech-to-text"}
      >
        <div className="section-heading">
          <Sparkles size={18} />
          <span>Speech to text</span>
          <small>{sttStatus?.acceleration.join(", ") ?? "cpu"}</small>
        </div>

        <label className="settings-field">
          <span>Whisper model path</span>
          <div className="settings-input-row">
            <input
              value={sttConfig.model_path}
              onChange={(event) => {
                const value = event.currentTarget.value;
                setSttConfig((current) => ({
                  ...current,
                  model_path: value,
                }));
              }}
              placeholder="Path to ggml-base.en.bin"
            />
            <button
              type="button"
              onClick={() => void saveAndCheckStt()}
              disabled={isSttBusy || isLoading}
            >
              Save
            </button>
          </div>
        </label>

        <div className="settings-split-row">
          <label className="settings-field">
            <span>Language</span>
            <input
              value={sttConfig.language ?? ""}
              onChange={(event) => {
                const value = event.currentTarget.value;
                setSttConfig((current) => ({
                  ...current,
                  language: value || null,
                }));
              }}
              placeholder="en or auto"
            />
          </label>
          <label className="settings-field">
            <span>Threads</span>
            <input
              min={1}
              max={32}
              type="number"
              value={sttConfig.threads}
              onChange={(event) => {
                const value = event.currentTarget.value;
                setSttConfig((current) => ({
                  ...current,
                  threads: Number(value) || 1,
                }));
              }}
            />
          </label>
        </div>

        <div
          className={`connection-status ${sttStatus?.state === "ready" ? "ready" : "offline"}`}
        >
          <SttStatusIcon size={19} />
          <div>
            <strong>
              {sttStatus?.state?.replace("_", " ") ?? "not configured"}
            </strong>
            <span>
              {sttStatus?.message ?? "Choose a Whisper ggml model file"}
            </span>
          </div>
          <small>{sttStatus?.threads ?? sttConfig.threads} threads</small>
        </div>

        <div className="stt-performance-grid">
          <div>
            <span>STT queue</span>
            <strong>
              {(sttQueueStatus?.pending_mic ?? 0) +
                (sttQueueStatus?.pending_system ?? 0) +
                (sttQueueStatus?.processing ?? 0)}
            </strong>
            <small>
              mic {sttQueueStatus?.pending_mic ?? 0} · system{" "}
              {sttQueueStatus?.pending_system ?? 0} · failed{" "}
              {sttQueueStatus?.failed ?? 0}
            </small>
          </div>
          <div>
            <span>Average RTF</span>
            <strong>
              {sttQueueStatus?.recent_average_real_time_factor == null
                ? "—"
                : `${sttQueueStatus.recent_average_real_time_factor.toFixed(2)}×`}
            </strong>
            <small>below 1.0 keeps up live</small>
          </div>
          <div>
            <span>Last chunk</span>
            <strong>
              {sttQueueStatus?.last_inference_ms == null
                ? "—"
                : `${(sttQueueStatus.last_inference_ms / 1000).toFixed(1)}s`}
            </strong>
            <small>
              model load{" "}
              {sttQueueStatus?.last_model_load_ms == null
                ? "—"
                : `${sttQueueStatus.last_model_load_ms}ms`}
            </small>
          </div>
        </div>

        <div className="settings-actions stt-actions">
          <button
            type="button"
            onClick={() => void transcribeLastCapture()}
            disabled={isSttBusy || audioStatus?.is_recording}
          >
            {isSttBusy ? "Transcribing" : "Transcribe Last Capture"}
          </button>
        </div>

        {transcription ? (
          <div className="transcript-preview">
            <div>
              <strong>Transcript</strong>
              <small>
                {formatDuration(transcription.audio.duration_ms)} audio ·{" "}
                {(transcription.elapsed_ms / 1000).toFixed(1)}s ·{" "}
                {transcription.real_time_factor.toFixed(2)}× RTF ·{" "}
                {transcription.raw_segment_count} segments
              </small>
            </div>
            <small className="transcript-diagnostics">
              language{" "}
              {transcription.language ?? `auto:${transcription.language_id}`} ·
              rms {formatDb(transcription.audio.rms_db)} · peak{" "}
              {formatDb(transcription.audio.peak_db)}
            </small>
            <p>{transcription.text || "No speech detected"}</p>
          </div>
        ) : null}
      </section>

      <section
        className="settings-section"
        hidden={settingsSection !== "entity-interests"}
      >
        <div className="section-heading">
          <Sparkles size={18} />
          <span>Entity interests</span>
          <small>
            {entityInterests.filter((interest) => interest.enabled).length}{" "}
            enabled
          </small>
        </div>

        <p className="settings-help">
          These categories guide note entity extraction. Rename and disable them
          freely; saved names are used in future extraction prompts.
        </p>

        <div className="entity-interest-list">
          {entityInterests.map((interest, index) => (
            <div className="entity-interest-row" key={interest.id ?? index}>
              <label className="entity-interest-toggle">
                <input
                  type="checkbox"
                  checked={interest.enabled}
                  onChange={(event) =>
                    updateEntityInterest(index, {
                      enabled: event.currentTarget.checked,
                    })
                  }
                />
              </label>
              <div className="entity-interest-fields">
                <input
                  value={interest.name}
                  onChange={(event) =>
                    updateEntityInterest(index, {
                      name: event.currentTarget.value,
                    })
                  }
                  placeholder="Entity category"
                />
                <input
                  value={interest.description}
                  onChange={(event) =>
                    updateEntityInterest(index, {
                      description: event.currentTarget.value,
                    })
                  }
                  placeholder="What should the extractor look for?"
                />
              </div>
              <button
                className="icon-button"
                type="button"
                onClick={() => removeEntityInterest(index)}
                title="Remove interest"
              >
                <X size={15} />
              </button>
            </div>
          ))}
        </div>

        <div className="settings-actions">
          <button type="button" onClick={addEntityInterest}>
            <Plus size={15} />
            Add interest
          </button>
          <button
            type="button"
            onClick={() => void saveEntityInterests()}
            disabled={isEntityInterestBusy || isLoading}
          >
            {isEntityInterestBusy ? "Saving" : "Save interests"}
          </button>
        </div>
      </section>

      <section
        className="settings-section"
        hidden={settingsSection !== "google"}
      >
        <div className="section-heading">
          <CalendarDays size={18} />
          <span>Google integrations</span>
          <small>
            {gmailConfig.has_access_token || calendarConfig.has_access_token
              ? "Connected"
              : "Not connected"}
          </small>
        </div>

        <label className="settings-field">
          <span>OAuth client ID</span>
          <input
            value={gmailConfig.client_id}
            onChange={(event) => {
              const value = event.currentTarget.value;
              setGmailConfig((current) => ({
                ...current,
                client_id: value,
              }));
            }}
            placeholder="Google OAuth web client ID"
          />
        </label>

        <label className="settings-field">
          <span>OAuth client secret</span>
          <input
            value={gmailConfig.client_secret}
            onChange={(event) => {
              const value = event.currentTarget.value;
              setGmailConfig((current) => ({
                ...current,
                client_secret: value,
              }));
            }}
            placeholder="Google OAuth web client secret"
            type="password"
          />
        </label>

        <div
          className={`connection-status ${gmailConfig.has_access_token ? "ready" : "offline"}`}
        >
          {gmailConfig.has_access_token ? (
            <CheckCircle2 size={19} />
          ) : (
            <CircleAlert size={19} />
          )}
          <div>
            <strong>
              Gmail {gmailConfig.has_access_token ? "connected" : "not connected"}
            </strong>
            <span>
              Scope: gmail.drafts.create
              {gmailConfig.has_refresh_token ? " · refresh enabled" : ""}
            </span>
          </div>
          {gmailConfig.access_token_expires_at ? (
            <small>
              {new Date(
                gmailConfig.access_token_expires_at * 1000,
              ).toLocaleTimeString()}
            </small>
          ) : null}
        </div>

        <div
          className={`connection-status ${calendarConfig.has_access_token ? "ready" : "offline"}`}
        >
          {calendarConfig.has_access_token ? (
            <CheckCircle2 size={19} />
          ) : (
            <CircleAlert size={19} />
          )}
          <div>
            <strong>
              Calendar{" "}
              {calendarConfig.has_access_token ? "connected" : "not connected"}
            </strong>
            <span>
              Scope: calendar.readonly
              {calendarConfig.has_refresh_token ? " · refresh enabled" : ""}
            </span>
          </div>
          {calendarConfig.access_token_expires_at ? (
            <small>
              {new Date(
                calendarConfig.access_token_expires_at * 1000,
              ).toLocaleTimeString()}
            </small>
          ) : null}
        </div>

        <p className="settings-help">
          Use a Google OAuth Web client with an authorized redirect URI of
          http://localhost. Smooth asks for Gmail draft creation and Calendar
          read-only access separately.
        </p>

        <div className="settings-actions">
          <button
            type="button"
            onClick={() => void saveGmailConfig()}
            disabled={isGmailBusy}
          >
            Save Google Settings
          </button>
          <button
            type="button"
            onClick={() => void connectGmail()}
            disabled={isGmailBusy}
          >
            {gmailConfig.has_access_token ? "Reconnect Gmail" : "Connect Gmail"}
          </button>
          <button
            type="button"
            onClick={() => void connectCalendar()}
            disabled={isCalendarBusy || isGmailBusy}
          >
            {calendarConfig.has_access_token
              ? "Reconnect Calendar"
              : "Connect Calendar"}
          </button>
          {gmailConfig.has_access_token || gmailConfig.has_refresh_token ? (
            <button
              type="button"
              onClick={() => void disconnectGmail()}
              disabled={isGmailBusy}
            >
              Disconnect
            </button>
          ) : null}
          {calendarConfig.has_access_token ||
          calendarConfig.has_refresh_token ? (
            <button
              type="button"
              onClick={() => void disconnectCalendar()}
              disabled={isCalendarBusy}
            >
              Disconnect Calendar
            </button>
          ) : null}
        </div>
      </section>

      <div
        className="settings-tabgroup"
        hidden={
          settingsSection !== "ai-local" &&
          settingsSection !== "ai-remote" &&
          settingsSection !== "ai-settings"
        }
      >
        <LlamaSettings
          onError={setSettingsError}
          view={
            settingsSection === "ai-remote"
              ? "remote"
              : settingsSection === "ai-settings"
                ? "settings"
                : "local"
          }
        />
      </div>

      <section
        className="settings-section"
        hidden={settingsSection !== "agent-tools"}
      >
        <div className="section-heading">
          <Wrench size={18} />
          <span>Agent tools</span>
          <small>{agentTools.length} registered</small>
        </div>

        <label className="settings-field">
          <span>Tool</span>
          <select
            value={selectedAgentTool}
            onChange={(event) => {
              const value = event.currentTarget.value;
              setSelectedAgentTool(value);
              setAgentToolInput(defaultAgentToolInput(value));
              setAgentToolOutput(null);
            }}
            disabled={!agentTools.length}
          >
            {agentTools.map((tool) => (
              <option key={tool.name} value={tool.name}>
                {tool.name}
              </option>
            ))}
          </select>
        </label>

        {selectedAgentToolDescriptor ? (
          <p className="settings-help">
            {selectedAgentToolDescriptor.description}
          </p>
        ) : null}

        <label className="settings-field agent-tool-input">
          <span>Input JSON</span>
          <textarea
            value={agentToolInput}
            onChange={(event) => {
              const value = event.currentTarget.value;
              setAgentToolInput(value);
            }}
            spellCheck={false}
          />
        </label>

        <div className="agent-tool-grid">
          <div>
            <strong>Input schema</strong>
            <pre>
              {selectedAgentToolDescriptor
                ? JSON.stringify(
                    selectedAgentToolDescriptor.input_schema,
                    null,
                    2,
                  )
                : "{}"}
            </pre>
          </div>
          <div>
            <strong>Output</strong>
            <pre>{agentToolOutput ?? "No run yet"}</pre>
          </div>
        </div>

        <div className="settings-actions agent-tool-actions">
          <button
            type="button"
            onClick={() => void runAgentTool()}
            disabled={isAgentToolBusy || !selectedAgentTool}
          >
            {isAgentToolBusy ? "Running" : "Run Tool"}
          </button>
        </div>

        <div className="agent-flow-panel">
          <div className="section-heading compact">
            <Sparkles size={16} />
            <span>Agent flow</span>
            <small>{agentRunResult?.model || "llama.cpp"}</small>
          </div>

          <label className="settings-field agent-tool-input">
            <span>Prompt</span>
            <textarea
              value={agentPrompt}
              onChange={(event) => {
                const value = event.currentTarget.value;
                setAgentPrompt(value);
              }}
              spellCheck={false}
            />
          </label>

          <div className="settings-actions agent-tool-actions">
            <button
              type="button"
              onClick={() => void runAgentFlow()}
              disabled={isAgentRunBusy || !agentPrompt.trim()}
            >
              {isAgentRunBusy ? "Running Agent" : "Run Agent"}
            </button>
          </div>

          {agentRunResult ? (
            <div className="agent-run-output">
              <div>
                <strong>Answer</strong>
                {agentRunResult.run_id ? (
                  <small>{agentRunResult.run_id}</small>
                ) : null}
                <p>{agentRunResult.answer || "No answer returned"}</p>
              </div>
              <div>
                <strong>Steps</strong>
                {agentRunResult.steps.length ? (
                  agentRunResult.steps.map((step, index) => (
                    <pre key={`${step.tool_name}-${index}`}>
                      {JSON.stringify(
                        {
                          tool: step.tool_name,
                          input: step.input,
                          output: step.output,
                          error: step.error,
                        },
                        null,
                        2,
                      )}
                    </pre>
                  ))
                ) : (
                  <p>No tools used</p>
                )}
              </div>
              {agentRunResult.raw_model_output ? (
                <div>
                  <strong>Raw model output</strong>
                  <pre>{agentRunResult.raw_model_output}</pre>
                </div>
              ) : null}
            </div>
          ) : null}
        </div>
      </section>

      <section
        className="settings-section"
        hidden={settingsSection !== "extraction-queue"}
      >
        <div className="section-heading">
          <Database size={18} />
          <span>Extraction queue</span>
          <small>Persistent</small>
        </div>

        <div className="queue-stats">
          <div>
            <strong>{queueStatus?.pending ?? 0}</strong>
            <span>Pending</span>
          </div>
          <div>
            <strong>{queueStatus?.processing ?? 0}</strong>
            <span>Processing</span>
          </div>
          <div className={queueStatus?.failed ? "failed" : ""}>
            <strong>{queueStatus?.failed ?? 0}</strong>
            <span>Failed</span>
          </div>
          <div>
            <strong>{queueStatus?.indexed ?? 0}</strong>
            <span>Indexed</span>
          </div>
        </div>

        <div className="settings-actions">
          <button
            type="button"
            onClick={() => void queueAllNotes()}
            disabled={isQueueBusy}
          >
            Queue Changed Notes
          </button>
          <button
            type="button"
            onClick={() => void retryFailedJobs()}
            disabled={isQueueBusy || !queueStatus?.failed}
          >
            Retry Failed
          </button>
        </div>
      </section>
        </div>
      </div>
    </div>
  );
}

type TreeSectionProps = {
  children: React.ReactNode;
  count: number;
  draggedNoteId?: string | null;
  dropTarget?: DropTarget | null;
  folderId?: string | null;
  icon: React.ReactNode;
  isOpen: boolean;
  onToggle: () => void;
  title: string;
  droppable?: boolean;
  onDropNote?: (noteId: string) => void;
  onDropTargetChange?: (target: DropTarget | null) => void;
  renamable?: boolean;
  startRenaming?: boolean;
  onRename?: (name: string) => void;
};

function TreeSection({
  children,
  count,
  draggedNoteId = null,
  dropTarget = null,
  folderId = null,
  icon,
  isOpen,
  onToggle,
  title,
  droppable = false,
  onDropNote,
  onDropTargetChange,
  renamable = false,
  startRenaming = false,
  onRename,
}: TreeSectionProps) {
  const [isDragOver, setIsDragOver] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(title);
  const renameInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (startRenaming) {
      setEditing(true);
    }
  }, [startRenaming]);

  useEffect(() => {
    if (editing) {
      setDraft(title);
      const input = renameInputRef.current;
      if (input) {
        input.scrollIntoView({ block: "nearest" });
        input.focus();
        input.select();
      }
    }
  }, [editing, title]);

  function commitRename() {
    setEditing(false);
    onRename?.(draft);
  }

  const onDragOver = (event: DragEvent<HTMLElement>) => {
    if (!droppable) {
      return;
    }

    if (!draggedNoteId && !hasDraggedNoteData(event)) {
      return;
    }

    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    setIsDragOver(true);
    onDropTargetChange?.({
      folderId,
      index: getDropIndex(event),
    });
  };

  const onDragLeave = (event: DragEvent<HTMLElement>) => {
    if (!droppable) {
      return;
    }

    const nextTarget = event.relatedTarget;
    if (
      nextTarget instanceof Node &&
      event.currentTarget.contains(nextTarget)
    ) {
      return;
    }

    setIsDragOver(false);
    if (dropTarget?.folderId === folderId) {
      onDropTargetChange?.(null);
    }
  };

  const onDrop = (event: DragEvent<HTMLElement>) => {
    if (!droppable) {
      return;
    }

    event.preventDefault();
    event.stopPropagation();
    setIsDragOver(false);
    onDropTargetChange?.(null);

    const noteId = draggedNoteId || readDraggedNoteId(event);
    if (noteId) {
      onDropNote?.(noteId);
    }
  };

  return (
    <section
      className={isDragOver ? "tree-section drag-over" : "tree-section"}
      onDragOver={droppable ? onDragOver : undefined}
      onDragLeave={droppable ? onDragLeave : undefined}
      onDrop={droppable ? onDrop : undefined}
    >
      {editing ? (
        <div className="tree-header editing">
          {isOpen ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
          {icon}
          <input
            ref={renameInputRef}
            className="tree-rename-input"
            value={draft}
            onChange={(event) => setDraft(event.currentTarget.value)}
            onBlur={commitRename}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                commitRename();
              } else if (event.key === "Escape") {
                event.preventDefault();
                setEditing(false);
              }
            }}
            aria-label={`Rename ${title}`}
          />
        </div>
      ) : (
        <button
          className={isDragOver ? "tree-header drag-over" : "tree-header"}
          type="button"
          onClick={onToggle}
          onDoubleClick={
            renamable
              ? (event) => {
                  event.preventDefault();
                  setEditing(true);
                }
              : undefined
          }
        >
          {isOpen ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
          {icon}
          <span>{title}</span>
          <small>{count}</small>
        </button>
      )}
      {isOpen ? <div className="tree-children">{children}</div> : null}
    </section>
  );
}

type NoteRowProps = {
  action?: React.ReactNode;
  activeId: string | null;
  indent?: boolean;
  muted?: boolean;
  note: NoteListItem;
  dragging?: boolean;
  onDragEnd?: () => void;
  onDragStart?: (id: string) => void;
  onOpen: (id: string) => Promise<void>;
  onToggleSelected?: (id: string) => void;
  onContextMenu?: (note: NoteListItem, x: number, y: number) => void;
  draggable?: boolean;
  selectable?: boolean;
  selectedIds?: string[];
};

function NoteRow({
  activeId,
  action,
  indent = false,
  dragging = false,
  muted = false,
  note,
  onDragEnd,
  onDragStart,
  onOpen,
  onToggleSelected,
  onContextMenu,
  draggable = false,
  selectable = true,
  selectedIds = [],
}: NoteRowProps) {
  const isSelected = selectedIds.includes(note.id);
  const className = [
    "note-row",
    note.id === activeId ? "active" : "",
    isSelected ? "selected" : "",
    dragging ? "dragging" : "",
    muted ? "muted" : "",
    indent ? "indented" : "",
    !selectable ? "no-select" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div
      className={className}
      data-note-id={note.id}
      draggable={draggable}
      onDragStart={
        draggable
          ? (event) => {
              onDragStart?.(note.id);
              event.dataTransfer.setData(
                "application/x-smooth-note-id",
                note.id,
              );
              event.dataTransfer.setData("text/note-id", note.id);
              event.dataTransfer.setData("text/plain", note.id);
              event.dataTransfer.effectAllowed = "move";
            }
          : undefined
      }
      onDragEnd={draggable ? onDragEnd : undefined}
      onContextMenu={
        onContextMenu
          ? (event) => {
              event.preventDefault();
              onContextMenu(note, event.clientX, event.clientY);
            }
          : undefined
      }
    >
      {selectable ? (
        <button
          className="row-lead"
          type="button"
          onClick={(event) => {
            event.stopPropagation();
            onToggleSelected?.(note.id);
          }}
          aria-pressed={isSelected}
          aria-label={`${isSelected ? "Unselect" : "Select"} ${note.title}`}
        >
          <FileText className="row-file-icon" size={15} />
          <span className="row-checkbox" aria-hidden="true" />
        </button>
      ) : (
        <FileText className="row-file-icon" size={15} />
      )}
      <button
        className="note-main"
        type="button"
        onClick={() => void onOpen(note.id)}
      >
        <span>{note.title}</span>
        <small>{note.excerpt || "No content"}</small>
      </button>
      {action}
    </div>
  );
}

function DropPlaceholder() {
  return (
    <div
      className="drop-placeholder"
      data-drop-placeholder="true"
      aria-hidden="true"
    >
      <span />
    </div>
  );
}

function animateNoteTreeLayout(update: () => void) {
  const movingElements = Array.from(
    document.querySelectorAll<HTMLElement>(
      ".notes-pane .note-row, .notes-pane .drop-placeholder",
    ),
  );
  const firstRects = new Map<string, DOMRect>();

  for (const element of movingElements) {
    const key = layoutAnimationKey(element);
    if (key) {
      firstRects.set(key, element.getBoundingClientRect());
    }
  }

  flushSync(update);

  window.requestAnimationFrame(() => {
    const nextElements = Array.from(
      document.querySelectorAll<HTMLElement>(
        ".notes-pane .note-row, .notes-pane .drop-placeholder",
      ),
    );

    for (const element of nextElements) {
      const key = layoutAnimationKey(element);
      const first = key ? firstRects.get(key) : null;
      if (!first) {
        continue;
      }

      const last = element.getBoundingClientRect();
      const deltaX = first.left - last.left;
      const deltaY = first.top - last.top;
      if (Math.abs(deltaX) < 0.5 && Math.abs(deltaY) < 0.5) {
        continue;
      }

      element.style.transition = "none";
      element.style.transform = `translate(${deltaX}px, ${deltaY}px)`;

      window.requestAnimationFrame(() => {
        element.style.transition = "transform 160ms var(--ease)";
        element.style.transform = "";
        window.setTimeout(() => {
          element.style.transition = "";
          element.style.transform = "";
        }, 180);
      });
    }
  });
}

function layoutAnimationKey(element: HTMLElement) {
  if (element.dataset.noteId) {
    return `note:${element.dataset.noteId}`;
  }
  if (element.dataset.dropPlaceholder) {
    return "drop-placeholder";
  }
  return null;
}

function hasDraggedNoteData(event: DragEvent<HTMLElement>) {
  return Array.from(event.dataTransfer.types).some((type) =>
    ["application/x-smooth-note-id", "text/note-id", "text/plain"].includes(
      type.toLowerCase(),
    ),
  );
}

function readDraggedNoteId(event: DragEvent<HTMLElement>) {
  return (
    event.dataTransfer.getData("application/x-smooth-note-id") ||
    event.dataTransfer.getData("text/note-id") ||
    event.dataTransfer.getData("text/plain")
  ).trim();
}

function getDropIndex(event: DragEvent<HTMLElement>) {
  const children = event.currentTarget.querySelector(".tree-children");
  if (!children) {
    return 0;
  }

  const rows = Array.from(
    children.querySelectorAll<HTMLElement>(":scope > .note-row:not(.dragging)"),
  );

  for (let index = 0; index < rows.length; index += 1) {
    const box = rows[index].getBoundingClientRect();
    if (event.clientY < box.top + box.height / 2) {
      return index;
    }
  }

  return rows.length;
}

type NoteContextMenuProps = {
  target: { note: NoteListItem; x: number; y: number };
  folders: Folder[];
  canLink: boolean;
  onClose: () => void;
  onOpen: (id: string) => void;
  onMove: (id: string, folderId: string | null) => void;
  onLink: () => void;
  onTrash: (id: string) => void;
  onRestore: (id: string) => void;
  onDelete: (id: string) => void;
};

function NoteContextMenu({
  target,
  folders,
  canLink,
  onClose,
  onOpen,
  onMove,
  onLink,
  onTrash,
  onRestore,
  onDelete,
}: NoteContextMenuProps) {
  const { note } = target;
  const trashed = Boolean(note.deleted_at);
  const run = (action: () => void) => () => {
    action();
    onClose();
  };

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
      }
    }
    window.addEventListener("pointerdown", onClose);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", onClose);
    return () => {
      window.removeEventListener("pointerdown", onClose);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("resize", onClose);
    };
  }, [onClose]);

  // Keep the menu inside the viewport.
  const left = Math.min(target.x, window.innerWidth - 220);
  const top = Math.min(target.y, window.innerHeight - 260);

  return (
    <div
      className="context-menu"
      style={{ left, top }}
      onPointerDown={(event) => event.stopPropagation()}
      role="menu"
    >
      {trashed ? (
        <>
          <button type="button" onClick={run(() => onRestore(note.id))}>
            <ArchiveRestore size={15} />
            Restore to Inbox
          </button>
          <div className="context-sep" />
          <button
            type="button"
            className="danger"
            onClick={run(() => onDelete(note.id))}
          >
            <X size={15} />
            Delete forever
          </button>
        </>
      ) : (
        <>
          <button type="button" onClick={run(() => onOpen(note.id))}>
            <FileText size={15} />
            Open
          </button>
          {canLink ? (
            <button type="button" onClick={run(() => onLink())}>
              <Link2 size={15} />
              Link selected
            </button>
          ) : null}
          <div className="context-submenu">
            <button type="button" className="context-submenu-trigger">
              <Folder size={15} />
              Move to
              <ChevronRight className="context-caret" size={14} />
            </button>
            <div className="context-submenu-list">
              <button type="button" onClick={run(() => onMove(note.id, null))}>
                <Inbox size={15} />
                Inbox
              </button>
              {folders.map((folder) => (
                <button
                  key={folder.id}
                  type="button"
                  onClick={run(() => onMove(note.id, folder.id))}
                >
                  <Folder size={15} />
                  {folder.name}
                </button>
              ))}
            </div>
          </div>
          <div className="context-sep" />
          <button
            type="button"
            className="danger"
            onClick={run(() => onTrash(note.id))}
          >
            <Trash2 size={15} />
            Move to trash
          </button>
        </>
      )}
    </div>
  );
}

type EntityStripProps = {
  note: NoteWithContent;
  onStatusChange: (noteId: string, status: string) => void;
  onJumpToMention: (surfaceText: string) => void;
};

function EntityStrip({
  note,
  onStatusChange,
  onJumpToMention,
}: EntityStripProps) {
  const [extraction, setExtraction] = useState<NoteExtractionView>({
    status: note.extraction_status,
    error: null,
    entities: [],
    mentions: [],
  });
  const [isQueuing, setIsQueuing] = useState(false);
  const [isExpanded, setIsExpanded] = useState(false);
  const [editingEntityId, setEditingEntityId] = useState<number | null>(null);
  const [entityNameDraft, setEntityNameDraft] = useState("");
  const [extractionProvider, setExtractionProvider] = useState<
    "default" | "local" | "inception"
  >("default");

  const refreshExtraction = useCallback(async () => {
    const nextExtraction = await invoke<NoteExtractionView>(
      "get_note_extraction",
      {
        id: note.id,
      },
    );
    setExtraction(nextExtraction);
    onStatusChange(note.id, nextExtraction.status);
    return nextExtraction;
  }, [note.id, onStatusChange]);

  useEffect(() => {
    setExtraction((current) => ({
      ...current,
      status: note.extraction_status,
    }));
    setIsExpanded(false);
    void refreshExtraction();
  }, [note.extraction_status, note.id, refreshExtraction]);

  useEffect(() => {
    if (!["queued", "processing"].includes(extraction.status)) {
      return;
    }

    const interval = window.setInterval(() => {
      void refreshExtraction();
    }, 2000);
    return () => window.clearInterval(interval);
  }, [extraction.status, refreshExtraction]);

  async function queueExtraction() {
    setIsQueuing(true);
    try {
      await invoke("enqueue_note_extraction", {
        id: note.id,
        selection:
          extractionProvider === "default"
            ? null
            : { provider: extractionProvider, model: null },
      });
      const nextExtraction = await refreshExtraction();
      toast.info(
        nextExtraction.status === "queued"
          ? "Extraction queued"
          : `Extraction ${nextExtraction.status.replace("_", " ")}`,
      );
    } catch (queueError) {
      toast.error(queueError);
    } finally {
      setIsQueuing(false);
    }
  }

  function beginRenameEntity(entity: NoteEntity) {
    setEditingEntityId(entity.id);
    setEntityNameDraft(entity.name);
  }

  async function submitRenameEntity(entity: NoteEntity) {
    const nextName = entityNameDraft.trim();
    if (!nextName || nextName === entity.name) {
      setEditingEntityId(null);
      setEntityNameDraft("");
      return;
    }

    try {
      await invoke("rename_entity", {
        entityId: entity.id,
        canonicalName: nextName,
      });
      await refreshExtraction();
      setEditingEntityId(null);
      setEntityNameDraft("");
      toast.success(`Entity renamed to ${nextName}`);
    } catch (renameError) {
      toast.error(String(renameError));
    }
  }

  function firstMention(entityId: number) {
    return (
      extraction.mentions.find(
        (mention) =>
          mention.entity_id === entityId &&
          mention.surface_text.trim().length > 0 &&
          mention.start_offset !== null,
      ) ??
      extraction.mentions.find(
        (mention) =>
          mention.entity_id === entityId &&
          mention.surface_text.trim().length > 0,
      ) ??
      null
    );
  }

  const statusLabel = extraction.status.replace("_", " ");
  if (extraction.status === "disabled") {
    return (
      <section className="entity-strip">
        <div className="entity-strip-header">
          <Sparkles size={14} />
          <span>Entities</span>
          <small className="entity-status disabled">disabled</small>
        </div>
        <p className="entity-empty">
          Entity extraction is off for meeting notes.
        </p>
      </section>
    );
  }

  const canQueue =
    !note.deleted_at &&
    !["queued", "processing"].includes(extraction.status) &&
    note.content.trim().length > 0;
  const visibleEntities = isExpanded
    ? extraction.entities
    : extraction.entities.slice(0, ENTITY_PREVIEW_LIMIT);
  const hiddenEntityCount = Math.max(
    0,
    extraction.entities.length - ENTITY_PREVIEW_LIMIT,
  );

  return (
    <section className="entity-strip">
      <div className="entity-strip-header">
        <Sparkles size={14} />
        <span>Entities</span>
        <small className={`entity-status ${extraction.status}`}>
          {statusLabel}
        </small>
        {canQueue ? (
          <>
            <select
              className="entity-provider-select"
              value={extractionProvider}
              onChange={(event) =>
                setExtractionProvider(
                  event.currentTarget.value as "default" | "local" | "inception",
                )
              }
              title="LLM provider for this extraction"
              aria-label="Extraction LLM provider"
            >
              <option value="default">Default</option>
              <option value="local">Local</option>
              <option value="inception">Inception</option>
            </select>
            <button
              type="button"
              onClick={() => void queueExtraction()}
              disabled={isQueuing}
            >
              {extraction.status === "failed" ? "Retry" : "Extract"}
            </button>
          </>
        ) : null}
      </div>
      {extraction.error ? (
        <p className="entity-error">{extraction.error}</p>
      ) : null}
      {extraction.entities.length > 0 ? (
        <div className="entity-chips">
          {visibleEntities.map((entity) => {
            const mention = firstMention(entity.id);
            const isEditing = editingEntityId === entity.id;
            return (
              <span
                className="entity-chip"
                key={entity.id}
                title={
                  mention
                    ? `Jump to "${mention.surface_text}"`
                    : entity.entity_type
                }
              >
                {isEditing ? (
                  <span className="entity-chip-main editing">
                    <small>{entity.entity_type}</small>
                    <input
                      className="entity-chip-input"
                      value={entityNameDraft}
                      autoFocus
                      onChange={(event) =>
                        setEntityNameDraft(event.currentTarget.value)
                      }
                      onClick={(event) => event.stopPropagation()}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") {
                          event.preventDefault();
                          void submitRenameEntity(entity);
                        }
                        if (event.key === "Escape") {
                          setEditingEntityId(null);
                          setEntityNameDraft("");
                        }
                      }}
                    />
                    {entity.mention_count > 1 ? (
                      <b>{entity.mention_count}</b>
                    ) : null}
                  </span>
                ) : (
                  <button
                    className="entity-chip-main"
                    type="button"
                    disabled={!mention}
                    onClick={() => {
                      if (mention) {
                        onJumpToMention(mention.surface_text);
                      }
                    }}
                  >
                    <small>{entity.entity_type}</small>
                    <span>{entity.name}</span>
                    {entity.mention_count > 1 ? (
                      <b>{entity.mention_count}</b>
                    ) : null}
                  </button>
                )}
                <button
                  className="entity-chip-rename"
                  type="button"
                  onClick={() =>
                    isEditing
                      ? void submitRenameEntity(entity)
                      : beginRenameEntity(entity)
                  }
                  title={isEditing ? "Save entity name" : "Rename entity"}
                >
                  {isEditing ? <CheckCircle2 size={13} /> : <Pencil size={12} />}
                </button>
              </span>
            );
          })}
          {hiddenEntityCount > 0 ? (
            <button
              className="entity-more"
              type="button"
              onClick={() => setIsExpanded((expanded) => !expanded)}
              aria-expanded={isExpanded}
            >
              <ChevronDown className={isExpanded ? "expanded" : ""} size={14} />
              {isExpanded ? "Less" : `${hiddenEntityCount} more`}
            </button>
          ) : null}
        </div>
      ) : (
        <p className="entity-empty">
          {extraction.status === "processing"
            ? "Extracting important entities..."
            : extraction.status === "queued"
              ? "Waiting for the local model..."
              : "No extracted entities"}
        </p>
      )}
    </section>
  );
}

type NoteEditorProps = {
  note: NoteWithContent | null;
  folders: Folder[];
  panelOpen: boolean;
  externalRevision: number;
  externalNoteId: string | null;
  entityJumpTarget: { nonce: number; surfaceText: string } | null;
  reminderJumpTarget: ReminderJumpTarget | null;
  onDismissReminderTarget: () => void;
  onTogglePanel: () => void;
  onCreate: () => Promise<void>;
  onSave: (
    id: string,
    title: string,
    content: string,
    folderId: string | null,
  ) => Promise<NoteWithContent>;
  onTrash: (id: string) => Promise<void>;
  onRestore: (id: string) => Promise<void>;
  onPermanentDelete: (id: string) => Promise<void>;
  onMove: (id: string, folderId: string | null) => Promise<void>;
  onExport: (
    id: string,
    title: string,
    content: string,
    folderId: string | null,
  ) => Promise<void>;
  onCreateGmailDraft: (input: GmailDraftInput) => Promise<void>;
};

function NoteEditor({
  note,
  folders,
  panelOpen,
  externalRevision,
  externalNoteId,
  entityJumpTarget,
  reminderJumpTarget,
  onDismissReminderTarget,
  onTogglePanel,
  onCreate,
  onSave,
  onTrash,
  onRestore,
  onPermanentDelete,
  onMove,
  onExport,
  onCreateGmailDraft,
}: NoteEditorProps) {
  const [draftTitle, setDraftTitle] = useState("");
  const [draftFolderId, setDraftFolderId] = useState("");
  const [folderMenuOpen, setFolderMenuOpen] = useState(false);
  const [editorRevision, setEditorRevision] = useState(0);
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [dictationState, setDictationState] = useState<DictationState>("idle");
  const [streamingTranscript, setStreamingTranscript] = useState("");
  const [emailDraft, setEmailDraft] = useState<GmailDraftInput | null>(null);
  const [isCreatingEmailDraft, setIsCreatingEmailDraft] = useState(false);
  const [reminderSelection, setReminderSelection] =
    useState<ReminderSelection | null>(null);
  const hasUnsavedChangesRef = useRef(false);
  const isLoadingNoteRef = useRef(false);
  const dictationActiveRef = useRef(false);
  const dictationChunkRef = useRef<Promise<void> | null>(null);
  const insertedDictationRef = useRef(false);
  const turndown = useMemo(
    () =>
      new TurndownService({
        headingStyle: "atx",
        bulletListMarker: "-",
      }),
    [],
  );

  useEffect(() => {
    if (!folderMenuOpen) {
      return;
    }
    const close = () => setFolderMenuOpen(false);
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [folderMenuOpen]);

  const editor = useEditor({
    extensions: [
      StarterKit,
      Image.configure({
        allowBase64: false,
        inline: false,
      }),
      Placeholder.configure({
        placeholder: "Start writing...",
      }),
    ],
    content: markdownToHtml(note?.content ?? ""),
    editorProps: {
      attributes: {
        class: "editor-surface",
      },
    },
    onUpdate: () => {
      if (isLoadingNoteRef.current) {
        return;
      }

      hasUnsavedChangesRef.current = true;
      setEditorRevision((revision) => revision + 1);
    },
    onSelectionUpdate: () => {
      setEditorRevision((revision) => revision + 1);
    },
  });

  useEffect(() => {
    isLoadingNoteRef.current = true;
    hasUnsavedChangesRef.current = false;
    setDraftTitle(note?.title ?? "");
    setDraftFolderId(note?.folder_id ?? "");
    setSaveState("idle");
    if (editor && !editor.isDestroyed) {
      editor.commands.setContent(markdownToHtml(note?.content ?? ""));
      editor.setEditable(!note?.deleted_at);
    }

    window.queueMicrotask(() => {
      isLoadingNoteRef.current = false;
    });
  }, [editor, note?.deleted_at, note?.id]);

  useEffect(() => {
    if (
      !externalRevision ||
      !editor ||
      editor.isDestroyed ||
      !note ||
      note.id !== externalNoteId
    ) {
      return;
    }

    isLoadingNoteRef.current = true;
    hasUnsavedChangesRef.current = false;
    setDraftTitle(note.title);
    setDraftFolderId(note.folder_id ?? "");
    editor.commands.setContent(markdownToHtml(note.content));
    window.queueMicrotask(() => {
      isLoadingNoteRef.current = false;
    });
  }, [editor, externalNoteId, externalRevision, note]);

  useEffect(() => {
    if (!editor || editor.isDestroyed || !entityJumpTarget?.surfaceText) {
      return;
    }
    const needle = entityJumpTarget.surfaceText.trim().toLowerCase();
    if (!needle) {
      return;
    }

    let match: { from: number; to: number } | null = null;
    editor.state.doc.descendants((node, pos) => {
      if (match || !node.isText || !node.text) {
        return true;
      }
      const index = node.text.toLowerCase().indexOf(needle);
      if (index >= 0) {
        match = {
          from: pos + index,
          to: pos + index + entityJumpTarget.surfaceText.trim().length,
        };
        return false;
      }
      return true;
    });

    if (match) {
      editor.commands.focus();
      editor.commands.setTextSelection(match);
      window.requestAnimationFrame(() => {
        window
          .getSelection()
          ?.anchorNode?.parentElement?.scrollIntoView({
            block: "center",
            behavior: "smooth",
          });
      });
    } else {
      toast.info("Entity text was not found in the editor");
    }
  }, [editor, entityJumpTarget]);

  useEffect(() => {
    if (
      !editor ||
      editor.isDestroyed ||
      !note ||
      !reminderJumpTarget ||
      reminderJumpTarget.noteId !== note.id
    ) {
      return;
    }

    const normalize = (value: string) =>
      value.replace(/\s+/g, " ").trim().toLowerCase();
    const needle = normalize(reminderJumpTarget.selectedText);
    if (!needle) return;

    const doc = editor.state.doc;
    const maxPosition = doc.content.size;
    const storedFrom = Math.max(
      1,
      Math.min(reminderJumpTarget.startOffset, maxPosition),
    );
    const storedTo = Math.max(
      storedFrom,
      Math.min(reminderJumpTarget.endOffset, maxPosition),
    );
    let match: { from: number; to: number } | null = null;

    if (
      normalize(doc.textBetween(storedFrom, storedTo, "\n", "\n")) === needle
    ) {
      match = { from: storedFrom, to: storedTo };
    }

    if (!match) {
      const expectedLength = reminderJumpTarget.selectedText.length;
      const contextBefore = normalize(reminderJumpTarget.contextBefore).slice(-80);
      let bestScore = Number.POSITIVE_INFINITY;
      for (let from = 1; from < maxPosition; from += 1) {
        const first = normalize(
          doc.textBetween(from, Math.min(from + 1, maxPosition), "\n", "\n"),
        );
        if (first && !needle.startsWith(first)) continue;

        for (let adjustment = -12; adjustment <= 32; adjustment += 1) {
          const to = Math.min(
            maxPosition,
            from + Math.max(1, expectedLength + adjustment),
          );
          if (normalize(doc.textBetween(from, to, "\n", "\n")) !== needle) {
            continue;
          }
          const before = normalize(
            doc.textBetween(Math.max(0, from - 200), from, "\n", "\n"),
          );
          const contextAfter = normalize(reminderJumpTarget.contextAfter).slice(
            0,
            80,
          );
          const after = normalize(
            doc.textBetween(to, Math.min(maxPosition, to + 200), "\n", "\n"),
          );
          const contextBonus =
            (contextBefore && before.endsWith(contextBefore) ? 50_000 : 0) +
            (contextAfter && after.startsWith(contextAfter) ? 50_000 : 0);
          const score =
            Math.abs(from - reminderJumpTarget.startOffset) - contextBonus;
          if (score < bestScore) {
            bestScore = score;
            match = { from, to };
          }
        }
      }
    }

    if (match) {
      editor.commands.focus();
      editor.commands.setTextSelection(match);
      window.requestAnimationFrame(() => {
        window
          .getSelection()
          ?.anchorNode?.parentElement?.scrollIntoView({
            block: "center",
            behavior: "smooth",
          });
      });
    } else {
      toast.info("The original reminder passage could not be located");
    }
  }, [editor, note, reminderJumpTarget]);

  useEffect(() => {
    if (!note || note.deleted_at || !editor || editor.isDestroyed) {
      return;
    }

    if (!hasUnsavedChangesRef.current) {
      return;
    }

    setSaveState("saving");
    const timeout = window.setTimeout(() => {
      const markdown = turndown.turndown(editor.getHTML());
      onSave(note.id, draftTitle, markdown, draftFolderId || null)
        .then(() => {
          hasUnsavedChangesRef.current = false;
          setSaveState("saved");
        })
        .catch(() => setSaveState("error"));
    }, 650);

    return () => window.clearTimeout(timeout);
  }, [
    draftFolderId,
    draftTitle,
    editor,
    editorRevision,
    note?.deleted_at,
    note?.id,
    onSave,
    turndown,
  ]);

  useEffect(() => {
    return () => {
      if (dictationActiveRef.current) {
        dictationActiveRef.current = false;
        void invoke<AudioCaptureStatus>("stop_audio_capture");
      }
    };
  }, []);

  async function toggleDictation() {
    if (!editor || editor.isDestroyed || !note || note.deleted_at) {
      return;
    }

    if (dictationState === "recording") {
      await stopStreamingDictation();
      return;
    }

    await startStreamingDictation();
  }

  async function startStreamingDictation() {
    if (!editor || editor.isDestroyed) {
      return;
    }

    insertedDictationRef.current = false;
    dictationActiveRef.current = true;
    setStreamingTranscript("");
    setDictationState("recording");
    try {
      await invoke<AudioCaptureStatus>("start_audio_capture");
      toast.info("Dictation started");
      void runDictationLoop();
    } catch (dictationError) {
      dictationActiveRef.current = false;
      setDictationState("idle");
      toast.error(dictationError);
    }
  }

  async function stopStreamingDictation() {
    dictationActiveRef.current = false;
    setDictationState("transcribing");
    try {
      await dictationChunkRef.current;
      await flushAndTranscribeDictationChunk(250);
      await invoke<AudioCaptureStatus>("stop_audio_capture");
      if (!insertedDictationRef.current) {
        toast.info("No speech detected");
      }
    } catch (dictationError) {
      toast.error(dictationError);
      try {
        await invoke<AudioCaptureStatus>("stop_audio_capture");
      } catch {
        // The capture worker will report the meaningful error through the first failure.
      }
    } finally {
      setDictationState("idle");
    }
  }

  async function runDictationLoop() {
    while (dictationActiveRef.current) {
      await wait(DICTATION_CHUNK_MS);
      if (!dictationActiveRef.current) {
        break;
      }
      await flushAndTranscribeDictationChunk(DICTATION_CHUNK_MS - 500);
    }
  }

  async function flushAndTranscribeDictationChunk(minDurationMs: number) {
    if (dictationChunkRef.current) {
      await dictationChunkRef.current;
      return;
    }

    const chunkPromise = (async () => {
      const preview = await invoke<AudioCapturePreview | null>(
        "flush_audio_capture_chunk",
        {
          minDurationMs,
        },
      );
      if (!preview) {
        return;
      }

      const result = await invoke<SttTranscription>("transcribe_capture_file", {
        path: preview.path,
      });
      insertDictationText(result.text);
    })();

    dictationChunkRef.current = chunkPromise;
    try {
      await chunkPromise;
    } finally {
      if (dictationChunkRef.current === chunkPromise) {
        dictationChunkRef.current = null;
      }
    }
  }

  function insertDictationText(text: string) {
    const cleanText = text.trim();
    if (!cleanText || !editor || editor.isDestroyed) {
      return;
    }

    const prefix = insertedDictationRef.current ? " " : "";
    editor.chain().focus().insertContent(`${prefix}${cleanText}`).run();
    insertedDictationRef.current = true;
    setStreamingTranscript((current) =>
      `${current}${prefix}${cleanText}`.trim().slice(-240),
    );
    hasUnsavedChangesRef.current = true;
    setEditorRevision((revision) => revision + 1);
  }

  async function exportCurrentNote() {
    if (!note || !editor || editor.isDestroyed || note.deleted_at) {
      return;
    }

    const markdown = turndown.turndown(editor.getHTML());
    await onExport(note.id, draftTitle, markdown, draftFolderId || null);
    hasUnsavedChangesRef.current = false;
    setSaveState("saved");
  }

  function openEmailDraft() {
    if (!note || !editor || editor.isDestroyed || note.deleted_at) {
      return;
    }

    const markdown = turndown.turndown(editor.getHTML());
    setEmailDraft({
      to: "",
      subject: draftTitle.trim() || "Untitled note",
      body: markdown,
    });
  }

  function openReminderDialog() {
    if (!editor || editor.isDestroyed || !note || note.deleted_at) return;
    const { from, to, empty } = editor.state.selection;
    if (empty) {
      toast.info("Select some note text first");
      return;
    }
    const selectedText = editor.state.doc.textBetween(from, to, "\n", "\n");
    if (!selectedText.trim()) {
      toast.info("Select some note text first");
      return;
    }
    setReminderSelection({
      selectedText,
      startOffset: from,
      endOffset: to,
      contextBefore: editor.state.doc.textBetween(
        Math.max(0, from - 180),
        from,
        "\n",
        "\n",
      ),
      contextAfter: editor.state.doc.textBetween(
        to,
        Math.min(editor.state.doc.content.size, to + 180),
        "\n",
        "\n",
      ),
    });
  }

  async function resolveFocusedReminder(status: "completed" | "dismissed") {
    if (!reminderJumpTarget) return;
    await invoke(
      status === "completed" ? "complete_reminder" : "dismiss_reminder",
      { id: reminderJumpTarget.id },
    );
    announceReminderChange();
    onDismissReminderTarget();
    editor?.commands.setTextSelection(editor.state.selection.to);
  }

  function hideFocusedReminder() {
    onDismissReminderTarget();
    editor?.commands.setTextSelection(editor.state.selection.to);
  }

  async function submitEmailDraft() {
    if (!emailDraft) {
      return;
    }

    setIsCreatingEmailDraft(true);
    try {
      await onCreateGmailDraft({
        to: emailDraft.to?.trim() || null,
        subject: emailDraft.subject,
        body: emailDraft.body,
      });
      setEmailDraft(null);
    } finally {
      setIsCreatingEmailDraft(false);
    }
  }

  if (!note) {
    return (
      <div className="empty-workspace">
        <div className="empty-state">
          <div className="empty-icon">
            <BookOpen size={28} />
          </div>
          <h2>Nothing open</h2>
          <p>Pick a note from the sidebar, or start a fresh one.</p>
          <button
            className="empty-cta"
            type="button"
            onClick={() => void onCreate()}
          >
            <Plus size={16} />
            New note
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="editor-layout">
      <header className="editor-header" data-tauri-drag-region>
        <div className="editor-meta">
          {note.deleted_at ? (
            <>
              <span className="status-pill trash">
                <Trash2 size={14} />
                Trash
              </span>
              <button
                className="secondary-action"
                type="button"
                onClick={() => void onRestore(note.id)}
              >
                <ArchiveRestore size={16} />
                Restore
              </button>
              <button
                className="secondary-action danger"
                type="button"
                onClick={() => void onPermanentDelete(note.id)}
              >
                <X size={16} />
                Delete Forever
              </button>
            </>
          ) : (
            <>
              <div className="pop-wrap" onPointerDown={(event) => event.stopPropagation()}>
                <button
                  type="button"
                  className="folder-select-trigger"
                  onClick={() => setFolderMenuOpen((open) => !open)}
                  aria-haspopup="menu"
                  aria-expanded={folderMenuOpen}
                  title="Move to folder"
                >
                  {draftFolderId ? <Folder size={15} /> : <Inbox size={15} />}
                  <span>
                    {folders.find((folder) => folder.id === draftFolderId)?.name ?? "Inbox"}
                  </span>
                  <ChevronDown className="folder-select-caret" size={14} />
                </button>
                {folderMenuOpen ? (
                  <div className="pop-menu" role="menu">
                    <button
                      type="button"
                      onClick={() => {
                        setFolderMenuOpen(false);
                        setDraftFolderId("");
                        void onMove(note.id, null);
                      }}
                    >
                      <Inbox size={15} />
                      Inbox
                    </button>
                    {folders.map((folder) => (
                      <button
                        key={folder.id}
                        type="button"
                        onClick={() => {
                          setFolderMenuOpen(false);
                          setDraftFolderId(folder.id);
                          void onMove(note.id, folder.id);
                        }}
                      >
                        <Folder size={15} />
                        {folder.name}
                      </button>
                    ))}
                  </div>
                ) : null}
              </div>
              <NoteInfoPopover note={note} />
              {saveState !== "idle" ? (
                <span className={`save-indicator ${saveState}`}>
                  {saveState === "saving"
                    ? "Saving"
                    : saveState === "saved"
                      ? "Saved"
                      : "Save failed"}
                </span>
              ) : null}
              <button
                className="icon-button danger"
                type="button"
                onClick={() => void onTrash(note.id)}
                title="Move to trash"
              >
                <Trash2 size={17} />
              </button>
              <button
                className="icon-button"
                type="button"
                onClick={() => void exportCurrentNote()}
                title="Export note as Markdown"
              >
                <Download size={17} />
              </button>
              <button
                className="icon-button"
                type="button"
                onClick={openEmailDraft}
                title="Create Gmail draft"
              >
                <Mail size={17} />
              </button>
            </>
          )}
          <button
            className={panelOpen ? "icon-button active" : "icon-button"}
            type="button"
            onClick={onTogglePanel}
            title="Toggle details panel (⌘\)"
          >
            <PanelRight size={17} />
          </button>
        </div>
        <input
          className="title-input"
          disabled={Boolean(note.deleted_at)}
          value={draftTitle}
          onChange={(event) => {
            hasUnsavedChangesRef.current = true;
            setDraftTitle(event.currentTarget.value);
          }}
          placeholder="Untitled note"
        />
      </header>

      <div
        className={
          note.deleted_at ? "editor-toolbar disabled" : "editor-toolbar"
        }
      >
        <button
          disabled={
            Boolean(note.deleted_at) || !editor || editor.state.selection.empty
          }
          type="button"
          onClick={openReminderDialog}
          title="Create reminder from selected text"
        >
          <Bell size={16} />
        </button>
        <span className="toolbar-divider" aria-hidden="true" />
        <button
          className={dictationState === "recording" ? "active" : ""}
          disabled={
            Boolean(note.deleted_at) || dictationState === "transcribing"
          }
          type="button"
          onClick={() => void toggleDictation()}
          title={
            dictationState === "recording"
              ? "Stop dictation"
              : dictationState === "transcribing"
                ? "Transcribing"
                : "Start dictation"
          }
        >
          {dictationState === "recording" ? (
            <Square size={15} />
          ) : dictationState === "transcribing" ? (
            <RefreshCw className="spin" size={16} />
          ) : (
            <Mic size={16} />
          )}
        </button>
        <span className="toolbar-divider" aria-hidden="true" />
        {dictationState !== "idle" ? (
          <span className="dictation-status">
            {dictationState === "recording"
              ? streamingTranscript || "Listening..."
              : "Transcribing..."}
          </span>
        ) : null}
        {dictationState !== "idle" ? (
          <span className="toolbar-divider" aria-hidden="true" />
        ) : null}
        <button
          className={editor?.isActive("bold") ? "active" : ""}
          disabled={Boolean(note.deleted_at)}
          type="button"
          onClick={() => editor?.chain().focus().toggleBold().run()}
          title="Bold"
        >
          <Bold size={16} />
        </button>
        <button
          className={editor?.isActive("italic") ? "active" : ""}
          disabled={Boolean(note.deleted_at)}
          type="button"
          onClick={() => editor?.chain().focus().toggleItalic().run()}
          title="Italic"
        >
          <Italic size={16} />
        </button>
        <button
          className={editor?.isActive("strike") ? "active" : ""}
          disabled={Boolean(note.deleted_at)}
          type="button"
          onClick={() => editor?.chain().focus().toggleStrike().run()}
          title="Strikethrough"
        >
          <Strikethrough size={16} />
        </button>
        <span className="toolbar-divider" aria-hidden="true" />
        <button
          className={editor?.isActive("heading", { level: 2 }) ? "active" : ""}
          disabled={Boolean(note.deleted_at)}
          type="button"
          onClick={() =>
            editor?.chain().focus().toggleHeading({ level: 2 }).run()
          }
          title="Heading"
        >
          <Heading2 size={16} />
        </button>
        <button
          className={editor?.isActive("bulletList") ? "active" : ""}
          disabled={Boolean(note.deleted_at)}
          type="button"
          onClick={() => editor?.chain().focus().toggleBulletList().run()}
          title="Bullet list"
        >
          <List size={16} />
        </button>
      </div>

      <EditorContent editor={editor} />
      {reminderJumpTarget && reminderJumpTarget.noteId === note.id ? (
        <aside className="reminder-focus-card">
          <div>
            <Bell size={16} />
            <strong>Reminder</strong>
            <button
              type="button"
              onClick={hideFocusedReminder}
              title="Hide reminder"
            >
              <X size={15} />
            </button>
          </div>
          {reminderJumpTarget.comment ? (
            <p>{reminderJumpTarget.comment}</p>
          ) : null}
          <q>{reminderJumpTarget.selectedText}</q>
          {reminderJumpTarget.status === "pending" ? (
            <footer>
              <button
                type="button"
                onClick={() => void resolveFocusedReminder("dismissed")}
              >
                Dismiss
              </button>
              <button
                type="button"
                onClick={() => void resolveFocusedReminder("completed")}
              >
                <CheckCircle2 size={15} />
                Complete
              </button>
            </footer>
          ) : null}
        </aside>
      ) : null}
      {reminderSelection ? (
        <ReminderCreateDialog
          noteId={note.id}
          selection={reminderSelection}
          onClose={() => setReminderSelection(null)}
          onCreated={() => {
            setReminderSelection(null);
            toast.success("Reminder created");
          }}
        />
      ) : null}
      {emailDraft ? (
        <EmailDraftDialog
          draft={emailDraft}
          isBusy={isCreatingEmailDraft}
          onChange={setEmailDraft}
          onClose={() => setEmailDraft(null)}
          onSubmit={() => void submitEmailDraft()}
        />
      ) : null}
    </div>
  );
}

type EmailDraftDialogProps = {
  draft: GmailDraftInput;
  isBusy: boolean;
  onChange: (draft: GmailDraftInput) => void;
  onClose: () => void;
  onSubmit: () => void;
};

function EmailDraftDialog({
  draft,
  isBusy,
  onChange,
  onClose,
  onSubmit,
}: EmailDraftDialogProps) {
  return (
    <div
      className="email-draft-backdrop"
      role="presentation"
      onMouseDown={onClose}
    >
      <section
        className="email-draft-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="email-draft-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header>
          <div>
            <h2 id="email-draft-title">Gmail draft</h2>
            <p>Create a draft email from this note.</p>
          </div>
          <button
            type="button"
            className="icon-button"
            onClick={onClose}
            title="Close"
          >
            <X size={18} />
          </button>
        </header>

        <label className="settings-field">
          <span>To</span>
          <input
            value={draft.to ?? ""}
            onChange={(event) =>
              onChange({ ...draft, to: event.currentTarget.value })
            }
            placeholder="Optional recipient"
            autoFocus
          />
        </label>
        <label className="settings-field">
          <span>Subject</span>
          <input
            value={draft.subject}
            onChange={(event) =>
              onChange({ ...draft, subject: event.currentTarget.value })
            }
            placeholder="Email subject"
          />
        </label>
        <label className="settings-field">
          <span>Body</span>
          <textarea
            value={draft.body}
            onChange={(event) =>
              onChange({ ...draft, body: event.currentTarget.value })
            }
            rows={12}
          />
        </label>

        <footer>
          <button type="button" onClick={onClose} disabled={isBusy}>
            Cancel
          </button>
          <button
            type="button"
            className="primary"
            onClick={onSubmit}
            disabled={isBusy || !draft.subject.trim()}
          >
            <Mail size={15} />
            {isBusy ? "Creating" : "Create Draft"}
          </button>
        </footer>
      </section>
    </div>
  );
}

function SpeakerRow({
  name,
  onRename,
}: {
  name: string;
  onRename: (oldName: string, newName: string) => void;
}) {
  const [value, setValue] = useState(name);

  useEffect(() => {
    setValue(name);
  }, [name]);

  function commit() {
    const next = value.trim();
    if (next && next !== name) {
      onRename(name, next);
    } else {
      setValue(name);
    }
  }

  return (
    <input
      className="speaker-input"
      value={value}
      onChange={(event) => setValue(event.currentTarget.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          event.currentTarget.blur();
        } else if (event.key === "Escape") {
          setValue(name);
          event.currentTarget.blur();
        }
      }}
      aria-label={`Rename speaker ${name}`}
    />
  );
}

function SpeakersSection({
  content,
  onRename,
}: {
  content: string;
  onRename: (oldName: string, newName: string) => void;
}) {
  const speakers = useMemo(() => parseSpeakers(content), [content]);
  if (speakers.length === 0) {
    return null;
  }
  return (
    <div className="context-section">
      <div className="context-heading">
        <span>Speakers</span>
        <small>{speakers.length}</small>
      </div>
      <div className="speaker-list">
        {speakers.map((name) => (
          <SpeakerRow key={name} name={name} onRename={onRename} />
        ))}
      </div>
    </div>
  );
}

type ContextPanelProps = {
  note: NoteWithContent;
  notes: NoteListItem[];
  linkedNotes: LinkedNote[];
  linkSuggestions: LinkSuggestion[];
  onOpenNote: (id: string) => Promise<void>;
  onLinkNote: (targetId: string, label?: string | null) => Promise<void>;
  onLinkSuggestion: (targetId: string) => Promise<void>;
  onExtractionStatusChange: (noteId: string, status: string) => void;
  onJumpToEntityMention: (surfaceText: string) => void;
  onCreateNoteFromContent: (
    content: string,
    sourcePrompt: string | null,
  ) => void;
  onRenameSpeaker: (oldName: string, newName: string) => void;
  onRenameLink: (
    sourceId: string,
    targetId: string,
    label: string | null,
  ) => Promise<void>;
  onUnlink: (sourceId: string, targetId: string) => Promise<void>;
};

function ContextPanel({
  note,
  notes,
  linkedNotes,
  linkSuggestions,
  onOpenNote,
  onLinkNote,
  onLinkSuggestion,
  onExtractionStatusChange,
  onJumpToEntityMention,
  onCreateNoteFromContent,
  onRenameSpeaker,
  onRenameLink,
  onUnlink,
}: ContextPanelProps) {
  const [tab, setTab] = useState<"details" | "links" | "chat" | "agents">(
    "details",
  );
  const [pickerOpen, setPickerOpen] = useState(false);
  const [linkSearch, setLinkSearch] = useState("");
  const [newLinkLabel, setNewLinkLabel] = useState("");
  const [editingLinkId, setEditingLinkId] = useState<string | null>(null);
  const [linkLabelDraft, setLinkLabelDraft] = useState("");
  const linkCount = linkedNotes.length + linkSuggestions.length;
  const linkedIds = useMemo(
    () => new Set(linkedNotes.map((linked) => linked.note.id)),
    [linkedNotes],
  );
  const linkCandidates = useMemo(() => {
    const query = linkSearch.trim().toLowerCase();
    return notes
      .filter((candidate) => {
        if (
          candidate.deleted_at ||
          candidate.id === note.id ||
          linkedIds.has(candidate.id)
        ) {
          return false;
        }

        if (!query) {
          return true;
        }

        return (
          candidate.title.toLowerCase().includes(query) ||
          candidate.excerpt.toLowerCase().includes(query)
        );
      })
      .slice(0, 20);
  }, [linkSearch, linkedIds, note.id, notes]);

  useEffect(() => {
    setPickerOpen(false);
    setLinkSearch("");
    setNewLinkLabel("");
    setEditingLinkId(null);
    setLinkLabelDraft("");
  }, [note.id]);

  async function addManualLink(targetId: string) {
    await onLinkNote(targetId, newLinkLabel);
    setPickerOpen(false);
    setLinkSearch("");
    setNewLinkLabel("");
  }

  function beginRenameLink(linked: LinkedNote) {
    if (linked.link.link_kind === "entity_sharing") {
      return;
    }

    setEditingLinkId(linked.note.id);
    setLinkLabelDraft(linked.link.label ?? "");
  }

  async function submitRenameLink(linked: LinkedNote) {
    await onRenameLink(note.id, linked.note.id, linkLabelDraft);
    setEditingLinkId(null);
    setLinkLabelDraft("");
  }

  return (
    <aside className="context-panel">
      <div className="panel-tabs">
        <div className="segmented" role="tablist">
          <button
            type="button"
            role="tab"
            aria-selected={tab === "details"}
            className={tab === "details" ? "active" : ""}
            onClick={() => setTab("details")}
          >
            Details
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={tab === "links"}
            className={tab === "links" ? "active" : ""}
            onClick={() => setTab("links")}
          >
            Links
            {linkCount > 0 ? <small>{linkCount}</small> : null}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={tab === "chat"}
            className={tab === "chat" ? "active" : ""}
            onClick={() => setTab("chat")}
          >
            Chat
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={tab === "agents"}
            className={tab === "agents" ? "active" : ""}
            onClick={() => setTab("agents")}
          >
            Agents
          </button>
        </div>
      </div>

      <div className="panel-pane" hidden={tab !== "details"}>
        <SpeakersSection content={note.content} onRename={onRenameSpeaker} />
        <EntityStrip
          note={note}
          onStatusChange={onExtractionStatusChange}
          onJumpToMention={onJumpToEntityMention}
        />
      </div>

      <div className="panel-pane chat" hidden={tab !== "chat"}>
        <NoteChat
          key={note.id}
          noteId={note.id}
          noteContent={note.content}
          onCreateNote={onCreateNoteFromContent}
        />
      </div>

      <div className="panel-pane" hidden={tab !== "agents"}>
        <NoteAgentsPanel key={note.id} note={note} />
      </div>

      <div className="panel-pane" hidden={tab !== "links"}>
        <div className="context-section">
          <div className="context-heading">
            <span>Linked notes</span>
            <small>{linkedNotes.length}</small>
            <button
              className={pickerOpen ? "ghost-icon active" : "ghost-icon"}
              type="button"
              onClick={() => setPickerOpen((open) => !open)}
              title="Link note"
            >
              <Plus size={14} />
            </button>
          </div>
          {pickerOpen ? (
            <div className="link-picker">
              <input
                className="link-picker-input"
                value={linkSearch}
                onChange={(event) => setLinkSearch(event.target.value)}
                placeholder="Find a note"
                autoFocus
              />
              <input
                className="link-picker-input"
                value={newLinkLabel}
                onChange={(event) => setNewLinkLabel(event.target.value)}
                placeholder="Link name (optional)"
              />
              <div className="link-picker-list">
                {linkCandidates.length === 0 ? (
                  <p className="context-empty">No available notes</p>
                ) : (
                  linkCandidates.map((candidate) => (
                    <button
                      className="link-picker-row"
                      key={candidate.id}
                      type="button"
                      onClick={() => void addManualLink(candidate.id)}
                    >
                      <span>{candidate.title || "Untitled"}</span>
                      <small>
                        {candidate.excerpt || formatTime(candidate.updated_at)}
                      </small>
                    </button>
                  ))
                )}
              </div>
            </div>
          ) : null}
          {linkedNotes.length === 0 ? (
            <p className="context-empty">No linked notes yet</p>
          ) : (
            <div className="context-links">
              {linkedNotes.map((linked) => (
                <div className="linked-row" key={linked.note.id}>
                  {editingLinkId === linked.note.id ? (
                    <form
                      className="linked-label-editor"
                      onSubmit={(event) => {
                        event.preventDefault();
                        void submitRenameLink(linked);
                      }}
                    >
                      <input
                        value={linkLabelDraft}
                        onChange={(event) =>
                          setLinkLabelDraft(event.target.value)
                        }
                        placeholder="Link name"
                        autoFocus
                      />
                      <button type="submit" title="Save link name">
                        <CheckCircle2 size={15} />
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          setEditingLinkId(null);
                          setLinkLabelDraft("");
                        }}
                        title="Cancel"
                      >
                        <X size={15} />
                      </button>
                    </form>
                  ) : (
                    <button
                      type="button"
                      onClick={() => void onOpenNote(linked.note.id)}
                    >
                      <span>{linked.note.title || "Untitled"}</span>
                      <small>
                        {linked.link.label
                          ? `${linked.link.label} · ${formatTime(linked.note.updated_at)}`
                          : formatTime(linked.note.updated_at)}
                      </small>
                    </button>
                  )}
                  {linked.link.link_kind === "entity_sharing" ? null : (
                    <button
                      className="ghost-icon"
                      type="button"
                      onClick={() => beginRenameLink(linked)}
                      title="Rename link"
                    >
                      <FileText size={15} />
                    </button>
                  )}
                  <button
                    className="ghost-icon danger"
                    type="button"
                    onClick={() => void onUnlink(note.id, linked.note.id)}
                    title="Unlink note"
                  >
                    <Unlink size={15} />
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>

        <div className="context-section">
          <div className="context-heading">
            <span>Suggested links</span>
            <small>{linkSuggestions.length}</small>
          </div>
          {linkSuggestions.length === 0 ? (
            <p className="context-empty">No entity matches yet</p>
          ) : (
            <div className="context-links">
              {linkSuggestions.map((suggestion) => (
                <div className="suggested-row" key={suggestion.note.id}>
                  <button
                    className="suggested-main"
                    type="button"
                    onClick={() => void onOpenNote(suggestion.note.id)}
                  >
                    <span>{suggestion.note.title || "Untitled"}</span>
                    <small>
                      {suggestion.shared_entity_count} shared{" "}
                      {suggestion.shared_entity_count === 1
                        ? "entity"
                        : "entities"}
                    </small>
                    <span className="suggested-entities">
                      {suggestion.shared_entities.map((entity) => (
                        <b key={entity.id}>{entity.name}</b>
                      ))}
                    </span>
                  </button>
                  <button
                    className="ghost-icon"
                    type="button"
                    onClick={() => void onLinkSuggestion(suggestion.note.id)}
                    title="Link suggested note"
                  >
                    <Link2 size={15} />
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </aside>
  );
}

export default App;
