import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowLeft,
  ArrowRight,
  BookOpen,
  Check,
  CheckCircle2,
  Cloud,
  Cpu,
  FolderTree,
  Command,
  Languages,
  Loader2,
  Mic,
  PanelRight,
  PenLine,
  Radio,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

// Tutorial links — dummy targets for now; swap for real docs when published.
const TUTORIAL_LINKS = {
  google: "https://smooth-notes.example.com/docs/google",
  slack: "https://smooth-notes.example.com/docs/slack",
  mcp: "https://smooth-notes.example.com/docs/mcp",
};

const LOCAL_MODEL_LABEL = "Gemma 4 12B (instruction-tuned)";
const LOCAL_MODEL_SIZE = "≈ 7.3 GB download";

// Roman-script languages only for now (whisper handles them well).
const LANGUAGES: { code: string; label: string }[] = [
  { code: "en", label: "English" },
  { code: "fr", label: "French" },
  { code: "de", label: "German" },
  { code: "es", label: "Spanish" },
  { code: "it", label: "Italian" },
  { code: "pt", label: "Portuguese" },
  { code: "nl", label: "Dutch" },
  { code: "sv", label: "Swedish" },
  { code: "da", label: "Danish" },
  { code: "no", label: "Norwegian" },
  { code: "fi", label: "Finnish" },
  { code: "pl", label: "Polish" },
  { code: "tr", label: "Turkish" },
  { code: "id", label: "Indonesian" },
];

type AnyConfig = Record<string, unknown>;

type LlamaStatusLite = {
  state: "offline" | "loading" | "ready" | "error";
  message: string;
  managed: { running: boolean } | null;
};

type DownloadEvent = {
  kind: string;
  downloaded: number;
  total: number | null;
  done: boolean;
  error: string | null;
};

function formatBytes(bytes: number) {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
  if (bytes >= 1024 ** 2) return `${Math.round(bytes / 1024 ** 2)} MB`;
  return `${Math.round(bytes / 1024)} KB`;
}

async function persistStep(completed: boolean, step: number) {
  try {
    await invoke("set_onboarding_status", { completed, step });
  } catch {
    /* non-fatal — worst case the wizard shows again */
  }
}

export default function OnboardingWizard({
  initialStep,
  onDone,
}: {
  initialStep: number;
  onDone: () => void;
}) {
  const [step, setStep] = useState(() => Math.min(Math.max(initialStep, 0), 3));

  function goTo(next: number) {
    const clamped = Math.min(Math.max(next, 0), 3);
    setStep(clamped);
    void persistStep(false, clamped);
  }

  function finish() {
    void persistStep(true, 3);
    onDone();
  }

  return (
    <div className="onboarding-backdrop">
      <section className="onboarding-card" role="dialog" aria-modal="true" aria-label="Welcome to Smooth">
        <header className="onboarding-top">
          <span className="brand-mark" aria-hidden="true">S</span>
          <div className="onboarding-dots" aria-label={`Step ${step + 1} of 4`}>
            {[0, 1, 2, 3].map((index) => (
              <span key={index} className={index === step ? "dot active" : "dot"} />
            ))}
          </div>
          <button type="button" className="onboarding-skip" onClick={finish}>
            Skip for now
          </button>
        </header>

        <div className="onboarding-body" key={step}>
          {step === 0 ? <WelcomeStep /> : null}
          {step === 1 ? <AiStep /> : null}
          {step === 2 ? <TranscriptionStep /> : null}
          {step === 3 ? <TourStep /> : null}
        </div>

        <footer className="onboarding-nav">
          {step > 0 ? (
            <button type="button" className="secondary-action" onClick={() => goTo(step - 1)}>
              <ArrowLeft size={15} /> Back
            </button>
          ) : <span />}
          {step < 3 ? (
            <button type="button" className="primary-action" onClick={() => goTo(step + 1)}>
              {step === 0 ? "Get started" : "Continue"} <ArrowRight size={15} />
            </button>
          ) : (
            <button type="button" className="primary-action" onClick={finish}>
              <Check size={15} /> Start using Smooth
            </button>
          )}
        </footer>
      </section>
    </div>
  );
}

function WelcomeStep() {
  return (
    <div className="onboarding-step onboarding-welcome">
      <h1>Welcome to Smooth</h1>
      <p>
        Your local-first knowledge bank: notes, meeting transcripts, and an AI
        that helps you connect them — all on your machine.
      </p>
      <p className="onboarding-hint">Setup takes about two minutes. Every step can be skipped.</p>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Step 1 — AI engine
// ---------------------------------------------------------------------------

function AiStep() {
  const [choice, setChoice] = useState<"remote" | "local" | null>(null);
  const [apiKey, setApiKey] = useState("");
  const [apiUrl, setApiUrl] = useState("");
  const [model, setModel] = useState("");
  const [context, setContext] = useState("128000");
  const [busy, setBusy] = useState(false);
  const [remoteResult, setRemoteResult] = useState<string | null>(null);
  const [remoteError, setRemoteError] = useState<string | null>(null);
  const [localState, setLocalState] = useState<"idle" | "starting" | "downloading" | "ready" | "error">("idle");
  const [localMessage, setLocalMessage] = useState<string | null>(null);
  const pollRef = useRef<number | null>(null);

  useEffect(() => () => {
    if (pollRef.current) window.clearInterval(pollRef.current);
  }, []);

  async function saveRemote() {
    setBusy(true);
    setRemoteError(null);
    setRemoteResult(null);
    try {
      const config = await invoke<AnyConfig>("get_llama_config");
      await invoke("save_llama_config", {
        config: {
          ...config,
          default_provider: "remote",
          remote_base_url: apiUrl.trim(),
          remote_model: model.trim(),
          remote_api_key: apiKey.trim() || null,
          clear_remote_api_key: false,
          remote_context_tokens: Number(context) || 128000,
        },
      });
      const models = await invoke<{ id: string }[]>("test_remote_llm_connection");
      setRemoteResult(
        models.some((entry) => entry.id === model.trim())
          ? `Connected — ${model.trim()} is available`
          : `Connected — ${models.length} model${models.length === 1 ? "" : "s"} available`,
      );
    } catch (error) {
      setRemoteError(String(error));
    } finally {
      setBusy(false);
    }
  }

  const pollLocal = useCallback(() => {
    if (pollRef.current) window.clearInterval(pollRef.current);
    pollRef.current = window.setInterval(async () => {
      try {
        const status = await invoke<LlamaStatusLite>("get_llama_status");
        if (status.state === "ready") {
          setLocalState("ready");
          setLocalMessage("Local AI is ready");
          if (pollRef.current) window.clearInterval(pollRef.current);
        } else if (status.state === "error") {
          setLocalState("error");
          setLocalMessage(status.message);
        } else {
          setLocalState("downloading");
          setLocalMessage(status.message);
        }
      } catch {
        /* keep polling */
      }
    }, 3000);
  }, []);

  async function enableLocal() {
    setBusy(true);
    setLocalState("starting");
    setLocalMessage(null);
    try {
      const config = await invoke<AnyConfig>("get_llama_config");
      await invoke("save_llama_config", {
        config: { ...config, default_provider: "local", mode: "managed" },
      });
      await invoke("start_llama_server");
      setLocalState("downloading");
      setLocalMessage("Downloading the model — you can keep going, this continues in the background.");
      pollLocal();
    } catch (error) {
      setLocalState("error");
      setLocalMessage(String(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="onboarding-step">
      <h2>Choose your AI engine</h2>
      <p className="onboarding-lede">
        Smooth uses an AI model for chat, summaries, and entity extraction. Pick
        what fits you — you can change this any time in Settings.
      </p>

      <div className="onboarding-choices">
        <button
          type="button"
          className={choice === "remote" ? "onboarding-choice selected" : "onboarding-choice"}
          onClick={() => setChoice("remote")}
        >
          <span className="onboarding-choice-head"><Cloud size={17} /> Remote AI</span>
          <ul>
            <li className="pro">Fast results, stronger models</li>
            <li className="pro">No big download</li>
            <li className="con">Your data is shared with the provider</li>
            <li className="con">Usage costs apply</li>
          </ul>
        </button>
        <button
          type="button"
          className={choice === "local" ? "onboarding-choice selected" : "onboarding-choice"}
          onClick={() => setChoice("local")}
        >
          <span className="onboarding-choice-head"><Cpu size={17} /> Local AI</span>
          <ul>
            <li className="pro">Complete data privacy — nothing leaves this Mac</li>
            <li className="pro">No usage costs</li>
            <li className="con">{LOCAL_MODEL_SIZE}, slower results</li>
            <li className="con">Smaller model, slower update cycle</li>
          </ul>
        </button>
      </div>

      {choice === "remote" ? (
        <div className="onboarding-detail">
          <label className="agent-field">
            <span>API key</span>
            <input
              type="password"
              value={apiKey}
              autoComplete="off"
              placeholder="Paste your API key"
              onChange={(event) => setApiKey(event.target.value)}
            />
          </label>
          <label className="agent-field">
            <span>API URL</span>
            <input
              value={apiUrl}
              placeholder="https://api.inceptionlabs.ai"
              onChange={(event) => setApiUrl(event.target.value)}
            />
          </label>
          <div className="onboarding-field-row">
            <label className="agent-field">
              <span>Model</span>
              <input
                value={model}
                placeholder="mercury-2"
                onChange={(event) => setModel(event.target.value)}
              />
            </label>
            <label className="agent-field narrow">
              <span>Context window</span>
              <input
                type="number"
                min={1024}
                value={context}
                onChange={(event) => setContext(event.target.value)}
              />
            </label>
          </div>
          <p className="onboarding-hint">
            Context defaults to 128k tokens — adjustable here and later in
            Settings. In the future this path will route through our server.
          </p>
          {remoteError ? <p className="form-error">{remoteError}</p> : null}
          {remoteResult ? (
            <p className="onboarding-ok"><CheckCircle2 size={15} /> {remoteResult}</p>
          ) : null}
          <button
            type="button"
            className="primary-action"
            disabled={busy || !apiUrl.trim() || !model.trim()}
            onClick={() => void saveRemote()}
          >
            {busy ? <Loader2 size={15} className="spin" /> : null}
            Verify and save
          </button>
        </div>
      ) : null}

      {choice === "local" ? (
        <div className="onboarding-detail">
          <p>
            <strong>{LOCAL_MODEL_LABEL}</strong> — {LOCAL_MODEL_SIZE}. Stored in
            your app library, runs fully offline.
          </p>
          {localState === "idle" || localState === "error" ? (
            <button
              type="button"
              className="primary-action"
              disabled={busy}
              onClick={() => void enableLocal()}
            >
              {busy ? <Loader2 size={15} className="spin" /> : null}
              Download and enable
            </button>
          ) : (
            <p className={localState === "ready" ? "onboarding-ok" : "onboarding-progressline"}>
              {localState === "ready" ? <CheckCircle2 size={15} /> : <Loader2 size={15} className="spin" />}
              {localMessage ?? "Starting local AI…"}
            </p>
          )}
          {localState === "error" && localMessage ? (
            <p className="form-error">{localMessage}</p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Step 2 — Transcription (optional)
// ---------------------------------------------------------------------------

function TranscriptionStep() {
  const [choice, setChoice] = useState<"english" | "multilingual" | null>(null);
  const [language, setLanguage] = useState("en");
  const [progress, setProgress] = useState<DownloadEvent | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [done, setDone] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [micState, setMicState] = useState<"idle" | "granted" | "failed">("idle");
  const [warmup, setWarmup] = useState<"idle" | "running" | "done" | "failed">("idle");

  useEffect(() => {
    const unlisten = listen<DownloadEvent>("stt-model-download", ({ payload }) => {
      setProgress(payload);
      if (payload.done) setDone(true);
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  async function warmUpDiarization() {
    setWarmup("running");
    try {
      await invoke("warm_up_diarization");
      setWarmup("done");
    } catch {
      // Non-fatal: the helper re-downloads on the first meeting instead.
      setWarmup("failed");
    }
  }

  async function download() {
    if (!choice) return;
    setDownloading(true);
    setError(null);
    try {
      await invoke("download_stt_model", {
        kind: choice,
        language: choice === "english" ? "en" : language,
      });
      setDone(true);
      // Pre-fetch the speaker-identification (diarization) models too, so the
      // first meeting doesn't stall on a mid-meeting download.
      void warmUpDiarization();
    } catch (downloadError) {
      setError(String(downloadError));
    } finally {
      setDownloading(false);
    }
  }

  async function requestMic() {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      stream.getTracks().forEach((track) => track.stop());
      setMicState("granted");
    } catch {
      setMicState("failed");
    }
  }

  const percent =
    progress && progress.total && !progress.done
      ? Math.min(100, Math.round((progress.downloaded / progress.total) * 100))
      : null;

  return (
    <div className="onboarding-step">
      <h2>Meeting transcription</h2>
      <p className="onboarding-lede">
        <strong>Optional</strong> — only needed if you want Smooth to take
        meeting notes. Skip freely and set it up later in Settings.
      </p>

      <div className="onboarding-choices">
        <button
          type="button"
          className={choice === "english" ? "onboarding-choice selected" : "onboarding-choice"}
          onClick={() => setChoice("english")}
        >
          <span className="onboarding-choice-head"><Mic size={17} /> English only</span>
          <ul>
            <li className="pro">Small download (≈ 148 MB)</li>
            <li className="pro">Low memory footprint</li>
            <li className="con">Transcribes English only</li>
          </ul>
        </button>
        <button
          type="button"
          className={choice === "multilingual" ? "onboarding-choice selected" : "onboarding-choice"}
          onClick={() => setChoice("multilingual")}
        >
          <span className="onboarding-choice-head"><Languages size={17} /> Multilingual</span>
          <ul>
            <li className="pro">English, French, German, Spanish and more</li>
            <li className="con">Larger download (≈ 1.5 GB)</li>
            <li className="con">Higher memory use</li>
          </ul>
        </button>
      </div>

      {choice ? (
        <div className="onboarding-detail">
          {choice === "multilingual" ? (
            <label className="agent-field narrow">
              <span>Primary meeting language</span>
              <select value={language} onChange={(event) => setLanguage(event.target.value)}>
                {LANGUAGES.map((entry) => (
                  <option key={entry.code} value={entry.code}>{entry.label}</option>
                ))}
              </select>
            </label>
          ) : null}

          {done ? (
            <>
              <p className="onboarding-ok"><CheckCircle2 size={15} /> Transcription model installed</p>
              {warmup === "running" ? (
                <p className="onboarding-progressline">
                  <Loader2 size={15} className="spin" /> Preparing speaker identification…
                </p>
              ) : null}
              {warmup === "done" ? (
                <p className="onboarding-ok"><CheckCircle2 size={15} /> Speaker identification ready</p>
              ) : null}
              {warmup === "failed" ? (
                <p className="onboarding-hint">
                  Speaker identification will finish setting up during your
                  first meeting.
                </p>
              ) : null}
            </>
          ) : downloading ? (
            <div className="onboarding-progress">
              <div className="onboarding-progress-bar">
                <div
                  className="onboarding-progress-fill"
                  style={{ width: percent !== null ? `${percent}%` : "100%" }}
                  data-indeterminate={percent === null || undefined}
                />
              </div>
              <span>
                {progress && progress.total
                  ? `${formatBytes(progress.downloaded)} of ${formatBytes(progress.total)}`
                  : "Downloading…"}
              </span>
            </div>
          ) : (
            <button type="button" className="primary-action" onClick={() => void download()}>
              Download model
            </button>
          )}
          {error ? <p className="form-error">{error}</p> : null}
        </div>
      ) : null}

      <div className="onboarding-permission">
        <div>
          <strong>Microphone access</strong>
          <p>Needed to capture your side of a meeting. macOS will ask once.</p>
        </div>
        {micState === "granted" ? (
          <span className="onboarding-ok"><CheckCircle2 size={15} /> Granted</span>
        ) : (
          <button type="button" className="secondary-action" onClick={() => void requestMic()}>
            <Mic size={15} /> Enable microphone
          </button>
        )}
      </div>
      {micState === "failed" ? (
        <p className="onboarding-hint">
          Couldn&rsquo;t request access here — macOS will prompt the first time
          you start a meeting, or grant it in System Settings › Privacy &
          Security › Microphone.
        </p>
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Step 3 — Tour + tutorials
// ---------------------------------------------------------------------------

const TOUR: { icon: typeof Mic; title: string; text: string }[] = [
  { icon: FolderTree, title: "Sidebar", text: "Folders, notes and search live on the left." },
  { icon: PenLine, title: "Editor", text: "Write in Markdown; entities are extracted automatically." },
  { icon: PanelRight, title: "Context panel", text: "Details, links, chat and tasks for the open note." },
  { icon: Radio, title: "Meeting capsule", text: "Start a meeting from the bar at the bottom — Smooth transcribes and summarizes." },
  { icon: Command, title: "Command palette", text: "Press ⌘K to jump anywhere or create anything." },
];

function TourStep() {
  return (
    <div className="onboarding-step">
      <h2>Find your way around</h2>
      <div className="onboarding-tour">
        {TOUR.map((item) => {
          const Icon = item.icon;
          return (
            <div className="onboarding-tour-row" key={item.title}>
              <span className="onboarding-tour-icon"><Icon size={16} /></span>
              <div>
                <strong>{item.title}</strong>
                <p>{item.text}</p>
              </div>
            </div>
          );
        })}
      </div>

      <h3 className="onboarding-subhead">Go further</h3>
      <div className="onboarding-links">
        <button type="button" onClick={() => void openUrl(TUTORIAL_LINKS.google)}>
          <BookOpen size={14} /> Connect Google Calendar &amp; Gmail
        </button>
        <button type="button" onClick={() => void openUrl(TUTORIAL_LINKS.slack)}>
          <BookOpen size={14} /> Connect Slack
        </button>
        <button type="button" onClick={() => void openUrl(TUTORIAL_LINKS.mcp)}>
          <BookOpen size={14} /> Use the built-in MCP server
        </button>
      </div>
    </div>
  );
}
