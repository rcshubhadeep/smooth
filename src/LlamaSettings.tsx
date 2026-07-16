import { invoke } from "@tauri-apps/api/core";
import {
  CheckCircle2,
  CircleAlert,
  Play,
  RefreshCw,
  Server,
  Square,
} from "lucide-react";
import { useCallback, useEffect, useState } from "react";

type LlamaConfig = {
  default_provider: "local" | "inception";
  always_obey_global_llm: boolean;
  mode: "managed" | "external";
  base_url: string;
  preferred_model: string | null;
  managed_model: string;
  context_size: number;
  gpu_layers: number;
  flash_attention: boolean;
  parallel: number;
  cache_ram_mb: number;
  context_checkpoints: number;
  cache_type_k: string;
  cache_type_v: string;
  spec_type: string;
  spec_draft_n_max: number;
  inception_base_url: string;
  inception_model: string;
  inception_api_key: string | null;
  clear_inception_api_key: boolean;
  inception_api_key_configured: boolean;
};

type LlamaModel = {
  id: string;
  owned_by: string | null;
  context_size: number | null;
  parameter_count: number | null;
  size_bytes: number | null;
};

type ManagedLlamaSnapshot = {
  running: boolean;
  endpoint: string | null;
  cache_dir: string | null;
  log_path: string | null;
  last_error: string | null;
};

type LlamaStatus = {
  state: "offline" | "loading" | "ready" | "error";
  base_url: string;
  message: string;
  latency_ms: number | null;
  checked_at: string;
  models: LlamaModel[];
  managed: ManagedLlamaSnapshot | null;
};

const defaultConfig: LlamaConfig = {
  default_provider: "local",
  always_obey_global_llm: false,
  mode: "managed",
  base_url: "http://127.0.0.1:8080",
  preferred_model: null,
  managed_model: "unsloth/gemma-4-12B-it-qat-GGUF:UD-Q4_K_XL",
  context_size: 8192,
  gpu_layers: 999,
  flash_attention: true,
  parallel: 1,
  cache_ram_mb: 2048,
  context_checkpoints: 2,
  cache_type_k: "q8_0",
  cache_type_v: "q8_0",
  spec_type: "draft-mtp",
  spec_draft_n_max: 2,
  inception_base_url: "https://api.inceptionlabs.ai",
  inception_model: "mercury-2",
  inception_api_key: null,
  clear_inception_api_key: false,
  inception_api_key_configured: false,
};

function formatLargeValue(value: number | null, suffix: string) {
  if (value === null) return null;
  return `${Intl.NumberFormat(undefined, { notation: "compact" }).format(value)}${suffix}`;
}

export default function LlamaSettings({
  onError,
  view,
}: {
  onError: (message: string) => void;
  view: "default" | "remote" | "local";
}) {
  const [config, setConfig] = useState(defaultConfig);
  const [status, setStatus] = useState<LlamaStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [remoteStatus, setRemoteStatus] = useState<string | null>(null);
  const [savingGlobalLock, setSavingGlobalLock] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const next = await invoke<LlamaStatus>("get_llama_status");
      setStatus(next);
      return next;
    } catch (error) {
      onError(String(error));
      return null;
    }
  }, [onError]);

  useEffect(() => {
    invoke<LlamaConfig>("get_llama_config")
      .then(setConfig)
      .then(refresh)
      .catch((error) => onError(String(error)))
      .finally(() => setLoading(false));
  }, [onError, refresh]);

  useEffect(() => {
    if (!status?.managed?.running || status.state === "ready") return undefined;
    const interval = window.setInterval(() => void refresh(), 2000);
    return () => window.clearInterval(interval);
  }, [refresh, status?.managed?.running, status?.state]);

  async function save() {
    setBusy(true);
    try {
      const saved = await invoke<LlamaConfig>("save_llama_config", { config });
      setConfig(saved);
      await refresh();
    } catch (error) {
      onError(String(error));
    } finally {
      setBusy(false);
    }
  }

  async function updateGlobalLock(enabled: boolean) {
    const previous = config.always_obey_global_llm;
    setConfig((current) => ({ ...current, always_obey_global_llm: enabled }));
    setSavingGlobalLock(true);
    try {
      const saved = await invoke<boolean>("set_always_obey_global_llm", { enabled });
      setConfig((current) => ({ ...current, always_obey_global_llm: saved }));
    } catch (error) {
      setConfig((current) => ({ ...current, always_obey_global_llm: previous }));
      onError(String(error));
    } finally {
      setSavingGlobalLock(false);
    }
  }

  async function testInception() {
    setBusy(true);
    setRemoteStatus(null);
    try {
      const saved = await invoke<LlamaConfig>("save_llama_config", { config });
      setConfig(saved);
      const models = await invoke<LlamaModel[]>("test_inception_connection");
      setRemoteStatus(
        models.some((model) => model.id === saved.inception_model)
          ? `${saved.inception_model} is available`
          : `Connected; ${models.length} model${models.length === 1 ? "" : "s"} available`,
      );
    } catch (error) {
      onError(String(error));
    } finally {
      setBusy(false);
    }
  }

  async function removeRemoteKey() {
    setBusy(true);
    try {
      const saved = await invoke<LlamaConfig>("save_llama_config", {
        config: {
          ...config,
          inception_api_key: null,
          clear_inception_api_key: true,
        },
      });
      setConfig(saved);
      await refresh();
    } catch (error) {
      onError(String(error));
    } finally {
      setBusy(false);
    }
  }

  async function start() {
    setBusy(true);
    try {
      const saved = await invoke<LlamaConfig>("save_llama_config", { config });
      setConfig(saved);
      setStatus(await invoke<LlamaStatus>("start_llama_server"));
    } catch (error) {
      onError(String(error));
    } finally {
      setBusy(false);
    }
  }

  async function stop() {
    setBusy(true);
    try {
      setStatus(await invoke<LlamaStatus>("stop_llama_server"));
    } catch (error) {
      onError(String(error));
    } finally {
      setBusy(false);
    }
  }

  const StatusIcon =
    status?.state === "ready"
      ? CheckCircle2
      : status?.state === "loading"
        ? RefreshCw
        : CircleAlert;

  return (
    <>
      {view === "default" ? (
      <section className="settings-section llama-settings">
        <div className="section-heading">
          <Server size={18} />
          <span>Default LLM</span>
        </div>
        <div className="segmented llama-mode-toggle" aria-label="Default LLM provider">
          <button
            type="button"
            className={config.default_provider === "local" ? "active" : ""}
            onClick={() => setConfig((current) => ({ ...current, default_provider: "local" }))}
          >
            Local
          </button>
          <button
            type="button"
            className={config.default_provider === "inception" ? "active" : ""}
            onClick={() => setConfig((current) => ({ ...current, default_provider: "inception" }))}
          >
            Remote
          </button>
        </div>
        <p className="settings-help">
          Automatic extraction and background tasks use this provider. Chat and agent runs can ask which provider to use.
        </p>
        <label className="settings-checkbox llama-global-lock">
          <input
            type="checkbox"
            checked={config.always_obey_global_llm}
            disabled={loading || savingGlobalLock}
            onChange={(event) => {
              const enabled = event.target.checked;
              void updateGlobalLock(enabled);
            }}
          />
          <span>
            <strong>Always obey the global LLM setting</strong>
            <small>Do not ask before chat or agent runs; always use the provider selected above.</small>
          </span>
        </label>
      </section>
      ) : null}

      {view === "remote" ? (
      <section className="settings-section llama-settings">
        <div className="section-heading">
          <span>Remote</span>
          <small>OpenAI-compatible remote API</small>
        </div>
        <label className="settings-field">
          <span>API key</span>
          <input
            type="password"
            value={config.inception_api_key ?? ""}
            onChange={(event) =>
              setConfig((current) => ({
                ...current,
                inception_api_key: event.target.value || null,
                clear_inception_api_key: false,
              }))
            }
            placeholder={config.inception_api_key_configured ? "Configured; enter a new key to replace" : "Enter API key"}
            autoComplete="off"
          />
        </label>
        <label className="settings-field">
          <span>Model</span>
          <input
            value={config.inception_model}
            onChange={(event) => setConfig((current) => ({ ...current, inception_model: event.target.value }))}
          />
        </label>
        <label className="settings-field">
          <span>API URL</span>
          <input
            value={config.inception_base_url}
            onChange={(event) => setConfig((current) => ({ ...current, inception_base_url: event.target.value }))}
          />
        </label>
        {remoteStatus ? <p className="settings-help">{remoteStatus}</p> : null}
        <div className="settings-actions">
          <button type="button" onClick={() => void testInception()} disabled={busy || loading}>
            <RefreshCw size={15} /> Test connection
          </button>
          {config.inception_api_key_configured ? (
            <button
              type="button"
              onClick={() => void removeRemoteKey()}
              disabled={busy || loading}
            >
              Remove saved API key
            </button>
          ) : null}
        </div>
      </section>
      ) : null}

      {view === "local" ? (
      <>
      <section className="settings-section llama-settings">
        <div className="section-heading">
          <Server size={18} />
          <span>Local model server</span>
          <button
            className="icon-button"
            type="button"
            onClick={() => void refresh()}
            disabled={busy || loading}
            title="Refresh model server status"
          >
            <RefreshCw className={busy ? "spin" : ""} size={16} />
          </button>
        </div>

        <div className="segmented llama-mode-toggle" aria-label="Model server mode">
          <button
            type="button"
            className={config.mode === "managed" ? "active" : ""}
            onClick={() => setConfig((current) => ({ ...current, mode: "managed" }))}
          >
            Managed
          </button>
          <button
            type="button"
            className={config.mode === "external" ? "active" : ""}
            onClick={() => setConfig((current) => ({ ...current, mode: "external" }))}
          >
            External
          </button>
        </div>

        {config.mode === "managed" ? (
          <>
            <label className="settings-field">
              <span>Hugging Face model</span>
              <input
                value={config.managed_model}
                onChange={(event) =>
                  setConfig((current) => ({
                    ...current,
                    managed_model: event.target.value,
                  }))
                }
              />
            </label>
            <div className="llama-number-grid">
              <label className="settings-field">
                <span>Context</span>
                <input
                  type="number"
                  min={512}
                  value={config.context_size}
                  onChange={(event) =>
                    setConfig((current) => ({
                      ...current,
                      context_size: Number(event.target.value),
                    }))
                  }
                />
              </label>
              <label className="settings-field">
                <span>RAM cache (MB)</span>
                <input
                  type="number"
                  min={0}
                  value={config.cache_ram_mb}
                  onChange={(event) =>
                    setConfig((current) => ({
                      ...current,
                      cache_ram_mb: Number(event.target.value),
                    }))
                  }
                />
              </label>
            </div>
          </>
        ) : (
          <label className="settings-field">
            <span>Server URL</span>
            <input
              value={config.base_url}
              onChange={(event) =>
                setConfig((current) => ({
                  ...current,
                  base_url: event.target.value,
                }))
              }
              placeholder="http://127.0.0.1:8080"
            />
          </label>
        )}

        <div className={`connection-status ${status?.state ?? "offline"}`}>
          <StatusIcon
            className={status?.state === "loading" ? "spin" : ""}
            size={19}
          />
          <div>
            <strong>{status?.state ?? (loading ? "checking" : "offline")}</strong>
            <span>{status?.message ?? "Checking llama.cpp"}</span>
          </div>
          {status?.latency_ms !== null && status?.latency_ms !== undefined ? (
            <small>{status.latency_ms} ms</small>
          ) : null}
        </div>

        {config.mode === "managed" && status?.managed?.cache_dir ? (
          <p className="settings-help llama-cache-path">
            Models: {status.managed.cache_dir}
          </p>
        ) : null}

        <div className="settings-actions">
          <button type="button" onClick={() => void save()} disabled={busy || loading}>
            Save
          </button>
          {config.mode === "managed" ? (
            status?.managed?.running ? (
              <button type="button" onClick={() => void stop()} disabled={busy}>
                <Square size={15} /> Stop
              </button>
            ) : (
              <button type="button" onClick={() => void start()} disabled={busy || loading}>
                <Play size={15} /> Start
              </button>
            )
          ) : null}
        </div>
      </section>

      <section className="settings-section">
        <div className="section-heading">
          <span>Model</span>
          <small>{status?.models.length ?? 0} discovered</small>
        </div>
        {config.mode === "external" ? (
          <label className="settings-field">
            <span>Preferred model</span>
            <select
              value={config.preferred_model ?? ""}
              onChange={(event) =>
                setConfig((current) => ({
                  ...current,
                  preferred_model: event.target.value || null,
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
        ) : null}
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
      </>
      ) : null}
    </>
  );
}
