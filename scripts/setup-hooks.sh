#!/bin/sh
# Point git at the repo's tracked hooks (.githooks/).
# Run once after cloning: `sh scripts/setup-hooks.sh`.
set -e
repo_root="$(git rev-parse --show-toplevel)"
git -C "$repo_root" config core.hooksPath .githooks
echo "✓ core.hooksPath set to .githooks (pre-commit / pre-push run cargo fmt --all -- --check)"
