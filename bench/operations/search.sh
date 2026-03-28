#!/usr/bin/env bash
# operations/search.sh — free-text search (FTS5 vs ValueSet/$expand)
#
# Representative fixture: "heart attack"
# Local:  FTS5 MATCH query on concepts_fts
# Remote: ValueSet/$expand with filter parameter

run_search() {
  local term="heart attack"
  printf '  → text search ("%s") ...\n' "$term" >&2

  local lms lsd
  lms=$(local_time_search "$term" 10)
  lsd=$TIMING_STDDEV

  local rms="-" rsd="-" notes=""
  if [[ -n "$BENCH_SERVER" ]]; then
    if rms=$(fhir_time_search "$term" 10 2>/dev/null); then
      rsd=$TIMING_STDDEV
    else
      rms="-"; rsd="-"; notes="fhir call failed"
    fi
  fi

  append_result "search" "text search (top 10)" "$lms" "$lsd" "$rms" "$rsd" "$notes"
}
