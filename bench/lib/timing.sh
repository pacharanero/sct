#!/usr/bin/env bash
# lib/timing.sh — timing primitives
#
# Usage: time_cmd RUNS WARMUP CMD [ARGS...]
# Outputs median wall-clock milliseconds to stdout.
# Sets globals: TIMING_MEDIAN  TIMING_STDDEV
#
# Uses hyperfine when available for statistical rigour (median, stddev over N
# runs). Falls back to manual bash timing using date +%s%N (Linux only).

TIMING_MEDIAN=0
TIMING_STDDEV=0

time_cmd() {
  local runs="$1" warmup="$2"; shift 2
  if command -v hyperfine >/dev/null 2>&1; then
    _time_cmd_hyperfine "$runs" "$warmup" "$@"
  else
    _time_cmd_manual "$runs" "$warmup" "$@"
  fi
}

_time_cmd_hyperfine() {
  local runs="$1" warmup="$2"; shift 2
  local tmpjson
  tmpjson=$(mktemp /tmp/bench_XXXXXX.json)

  # Build a bash-safe command string. printf %q produces bash-compatible quoting.
  local cmd_str
  cmd_str=$(printf '%q ' "$@")

  hyperfine \
    --runs "$runs" \
    --warmup "$warmup" \
    --export-json "$tmpjson" \
    --shell bash \
    "$cmd_str" >/dev/null 2>&1

  TIMING_MEDIAN=$(jq -r '(.results[0].median * 1000 + 0.5) | floor' "$tmpjson")
  TIMING_STDDEV=$(jq -r '(.results[0].stddev * 1000 + 0.5) | floor' "$tmpjson")
  rm -f "$tmpjson"
  echo "$TIMING_MEDIAN"
}

_time_cmd_manual() {
  local runs="$1" warmup="$2"; shift 2
  local total=$(( warmup + runs ))
  local -a all_times=()
  local i t0 t1

  for (( i=0; i<total; i++ )); do
    t0=$(date +%s%N)
    "$@" >/dev/null 2>&1
    t1=$(date +%s%N)
    all_times+=( $(( (t1 - t0) / 1000000 )) )
  done

  local -a times=("${all_times[@]:$warmup}")
  local n=${#times[@]}

  # Sort, compute median
  local -a sorted
  IFS=$'\n' read -r -d '' -a sorted \
    < <(printf '%d\n' "${times[@]}" | sort -n; printf '\0') || true
  local mid=$(( n / 2 ))
  TIMING_MEDIAN=${sorted[$mid]:-0}

  # Compute stddev with awk
  TIMING_STDDEV=$(printf '%d\n' "${times[@]}" | awk -v n="$n" '
    { sum += $1; sumsq += $1*$1 }
    END {
      if (n > 0) {
        mean = sum / n
        var = (sumsq / n) - (mean * mean)
        printf "%d", (var > 0) ? int(sqrt(var)) : 0
      } else { print 0 }
    }
  ')
  echo "$TIMING_MEDIAN"
}

# time_manual_loop CMD [ARGS...]
# Like _time_cmd_manual but does NOT discard warmup and uses BENCH_RUNS /
# BENCH_WARMUP globals. Useful for operations that can't be timed with
# hyperfine (e.g. iterative multi-call sequences where each call depends on
# the result of the previous one).
time_manual_loop() {
  _time_cmd_manual "${BENCH_RUNS:-5}" "${BENCH_WARMUP:-1}" "$@"
}

# timing_tool_name — human-readable name for the active timing tool.
timing_tool_name() {
  if command -v hyperfine >/dev/null 2>&1; then
    local v; v=$(hyperfine --version 2>/dev/null | head -1)
    echo "${v:-hyperfine}"
  else
    echo "bash date(1) fallback"
  fi
}
