#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

exec python3 .github/scripts/check-config-compat.py "$@"
