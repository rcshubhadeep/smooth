#!/bin/bash
# Claude Desktop launches this script without your shell's PATH, so a plain
# "npx" call often fails to find Node. This script reads NODE_DIR from the
# environment (set it in the "env" block of your Claude Desktop MCP config)
# so you don't have to edit this file. NODE_DIR is the directory that
# contains your `node` and `npx` binaries: run `which npx` in a terminal and
# drop the trailing "/npx" from its output.
# For example, with nvm it might be: /Users/you/.nvm/versions/node/v24.7.0/bin
set -euo pipefail

if [ -z "${NODE_DIR:-}" ]; then
  echo "mcp-remote-wrapper: NODE_DIR is not set. Add it to the \"env\" block of your Claude Desktop MCP config (the directory containing node and npx)." >&2
  exit 1
fi

if [ ! -x "$NODE_DIR/node" ] || [ ! -x "$NODE_DIR/npx" ]; then
  echo "mcp-remote-wrapper: could not find executable node and npx in NODE_DIR=$NODE_DIR" >&2
  exit 1
fi

export PATH="$NODE_DIR:$PATH"
exec "$NODE_DIR/node" "$NODE_DIR/npx" "$@"
