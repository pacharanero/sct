#!/usr/bin/env bash
# operations/lookup.sh — single concept lookup by SCTID
#
# Representative fixture: 22298006 (Myocardial infarction)
# Local:  SELECT on concepts table (exact primary-key lookup)
# Remote: CodeSystem/$lookup?system=http://snomed.info/sct&code=...

run_lookup() {
  local code="22298006"
  printf '  → concept lookup (%s) ...\n' "$code" >&2

  local lms lsd
  lms=$(local_time_lookup "$code")
  lsd=$TIMING_STDDEV

  local rms="-" rsd="-" notes=""
  if [[ -n "$BENCH_SERVER" ]]; then
    if rms=$(fhir_time_lookup "$code" 2>/dev/null); then
      rsd=$TIMING_STDDEV
    else
      rms="-"; rsd="-"; notes="fhir call failed"
    fi
  fi

  append_result "lookup" "concept lookup" "$lms" "$lsd" "$rms" "$rsd" "$notes"
}
