#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace_dir"

python_command=""
for candidate in python3 python; do
  if command -v "$candidate" >/dev/null 2>&1 \
    && "$candidate" -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 11) else 1)' >/dev/null 2>&1; then
    python_command="$candidate"
    break
  fi
done
if [[ -n "$python_command" ]]; then
  exec "$python_command" scripts/gates.py "$@"
fi

if command -v uv >/dev/null 2>&1 \
  && uv run --python 3.14.7 --no-project python -c 'import sys; raise SystemExit(0 if sys.version_info[:3] == (3, 14, 7) else 1)' >/dev/null 2>&1; then
  exec uv run --python 3.14.7 --no-project python scripts/gates.py "$@"
fi

printf '%s\n' \
  'Python 3.11+ is required; install Python or pinned uv Python 3.14.7 for the compatibility gates.' >&2
exit 1
