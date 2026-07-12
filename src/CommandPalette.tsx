import { FileText, Plus, Search, Settings, Sun } from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { semanticSearch, type SemanticSearchResult } from "./semantic";

export type CommandPaletteNote = {
  id: string;
  title: string;
  excerpt: string;
};

type Props = {
  notes: CommandPaletteNote[];
  onClose: () => void;
  onOpenNote: (id: string) => void;
  onCreateNote: () => void;
  onOpenSettings: () => void;
  onToggleTheme: () => void;
};

type MatchTier = "warm" | "middle" | "cool";

type PaletteItem =
  | {
      kind: "command";
      id: string;
      label: string;
      icon: ReactNode;
      run: () => void;
    }
  | {
      kind: "note";
      id: string;
      label: string;
      sub: string;
      tier: MatchTier | null;
    };

export default function CommandPalette({
  notes,
  onClose,
  onOpenNote,
  onCreateNote,
  onOpenSettings,
  onToggleTheme,
}: Props) {
  const [query, setQuery] = useState("");
  const [semanticResults, setSemanticResults] = useState<
    SemanticSearchResult[] | null
  >(null);
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const activeRef = useRef<HTMLButtonElement>(null);

  useEffect(() => inputRef.current?.focus(), []);

  const cleanQuery = query.trim().toLowerCase();

  useEffect(() => {
    const searchText = query.trim();
    setSemanticResults(null);
    if (!searchText) return;

    let cancelled = false;
    const timer = window.setTimeout(() => {
      void semanticSearch(searchText)
        .then((results) => {
          if (!cancelled) setSemanticResults(results);
        })
        .catch(() => {
          // Immediate lexical matches remain visible if the index is unavailable.
        });
    }, 180);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [query]);

  const commands = useMemo<PaletteItem[]>(
    () => [
      {
        kind: "command",
        id: "new-note",
        label: "New note",
        icon: <Plus size={16} />,
        run: onCreateNote,
      },
      {
        kind: "command",
        id: "settings",
        label: "Open settings",
        icon: <Settings size={16} />,
        run: onOpenSettings,
      },
      {
        kind: "command",
        id: "theme",
        label: "Toggle theme",
        icon: <Sun size={16} />,
        run: onToggleTheme,
      },
    ],
    [onCreateNote, onOpenSettings, onToggleTheme],
  );

  const items = useMemo<PaletteItem[]>(() => {
    const matchedCommands = cleanQuery
      ? commands.filter((command) =>
          command.label.toLowerCase().includes(cleanQuery),
        )
      : commands;
    const notesById = new Map(notes.map((note) => [note.id, note]));
    const rankedNotes = cleanQuery
      ? semanticResults
        ? semanticResults
            .map((result, rank) => {
              const note = notesById.get(result.id);
              return note ? { note, tier: tierForRank(rank, semanticResults.length) } : null;
            })
            .filter(
              (entry): entry is { note: CommandPaletteNote; tier: MatchTier } =>
                entry !== null,
            )
        : notes
            .filter((note) =>
              `${note.title} ${note.excerpt}`
                .toLowerCase()
                .includes(cleanQuery),
            )
            .map((note) => ({ note, tier: null }))
      : notes.map((note) => ({ note, tier: null }));

    const matchedNotes = rankedNotes.slice(0, 50).map<PaletteItem>(({ note, tier }) => ({
      kind: "note",
      id: note.id,
      label: note.title || "Untitled",
      sub: note.excerpt || "No content",
      tier,
    }));
    return [...matchedCommands, ...matchedNotes];
  }, [cleanQuery, commands, notes, semanticResults]);

  useEffect(() => setActiveIndex(0), [cleanQuery]);
  useEffect(() => activeRef.current?.scrollIntoView({ block: "nearest" }), [activeIndex]);

  function activate(item: PaletteItem) {
    if (item.kind === "command") item.run();
    else onOpenNote(item.id);
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
      if (item) activate(item);
    } else if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    }
  }

  return (
    <div className="palette-overlay" onMouseDown={onClose}>
      <div className="palette" role="dialog" aria-modal="true" onMouseDown={(event) => event.stopPropagation()}>
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
            items.map((item, index) => {
              const tierClass = item.kind === "note" && item.tier ? ` match-${item.tier}` : "";
              return (
                <button
                  key={`${item.kind}:${item.id}`}
                  ref={index === activeIndex ? activeRef : null}
                  className={`palette-item${tierClass}${index === activeIndex ? " active" : ""}`}
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
                  {item.kind === "command" ? <span className="palette-item-tag">Command</span> : null}
                </button>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
}

function tierForRank(rank: number, total: number): MatchTier {
  const percentile = total <= 1 ? 0 : rank / total;
  if (percentile < 0.2) return "warm";
  if (percentile < 0.55) return "middle";
  return "cool";
}
