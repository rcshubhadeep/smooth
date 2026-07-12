import { invoke } from "@tauri-apps/api/core";
import { CheckCircle2, CircleAlert, MessageSquare } from "lucide-react";
import { useEffect, useState } from "react";

type SlackConfig = {
  has_app_token: boolean;
  has_bot_token: boolean;
  connected: boolean;
  last_error: string | null;
  folder_name: string;
  trigger: string;
};

export default function SlackSettings() {
  const [config, setConfig] = useState<SlackConfig | null>(null);
  const [appToken, setAppToken] = useState("");
  const [botToken, setBotToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    const next = await invoke<SlackConfig>("get_slack_config");
    setConfig(next);
  }

  useEffect(() => {
    void refresh().catch((loadError) => setError(String(loadError)));
    const timer = window.setInterval(() => {
      void refresh().catch(() => undefined);
    }, 3000);
    return () => window.clearInterval(timer);
  }, []);

  async function save() {
    setBusy(true);
    setError(null);
    try {
      const next = await invoke<SlackConfig>("save_slack_config", {
        config: { app_token: appToken, bot_token: botToken },
      });
      setConfig(next);
      setAppToken("");
      setBotToken("");
    } catch (saveError) {
      setError(String(saveError));
    } finally {
      setBusy(false);
    }
  }

  async function disconnect() {
    setBusy(true);
    setError(null);
    try {
      setConfig(await invoke<SlackConfig>("clear_slack_config"));
    } catch (clearError) {
      setError(String(clearError));
    } finally {
      setBusy(false);
    }
  }

  const configured = Boolean(config?.has_app_token && config?.has_bot_token);
  const canSave = Boolean(
    (appToken || config?.has_app_token) &&
      (botToken || config?.has_bot_token) &&
      (appToken || botToken),
  );
  const visibleError = error ?? config?.last_error;

  return (
    <section className="settings-section">
      <div className="section-heading">
        <MessageSquare size={18} />
        <span>Slack</span>
        <small>{config?.connected ? "Connected" : configured ? "Connecting" : "Not configured"}</small>
      </div>

      <label className="settings-field">
        <span>Socket Mode app token</span>
        <input
          type="password"
          value={appToken}
          onChange={(event) => setAppToken(event.currentTarget.value)}
          placeholder={config?.has_app_token ? "Configured (xapp-)" : "xapp-..."}
          autoComplete="off"
        />
      </label>

      <label className="settings-field">
        <span>Bot token</span>
        <input
          type="password"
          value={botToken}
          onChange={(event) => setBotToken(event.currentTarget.value)}
          placeholder={config?.has_bot_token ? "Configured (xoxb-)" : "xoxb-..."}
          autoComplete="off"
        />
      </label>

      <div className={`connection-status ${config?.connected ? "ready" : "offline"}`}>
        {config?.connected ? <CheckCircle2 size={19} /> : <CircleAlert size={19} />}
        <div>
          <strong>{config?.connected ? "Socket Mode connected" : "Socket Mode offline"}</strong>
          <span>{visibleError ?? `Trigger: ${config?.trigger ?? "@smooth create note"}`}</span>
        </div>
        <small>{config?.folder_name ?? "Notes From Slack"}</small>
      </div>

      <p className="settings-help">
        Requires the app_mention event, app_mentions:read, channels:history, and chat:write.
        Smooth only imports a thread when mentioned inside it with “create note”. Enter both
        tokens the first time; after that, leave a configured token blank to keep it unchanged.
      </p>

      <div className="settings-actions">
        <button type="button" onClick={() => void save()} disabled={busy || !canSave}>
          {busy ? "Saving" : "Save Slack Settings"}
        </button>
        {configured ? (
          <button type="button" onClick={() => void disconnect()} disabled={busy}>
            Disconnect Slack
          </button>
        ) : null}
      </div>
    </section>
  );
}
