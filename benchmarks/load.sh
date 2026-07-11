#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
# SPDX-License-Identifier: AGPL-3.0-or-later

# load.sh - concurrent load-testing harness for `sct serve`
#
# Unlike bench.sh (single-request, one operation at a time), this drives
# `sct serve` under sustained concurrency and reports how it behaves as load
# climbs: throughput, tail latency, and where it saturates. It is a
# single-server test - no comparator - so the results are freely publishable.
#
# For each operation and each concurrency level it runs a fixed-duration load
# (keep-alive, so per-request client overhead is amortised) and records
# requests/sec, latency percentiles (p50/p95/p99/p99.9) and the error rate.
#
# Usage:
#   benchmarks/load.sh --url <sct-fhir-base> [OPTIONS]
#
# Options:
#   --url URL            sct serve FHIR base, e.g. http://localhost:8080/fhir
#                        (required)
#   --concurrencies LIST comma-separated concurrency levels
#                        (default: 1,2,4,8,16,32,64,128)
#   --duration DUR       load duration per level, oha syntax (default: 10s)
#   --warmup DUR         untimed warmup before each operation (default: 3s)
#   --operations LIST    comma-separated subset of:
#                          lookup,validate,subsumes,search,children,expand
#                        (default: lookup,validate,subsumes,search,expand)
#   --tool TOOL          load generator: oha | bombardier | auto (default: auto)
#   --stat-container NAME sample `docker stats` for NAME mid-run to record the
#                        server's memory/CPU under load (host runs only; needs
#                        access to the docker CLI)
#   --timeout SECS       per-request timeout (default: 30)
#   --write-report       write a timestamped markdown report to
#                        benchmarks/reports/
#   --help               show this message
#
# Recommended load generator: oha (https://github.com/hatoo/oha) - a single
# static binary with keep-alive and JSON output. Install with `cargo install
# oha`, or download a release binary. bombardier is supported as a fallback.

# FHIR $-operation names ($lookup, $expand, $subsumes, $validate-code) appear as
# literal text in single-quoted format strings and labels throughout; no
# in-single-quote expansion is ever intended here.
# shellcheck disable=SC2016
set -uo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ── defaults ──────────────────────────────────────────────────────────────────
LOAD_URL=""
LOAD_CONCURRENCIES="1,2,4,8,16,32,64,128"
LOAD_DURATION="10s"
LOAD_WARMUP="3s"
LOAD_OPERATIONS="lookup,validate,subsumes,search,expand"
LOAD_TOOL="auto"
LOAD_STAT_CONTAINER=""
LOAD_TIMEOUT=30
LOAD_WRITE_REPORT=false

# ── argument parsing ──────────────────────────────────────────────────────────
_die() { printf 'error: %s\n' "$*" >&2; exit 1; }

_show_help() {
  sed -n '/^# Usage:/,/^[^#]/{ /^#/{ s/^# \?//; p } }' "${BASH_SOURCE[0]}"
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --url)             LOAD_URL="$2"; shift 2 ;;
    --concurrencies)   LOAD_CONCURRENCIES="$2"; shift 2 ;;
    --duration)        LOAD_DURATION="$2"; shift 2 ;;
    --warmup)          LOAD_WARMUP="$2"; shift 2 ;;
    --operations)      LOAD_OPERATIONS="$2"; shift 2 ;;
    --tool)            LOAD_TOOL="$2"; shift 2 ;;
    --stat-container)  LOAD_STAT_CONTAINER="$2"; shift 2 ;;
    --timeout)         LOAD_TIMEOUT="$2"; shift 2 ;;
    --write-report)    LOAD_WRITE_REPORT=true; shift ;;
    --help|-h)         _show_help ;;
    *)                 _die "unknown option: $1" ;;
  esac
done

[[ -n "$LOAD_URL" ]] || _die "--url <sct-fhir-base> is required (e.g. http://localhost:8080/fhir)"
LOAD_URL="${LOAD_URL%/}"

# ── tool selection ────────────────────────────────────────────────────────────
if [[ "$LOAD_TOOL" == "auto" ]]; then
  if command -v oha >/dev/null 2>&1; then
    LOAD_TOOL="oha"
  elif command -v bombardier >/dev/null 2>&1; then
    LOAD_TOOL="bombardier"
  else
    _die $'no load generator found. Install one of:\n  oha:        cargo install oha   (or a release binary from https://github.com/hatoo/oha)\n  bombardier: go install github.com/codesenberg/bombardier@latest'
  fi
fi
command -v "$LOAD_TOOL" >/dev/null 2>&1 || _die "load tool not found on PATH: $LOAD_TOOL"
command -v jq  >/dev/null 2>&1 || _die "missing required tool: jq"
command -v awk >/dev/null 2>&1 || _die "missing required tool: awk"

# ── URL encoding + operation → URL map ────────────────────────────────────────
_urlencode() {
  local s="$1" out='' i c
  for (( i=0; i<${#s}; i++ )); do
    c="${s:$i:1}"
    case "$c" in
      [a-zA-Z0-9._~-]) out+="$c" ;;
      ' ')             out+='%20' ;;
      *)               out+=$(printf '%%%02X' "'$c") ;;
    esac
  done
  printf '%s' "$out"
}

# _op_url OP - prints the full GET URL for operation OP (empty if unknown).
# The FHIR $-operations carry a literal `$`; the single-quoted format strings
# keep it literal (no shell interpolation).
_op_url() {
  local op="$1" vs
  case "$op" in
    lookup)
      printf '%s/CodeSystem/$lookup?system=http://snomed.info/sct&code=22298006&property=display&property=designation' "$LOAD_URL" ;;
    validate)
      printf '%s/CodeSystem/$validate-code?system=http://snomed.info/sct&code=22298006' "$LOAD_URL" ;;
    subsumes)
      printf '%s/CodeSystem/$subsumes?system=http://snomed.info/sct&codeA=46635009&codeB=73211009' "$LOAD_URL" ;;
    search)
      printf '%s/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs&filter=heart%%20attack&count=20' "$LOAD_URL" ;;
    children)
      vs=$(_urlencode 'http://snomed.info/sct?fhir_vs=ecl/<!73211009')
      printf '%s/ValueSet/$expand?url=%s&count=1000' "$LOAD_URL" "$vs" ;;
    expand)
      vs=$(_urlencode 'http://snomed.info/sct?fhir_vs=ecl/<<73211009')
      printf '%s/ValueSet/$expand?url=%s&count=100' "$LOAD_URL" "$vs" ;;
    *) return 1 ;;
  esac
}

_op_label() {
  case "$1" in
    lookup)   echo 'CodeSystem/$lookup' ;;
    validate) echo 'CodeSystem/$validate-code' ;;
    subsumes) echo 'CodeSystem/$subsumes' ;;
    search)   echo 'ValueSet/$expand (text filter)' ;;
    children) echo 'ValueSet/$expand (ECL <! direct children)' ;;
    expand)   echo 'ValueSet/$expand (ECL << subtree)' ;;
    *)        echo "$1" ;;
  esac
}

# ── one load run → tab-separated "rps p50 p95 p99 p99_9 errpct" (ms) ──────────
# Runs the chosen tool for one (url, concurrency) and normalises the metrics.
_run_oha() {
  local url="$1" conc="$2" json
  json=$(oha -z "$LOAD_DURATION" -c "$conc" --no-tui --output-format json \
    -H "Accept: application/fhir+json" \
    -t "${LOAD_TIMEOUT}s" \
    "$url" 2>/dev/null) || { echo "ERR ERR ERR ERR ERR ERR"; return; }
  # oha reports latencies in seconds; ×1000 → ms. Success rate 0..1.
  printf '%s' "$json" | jq -r '
    (.summary.requestsPerSec // 0) as $rps
    | (.summary.successRate // 1) as $sr
    | (.latencyPercentiles // {}) as $p
    | [ $rps,
        (($p.p50   // 0) * 1000),
        (($p.p95   // 0) * 1000),
        (($p.p99   // 0) * 1000),
        (($p."p99.9" // ($p.p99 // 0)) * 1000),
        ((1 - $sr) * 100)
      ] | @tsv'
}

_run_bombardier() {
  local url="$1" conc="$2" json
  json=$(bombardier -c "$conc" -d "$LOAD_DURATION" -l -p r -o json \
    -H "Accept: application/fhir+json" \
    --timeout "${LOAD_TIMEOUT}s" \
    "$url" 2>/dev/null) || { echo "ERR ERR ERR ERR ERR ERR"; return; }
  # bombardier: latencies in microseconds; ÷1000 → ms. req2xx/req etc.
  printf '%s' "$json" | jq -r '
    .result as $r
    | ($r.rps.mean // 0) as $rps
    | ($r.latencyPercentiles // {}) as $p
    | (($r.req1xx+$r.req2xx+$r.req3xx+$r.req4xx+$r.req5xx+$r.others) // 0) as $tot
    | (($r.req4xx + $r.req5xx + $r.others) // 0) as $bad
    | [ $rps,
        (($p.p50 // 0) / 1000),
        (($p.p95 // 0) / 1000),
        (($p.p99 // 0) / 1000),
        (($p."p99.9" // ($p.p99 // 0)) / 1000),
        (if $tot > 0 then ($bad / $tot * 100) else 0 end)
      ] | @tsv'
}

_run_load() {
  case "$LOAD_TOOL" in
    oha)        _run_oha "$@" ;;
    bombardier) _run_bombardier "$@" ;;
  esac
}

# ── docker stats sampling (optional) ──────────────────────────────────────────
# Prints "MEM_MiB CPU_PCT" for the configured container, or empty if disabled /
# unavailable. Best-effort - a container run of load.sh usually can't see the
# host docker daemon, so this is for host-side runs.
_sample_stats() {
  [[ -n "$LOAD_STAT_CONTAINER" ]] || return 0
  command -v docker >/dev/null 2>&1 || return 0
  docker stats --no-stream --format '{{.MemUsage}}\t{{.CPUPerc}}' "$LOAD_STAT_CONTAINER" 2>/dev/null \
    | awk -F'\t' '{ split($1, m, " "); gsub(/MiB|MB|GiB|%/,"",m[1]);
                    mem=m[1]; if ($1 ~ /GiB|GB/) mem=mem*1024;
                    cpu=$2; gsub(/%/,"",cpu); printf "%.1f %s", mem, cpu }'
}

# ── main ──────────────────────────────────────────────────────────────────────
LOAD_DATE=$(date +%Y-%m-%d)
LOAD_DATETIME=$(date +%Y-%m-%dT%H%M%S)

printf 'sct load test - %s\n' "$LOAD_DATE" >&2
printf 'target : %s\n' "$LOAD_URL" >&2
printf 'tool   : %s | duration/level: %s | warmup: %s\n' \
  "$LOAD_TOOL" "$LOAD_DURATION" "$LOAD_WARMUP" >&2

# Connectivity check.
if ! curl -sf --max-time "$LOAD_TIMEOUT" -H 'Accept: application/fhir+json' \
     "${LOAD_URL}/metadata" >/dev/null 2>&1; then
  _die "sct FHIR endpoint unreachable: ${LOAD_URL}/metadata"
fi
printf 'endpoint ok\n\n' >&2

IFS=',' read -ra CONCS <<< "$LOAD_CONCURRENCIES"
IFS=',' read -ra OPS   <<< "$LOAD_OPERATIONS"

TMPDIR_LOAD=$(mktemp -d /tmp/load_XXXXXX)
RESULTS_TSV="${TMPDIR_LOAD}/results.tsv"   # op conc rps p50 p95 p99 p999 errpct mem cpu
: > "$RESULTS_TSV"
PEAK_MEM=""

for op in "${OPS[@]}"; do
  op="${op// /}"
  url=$(_op_url "$op") || { printf 'warning: unknown operation "%s" - skipped\n' "$op" >&2; continue; }

  printf '━━ %s  [%s]\n' "$op" "$(_op_label "$op")" >&2
  printf '  %6s │ %12s │ %8s │ %8s │ %8s │ %8s │ %7s\n' \
    conc 'req/s' 'p50 ms' 'p95 ms' 'p99 ms' 'p99.9' 'err %' >&2
  printf '  ───────┼──────────────┼──────────┼──────────┼──────────┼──────────┼────────\n' >&2

  # One short warmup at low concurrency (untimed) so the first level is fair.
  oha -z "$LOAD_WARMUP" -c 4 --no-tui --output-format json -H "Accept: application/fhir+json" "$url" >/dev/null 2>&1 || true

  for conc in "${CONCS[@]}"; do
    conc="${conc// /}"
    # Sample container memory partway through the run, if requested.
    stat_line=""
    if [[ -n "$LOAD_STAT_CONTAINER" ]]; then
      ( sleep 2; _sample_stats > "${TMPDIR_LOAD}/stat.$conc" ) &
    fi
    read -r rps p50 p95 p99 p999 errpct < <(_run_load "$url" "$conc")
    if [[ -n "$LOAD_STAT_CONTAINER" ]]; then
      wait 2>/dev/null || true
      stat_line=$(cat "${TMPDIR_LOAD}/stat.$conc" 2>/dev/null || true)
    fi
    mem="${stat_line%% *}"; cpu="${stat_line#* }"; [[ "$mem" == "$cpu" ]] && cpu=""
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$op" "$conc" "$rps" "$p50" "$p95" "$p99" "$p999" "$errpct" "${mem:-}" "${cpu:-}" \
      >> "$RESULTS_TSV"
    # Live row.
    if [[ "$rps" == "ERR" ]]; then
      printf '  %6s │ %12s │ %8s │ %8s │ %8s │ %8s │ %7s\n' "$conc" ERR - - - - - >&2
    else
      # Round req/s to an integer with awk (portable to mawk, unlike printf %'d).
      printf '  %6s │ %12s │ %8.2f │ %8.2f │ %8.2f │ %8.2f │ %6.2f%%\n' \
        "$conc" \
        "$(awk -v x="$rps" 'BEGIN{printf "%.0f", x+0}')" \
        "$p50" "$p95" "$p99" "$p999" "$errpct" >&2
    fi
    [[ -n "$mem" ]] && { PEAK_MEM=$(awk -v a="${PEAK_MEM:-0}" -v b="$mem" 'BEGIN{print (b>a)?b:a}'); }
  done

  # Saturation knee: the concurrency at which req/s peaks for this op.
  knee=$(awk -F'\t' -v op="$op" '$1==op && $3!="ERR" { if ($3+0 > best+0){best=$3; bc=$2} } END{ printf "%.0f req/s @ %d clients", best, bc }' "$RESULTS_TSV")
  printf '  peak: %s\n\n' "$knee" >&2
done

# ── optional markdown report ──────────────────────────────────────────────────
if $LOAD_WRITE_REPORT; then
  mkdir -p "${BENCH_DIR}/reports"
  slug=$(printf '%s' "$LOAD_URL" | sed 's|^[a-z]*://||' | tr -cs 'a-zA-Z0-9' '-' | sed 's/-$//')
  outfile="${BENCH_DIR}/reports/${LOAD_DATETIME}-loadtest-${slug}.md"
  {
    printf '# sct serve load test - %s\n\n' "$LOAD_DATE"
    printf -- '- target: `%s`\n- tool: `%s`, %s per level, warmup %s\n' \
      "$LOAD_URL" "$LOAD_TOOL" "$LOAD_DURATION" "$LOAD_WARMUP"
    [[ -n "$PEAK_MEM" ]] && printf -- '- server peak memory under load: **%.0f MiB** (`%s`)\n' "$PEAK_MEM" "$LOAD_STAT_CONTAINER"
    printf '\n'
    for op in "${OPS[@]}"; do
      op="${op// /}"; _op_url "$op" >/dev/null 2>&1 || continue
      printf '## %s\n\n' "$(_op_label "$op")"
      printf '| concurrency | req/s | p50 ms | p95 ms | p99 ms | p99.9 ms | err %% |%s\n' \
        "$([[ -n "$LOAD_STAT_CONTAINER" ]] && printf ' mem MiB |')"
      printf '|---|---|---|---|---|---|---|%s\n' \
        "$([[ -n "$LOAD_STAT_CONTAINER" ]] && printf '---|')"
      awk -F'\t' -v op="$op" -v sc="$LOAD_STAT_CONTAINER" '
        $1==op {
          printf "| %s | %.0f | %.2f | %.2f | %.2f | %.2f | %.2f |", $2,$3,$4,$5,$6,$7,$8
          if (sc != "") printf " %s |", ($9==""?"-":$9)
          printf "\n"
        }' "$RESULTS_TSV"
      printf '\n'
    done
  } > "$outfile"
  printf 'wrote report to %s\n' "$outfile" >&2
fi

rm -rf "$TMPDIR_LOAD"
