#!/usr/bin/env bash
#
# GECKO bootstrap — a THIN front door.
#
# Contract (see packaging plan P7.3): this script only
#   (a) checks prerequisites gecko cannot provide for itself, and
#   (b) sequences already-tested subcommands (gecko / docker / cargo).
# It implements NO orchestration of its own — no readiness polling, no checksum
# verification, no TypeDB install. Those live in `gecko` (tested). It is idempotent:
# safe to re-run; it asks `gecko doctor` what is already done and only fills the gaps.
#
# Usage:
#   ./scripts/setup.sh [--typedb-mode native|docker|external]
#                      [--yes] [--no-model] [--force]
#
#   --typedb-mode   choose TypeDB backend non-interactively (default: prompt → native)
#   --yes           assume "yes" to prompts (non-interactive)
#   --no-model      skip the ~1.3GB model fetch (do it later with `gecko model fetch`)
#   --force         run steps even if `gecko doctor` reports healthy

set -euo pipefail

# ------------------------------------------------------------------ styling
if [[ -t 1 ]]; then
  BOLD=$'\033[1m'; DIM=$'\033[2m'; RED=$'\033[31m'; GRN=$'\033[32m'
  YLW=$'\033[33m'; BLU=$'\033[34m'; RST=$'\033[0m'
else
  BOLD=''; DIM=''; RED=''; GRN=''; YLW=''; BLU=''; RST=''
fi
say()  { printf '%s\n' "${BLU}▸${RST} $*"; }
ok()   { printf '%s\n' "${GRN}✓${RST} $*"; }
warn() { printf '%s\n' "${YLW}⚠${RST} $*"; }
die()  { printf '%s\n' "${RED}✗ $*${RST}" >&2; exit 1; }

# ------------------------------------------------------------------ args
TYPEDB_MODE=""; ASSUME_YES=0; NO_MODEL=0; FORCE=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --typedb-mode) TYPEDB_MODE="${2:-}"; shift 2 ;;
    --yes|-y)      ASSUME_YES=1; shift ;;
    --no-model)    NO_MODEL=1; shift ;;
    --force)       FORCE=1; shift ;;
    -h|--help)     grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)             die "unknown flag: $1" ;;
  esac
done

confirm() { # confirm "question"  -> 0 yes / 1 no
  [[ $ASSUME_YES -eq 1 ]] && return 0
  [[ -t 0 ]] || return 0            # non-interactive w/o --yes: default yes, caller gates elsewhere
  local reply; read -r -p "${BOLD}$1${RST} [Y/n] " reply || true
  [[ -z "$reply" || "$reply" =~ ^[Yy]$ ]]
}

cd "$(dirname "$0")/.."             # repo root
say "GECKO setup — repo root: ${DIM}$(pwd)${RST}"

# ------------------------------------------------------------------ resolve `gecko`
# Prefer an installed binary; otherwise run from source via cargo (repo is self-sufficient).
if command -v gecko >/dev/null 2>&1; then
  GECKO_CMD=(command gecko)         # `command` bypasses any same-named function/alias
  ok "using installed gecko: $(command -v gecko)"
else
  command -v cargo >/dev/null 2>&1 || die "neither 'gecko' nor 'cargo' found — install Rust (https://rustup.rs) or a gecko release binary"
  say "no installed gecko — building from source (stub build; the model is not needed to build)…"
  cargo build --no-default-features --bin gecko
  GECKO_CMD=(cargo run --quiet --no-default-features --bin gecko --)
  ok "gecko built from source"
fi
# helper (distinct name — must NOT be 'gecko', or it would shadow the binary and recurse)
run_gecko() { "${GECKO_CMD[@]}" "$@"; }

# ------------------------------------------------------------------ config bootstrap (gecko owns it)
if [[ ! -f gecko.toml ]]; then
  say "no gecko.toml — writing defaults via 'gecko init'"
  run_gecko init
  ok "gecko.toml created"
fi

# ------------------------------------------------------------------ idempotency: ask doctor first
if [[ $FORCE -eq 0 ]] && run_gecko doctor --quiet >/dev/null 2>&1; then
  ok "'gecko doctor' reports healthy — nothing to do. (re-run with --force to redo setup)"
  run_gecko doctor || true
  exit 0
fi

# ------------------------------------------------------------------ choose TypeDB mode (ASK, don't guess)
if [[ -z "$TYPEDB_MODE" ]]; then
  HAVE_DOCKER=0; command -v docker >/dev/null 2>&1 && HAVE_DOCKER=1
  if [[ $ASSUME_YES -eq 1 || ! -t 0 ]]; then
    TYPEDB_MODE="native"   # non-interactive default = the recommended, self-managed path
  else
    echo
    echo "  ${BOLD}How should TypeDB run?${RST}"
    echo "    ${BOLD}1) native${RST}   gecko-managed child process (recommended)"
    echo "    ${BOLD}2) docker${RST}   docker compose $([[ $HAVE_DOCKER -eq 0 ]] && echo "${DIM}(docker not detected)${RST}")"
    echo "    ${BOLD}3) external${RST} you run TypeDB yourself; gecko.toml [typedb] endpoint points at it"
    read -r -p "  choose [1]: " choice || true
    case "${choice:-1}" in
      1|"") TYPEDB_MODE="native" ;;
      2)    TYPEDB_MODE="docker" ;;
      3)    TYPEDB_MODE="external" ;;
      *)    die "invalid choice: $choice" ;;
    esac
  fi
fi
ok "TypeDB mode: ${BOLD}${TYPEDB_MODE}${RST}"

# ------------------------------------------------------------------ model (explicit; never a silent 1.3GB pull)
if [[ $NO_MODEL -eq 1 ]]; then
  warn "skipping model fetch (--no-model). Run 'gecko model fetch' before using semantic retrieval."
else
  if run_gecko doctor --check model --quiet >/dev/null 2>&1; then
    ok "embedding model already present in cache"
  else
    if confirm "Fetch the embedding model now (~1.3GB, one-time)?"; then
      say "fetching model via 'gecko model fetch' (gecko handles caching + checksum)…"
      run_gecko model fetch
      ok "model staged"
    else
      warn "model not fetched — run 'gecko model fetch' later (semantic retrieval needs it)"
    fi
  fi
fi

# ------------------------------------------------------------------ start TypeDB per chosen mode (delegate)
case "$TYPEDB_MODE" in
  native)
    say "starting orchestrated TypeDB via 'gecko up'…"
    run_gecko up
    ;;
  docker)
    command -v docker >/dev/null 2>&1 || die "docker mode chosen but docker is not installed"
    [[ -f docker-compose.yml ]] || die "docker-compose.yml not found in repo root"
    say "starting TypeDB via 'docker compose up -d'…"
    docker compose up -d
    ;;
  external)
    warn "external mode — ensure your TypeDB is running and gecko.toml [typedb] endpoint is correct"
    ;;
  *) die "unknown TypeDB mode: $TYPEDB_MODE" ;;
esac

# ------------------------------------------------------------------ final health report
echo
say "final health check:"
if run_gecko doctor; then
  echo
  ok "${BOLD}setup complete.${RST}  Next:  ${DIM}gecko sync sample-bundle${RST}   (or: just sync)"
else
  echo
  die "setup finished but 'gecko doctor' reports problems above — address the ✗ items and re-run."
fi
