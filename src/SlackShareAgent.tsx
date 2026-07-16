import { invoke } from "@tauri-apps/api/core";
import { CheckCircle2, Loader2, MessageSquare, Send, X } from "lucide-react";
import { useEffect, useState } from "react";
import type { AgentNoteRef, AgentRunResult } from "./Agents";
import type { LlmProvider } from "./llmPreferences";

type NoteWithContent = AgentNoteRef & { content: string };
type Mode = "as-written" | "ai";

export default function SlackShareAgent({
  note,
  provider,
  onClose,
}: {
  note: AgentNoteRef;
  provider: LlmProvider | null;
  onClose: () => void;
}) {
  const [destination, setDestination] = useState(
    () => localStorage.getItem("smooth-slack-destination") ?? "",
  );
  const [mode, setMode] = useState<Mode>("as-written");
  const [instructions, setInstructions] = useState(
    "Rewrite this as a concise Slack update. Preserve facts, decisions, owners, and next steps.",
  );
  const [message, setMessage] = useState("");
  const [loading, setLoading] = useState(true);
  const [processing, setProcessing] = useState(false);
  const [sending, setSending] = useState(false);
  const [sent, setSent] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<NoteWithContent>("get_note", { id: note.id })
      .then((loaded) => setMessage(slackMessage(loaded.title, loaded.content)))
      .catch((loadError) => setError(String(loadError)))
      .finally(() => setLoading(false));
  }, [note.id]);

  async function prepareWithAi() {
    setProcessing(true);
    setError(null);
    try {
      const result = await invoke<AgentRunResult>("agent_run", {
        agentId: "share-note-slack",
        prompt: [
          "You are preparing one note for an explicit, user-approved Slack post.",
          `Note id: ${note.id}`,
          `Note title: ${note.title || "(untitled)"}`,
          "Call read_note with this exact note id.",
          "Return only the proposed Slack message, with no preamble or explanation.",
          "Do not invent any facts and do not send anything yourself.",
          `Editing instruction: ${instructions.trim()}`,
        ].join("\n"),
        maxSteps: 3,
        selection: provider ? { provider, model: null } : null,
      });
      setMessage(result.answer.trim());
    } catch (processError) {
      setError(String(processError));
    } finally {
      setProcessing(false);
    }
  }

  async function sendMessage() {
    setSending(true);
    setError(null);
    try {
      await invoke("post_note_to_slack", {
        request: { destination: destination.trim(), text: message.trim() },
      });
      localStorage.setItem("smooth-slack-destination", destination.trim());
      setSent(true);
    } catch (sendError) {
      setError(String(sendError));
    } finally {
      setSending(false);
    }
  }

  return (
    <div className="slack-share-backdrop" onMouseDown={onClose}>
      <section
        className="slack-share-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="slack-share-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header>
          <div>
            <MessageSquare size={18} />
            <div>
              <h2 id="slack-share-title">Share note to Slack</h2>
              <p>{note.title || "Untitled"}</p>
            </div>
          </div>
          <button type="button" onClick={onClose} title="Close">
            <X size={17} />
          </button>
        </header>

        {sent ? (
          <div className="slack-share-success">
            <CheckCircle2 size={22} />
            <strong>Posted to Slack</strong>
            <button type="button" onClick={onClose}>Done</button>
          </div>
        ) : (
          <>
            <label className="settings-field">
              <span>Destination</span>
              <input
                value={destination}
                onChange={(event) => setDestination(event.currentTarget.value)}
                placeholder="Channel ID or Slack message URL"
                autoFocus
              />
            </label>

            <div className="slack-share-mode" role="group" aria-label="Message preparation">
              <button
                className={mode === "as-written" ? "active" : ""}
                type="button"
                onClick={() => setMode("as-written")}
              >
                As written
              </button>
              <button
                className={mode === "ai" ? "active" : ""}
                type="button"
                onClick={() => setMode("ai")}
              >
                Process with AI
              </button>
            </div>
            {mode === "ai" ? (
              <div className="slack-share-ai">
                <label className="settings-field">
                  <span>Processing instruction</span>
                  <textarea
                    value={instructions}
                    onChange={(event) => setInstructions(event.currentTarget.value)}
                  />
                </label>
                <button
                  type="button"
                  onClick={() => void prepareWithAi()}
                  disabled={processing || !instructions.trim()}
                >
                  {processing ? <Loader2 size={14} className="spin" /> : null}
                  {processing ? "Preparing" : "Prepare message"}
                </button>
              </div>
            ) : null}

            <label className="settings-field slack-share-preview">
              <span>Message preview</span>
              <textarea
                value={message}
                onChange={(event) => setMessage(event.currentTarget.value)}
                disabled={loading || processing}
                placeholder={loading ? "Loading note" : "Slack message"}
              />
            </label>

            {error ? <p className="slack-share-error">{error}</p> : null}

            <footer>
              <button type="button" onClick={onClose}>Cancel</button>
              <button
                className="primary"
                type="button"
                onClick={() => void sendMessage()}
                disabled={sending || loading || processing || !destination.trim() || !message.trim()}
              >
                {sending ? <Loader2 size={14} className="spin" /> : <Send size={14} />}
                {sending ? "Sending" : "Send to Slack"}
              </button>
            </footer>
          </>
        )}
      </section>
    </div>
  );
}

function slackMessage(title: string, content: string): string {
  const heading = title.trim() ? `*${title.trim().replace(/\*/g, "")}*` : "";
  return [heading, content.trim()].filter(Boolean).join("\n\n");
}
