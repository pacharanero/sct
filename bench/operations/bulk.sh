#!/usr/bin/env bash
# operations/bulk.sh — resolve all fixture concepts in one request/query
#
# Local:  Single SELECT ... WHERE id IN (...) across all fixture IDs
# Remote: FHIR batch Bundle POST (falls back to sequential $lookup calls
#         if the server does not support batch).
#
# This operation most clearly illustrates the per-round-trip cost of HTTP.

run_bulk() {
  # Load IDs from fixtures file (strip comments and labels)
  local -a ids=()
  while IFS='|' read -r id _rest; do
    [[ "$id" =~ ^[[:space:]]*# ]] && continue
    [[ -z "$id" ]] && continue
    ids+=( "$id" )
  done < "${BENCH_DIR}/fixtures/concepts.txt"

  local n="${#ids[@]}"
  printf '  → bulk lookup (%d concepts) ...\n' "$n" >&2

  local lms lsd
  lms=$(local_time_bulk "${ids[@]}")
  lsd=$TIMING_STDDEV

  local rms="-" rsd="-" notes=""
  if [[ -n "$BENCH_SERVER" ]]; then
    if rms=$(fhir_time_bulk "${ids[@]}" 2>/dev/null); then
      rsd=$TIMING_STDDEV
      if [[ "${FHIR_BULK_MODE:-}" == "sequential" ]]; then
        notes="server does not support FHIR batch; ${n} sequential \$lookup calls issued"
      else
        notes="FHIR batch bundle (${n} entries)"
      fi
    else
      rms="-"; rsd="-"; notes="fhir call failed"
    fi
  fi

  append_result "bulk" "bulk lookup (${n} concepts)" \
    "$lms" "$lsd" "$rms" "$rsd" "$notes"
}
