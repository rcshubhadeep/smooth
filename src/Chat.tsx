import { Channel, invoke } from "@tauri-apps/api/core";
import { ArrowUp, Check, Copy, FilePlus, Sparkles, Trash2 } from "lucide-react";
import { marked } from "marked";
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";

type ChatMessage = {
  id: string;
  note_id: string;
  role: string;
  content: string;
  created_at: string;
};

type ChatStreamEvent =
  | { type: "status"; message: string }
  | { type: "delta"; delta: string }
  | { type: "done"; message: ChatMessage }
  | { type: "error"; message: string };

function renderMarkdown(text: string) {
  return { __html: marked.parse(text, { async: false }) as string };
}

const SUGGESTIONS = [
  "Summarize this note",
  "What are the action items?",
  "What questions are still open?",
];

let tempCounter = 0;
function tempId() {
  tempCounter += 1;
  return `local-${tempCounter}`;
}

type ChatStreamState = {
  noteId: string;
  isSending: boolean;
  streaming: string | null;
  status: string | null;
  error: string | null;
  completedMessage: ChatMessage | null;
};

const emptyStreamState = (noteId: string): ChatStreamState => ({
  noteId,
  isSending: false,
  streaming: null,
  status: null,
  error: null,
  completedMessage: null,
});

const chatStreamStore = (() => {
  const states = new Map<string, ChatStreamState>();
  const emptyStates = new Map<string, ChatStreamState>();
  const listeners = new Set<() => void>();

  function snapshot(noteId: string) {
    const activeState = states.get(noteId);
    if (activeState) {
      return activeState;
    }

    let emptyState = emptyStates.get(noteId);
    if (!emptyState) {
      emptyState = emptyStreamState(noteId);
      emptyStates.set(noteId, emptyState);
    }
    return emptyState;
  }

  function emit() {
    listeners.forEach((listener) => listener());
  }

  function update(noteId: string, patch: Partial<ChatStreamState>) {
    states.set(noteId, { ...snapshot(noteId), ...patch, noteId });
    emit();
  }

  return {
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    snapshot,
    start(noteId: string) {
      update(noteId, {
        isSending: true,
        streaming: null,
        status: null,
        error: null,
        completedMessage: null,
      });
    },
    status(noteId: string, message: string) {
      update(noteId, { status: message });
    },
    delta(noteId: string, delta: string) {
      const current = snapshot(noteId);
      update(noteId, {
        status: null,
        streaming: (current.streaming ?? "") + delta,
      });
    },
    done(noteId: string, message: ChatMessage) {
      update(noteId, {
        isSending: false,
        streaming: null,
        status: null,
        error: null,
        completedMessage: message,
      });
    },
    error(noteId: string, message: string) {
      update(noteId, {
        isSending: false,
        streaming: null,
        status: null,
        error: message,
      });
    },
    clear(noteId: string) {
      states.delete(noteId);
      emit();
    },
    consumeCompleted(noteId: string) {
      const current = snapshot(noteId);
      if (!current.completedMessage) {
        return null;
      }
      update(noteId, { completedMessage: null });
      return current.completedMessage;
    },
  };
})();

export default function NoteChat({
  noteId,
  noteContent,
  onCreateNote,
}: {
  noteId: string;
  noteContent: string;
  onCreateNote: (content: string, sourcePrompt: string | null) => void;
}) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const streamState = useSyncExternalStore(
    chatStreamStore.subscribe,
    () => chatStreamStore.snapshot(noteId),
    () => chatStreamStore.snapshot(noteId),
  );
  const { streaming, status, isSending, error } = streamState;
  const scrollRef = useRef<HTMLDivElement>(null);
  const noteContentRef = useRef(noteContent);
  noteContentRef.current = noteContent;

  useEffect(() => {
    let active = true;
    invoke<ChatMessage[]>("get_chat_messages", { noteId })
      .then((history) => {
        if (active) {
          setMessages(history);
        }
      })
      .catch(() => {
        /* a fresh note simply has no history */
      });
    return () => {
      active = false;
    };
  }, [noteId]);

  useEffect(() => {
    const completed = chatStreamStore.consumeCompleted(noteId);
    if (!completed) {
      return;
    }
    setMessages((current) =>
      current.some((message) => message.id === completed.id)
        ? current
        : [...current, completed],
    );
  }, [noteId, streamState.completedMessage]);

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: "smooth" });
  }, [messages, streaming]);

  const send = useCallback(
    async (text: string) => {
      const question = text.trim();
      if (!question || isSending) {
        return;
      }

      setInput("");
      chatStreamStore.start(noteId);
      setMessages((current) => [
        ...current,
        {
          id: tempId(),
          note_id: noteId,
          role: "user",
          content: question,
          created_at: String(Date.now()),
        },
      ]);

      const channel = new Channel<ChatStreamEvent>();
      channel.onmessage = (event) => {
        if (event.type === "status") {
          chatStreamStore.status(noteId, event.message);
        } else if (event.type === "delta") {
          chatStreamStore.delta(noteId, event.delta);
        } else if (event.type === "done") {
          chatStreamStore.done(noteId, event.message);
        } else {
          chatStreamStore.error(noteId, event.message);
        }
      };

      try {
        await invoke("send_chat_message", {
          noteId,
          content: question,
          noteContent: noteContentRef.current,
          onEvent: channel,
        });
      } catch (invokeError) {
        chatStreamStore.error(noteId, String(invokeError));
      }
    },
    [isSending, noteId],
  );

  async function clearChat() {
    try {
      await invoke("clear_chat", { noteId });
      setMessages([]);
      chatStreamStore.clear(noteId);
    } catch (clearError) {
      chatStreamStore.error(noteId, String(clearError));
    }
  }

  async function copyMessage(id: string, text: string) {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedId(id);
      window.setTimeout(() => setCopiedId((current) => (current === id ? null : current)), 1500);
    } catch {
      chatStreamStore.error(noteId, "Couldn't copy to the clipboard");
    }
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void send(input);
    }
  }

  const pending = status ?? (isSending && streaming === null ? "Thinking…" : null);
  const isEmpty = messages.length === 0 && streaming === null && !isSending;

  return (
    <div className="chat-pane">
      <div className="chat-scroll" ref={scrollRef}>
        {isEmpty ? (
          <div className="chat-empty">
            <div className="chat-empty-icon">
              <Sparkles size={22} />
            </div>
            <p>Ask anything about this note</p>
            <div className="chat-suggestions">
              {SUGGESTIONS.map((suggestion) => (
                <button
                  key={suggestion}
                  type="button"
                  className="chat-suggestion"
                  onClick={() => void send(suggestion)}
                >
                  {suggestion}
                </button>
              ))}
            </div>
          </div>
        ) : (
          <>
            {messages.map((message, index) =>
              message.role === "assistant" ? (
                <div className="chat-msg assistant" key={message.id}>
                  <div
                    className="chat-bubble markdown"
                    dangerouslySetInnerHTML={renderMarkdown(message.content)}
                  />
                  <div className="chat-actions">
                    <button
                      type="button"
                      className="chat-action"
                      title={copiedId === message.id ? "Copied" : "Copy to clipboard"}
                      onClick={() => void copyMessage(message.id, message.content)}
                    >
                      {copiedId === message.id ? <Check size={14} /> : <Copy size={14} />}
                    </button>
                    <button
                      type="button"
                      className="chat-action"
                      title="Create a note from this"
                      onClick={() => {
                        const sourcePrompt =
                          messages
                            .slice(0, index)
                            .reverse()
                            .find((candidate) => candidate.role === "user")
                            ?.content ?? null;
                        onCreateNote(message.content, sourcePrompt);
                      }}
                    >
                      <FilePlus size={14} />
                    </button>
                  </div>
                </div>
              ) : (
                <div className="chat-msg user" key={message.id}>
                  <div className="chat-bubble">{message.content}</div>
                </div>
              ),
            )}
            {streaming !== null ? (
              <div className="chat-msg assistant">
                <div className="chat-bubble">
                  {streaming}
                  <span className="chat-caret" />
                </div>
              </div>
            ) : null}
            {pending ? (
              <div className="chat-msg assistant">
                <div className="chat-status">
                  <span className="chat-status-dots">
                    <i />
                    <i />
                    <i />
                  </span>
                  {pending}
                </div>
              </div>
            ) : null}
          </>
        )}
        {error ? <p className="chat-error">{error}</p> : null}
      </div>

      <div className="chat-input-bar">
        {messages.length > 0 ? (
          <button
            type="button"
            className="chat-clear"
            onClick={() => void clearChat()}
            title="Clear conversation"
          >
            <Trash2 size={15} />
          </button>
        ) : null}
        <textarea
          className="chat-input"
          value={input}
          rows={1}
          placeholder="Ask about this note…"
          onChange={(event) => setInput(event.currentTarget.value)}
          onKeyDown={onKeyDown}
        />
        <button
          type="button"
          className="chat-send"
          disabled={isSending || !input.trim()}
          onClick={() => void send(input)}
          title="Send (Enter)"
        >
          <ArrowUp size={17} />
        </button>
      </div>
    </div>
  );
}
