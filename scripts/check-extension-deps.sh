#!/usr/bin/env bash
# Fails unless every workspace crate under extensions/ is free of gecko-engine
# and typedb-driver in its normal, build and dev dependency trees, so extensions
# reach the host only through gecko-extension-api.
set -euo pipefail

meta=$(cargo metadata --no-deps --format-version 1 --locked)
root=$(printf '%s' "$meta" | jq -r .workspace_root)
crates=$(printf '%s' "$meta" | jq -r --arg dir "$root/extensions/" \
  '.packages[] | select(.manifest_path | startswith($dir)) | .name')

if [ -z "$crates" ]; then
  echo "::error::no workspace crate found under extensions/"
  exit 1
fi

failed=0
for c in $crates; do
  tree=$(cargo tree -p "$c" --all-features --target all -e normal,build,dev --prefix none --locked)
  first=${tree%%$'\n'*}
  case "$first" in
    "$c v"*) ;;
    *)
      echo "::error::cargo tree for $c printed no tree"
      exit 1
      ;;
  esac
  # grep exits 0 on a match, 1 on none, and 2 or higher on its own failure.
  rc=0
  bad=$(printf '%s\n' "$tree" | grep -E '^(gecko-engine|typedb-driver) ') || rc=$?
  case "$rc" in
    0)
      while IFS= read -r line; do
        echo "::error::$c depends on $line"
      done <<EOF
$(printf '%s\n' "$bad" | sort -u)
EOF
      failed=1
      ;;
    1) ;;
    *)
      echo "::error::grep failed with status $rc while checking $c"
      exit 1
      ;;
  esac
done

if [ "$failed" -ne 0 ]; then
  exit 1
fi

echo "OK: $(printf '%s' "$crates" | tr '\n' ' ' | sed 's/ $//') depend on neither gecko-engine nor typedb-driver."
