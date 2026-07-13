# Tauri + React + Typescript

This template should help get you started developing with Tauri, React and Typescript in Vite.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

## Example claude MCP configuration

- Download the `mcp-remote-wrapper.sh` script and place it in a directory.
- Run `which npx` to find the path to npx on your system.
- Change the npx executable path to match your system. For example, if `which npx` returns `/usr/local/bin/npx`, update the `mcp-remote-wrapper.sh` script to use `/usr/local/bin/npx` instead of `npx`.
- Make the script executable: `chmod +x <DOWNLOAD_DIR>/mcp-remote-wrapper.sh`
- Update the Claude MCP configuration to use the `mcp-remote-wrapper.sh` script. Like so:

```json
"mcpServers": {
    "smooth-bridge": {
      "command": "<DOWNLOAD_DIR>/mcp-remote-wrapper.sh",
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
- Do **NOT** commit your token to version control.
- **REMEMBER** to update the token if you have changed it.
