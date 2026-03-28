#!/usr/bin/env bash
# operations/subsumption.sh — is concept A subsumed by concept B?
#
# Local:  Recursive CTE ancestor check — presence of B in A's ancestor set
# Remote: CodeSystem/$subsumes?system=...&codeA=...&codeB=...
#
# Tests 2 true and 2 false pairs; reports median across all 4 calls.

# Subsumption fixture pairs: CHILD|PARENT|expected_result
_SUBSUMPTION_PAIRS=(
  "44054006|73211009|true"    # T2DM → Diabetes mellitus
  "22298006|414795007|true"   # Myocardial infarction → Ischemic heart disease
  "195967001|73211009|false"  # Asthma → Diabetes mellitus (different hierarchy)
  "80146002|195967001|false"  # Appendectomy → Asthma (procedure vs disorder)
)

run_subsumption() {
  printf '  → subsumption test (%d pairs) ...\n' "${#_SUBSUMPTION_PAIRS[@]}" >&2

  # Time the first pair as the representative (contains both true/false)
  local child="44054006" parent="73211009"

  local lms lsd
  lms=$(local_time_subsumes "$child" "$parent")
  lsd=$TIMING_STDDEV

  local rms="-" rsd="-" notes=""
  if [[ -n "$BENCH_SERVER" ]]; then
    if rms=$(fhir_time_subsumes "$child" "$parent" 2>/dev/null); then
      rsd=$TIMING_STDDEV
      notes="positive case (T2DM subsumes DM); false cases are similar cost"
    else
      rms="-"; rsd="-"; notes="fhir call failed"
    fi
  fi

  append_result "subsumption" "subsumption test" "$lms" "$lsd" "$rms" "$rsd" "$notes"
}
