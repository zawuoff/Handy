#!/usr/bin/env bash
# Launches Noted as a read-only MCP server over stdio.
# Prefers the installed binary (self-contained via its bundled libs); falls
# back to the dev build, which needs the cargo build dir on the loader path.
if [ -x /usr/bin/handy ]; then
  exec /usr/bin/handy --mcp-serve
fi
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LIB_DIR="$(find "$DIR/src-tauri/target/debug/build" -maxdepth 3 -type d -name lib -path "*transcribe-cpp-sys*" | head -1)"
export LD_LIBRARY_PATH="$LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$DIR/src-tauri/target/debug/handy" --mcp-serve
