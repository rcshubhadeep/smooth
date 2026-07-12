import { invoke } from "@tauri-apps/api/core";
import { Check, Copy, Network } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

type McpStatus = {
  enabled: boolean;
  endpoint: string;
  bearer_token: string;
  read_only: boolean;
  tools: string[];
};

export default function McpSettings() {
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<"endpoint" | "config" | null>(null);
  const [tokenInput, setTokenInput] = useState("");
  const [isSavingToken, setIsSavingToken] = useState(false);

  useEffect(() => {
    invoke<McpStatus>("get_mcp_status").then((loaded) => {
      setStatus(loaded);
      setTokenInput(loaded.bearer_token);
    }).catch((loadError) => {
      setError(String(loadError));
    });
  }, []);

  async function saveToken() {
    setIsSavingToken(true);
    setError(null);
    try {
      const updated = await invoke<McpStatus>("set_mcp_bearer_token", {
        token: tokenInput,
      });
      setStatus(updated);
      setTokenInput(updated.bearer_token);
    } catch (saveError) {
      setError(String(saveError));
    } finally {
      setIsSavingToken(false);
    }
  }

  async function regenerateToken() {
    setIsSavingToken(true);
    setError(null);
    try {
      const updated = await invoke<McpStatus>("set_mcp_bearer_token", {
        token: null,
      });
      setStatus(updated);
      setTokenInput(updated.bearer_token);
    } catch (saveError) {
      setError(String(saveError));
    } finally {
      setIsSavingToken(false);
    }
  }

  const clientConfig = useMemo(() => {
    if (!status) return "";
    return JSON.stringify(
      {
        mcpServers: {
          smooth: {
            url: status.endpoint,
            headers: {
              Authorization: `Bearer ${status.bearer_token}`,
            },
          },
        },
      },
      null,
      2,
    );
  }, [status]);

  async function copy(value: string, target: "endpoint" | "config") {
    await navigator.clipboard.writeText(value);
    setCopied(target);
    window.setTimeout(() => setCopied(null), 1400);
  }

  return (
    <section className="settings-section">
      <div className="section-heading">
        <Network size={18} />
        <span>MCP server</span>
        <small>{status?.read_only ? "Read only" : "Unavailable"}</small>
      </div>

      {error ? <p className="settings-help error">{error}</p> : null}
      {status ? (
        <>
          <div className="mcp-endpoint-row">
            <div>
              <strong>Local endpoint</strong>
              <code>{status.endpoint}</code>
            </div>
            <button
              type="button"
              onClick={() => void copy(status.endpoint, "endpoint")}
              title="Copy endpoint"
            >
              {copied === "endpoint" ? <Check size={15} /> : <Copy size={15} />}
            </button>
          </div>

          <label className="settings-field">
            <span>Auth header (Bearer token)</span>
            <div className="settings-input-row">
              <input
                value={tokenInput}
                onChange={(event) => setTokenInput(event.currentTarget.value)}
                spellCheck={false}
                placeholder="Bearer token"
              />
              <button
                type="button"
                onClick={() => void saveToken()}
                disabled={isSavingToken || !tokenInput.trim()}
              >
                Save
              </button>
              <button
                type="button"
                onClick={() => void regenerateToken()}
                disabled={isSavingToken}
              >
                Regenerate
              </button>
            </div>
          </label>
          <p className="settings-help">
            Clients must send this as{" "}
            <code>Authorization: Bearer {"<token>"}</code>. Changing it takes
            effect immediately for new requests.
          </p>

          <div className="mcp-tools">
            {status.tools.map((tool) => (
              <code key={tool}>{tool}</code>
            ))}
          </div>

          <div className="mcp-config">
            <div>
              <strong>Client configuration</strong>
              <button
                type="button"
                onClick={() => void copy(clientConfig, "config")}
                title="Copy client configuration"
              >
                {copied === "config" ? <Check size={15} /> : <Copy size={15} />}
              </button>
            </div>
            <pre>{clientConfig}</pre>
          </div>
        </>
      ) : (
        <p className="settings-help">Loading MCP server configuration</p>
      )}
    </section>
  );
}
