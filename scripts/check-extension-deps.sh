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
  tree=$(cargo tree -p "$c" --all-features -e normal,build,dev --prefix none --locked)
  first=${tree%%$'\n'*}
  case "$first" in
    "$c v"*) ;;
    *)
      echo "::error::cargo tree for $c printed no tree"
      exit 1
      ;;
  esac
  if bad=$(printf '%s\n' "$tree" | grep -E '^(gecko-engine|typedb-driver) ' | sort -u); then
    while IFS= read -r line; do
      echo "::error::$c depends on $line"
    done <<EOF
$bad
EOF
    failed=1
  fi
done

if [ "$failed" -ne 0 ]; then
  exit 1
fi

echo "OK: $(printf '%s' "$crates" | tr '\n' ' ' | sed 's/ $//') depend on neither gecko-engine nor typedb-driver."
