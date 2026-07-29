import { invoke } from "@tauri-apps/api/core";
import {
  FilePlus2,
  FileText,
  Loader2,
  NotebookTabs,
  Save,
  Trash2,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  listTasks,
  taskMentionHandle,
  type AgentDefinition,
} from "./Agents";

export type NoteTemplate = {
  id: string;
  name: string;
  content: string;
  createdAt: string;
  updatedAt: string;
};

export type MeetingTemplateAssignment = {
  transcriptNoteId: string;
  templateId: string | null;
  templateName: string;
  templateContent: string;
  assignedAt: string;
};

export async function listNoteTemplates(): Promise<NoteTemplate[]> {
  return invoke<NoteTemplate[]>("list_note_templates");
}

/**
 * Remove only template lines that still exist unchanged. Lines added or edited
 * by the user are not part of the longest common subsequence and survive.
 */
export function removeTemplateScaffolding(
  currentContent: string,
  templateContent: string,
) {
  const current = currentContent.replace(/\r\n?/g, "\n").split("\n");
  const template = templateContent.replace(/\r\n?/g, "\n").split("\n");
  const rows = template.length + 1;
  const columns = current.length + 1;
  const lengths = Array.from({ length: rows }, () =>
    new Uint16Array(columns),
  );

  for (let templateIndex = template.length - 1; templateIndex >= 0; templateIndex -= 1) {
    for (let currentIndex = current.length - 1; currentIndex >= 0; currentIndex -= 1) {
      lengths[templateIndex][currentIndex] =
        template[templateIndex] === current[currentIndex]
          ? lengths[templateIndex + 1][currentIndex + 1] + 1
          : Math.max(
              lengths[templateIndex + 1][currentIndex],
              lengths[templateIndex][currentIndex + 1],
            );
    }
  }

  const matchedCurrentLines = new Set<number>();
  let templateIndex = 0;
  let currentIndex = 0;
  while (templateIndex < template.length && currentIndex < current.length) {
    if (template[templateIndex] === current[currentIndex]) {
      matchedCurrentLines.add(currentIndex);
      templateIndex += 1;
      currentIndex += 1;
    } else if (
      lengths[templateIndex + 1][currentIndex] >=
      lengths[templateIndex][currentIndex + 1]
    ) {
      templateIndex += 1;
    } else {
      currentIndex += 1;
    }
  }

  return current
    .filter((_, index) => !matchedCurrentLines.has(index))
    .join("\n")
    .replace(/^\s*\n/, "")
    .replace(/\n\s*$/, "")
    .trim();
}

export function combineTemplateWithNotes(
  templateContent: string,
  preservedNotes: string,
) {
  return [templateContent.trim(), preservedNotes.trim()]
    .filter(Boolean)
    .join("\n\n");
}

export default function NoteTemplateSettings({
  onError,
  onSuccess,
}: {
  onError: (error: unknown) => void;
  onSuccess: (message: string) => void;
}) {
  const [templates, setTemplates] = useState<NoteTemplate[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [content, setContent] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [tasks, setTasks] = useState<AgentDefinition[]>([]);
  const [mentionQuery, setMentionQuery] = useState<{
    start: number;
    end: number;
    query: string;
  } | null>(null);
  const [activeMentionIndex, setActiveMentionIndex] = useState(0);
  const editorRef = useRef<HTMLTextAreaElement | null>(null);
  const mentionOptions = useMemo(() => {
    if (!mentionQuery) {
      return [];
    }
    const query = mentionQuery.query.toLowerCase();
    return tasks
      .filter((task) => {
        const handle = taskMentionHandle(task);
        return (
          !query ||
          handle.includes(query) ||
          task.name.toLowerCase().includes(query)
        );
      })
      .slice(0, 8);
  }, [mentionQuery, tasks]);

  async function refresh(preferredId?: string | null) {
    const rows = await listNoteTemplates();
    setTemplates(rows);
    const nextId =
      preferredId && rows.some(({ id }) => id === preferredId)
        ? preferredId
        : rows[0]?.id ?? null;
    setSelectedId(nextId);
    const selected = rows.find(({ id }) => id === nextId);
    setName(selected?.name ?? "");
    setContent(selected?.content ?? "");
  }

  useEffect(() => {
    void refresh()
      .catch(onError)
      .finally(() => setLoading(false));
  }, [onError]);

  useEffect(() => {
    void listTasks()
      .then((rows) =>
        setTasks(rows.filter(({ scope }) => scope === "note").slice(0, 50)),
      )
      .catch(onError);
  }, [onError]);

  function selectTemplate(template: NoteTemplate) {
    setSelectedId(template.id);
    setName(template.name);
    setContent(template.content);
  }

  function startNewTemplate() {
    setSelectedId(null);
    setName("");
    setContent("");
  }

  function updateMentionQuery(nextContent: string, caret: number) {
    const beforeCaret = nextContent.slice(0, caret);
    const match = beforeCaret.match(/(?:^|[\s,(])@([a-z0-9_-]*)$/i);
    if (!match) {
      setMentionQuery(null);
      return;
    }
    setMentionQuery({
      start: beforeCaret.lastIndexOf("@"),
      end: caret,
      query: match[1],
    });
    setActiveMentionIndex(0);
  }

  function chooseMention(task: AgentDefinition) {
    if (!mentionQuery) {
      return;
    }
    const handle = `@${taskMentionHandle(task)}`;
    const nextContent =
      content.slice(0, mentionQuery.start) +
      handle +
      content.slice(mentionQuery.end);
    const caret = mentionQuery.start + handle.length;
    setContent(nextContent);
    setMentionQuery(null);
    window.requestAnimationFrame(() => {
      editorRef.current?.focus();
      editorRef.current?.setSelectionRange(caret, caret);
    });
  }

  async function saveTemplate() {
    setSaving(true);
    try {
      const saved = selectedId
        ? await invoke<NoteTemplate>("update_note_template", {
            id: selectedId,
            input: { name, content },
          })
        : await invoke<NoteTemplate>("create_note_template", {
            input: { name, content },
          });
      await refresh(saved.id);
      window.dispatchEvent(new Event("smooth-note-templates-changed"));
      onSuccess(selectedId ? "Note template updated" : "Note template created");
    } catch (error) {
      onError(error);
    } finally {
      setSaving(false);
    }
  }

  async function deleteTemplate() {
    if (!selectedId) {
      return;
    }
    const selected = templates.find(({ id }) => id === selectedId);
    if (!window.confirm(`Delete “${selected?.name ?? "this template"}”?`)) {
      return;
    }
    setSaving(true);
    try {
      await invoke("delete_note_template", { id: selectedId });
      await refresh(null);
      window.dispatchEvent(new Event("smooth-note-templates-changed"));
      onSuccess("Note template deleted");
    } catch (error) {
      onError(error);
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="settings-section note-template-settings">
      <div className="section-heading">
        <NotebookTabs size={18} />
        <span>Note templates</span>
        <small>{templates.length} saved</small>
      </div>

      <p className="note-template-intro">
        Define the starting structure for meeting notes. Empty headings are
        completed after the meeting, and valid <code>@task</code> mentions run
        in the order written.
      </p>

      <div className="note-template-layout">
        <aside className="note-template-list">
          <button
            type="button"
            className={selectedId === null ? "active new" : "new"}
            onClick={startNewTemplate}
          >
            <FilePlus2 size={15} />
            New template
          </button>
          {loading ? (
            <span className="note-template-loading">
              <Loader2 className="spin" size={15} />
              Loading
            </span>
          ) : (
            templates.map((template) => (
              <button
                key={template.id}
                type="button"
                className={selectedId === template.id ? "active" : ""}
                onClick={() => selectTemplate(template)}
              >
                <FileText size={14} />
                <span>{template.name}</span>
              </button>
            ))
          )}
        </aside>

        <div className="note-template-editor">
          <label className="settings-field">
            <span>Name</span>
            <input
              value={name}
              onChange={(event) => setName(event.currentTarget.value)}
              placeholder="Weekly product review"
            />
          </label>
          <label className="settings-field">
            <span>Template</span>
            <div className="note-template-content-editor">
              <textarea
                ref={editorRef}
                value={content}
                onChange={(event) => {
                  const nextContent = event.currentTarget.value;
                  const caret = event.currentTarget.selectionStart;
                  setContent(nextContent);
                  const inputType = (
                    event.nativeEvent as InputEvent | undefined
                  )?.inputType;
                  if (inputType?.startsWith("delete")) {
                    setMentionQuery(null);
                  } else {
                    updateMentionQuery(nextContent, caret);
                  }
                }}
                onClick={(event) =>
                  updateMentionQuery(
                    event.currentTarget.value,
                    event.currentTarget.selectionStart,
                  )
                }
                onKeyDown={(event) => {
                  if (!mentionQuery || mentionOptions.length === 0) {
                    return;
                  }
                  if (event.key === "ArrowDown") {
                    event.preventDefault();
                    setActiveMentionIndex(
                      (index) => (index + 1) % mentionOptions.length,
                    );
                  } else if (event.key === "ArrowUp") {
                    event.preventDefault();
                    setActiveMentionIndex(
                      (index) =>
                        (index - 1 + mentionOptions.length) %
                        mentionOptions.length,
                    );
                  } else if (event.key === "Enter" || event.key === "Tab") {
                    event.preventDefault();
                    chooseMention(
                      mentionOptions[
                        Math.min(activeMentionIndex, mentionOptions.length - 1)
                      ],
                    );
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    setMentionQuery(null);
                  }
                }}
                placeholder={"@summary, @draft\n\n# To do\n\n# Main talking point"}
                rows={14}
              />
              {mentionQuery && mentionOptions.length > 0 ? (
                <div
                  className="note-template-task-mentions"
                  role="listbox"
                  aria-label="Available tasks"
                >
                  {mentionOptions.map((task, index) => (
                    <button
                      key={task.id}
                      className={index === activeMentionIndex ? "active" : ""}
                      type="button"
                      role="option"
                      aria-selected={index === activeMentionIndex}
                      onMouseDown={(event) => event.preventDefault()}
                      onMouseEnter={() => setActiveMentionIndex(index)}
                      onClick={() => chooseMention(task)}
                    >
                      <span>@{taskMentionHandle(task)}</span>
                      <small>{task.name}</small>
                    </button>
                  ))}
                </div>
              ) : null}
            </div>
          </label>
          <div className="settings-actions">
            {selectedId ? (
              <button
                type="button"
                className="danger"
                onClick={() => void deleteTemplate()}
                disabled={saving}
              >
                <Trash2 size={14} />
                Delete
              </button>
            ) : null}
            <button
              type="button"
              className="primary"
              onClick={() => void saveTemplate()}
              disabled={saving || !name.trim() || !content.trim()}
            >
              {saving ? <Loader2 className="spin" size={14} /> : <Save size={14} />}
              {selectedId ? "Save changes" : "Create template"}
            </button>
          </div>
        </div>
      </div>
    </section>
  );
}
