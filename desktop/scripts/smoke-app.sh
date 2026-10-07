#!/usr/bin/env bash
# Launch TT Desktop against a temporary, isolated tt registry and store.
#
# Nothing here reads or writes the user's real ~/.local/share/tt: the app is
# started with TT_CONFIG and XDG_DATA_HOME pointing at a fresh temp directory,
# pre-seeded with one registered project and two tasks. Ctrl-C quits.
#
#   ./scripts/smoke-app.sh                          # release bundle
#   ./scripts/smoke-app.sh --debug                  # debug bundle
#   ./scripts/smoke-app.sh --binary                 # raw release binary
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
mode="${1:-release}"

case "$mode" in
  --debug) target="debug" ;;
  --binary) target="release" ;;
  *) target="release" ;;
esac

if [[ "$mode" == "--binary" ]]; then
  app=""
  binary="$here/../src-tauri/target/$target/tt-desktop"
else
  app="$here/../src-tauri/target/$target/bundle/macos/TT Desktop.app"
  binary="$app/Contents/MacOS/tt-desktop"
fi

if [[ ! -x "$binary" ]]; then
  echo "not built: $binary" >&2
  echo "run: npm run tauri:build -- --bundles app" >&2
  exit 1
fi

fixture="$(mktemp -d)"
data="$fixture/data"
project="$fixture/demo-project"
store="$data/tt/demo"
mkdir -p "$project" "$store"

cat > "$data/tt/config.toml" <<EOF
[[project]]
path = "$project"
slug = "demo"
never_ask_nested = false
EOF

cat > "$store/abc1234567.md" <<'EOF'
---
id: abc1234567
title: Smoke-test root
state: open
tags:
- smoke
---
Welcome to the isolated smoke test. Link to [[def1234567]].
EOF

cat > "$store/def1234567.md" <<'EOF'
---
id: def1234567
title: Child task
state: open
parent: abc1234567
---
A sub-task.
EOF

echo "fixture:    $fixture"
echo "config:     $data/tt/config.toml"
echo "store:      $store"
echo "binary:     $binary"
[[ -n "$app" ]] && echo "bundle:     $app"
echo "launching..."
echo

TT_CONFIG="$data/tt/config.toml" \
XDG_DATA_HOME="$data" \
  exec "$binary"
