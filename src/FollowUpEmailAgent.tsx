import { invoke } from "@tauri-apps/api/core";
import { CheckCircle2, Loader2, Mail, RefreshCw, Send, X } from "lucide-react";
import { useEffect, useState } from "react";
import type { AgentNoteRef } from "./Agents";

type PreparedFollowUp = {
  run_id: string;
  model: string;
  subject: string;
  body: string;
  used_summary: boolean;
};

type GmailDraftResult = {
  id: string;
  message_id: string | null;
};

export default function FollowUpEmailAgent({
  note,
  onClose,
}: {
  note: AgentNoteRef;
  onClose: () => void;
}) {
  const [to, setTo] = useState("");
  const [draft, setDraft] = useState<PreparedFollowUp | null>(null);
  const [preparing, setPreparing] = useState(true);
  const [creating, setCreating] = useState(false);
  const [createdId, setCreatedId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function prepare() {
    setPreparing(true);
    setError(null);
    try {
      const prepared = await invoke<PreparedFollowUp>("prepare_follow_up_email", {
        request: { note_id: note.id },
      });
      setDraft(prepared);
    } catch (prepareError) {
      setError(String(prepareError));
    } finally {
      setPreparing(false);
    }
  }

  useEffect(() => {
    void prepare();
  }, [note.id]);

  async function createDraft() {
    if (!draft) return;
    setCreating(true);
    setError(null);
    try {
      const result = await invoke<GmailDraftResult>("create_gmail_draft", {
        draft: {
          to: to.trim() || null,
          subject: draft.subject.trim(),
          body: draft.body.trim(),
        },
      });
      setCreatedId(result.id);
    } catch (createError) {
      setError(String(createError));
    } finally {
      setCreating(false);
    }
  }

  return (
    <div className="follow-up-backdrop" onMouseDown={onClose}>
      <section
        className="follow-up-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="follow-up-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header>
          <div>
            <Mail size={18} />
            <div>
              <h2 id="follow-up-title">Follow-up email</h2>
              <p>{note.title || "Untitled"}</p>
            </div>
          </div>
          <button type="button" onClick={onClose} title="Close">
            <X size={17} />
          </button>
        </header>

        {createdId ? (
          <div className="follow-up-success">
            <CheckCircle2 size={22} />
            <strong>Gmail draft created</strong>
            <span>{createdId}</span>
            <button type="button" onClick={onClose}>Done</button>
          </div>
        ) : preparing ? (
          <div className="follow-up-loading">
            <Loader2 size={20} className="spin" />
            <strong>Preparing follow-up</strong>
            <span>Reading the complete meeting transcript…</span>
          </div>
        ) : draft ? (
          <>
            <label className="settings-field">
              <span>To</span>
              <input
                value={to}
                onChange={(event) => setTo(event.currentTarget.value)}
                placeholder="Optional recipient"
                autoFocus
              />
            </label>
            <label className="settings-field">
              <span>Subject</span>
              <input
                value={draft.subject}
                onChange={(event) =>
                  setDraft({ ...draft, subject: event.currentTarget.value })
                }
              />
            </label>
            <label className="settings-field follow-up-body">
              <span>Body</span>
              <textarea
                value={draft.body}
                onChange={(event) =>
                  setDraft({ ...draft, body: event.currentTarget.value })
                }
              />
            </label>
            <div className="follow-up-meta">
              <span>{draft.model}</span>
              {draft.used_summary ? <span>Long transcript reduced in stages</span> : null}
            </div>
            {error ? <p className="follow-up-error">{error}</p> : null}
            <footer>
              <button type="button" onClick={() => void prepare()} disabled={creating}>
                <RefreshCw size={14} />
                Regenerate
              </button>
              <button
                className="primary"
                type="button"
                onClick={() => void createDraft()}
                disabled={creating || !draft.subject.trim() || !draft.body.trim()}
              >
                {creating ? <Loader2 size={14} className="spin" /> : <Send size={14} />}
                {creating ? "Creating" : "Create Gmail draft"}
              </button>
            </footer>
          </>
        ) : (
          <div className="follow-up-loading error">
            <Mail size={20} />
            <strong>Could not prepare the email</strong>
            <span>{error}</span>
            <button type="button" onClick={() => void prepare()}>Try again</button>
          </div>
        )}
      </section>
    </div>
  );
}
