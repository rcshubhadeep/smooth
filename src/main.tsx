import { Component, type ReactNode } from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

// Only opt into the translucent (vibrancy) treatment when running inside the
// Tauri macOS shell — where the native NSVisualEffectView backdrop actually
// exists. In a plain browser this class is absent, so backgrounds stay opaque.
if (
  "__TAURI_INTERNALS__" in window &&
  navigator.userAgent.includes("Macintosh")
) {
  document.documentElement.classList.add("is-macos");
}

class RootErrorBoundary extends Component<
  { children: ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    if (this.state.error) {
      return (
        <main style={{ padding: 24, fontFamily: "system-ui, sans-serif" }}>
          <h1>Smooth hit a UI error</h1>
          <pre style={{ whiteSpace: "pre-wrap" }}>{this.state.error.message}</pre>
          <button type="button" onClick={() => window.location.reload()}>
            Reload
          </button>
        </main>
      );
    }

    return this.props.children;
  }
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <RootErrorBoundary>
    <App />
  </RootErrorBoundary>,
);
