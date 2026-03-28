#!/usr/bin/env bash
# operations/ancestors.sh — full ancestor chain from leaf to root
#
# Representative fixture: 44054006 (Type 2 diabetes mellitus, depth ~8)
# Local:  Recursive CTE on concept_isa — single query, all ancestors at once
# Remote: Sequential CodeSystem/$lookup?property=parent calls (one per hop).
#         This accurately reflects the real cost of ancestor traversal via FHIR,
#         since most servers do not support the non-standard property=ancestor.
#         FHIR_ANCESTOR_HOPS is set by fhir_time_ancestors_iterative.

run_ancestors() {
  local code="44054006"
  printf '  → ancestor chain (%s) ...\n' "$code" >&2

  local lms lsd
  lms=$(local_time_ancestors "$code")
  lsd=$TIMING_STDDEV

  # Depth from local DB (used in notes only; FHIR discovers it during traversal)
  local depth
  depth=$(local_concept_depth "$code")

  local rms="-" rsd="-" notes=""
  if [[ -n "$BENCH_SERVER" ]]; then
    if rms=$(fhir_time_ancestors_iterative "$code" 2>/dev/null); then
      rsd=$TIMING_STDDEV
      notes="${FHIR_ANCESTOR_HOPS} sequential \$lookup calls (one per IS-A hop)"
    else
      rms="-"; rsd="-"
      notes="fhir call failed or server returned no parent property"
    fi
  fi

  append_result "ancestors" "ancestor chain (depth ~${depth})" \
    "$lms" "$lsd" "$rms" "$rsd" "$notes"
}
