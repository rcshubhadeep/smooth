import { Channel, invoke } from "@tauri-apps/api/core";
import { ArrowUp, Sparkles, Trash2 } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

type ChatMessage = {
  id: string;
  note_id: string;
  role: string;
  content: string;
  created_at: string;
};

type ChatStreamEvent =
  | { type: "delta"; delta: string }
  | { type: "done"; message: ChatMessage }
  | { type: "error"; message: string };

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

export default function NoteChat({ noteId }: { noteId: string }) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [streaming, setStreaming] = useState<string | null>(null);
  const [isSending, setIsSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

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
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: "smooth" });
  }, [messages, streaming]);

  const send = useCallback(
    async (text: string) => {
      const question = text.trim();
      if (!question || isSending) {
        return;
      }

      setError(null);
      setInput("");
      setIsSending(true);
      setStreaming("");
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
        if (event.type === "delta") {
          setStreaming((current) => (current ?? "") + event.delta);
        } else if (event.type === "done") {
          setMessages((current) => [...current, event.message]);
          setStreaming(null);
          setIsSending(false);
        } else {
          setError(event.message);
          setStreaming(null);
          setIsSending(false);
        }
      };

      try {
        await invoke("send_chat_message", { noteId, content: question, onEvent: channel });
      } catch (invokeError) {
        setError(String(invokeError));
        setStreaming(null);
        setIsSending(false);
      }
    },
    [isSending, noteId],
  );

  async function clearChat() {
    try {
      await invoke("clear_chat", { noteId });
      setMessages([]);
      setStreaming(null);
      setError(null);
    } catch (clearError) {
      setError(String(clearError));
    }
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void send(input);
    }
  }

  const isEmpty = messages.length === 0 && streaming === null;

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
            {messages.map((message) => (
              <div className={`chat-msg ${message.role}`} key={message.id}>
                <div className="chat-bubble">{message.content}</div>
              </div>
            ))}
            {streaming !== null ? (
              <div className="chat-msg assistant">
                <div className="chat-bubble">
                  {streaming}
                  <span className="chat-caret" />
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
