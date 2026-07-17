import { Cloud, Cpu, X } from "lucide-react";
import { useState } from "react";
import type { LlmProvider } from "./llmPreferences";

export default function LlmRunChoiceDialog({
  defaultProvider,
  actionLabel,
  onCancel,
  onChoose,
}: {
  defaultProvider: LlmProvider;
  actionLabel: string;
  onCancel: () => void;
  onChoose: (provider: LlmProvider, remember: boolean) => void;
}) {
  const [alwaysObey, setAlwaysObey] = useState(false);
  const defaultLabel = defaultProvider === "remote" ? "Remote" : "Local";

  return (
    <div className="llm-choice-backdrop" onMouseDown={onCancel}>
      <section
        className="llm-choice-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="llm-choice-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header>
          <div>
            <h2 id="llm-choice-title">Choose an LLM</h2>
            <p>{actionLabel}</p>
          </div>
          <button type="button" className="ghost-icon" onClick={onCancel} title="Cancel">
            <X size={16} />
          </button>
        </header>

        <div className="llm-choice-options">
          <button type="button" onClick={() => onChoose("local", alwaysObey)}>
            <Cpu size={18} />
            <span>
              <strong>Local</strong>
              <small>Runs on this device</small>
            </span>
            {defaultProvider === "local" ? <em>Default</em> : null}
          </button>
          <button type="button" onClick={() => onChoose("remote", alwaysObey)}>
            <Cloud size={18} />
            <span>
              <strong>Remote</strong>
              <small>Uses your OpenAI-compatible API</small>
            </span>
            {defaultProvider === "remote" ? <em>Default</em> : null}
          </button>
        </div>

        <label className="llm-choice-remember">
          <input
            type="checkbox"
            checked={alwaysObey}
            onChange={(event) => setAlwaysObey(event.target.checked)}
          />
          <span>
            <strong>Always obey the global setting</strong>
            <small>
              Stop asking and always use your default provider ({defaultLabel}).
              Turn this off again in Settings › AI › Settings.
            </small>
          </span>
        </label>
      </section>
    </div>
  );
}
