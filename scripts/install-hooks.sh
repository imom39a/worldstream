#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
git -C "$workspace_dir" config core.hooksPath .githooks
printf '%s\n' 'Configured core.hooksPath=.githooks for this checkout.'
