#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEPLOY_ROOT="${CODEX_SPUR_DEPLOY_ROOT:-$HOME/Sites/codex-spur}"
SHA="${GIT_SHA:-$(git -C "$ROOT_DIR" rev-parse --short HEAD)}"
RELEASE_DIR="$DEPLOY_ROOT/releases/$SHA"
CURRENT_LINK="$DEPLOY_ROOT/current"

npm --prefix "$ROOT_DIR" run site:build
mkdir -p "$DEPLOY_ROOT/releases"
python3 - "$RELEASE_DIR" <<'PY'
from pathlib import Path
import shutil, sys
path = Path(sys.argv[1])
if path.exists():
    shutil.rmtree(path)
PY
cp -R "$ROOT_DIR/website/dist" "$RELEASE_DIR"
ln -sfn "$RELEASE_DIR" "$CURRENT_LINK"

# Keep current plus two previous releases. BSD utilities on macOS lack sort -z,
# so pruning is handled with a small, deterministic Python block.
python3 - "$DEPLOY_ROOT/releases" "$RELEASE_DIR" <<'PY'
from pathlib import Path
import shutil, sys
root = Path(sys.argv[1])
current = Path(sys.argv[2]).resolve()
releases = sorted((p for p in root.iterdir() if p.is_dir()), key=lambda p: p.stat().st_mtime, reverse=True)
keep = {current, *(p.resolve() for p in releases[:3])}
for path in releases:
    if path.resolve() not in keep:
        shutil.rmtree(path)
PY
printf 'Deployed %s to %s\n' "$SHA" "$CURRENT_LINK"
