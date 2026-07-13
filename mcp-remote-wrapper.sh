#!/bin/bash
# Claude Desktop launches this script without your shell's PATH, so a plain
# "npx" call often fails to find Node. Set NODE_DIR to the directory that
# contains your `node` and `npx` binaries (run `which npx` in a terminal and
# drop the trailing "/npx" from its output), then leave the rest as-is.
# For an example, if you are using nvm then maybe something like: /Users/shubhadeeproychowdhury/.nvm/versions/node/v24.7.0/bin
NODE_DIR="/path/to/your/node/bin"
export PATH="$NODE_DIR:$PATH"
exec "$NODE_DIR/node" "$NODE_DIR/npx" "$@"
