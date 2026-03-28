#!/usr/bin/env bash
# operations/children.sh — direct children of a concept
#
# Representative fixture: 73211009 (Diabetes mellitus — ~20 direct children)
# Local:  JOIN on concept_isa table
# Remote: ValueSet/$expand with ECL expression <!PARENT (direct children)

run_children() {
  local parent="73211009"
  printf '  → direct children (%s) ...\n' "$parent" >&2

  local lms lsd
  lms=$(local_time_children "$parent")
  lsd=$TIMING_STDDEV

  local rms="-" rsd="-" notes=""
  if [[ -n "$BENCH_SERVER" ]]; then
    if rms=$(fhir_time_children "$parent" 2>/dev/null); then
      rsd=$TIMING_STDDEV
    else
      rms="-"; rsd="-"; notes="fhir call failed"
    fi
  fi

  append_result "children" "direct children" "$lms" "$lsd" "$rms" "$rsd" "$notes"
}
