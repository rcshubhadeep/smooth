# Tauri + React + Typescript

This template should help get you started developing with Tauri, React and Typescript in Vite.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

## Example claude MCP configuration

```json
"mcpServers": {
    "smooth-bridge": {
      "command": "/Users/shubhadeeproychowdhury/work/personal/smooth/mcp-remote-wrapper.sh",
      "args": [
        "-y",
        "mcp-remote",
        "http://127.0.0.1:17843/mcp",
        "--header",
        "Authorization:${AUTH_HEADER}",
        "--transport",
        "http-only"
      ],
      "env": {
        "AUTH_HEADER": "Bearer <YOUR TOKEN>"
      }
    }
  }
```
Do **NOT** commit your token to version control.
**REMEMBER** to update the token if you have changed it.
