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
  const [remember, setRemember] = useState(false);

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
          <button type="button" onClick={() => onChoose("local", remember)}>
            <Cpu size={18} />
            <span>
              <strong>Local</strong>
              <small>Runs on this device</small>
            </span>
            {defaultProvider === "local" ? <em>Default</em> : null}
          </button>
          <button type="button" onClick={() => onChoose("inception", remember)}>
            <Cloud size={18} />
            <span>
              <strong>Inception</strong>
              <small>Uses Mercury 2 remotely</small>
            </span>
            {defaultProvider === "inception" ? <em>Default</em> : null}
          </button>
        </div>

        <label className="llm-choice-remember">
          <input
            type="checkbox"
            checked={remember}
            onChange={(event) => setRemember(event.currentTarget.checked)}
          />
          <span>Remember my choice for this session</span>
        </label>
      </section>
    </div>
  );
}
