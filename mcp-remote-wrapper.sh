#!/bin/bash
# Please replace the path for your Node.js (nvm) installation
export PATH="/Users/shubhadeeproychowdhury/.nvm/versions/node/v24.7.0/bin:$PATH"
exec /Users/shubhadeeproychowdhury/.nvm/versions/node/v24.7.0/bin/node \
     /Users/shubhadeeproychowdhury/.nvm/versions/node/v24.7.0/bin/npx "$@"
EOF
chmod +x ~/mcp-remote-wrapper.sh
