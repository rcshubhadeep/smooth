import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import Placeholder from "@tiptap/extension-placeholder";
import { EditorContent, useEditor } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import {
  ArchiveRestore,
  ArrowLeft,
  ArrowDownUp,
  Bold,
  BookOpen,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  CircleAlert,
  Database,
  FileText,
  Folder,
  FolderPlus,
  FolderOpen,
  Heading2,
  Inbox,
  Info,
  Italic,
  Link2,
  List,
  Mic,
  Monitor,
  Moon,
  PanelRight,
  Play,
  Plus,
  RefreshCw,
  Search,
  Server,
  Settings,
  Sparkles,
  Square,
  Strikethrough,
  Sun,
  Trash2,
  Unlink,
  X,
} from "lucide-react";
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
import "./App.css";

type ThemeMode = "light" | "dark" | "system";
type ViewMode = "notes" | "settings";
type SaveState = "idle" | "saving" | "saved" | "error";
type SortMode = "updated-desc" | "updated-asc" | "created-desc" | "created-asc";
type DictationState = "idle" | "recording" | "transcribing";

type Folder = {
  id: string;
  name: string;
  created_at: string;
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
};

type BankSnapshot = {
  notes: NoteListItem[];
  folders: Folder[];
  links: NoteLink[];
};

type LlamaConfig = {
  base_url: string;
  preferred_model: string | null;
};

type LlamaModel = {
  id: string;
  owned_by: string | null;
  context_size: number | null;
  parameter_count: number | null;
  size_bytes: number | null;
};

type LlamaStatus = {
  state: "offline" | "loading" | "ready" | "error";
  base_url: string;
  message: string;
  latency_ms: number | null;
  checked_at: string;
  models: LlamaModel[];
};

type ExtractionQueueStatus = {
  pending: number;
  processing: number;
  failed: number;
  indexed: number;
  not_indexed: number;
};

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
  model_path: string;
};

type NoteEntity = {
  id: number;
  name: string;
  entity_type: string;
  mention_count: number;
};

type NoteExtractionView = {
  status: string;
  error: string | null;
  entities: NoteEntity[];
};

type LinkSuggestion = {
  note: NoteListItem;
  shared_entities: NoteEntity[];
  shared_entity_count: number;
  shared_mention_count: number;
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

function formatLargeValue(value: number | null, suffix = "") {
  if (value === null) {
    return null;
  }

  if (value >= 1_000_000_000) {
    return `${(value / 1_000_000_000).toFixed(1)}B${suffix}`;
  }
  if (value >= 1_000_000) {
    return `${(value / 1_000_000).toFixed(1)}M${suffix}`;
  }
  return `${value.toLocaleString()}${suffix}`;
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

function formatDb(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return "silent";
  }
  return `${value.toFixed(1)} dB`;
}

function wait(ms: number) {
  return new Promise<void>((resolve) => {
    window.setTimeout(resolve, ms);
  });
}

function sortNotes(notes: NoteListItem[], mode: SortMode) {
  return [...notes].sort((first, second) => {
    const [field, direction] = mode.split("-") as ["updated" | "created", "asc" | "desc"];
    const firstValue = dateValue(field === "updated" ? first.updated_at : first.created_at);
    const secondValue = dateValue(field === "updated" ? second.updated_at : second.created_at);
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
  return String(value).replace(/^Error:\s*/i, "").trim();
}

const toast = {
  error: (message: unknown) => toastStore.push("error", cleanErrorMessage(message)),
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
  const [query, setQuery] = useState("");
  const [newFolderName, setNewFolderName] = useState("");
  const [isFolderFormOpen, setIsFolderFormOpen] = useState(false);
  const [collapsedSections, setCollapsedSections] = useState<string[]>([]);
  const [sortMode, setSortMode] = useState<SortMode>(() => {
    return (localStorage.getItem("smooth-note-sort") as SortMode | null) ?? "updated-desc";
  });
  const [theme, setTheme] = useState<ThemeMode>(() => {
    return (localStorage.getItem("smooth-theme") as ThemeMode | null) ?? "system";
  });
  const [view, setView] = useState<ViewMode>("notes");
  const setError = (message: string | null) => {
    if (message) {
      toast.error(message);
    }
  };
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [panelOpen, setPanelOpen] = useState(true);
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
  const searchRef = useRef<HTMLInputElement>(null);

  const MIN_EDITOR = 360;

  function resizeSidebar(clientX: number) {
    if (clientX < 0) {
      setSidebarWidth(340);
      return;
    }
    const reserved = activeNote && panelOpen ? panelWidth : 0;
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
    () => sortNotes(snapshot.notes.filter((note) => note.deleted_at), sortMode),
    [snapshot.notes, sortMode],
  );

  const filteredNotes = useMemo(() => {
    const cleanQuery = query.trim().toLowerCase();
    if (!cleanQuery) {
      return sortNotes(activeNotes, sortMode);
    }

    return sortNotes(
      activeNotes.filter((note) => {
        const folderName =
          snapshot.folders.find((folder) => folder.id === note.folder_id)?.name ?? "";
        return `${note.title} ${note.excerpt} ${folderName}`
          .toLowerCase()
          .includes(cleanQuery);
      }),
      sortMode,
    );
  }, [activeNotes, query, snapshot.folders, sortMode]);

  const folderGroups = useMemo(() => {
    return snapshot.folders.map((folder) => ({
      folder,
      notes: filteredNotes.filter((note) => note.folder_id === folder.id),
    }));
  }, [filteredNotes, snapshot.folders]);

  const inboxNotes = filteredNotes.filter((note) => !note.folder_id);

  const paletteNotes = useMemo(
    () => sortNotes(snapshot.notes.filter((note) => !note.deleted_at), "updated-desc"),
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
        setPaletteOpen(false);
        searchRef.current?.focus();
      } else if (meta && event.key === "\\") {
        event.preventDefault();
        setPanelOpen((open) => !open);
      }
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  function isSectionOpen(id: string) {
    return !collapsedSections.includes(id);
  }

  function toggleSection(id: string) {
    setCollapsedSections((current) =>
      current.includes(id)
        ? current.filter((sectionId) => sectionId !== id)
        : [...current, id],
    );
  }

  const linkedNotes = useMemo(() => {
    const sourceIds =
      activeNote?.id !== undefined
        ? [activeNote.id]
        : selectedIds.length > 0
          ? selectedIds
          : [];

    const linkedIds = new Set<string>();
    for (const link of snapshot.links) {
      if (sourceIds.length === 0) {
        linkedIds.add(link.source_id);
        linkedIds.add(link.target_id);
      } else if (sourceIds.includes(link.source_id)) {
        linkedIds.add(link.target_id);
      } else if (sourceIds.includes(link.target_id)) {
        linkedIds.add(link.source_id);
      }
    }

    return snapshot.notes.filter((note) => linkedIds.has(note.id) && !note.deleted_at);
  }, [activeNote?.id, selectedIds, snapshot.links, snapshot.notes]);

  const loadBank = useCallback(async () => {
    const bank = await invoke<BankSnapshot>("get_bank");
    setSnapshot(bank);
    return bank;
  }, []);

  useEffect(() => {
    loadBank().catch((loadError: unknown) => setError(String(loadError)));
  }, [loadBank]);

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
      setView("notes");
    } catch (openError) {
      setError(String(openError));
    }
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

  const saveNote = useCallback(async (
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
    setActiveNote((current) => (current?.id === id ? saved : current));
    setSnapshot((current) => ({
      ...current,
      notes: current.notes.map((note) =>
        note.id === id
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
    return saved;
  }, []);

  async function createFolder() {
    if (!newFolderName.trim()) {
      return;
    }

    try {
      setError(null);
      const bank = await invoke<BankSnapshot>("create_folder", { name: newFolderName });
      setSnapshot(bank);
      setNewFolderName("");
      setIsFolderFormOpen(false);
    } catch (folderError) {
      setError(String(folderError));
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
      const name = folderId
        ? (snapshot.folders.find((folder) => folder.id === folderId)?.name ?? "folder")
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
    if (dropTarget?.folderId === target?.folderId && dropTarget?.index === target?.index) {
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

    const index = Math.max(0, Math.min(dropTarget?.index ?? rows.length, rows.length));
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

  async function trashNote(id: string) {
    try {
      setError(null);
      const bank = await invoke<BankSnapshot>("trash_note", { id });
      setSnapshot(bank);
      setSelectedIds((current) => current.filter((selectedId) => selectedId !== id));
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
      setSelectedIds((current) => current.filter((selectedId) => selectedId !== id));
      setActiveNote((current) => (current?.id === id ? null : current));
    } catch (deleteError) {
      setError(String(deleteError));
    }
  }

  async function linkSelectedNotes() {
    try {
      setError(null);
      const bank = await invoke<BankSnapshot>("link_notes", { ids: selectedIds });
      setSnapshot(bank);
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

  async function unlinkNotes(sourceId: string, targetId: string) {
    try {
      setError(null);
      const bank = await invoke<BankSnapshot>("unlink_notes", { sourceId, targetId });
      setSnapshot(bank);
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

  const updateNoteExtractionStatus = useCallback((id: string, status: string) => {
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
  }, []);

  function cycleTheme() {
    setTheme((current) =>
      current === "system" ? "light" : current === "light" ? "dark" : "system",
    );
  }

  const ThemeIcon = theme === "system" ? Monitor : theme === "light" ? Sun : Moon;

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
          <h1>Knowledge Bank</h1>
          <button className="icon-button primary" type="button" onClick={createNote} title="New note (⌘N)">
            <Plus size={18} />
          </button>
        </div>

        <div className="sidebar-controls">
          <label className="search-field">
            <Search size={16} />
            <input
              ref={searchRef}
              value={query}
              onChange={(event) => setQuery(event.currentTarget.value)}
              placeholder="Search notes"
            />
          </label>

          <div className="controls-row">
            <button
              className="ghost-button"
              type="button"
              onClick={() => setIsFolderFormOpen((isOpen) => !isOpen)}
            >
              <FolderPlus size={16} />
              New folder
            </button>

            <label className="sort-control">
              <ArrowDownUp size={15} />
              <select
                value={sortMode}
                onChange={(event) => setSortMode(event.currentTarget.value as SortMode)}
                aria-label="Sort notes"
              >
                <option value="updated-desc">Updated newest</option>
                <option value="updated-asc">Updated oldest</option>
                <option value="created-desc">Created newest</option>
                <option value="created-asc">Created oldest</option>
              </select>
            </label>
          </div>

          {isFolderFormOpen ? (
            <form
              className="folder-form"
              onSubmit={(event) => {
                event.preventDefault();
                void createFolder();
              }}
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.preventDefault();
                  setNewFolderName("");
                  setIsFolderFormOpen(false);
                }
              }}
            >
              <input
                value={newFolderName}
                onChange={(event) => setNewFolderName(event.currentTarget.value)}
                placeholder="Folder name"
              />
              <button type="submit">Create</button>
            </form>
          ) : null}

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
                Link
              </button>
              <select
                aria-label="Move selected notes"
                defaultValue="__move__"
                onChange={(event) => {
                  const folderId =
                    event.currentTarget.value === "__inbox__"
                      ? null
                      : event.currentTarget.value;
                  event.currentTarget.value = "__move__";
                  void moveSelected(folderId);
                }}
              >
                <option value="__move__" disabled>
                  Move
                </option>
                <option value="__inbox__">Inbox</option>
                {snapshot.folders.map((folder) => (
                  <option key={folder.id} value={folder.id}>
                    {folder.name}
                  </option>
                ))}
              </select>
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
                isSectionOpen(folder.id) ? <FolderOpen size={16} /> : <Folder size={16} />
              }
              isOpen={isSectionOpen(folder.id)}
              onToggle={() => toggleSection(folder.id)}
              onDropTargetChange={updateDropTarget}
              title={folder.name}
              droppable
              onDropNote={(id) => void moveNoteToFolder(id, folder.id)}
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
                  onContextMenu={(target, x, y) => setMenuTarget({ note: target, x, y })}
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
        </div>

        <div className="sidebar-footer">
          <button
            className={view === "settings" ? "icon-button active" : "icon-button"}
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
          <SettingsView onClose={() => setView("notes")} />
        ) : (
          <div
            className={
              activeNote && panelOpen ? "notes-workspace with-panel" : "notes-workspace"
            }
            style={
              activeNote && panelOpen
                ? { gridTemplateColumns: `minmax(0, 1fr) ${panelWidth}px` }
                : undefined
            }
          >
            <NoteEditor
              note={activeNote}
              folders={snapshot.folders}
              panelOpen={panelOpen}
              onTogglePanel={() => setPanelOpen((open) => !open)}
              onCreate={createNote}
              onSave={saveNote}
              onTrash={trashNote}
              onRestore={restoreNote}
              onPermanentDelete={permanentDeleteNote}
              onMove={moveNote}
            />
            {activeNote && panelOpen ? (
              <>
                <ResizeHandle
                  side="right"
                  ariaLabel="Resize details panel"
                  style={{ right: panelWidth }}
                  onResize={resizePanel}
                />
                <ContextPanel
                  note={activeNote}
                  linkedNotes={linkedNotes}
                  linkSuggestions={linkSuggestions}
                  onOpenNote={openNote}
                  onLinkSuggestion={linkSuggestedNote}
                  onExtractionStatusChange={updateNoteExtractionStatus}
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

      <ToastViewport />
    </div>
  );
}

type CommandPaletteProps = {
  notes: NoteListItem[];
  onClose: () => void;
  onOpenNote: (id: string) => void;
  onCreateNote: () => void;
  onOpenSettings: () => void;
  onToggleTheme: () => void;
};

type PaletteItem =
  | { kind: "command"; id: string; label: string; icon: React.ReactNode; run: () => void }
  | { kind: "note"; id: string; label: string; sub: string };

function CommandPalette({
  notes,
  onClose,
  onOpenNote,
  onCreateNote,
  onOpenSettings,
  onToggleTheme,
}: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const activeRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const cleanQuery = query.trim().toLowerCase();

  const commands = useMemo<PaletteItem[]>(
    () => [
      { kind: "command", id: "new-note", label: "New note", icon: <Plus size={16} />, run: onCreateNote },
      { kind: "command", id: "settings", label: "Open settings", icon: <Settings size={16} />, run: onOpenSettings },
      { kind: "command", id: "theme", label: "Toggle theme", icon: <Sun size={16} />, run: onToggleTheme },
    ],
    [onCreateNote, onOpenSettings, onToggleTheme],
  );

  const items = useMemo<PaletteItem[]>(() => {
    const matchedCommands = cleanQuery
      ? commands.filter((command) => command.label.toLowerCase().includes(cleanQuery))
      : commands;
    const matchedNotes = (
      cleanQuery
        ? notes.filter((note) =>
            `${note.title} ${note.excerpt}`.toLowerCase().includes(cleanQuery),
          )
        : notes
    )
      .slice(0, 50)
      .map<PaletteItem>((note) => ({
        kind: "note",
        id: note.id,
        label: note.title || "Untitled",
        sub: note.excerpt || "No content",
      }));
    return [...matchedCommands, ...matchedNotes];
  }, [cleanQuery, commands, notes]);

  useEffect(() => {
    setActiveIndex(0);
  }, [cleanQuery]);

  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest" });
  }, [activeIndex]);

  function activate(item: PaletteItem) {
    if (item.kind === "command") {
      item.run();
    } else {
      onOpenNote(item.id);
    }
    onClose();
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex((index) => Math.min(index + 1, Math.max(items.length - 1, 0)));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex((index) => Math.max(index - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      const item = items[activeIndex];
      if (item) {
        activate(item);
      }
    } else if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    }
  }

  return (
    <div className="palette-overlay" onMouseDown={onClose}>
      <div
        className="palette"
        role="dialog"
        aria-modal="true"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="palette-input">
          <Search size={18} />
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => setQuery(event.currentTarget.value)}
            onKeyDown={onKeyDown}
            placeholder="Search notes or run a command…"
            aria-label="Command palette"
          />
          <kbd>esc</kbd>
        </div>
        <div className="palette-list">
          {items.length === 0 ? (
            <p className="palette-empty">No matches</p>
          ) : (
            items.map((item, index) => (
              <button
                key={`${item.kind}:${item.id}`}
                ref={index === activeIndex ? activeRef : null}
                className={index === activeIndex ? "palette-item active" : "palette-item"}
                type="button"
                onMouseMove={() => setActiveIndex(index)}
                onClick={() => activate(item)}
              >
                <span className="palette-item-icon">
                  {item.kind === "command" ? item.icon : <FileText size={16} />}
                </span>
                <span className="palette-item-body">
                  <span className="palette-item-label">{item.label}</span>
                  {item.kind === "note" ? <small>{item.sub}</small> : null}
                </span>
                {item.kind === "command" ? (
                  <span className="palette-item-tag">Command</span>
                ) : null}
              </button>
            ))
          )}
        </div>
      </div>
    </div>
  );
}

type SettingsViewProps = {
  onClose: () => void;
};

function SettingsView({ onClose }: SettingsViewProps) {
  const [config, setConfig] = useState<LlamaConfig>({
    base_url: "http://127.0.0.1:8080",
    preferred_model: null,
  });
  const [status, setStatus] = useState<LlamaStatus | null>(null);
  const [queueStatus, setQueueStatus] = useState<ExtractionQueueStatus | null>(null);
  const [audioStatus, setAudioStatus] = useState<AudioCaptureStatus | null>(null);
  const [sttConfig, setSttConfig] = useState<SttConfig>({
    model_path: "",
    language: "en",
    threads: 4,
  });
  const [sttStatus, setSttStatus] = useState<SttStatus | null>(null);
  const [transcription, setTranscription] = useState<SttTranscription | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [isChecking, setIsChecking] = useState(false);
  const [isQueueBusy, setIsQueueBusy] = useState(false);
  const [isAudioBusy, setIsAudioBusy] = useState(false);
  const [isSttBusy, setIsSttBusy] = useState(false);
  const setSettingsError = (message: string | null) => {
    if (message) {
      toast.error(message);
    }
  };

  const checkStatus = useCallback(async () => {
    setIsChecking(true);
    setSettingsError(null);
    try {
      const nextStatus = await invoke<LlamaStatus>("get_llama_status");
      setStatus(nextStatus);
      return nextStatus;
    } catch (statusError) {
      setSettingsError(String(statusError));
      return null;
    } finally {
      setIsChecking(false);
    }
  }, []);

  const refreshAudioStatus = useCallback(async () => {
    const nextAudioStatus = await invoke<AudioCaptureStatus>("get_audio_capture_status");
    setAudioStatus(nextAudioStatus);
    return nextAudioStatus;
  }, []);

  const refreshSttStatus = useCallback(async () => {
    const nextStatus = await invoke<SttStatus>("get_stt_status");
    setSttStatus(nextStatus);
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
      invoke<LlamaConfig>("get_llama_config").then((savedConfig) => {
        setConfig(savedConfig);
        return checkStatus();
      }),
      refreshQueueStatus(),
      refreshAudioStatus(),
      invoke<SttConfig>("get_stt_config").then((savedConfig) => {
        setSttConfig(savedConfig);
        return refreshSttStatus();
      }),
    ])
      .catch((loadError) => setSettingsError(String(loadError)))
      .finally(() => setIsLoading(false));
  }, [checkStatus, refreshAudioStatus, refreshQueueStatus, refreshSttStatus]);

  useEffect(() => {
    const interval = window.setInterval(() => {
      void refreshQueueStatus();
    }, 3000);
    return () => window.clearInterval(interval);
  }, [refreshQueueStatus]);

  useEffect(() => {
    if (!audioStatus?.is_recording) {
      return undefined;
    }

    const interval = window.setInterval(() => {
      void refreshAudioStatus();
    }, 500);
    return () => window.clearInterval(interval);
  }, [audioStatus?.is_recording, refreshAudioStatus]);

  async function saveAndTest() {
    setIsChecking(true);
    setSettingsError(null);
    try {
      const savedConfig = await invoke<LlamaConfig>("save_llama_config", { config });
      setConfig(savedConfig);
      const nextStatus = await invoke<LlamaStatus>("get_llama_status");
      setStatus(nextStatus);
    } catch (saveError) {
      setSettingsError(String(saveError));
    } finally {
      setIsChecking(false);
    }
  }

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
      const nextAudioStatus = await invoke<AudioCaptureStatus>("start_audio_capture");
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
      const nextAudioStatus = await invoke<AudioCaptureStatus>("stop_audio_capture");
      setAudioStatus(nextAudioStatus);
    } catch (audioError) {
      setSettingsError(String(audioError));
    } finally {
      setIsAudioBusy(false);
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

  const StatusIcon =
    status?.state === "ready"
      ? CheckCircle2
      : status?.state === "loading"
        ? RefreshCw
        : CircleAlert;
  const SttStatusIcon = sttStatus?.state === "ready" ? CheckCircle2 : CircleAlert;
  const audioPreviewUrl = audioStatus?.last_preview
    ? convertFileSrc(audioStatus.last_preview.path)
    : null;

  return (
    <div className="settings-view">
      <header className="settings-header" data-tauri-drag-region>
        <div>
          <p className="eyebrow">Settings</p>
          <h2>Local AI</h2>
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
              void checkStatus();
              void refreshQueueStatus();
              void refreshAudioStatus();
              void refreshSttStatus();
            }}
            disabled={isChecking || isLoading}
            title="Refresh connection status"
          >
            <RefreshCw className={isChecking ? "spin" : ""} size={17} />
          </button>
        </div>
      </header>

      <section className="settings-section">
        <div className="section-heading">
          <Mic size={18} />
          <span>Audio capture</span>
          <small>{audioStatus?.is_recording ? "Recording" : "Idle"}</small>
        </div>

        <div className={audioStatus?.is_recording ? "audio-capture-card recording" : "audio-capture-card"}>
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
              <small>{formatDuration(audioStatus.last_preview.duration_ms)}</small>
            </div>
            <audio controls src={audioPreviewUrl} />
          </div>
        ) : null}
      </section>

      <section className="settings-section">
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
              onChange={(event) =>
                setSttConfig((current) => ({
                  ...current,
                  model_path: event.currentTarget.value,
                }))
              }
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
              onChange={(event) =>
                setSttConfig((current) => ({
                  ...current,
                  language: event.currentTarget.value || null,
                }))
              }
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
              onChange={(event) =>
                setSttConfig((current) => ({
                  ...current,
                  threads: Number(event.currentTarget.value) || 1,
                }))
              }
            />
          </label>
        </div>

        <div className={`connection-status ${sttStatus?.state === "ready" ? "ready" : "offline"}`}>
          <SttStatusIcon size={19} />
          <div>
            <strong>{sttStatus?.state?.replace("_", " ") ?? "not configured"}</strong>
            <span>{sttStatus?.message ?? "Choose a Whisper ggml model file"}</span>
          </div>
          <small>{sttStatus?.threads ?? sttConfig.threads} threads</small>
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
                {transcription.raw_segment_count} segments
              </small>
            </div>
            <small className="transcript-diagnostics">
              language {transcription.language ?? `auto:${transcription.language_id}`} · rms{" "}
              {formatDb(transcription.audio.rms_db)} · peak{" "}
              {formatDb(transcription.audio.peak_db)}
            </small>
            <p>{transcription.text || "No speech detected"}</p>
          </div>
        ) : null}
      </section>

      <section className="settings-section">
        <div className="section-heading">
          <Server size={18} />
          <span>llama.cpp server</span>
        </div>

        <label className="settings-field">
          <span>Server URL</span>
          <div className="settings-input-row">
            <input
              value={config.base_url}
              onChange={(event) =>
                setConfig((current) => ({
                  ...current,
                  base_url: event.currentTarget.value,
                }))
              }
              placeholder="http://127.0.0.1:8080"
            />
            <button
              type="button"
              onClick={() => void saveAndTest()}
              disabled={isChecking || isLoading}
            >
              {isChecking ? "Checking" : "Save & Test"}
            </button>
          </div>
        </label>


        <div className={`connection-status ${status?.state ?? "offline"}`}>
          <StatusIcon className={status?.state === "loading" ? "spin" : ""} size={19} />
          <div>
            <strong>{status?.state ?? (isLoading ? "checking" : "offline")}</strong>
            <span>{status?.message ?? "Checking llama.cpp connection"}</span>
          </div>
          {status?.latency_ms !== null && status?.latency_ms !== undefined ? (
            <small>{status.latency_ms} ms</small>
          ) : null}
        </div>
      </section>

      <section className="settings-section">
        <div className="section-heading">
          <span>Model</span>
          <small>{status?.models.length ?? 0} discovered</small>
        </div>

        <label className="settings-field">
          <span>Preferred model</span>
          <select
            value={config.preferred_model ?? ""}
            onChange={(event) =>
              setConfig((current) => ({
                ...current,
                preferred_model: event.currentTarget.value || null,
              }))
            }
            disabled={!status?.models.length}
          >
            <option value="">Server default</option>
            {status?.models.map((model) => (
              <option key={model.id} value={model.id}>
                {model.id}
              </option>
            ))}
          </select>
        </label>

        {status?.models.map((model) => (
          <div className="model-row" key={model.id}>
            <div>
              <strong>{model.id}</strong>
              <span>{model.owned_by ?? "llama.cpp"}</span>
            </div>
            <div className="model-meta">
              {formatLargeValue(model.parameter_count, " params") ? (
                <span>{formatLargeValue(model.parameter_count, " params")}</span>
              ) : null}
              {model.context_size ? (
                <span>{model.context_size.toLocaleString()} context</span>
              ) : null}
            </div>
          </div>
        ))}
      </section>

      <section className="settings-section">
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
}: TreeSectionProps) {
  const [isDragOver, setIsDragOver] = useState(false);

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
    if (nextTarget instanceof Node && event.currentTarget.contains(nextTarget)) {
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
      <button
        className={isDragOver ? "tree-header drag-over" : "tree-header"}
        type="button"
        onClick={onToggle}
      >
        {isOpen ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
        {icon}
        <span>{title}</span>
        <small>{count}</small>
      </button>
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
              event.dataTransfer.setData("application/x-smooth-note-id", note.id);
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
      <button className="note-main" type="button" onClick={() => void onOpen(note.id)}>
        <span>{note.title}</span>
        <small>{note.excerpt || "No content"}</small>
      </button>
      {action}
    </div>
  );
}

function DropPlaceholder() {
  return (
    <div className="drop-placeholder" data-drop-placeholder="true" aria-hidden="true">
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
          <button type="button" className="danger" onClick={run(() => onDelete(note.id))}>
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
          <button type="button" className="danger" onClick={run(() => onTrash(note.id))}>
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
};

function EntityStrip({ note, onStatusChange }: EntityStripProps) {
  const [extraction, setExtraction] = useState<NoteExtractionView>({
    status: note.extraction_status,
    error: null,
    entities: [],
  });
  const [isQueuing, setIsQueuing] = useState(false);
  const [isExpanded, setIsExpanded] = useState(false);

  const refreshExtraction = useCallback(async () => {
    const nextExtraction = await invoke<NoteExtractionView>("get_note_extraction", {
      id: note.id,
    });
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
      await invoke("enqueue_note_extraction", { id: note.id });
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

  const statusLabel = extraction.status.replace("_", " ");
  const canQueue =
    !note.deleted_at &&
    !["queued", "processing"].includes(extraction.status) &&
    note.content.trim().length > 0;
  const visibleEntities = isExpanded
    ? extraction.entities
    : extraction.entities.slice(0, ENTITY_PREVIEW_LIMIT);
  const hiddenEntityCount = Math.max(0, extraction.entities.length - ENTITY_PREVIEW_LIMIT);

  return (
    <section className="entity-strip">
      <div className="entity-strip-header">
        <Sparkles size={14} />
        <span>Entities</span>
        <small className={`entity-status ${extraction.status}`}>{statusLabel}</small>
        {canQueue ? (
          <button type="button" onClick={() => void queueExtraction()} disabled={isQueuing}>
            {extraction.status === "failed" ? "Retry" : "Extract"}
          </button>
        ) : null}
      </div>
      {extraction.error ? <p className="entity-error">{extraction.error}</p> : null}
      {extraction.entities.length > 0 ? (
        <div className="entity-chips">
          {visibleEntities.map((entity) => (
            <span className="entity-chip" key={entity.id} title={entity.entity_type}>
              <small>{entity.entity_type}</small>
              {entity.name}
              {entity.mention_count > 1 ? <b>{entity.mention_count}</b> : null}
            </span>
          ))}
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
};

function NoteEditor({
  note,
  folders,
  panelOpen,
  onTogglePanel,
  onCreate,
  onSave,
  onTrash,
  onRestore,
  onPermanentDelete,
  onMove,
}: NoteEditorProps) {
  const [draftTitle, setDraftTitle] = useState("");
  const [draftFolderId, setDraftFolderId] = useState("");
  const [editorRevision, setEditorRevision] = useState(0);
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [dictationState, setDictationState] = useState<DictationState>("idle");
  const [streamingTranscript, setStreamingTranscript] = useState("");
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

  const editor = useEditor({
    extensions: [
      StarterKit,
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
      const preview = await invoke<AudioCapturePreview | null>("flush_audio_capture_chunk", {
        minDurationMs,
      });
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

  if (!note) {
    return (
      <div className="empty-workspace">
        <div className="empty-state">
          <div className="empty-icon">
            <BookOpen size={28} />
          </div>
          <h2>Nothing open</h2>
          <p>Pick a note from the sidebar, or start a fresh one.</p>
          <button className="empty-cta" type="button" onClick={() => void onCreate()}>
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
              <select
                value={draftFolderId}
                onChange={(event) => {
                  const folderId = event.currentTarget.value;
                  setDraftFolderId(folderId);
                  void onMove(note.id, folderId || null);
                }}
                aria-label="Move note to folder"
              >
                <option value="">Inbox</option>
                {folders.map((folder) => (
                  <option key={folder.id} value={folder.id}>
                    {folder.name}
                  </option>
                ))}
              </select>
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

      <div className={note.deleted_at ? "editor-toolbar disabled" : "editor-toolbar"}>
        <button
          className={dictationState === "recording" ? "active" : ""}
          disabled={Boolean(note.deleted_at) || dictationState === "transcribing"}
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
          onClick={() => editor?.chain().focus().toggleHeading({ level: 2 }).run()}
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
    </div>
  );
}

type ContextPanelProps = {
  note: NoteWithContent;
  linkedNotes: NoteListItem[];
  linkSuggestions: LinkSuggestion[];
  onOpenNote: (id: string) => Promise<void>;
  onLinkSuggestion: (targetId: string) => Promise<void>;
  onExtractionStatusChange: (noteId: string, status: string) => void;
  onUnlink: (sourceId: string, targetId: string) => Promise<void>;
};

function ContextPanel({
  note,
  linkedNotes,
  linkSuggestions,
  onOpenNote,
  onLinkSuggestion,
  onExtractionStatusChange,
  onUnlink,
}: ContextPanelProps) {
  const [tab, setTab] = useState<"details" | "links">("details");
  const linkCount = linkedNotes.length + linkSuggestions.length;

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
        </div>
      </div>

      <div className="panel-pane" hidden={tab !== "details"}>
        <EntityStrip note={note} onStatusChange={onExtractionStatusChange} />
      </div>

      <div className="panel-pane" hidden={tab !== "links"}>
        <div className="context-section">
          <div className="context-heading">
            <span>Linked notes</span>
            <small>{linkedNotes.length}</small>
          </div>
          {linkedNotes.length === 0 ? (
            <p className="context-empty">No linked notes yet</p>
          ) : (
            <div className="context-links">
              {linkedNotes.map((linked) => (
                <div className="linked-row" key={linked.id}>
                  <button type="button" onClick={() => void onOpenNote(linked.id)}>
                    <span>{linked.title || "Untitled"}</span>
                    <small>{formatTime(linked.updated_at)}</small>
                  </button>
                  <button
                    className="ghost-icon danger"
                    type="button"
                    onClick={() => void onUnlink(note.id, linked.id)}
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
                      {suggestion.shared_entity_count === 1 ? "entity" : "entities"}
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
