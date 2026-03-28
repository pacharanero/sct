#!/usr/bin/env bash
# bench.sh — sct benchmark suite entry point
#
# Compares sct (local SQLite) against a FHIR R4 terminology server across
# six operations and renders a fair, like-for-like timing report.
#
# Usage:
#   bench/bench.sh [OPTIONS]
#
# Options:
#   --server URL        FHIR base URL  e.g. https://terminology.openehr.org/fhir
#   --db PATH           snomed.db path (default: ./snomed.db)
#   --runs N            timed iterations per operation (default: 5)
#   --warmup N          warmup iterations before timing (default: 1)
#   --operations LIST   comma-separated subset: lookup,search,children,
#                       ancestors,subsumption,bulk  (default: all)
#   --format FORMAT     table (default) | json | csv
#   --no-remote         skip FHIR calls entirely
#   --timeout SECS      per-request curl timeout (default: 30)
#   --output FILE       write report to FILE in addition to stdout
#   --write-benchmarks  write results to benchmarks.md in the current directory
#   --help              show this message

set -uo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ── defaults ──────────────────────────────────────────────────────────────────
BENCH_DB="./snomed.db"
BENCH_SERVER=""
BENCH_RUNS=5
BENCH_WARMUP=1
BENCH_TIMEOUT=30
BENCH_FORMAT="table"
BENCH_OPERATIONS="lookup,search,children,ancestors,subsumption,bulk"
BENCH_WRITE_BENCHMARKS=false
BENCH_OUTPUT_FILE=""

# ── argument parsing ──────────────────────────────────────────────────────────
_die() { printf 'error: %s\n' "$*" >&2; exit 1; }

_show_help() {
  sed -n '/^# Usage:/,/^[^#]/{ /^#/{ s/^# \?//; p } }' "${BASH_SOURCE[0]}"
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --server)            BENCH_SERVER="$2"; shift 2 ;;
    --db)                BENCH_DB="$2"; shift 2 ;;
    --runs)              BENCH_RUNS="$2"; shift 2 ;;
    --warmup)            BENCH_WARMUP="$2"; shift 2 ;;
    --operations)        BENCH_OPERATIONS="$2"; shift 2 ;;
    --format)            BENCH_FORMAT="$2"; shift 2 ;;
    --no-remote)         BENCH_SERVER=""; shift ;;
    --timeout)           BENCH_TIMEOUT="$2"; shift 2 ;;
    --output)            BENCH_OUTPUT_FILE="$2"; shift 2 ;;
    --write-benchmarks)  BENCH_WRITE_BENCHMARKS=true; shift ;;
    --help|-h)           _show_help ;;
    *)                   _die "unknown option: $1" ;;
  esac
done

# ── dependency check ──────────────────────────────────────────────────────────
_check_deps() {
  local missing=()
  for cmd in sqlite3 jq awk; do
    command -v "$cmd" >/dev/null 2>&1 || missing+=("$cmd")
  done
  if [[ -n "$BENCH_SERVER" ]]; then
    command -v curl >/dev/null 2>&1 || missing+=("curl")
  fi
  if (( ${#missing[@]} > 0 )); then
    _die "missing required tools: ${missing[*]}"
  fi
  if ! command -v hyperfine >/dev/null 2>&1; then
    printf 'note: hyperfine not found — using bash manual timing (less accurate).\n' >&2
    printf '      install: cargo install hyperfine\n\n' >&2
  fi
}

# ── validate DB ───────────────────────────────────────────────────────────────
_check_db() {
  [[ -f "$BENCH_DB" ]] || _die "database not found: $BENCH_DB (run sct sqlite first)"
  sqlite3 "$BENCH_DB" "SELECT COUNT(*) FROM concepts LIMIT 1" >/dev/null 2>&1 \
    || _die "cannot query database: $BENCH_DB"
}

# ── source library files ──────────────────────────────────────────────────────
# shellcheck source=lib/timing.sh
source "${BENCH_DIR}/lib/timing.sh"
# shellcheck source=lib/local.sh
source "${BENCH_DIR}/lib/local.sh"
# shellcheck source=lib/fhir.sh
source "${BENCH_DIR}/lib/fhir.sh"
# shellcheck source=lib/report.sh
source "${BENCH_DIR}/lib/report.sh"

# ── shared result accumulator ─────────────────────────────────────────────────
# Columns (tab-separated): op | label | local_ms | local_sd | remote_ms | remote_sd | notes
BENCH_TMPDIR=$(mktemp -d /tmp/bench_XXXXXX)
BENCH_RESULTS_TSV="${BENCH_TMPDIR}/results.tsv"
BENCH_DATE=$(date +%Y-%m-%d)

# Called by each operations/*.sh to write one result row.
append_result() {
  local op="$1" label="$2" lms="$3" lsd="$4" rms="$5" rsd="$6" notes="${7:-}"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$op" "$label" "$lms" "$lsd" "$rms" "$rsd" "$notes" \
    >> "$BENCH_RESULTS_TSV"
}

# ── main ──────────────────────────────────────────────────────────────────────
_check_deps
_check_db

printf 'sct benchmark — %s\n' "$BENCH_DATE" >&2
printf 'db: %s\n' "$(realpath "$BENCH_DB" 2>/dev/null || printf '%s' "$BENCH_DB")" >&2

# Resolve DB to absolute path for consistent display in report.
BENCH_DB="$(realpath "$BENCH_DB" 2>/dev/null || printf '%s' "$BENCH_DB")"

# Collect SNOMED metadata from the DB.
local_snomed_info
printf 'snomed version: %s (%s active concepts)\n' \
  "${SNOMED_VERSION:-?}" "${SNOMED_CONCEPT_COUNT:-?}" >&2

# Check remote server connectivity.
FHIR_PING_MS=0
if [[ -n "$BENCH_SERVER" ]]; then
  printf 'checking remote: %s ...\n' "$BENCH_SERVER" >&2
  if check_fhir_server; then
    printf 'remote ok (ping: %s ms)\n' "$FHIR_PING_MS" >&2
  else
    printf 'warning: remote server unreachable — running local-only benchmark.\n' >&2
    BENCH_SERVER=""
  fi
fi

printf 'runs: %s (warmup: %s) | timing: %s\n\n' \
  "$BENCH_RUNS" "$BENCH_WARMUP" "$(timing_tool_name)" >&2

# Run requested operations.
IFS=',' read -ra OPS <<< "$BENCH_OPERATIONS"
for op in "${OPS[@]}"; do
  op="${op// /}"  # trim whitespace
  opfile="${BENCH_DIR}/operations/${op}.sh"
  if [[ ! -f "$opfile" ]]; then
    printf 'warning: unknown operation "%s" — skipped.\n' "$op" >&2
    continue
  fi
  # shellcheck source=/dev/null
  source "$opfile"
  "run_${op}"
done

# Render report.
case "$BENCH_FORMAT" in
  table) render_table "$BENCH_RESULTS_TSV" ;;
  json)  render_json  "$BENCH_RESULTS_TSV" ;;
  csv)   render_csv   "$BENCH_RESULTS_TSV" ;;
  *)     _die "unknown format: $BENCH_FORMAT (use table, json, or csv)" ;;
esac

# Write to --output FILE if requested.
if [[ -n "$BENCH_OUTPUT_FILE" ]]; then
  case "$BENCH_FORMAT" in
    table) render_table "$BENCH_RESULTS_TSV" > "$BENCH_OUTPUT_FILE" ;;
    json)  render_json  "$BENCH_RESULTS_TSV" > "$BENCH_OUTPUT_FILE" ;;
    csv)   render_csv   "$BENCH_RESULTS_TSV" > "$BENCH_OUTPUT_FILE" ;;
  esac
  printf '\nwrote report to %s\n' "$BENCH_OUTPUT_FILE" >&2
fi

# Write benchmarks.md if requested.
if $BENCH_WRITE_BENCHMARKS; then
  render_markdown "$BENCH_RESULTS_TSV" "./benchmarks.md"
fi

# Clean up temp files.
rm -rf "$BENCH_TMPDIR"
