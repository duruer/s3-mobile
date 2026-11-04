#!/usr/bin/env bash
# Snapshot the settlers-3 working tree into refs/wip/autosave every 20 min.
# Uses a private index, so HEAD, the real index and the working tree are never touched.
set -u
cd "$(dirname "$0")/.." || exit 1
export GIT_INDEX_FILE="$PWD/.git/autosave-index"
while true; do
  git add -A >/dev/null 2>&1
  tree=$(git write-tree 2>/dev/null) || { sleep 1200; continue; }
  parent=$(git rev-parse -q --verify refs/wip/autosave || git rev-parse -q --verify HEAD)
  if [ "$(git rev-parse -q --verify "$parent^{tree}")" != "$tree" ]; then
    c=$(git -c user.name=autosave -c user.email=autosave@localhost commit-tree "$tree" -p "$parent" -m "autosave $(date -Is)")
    git update-ref refs/wip/autosave "$c"
  fi
  sleep 1200
done
