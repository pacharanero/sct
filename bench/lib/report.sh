#!/usr/bin/env bash
# lib/report.sh — render benchmark results as table, json, csv, or markdown.
#
# Reads from BENCH_RESULTS_TSV (tab-separated: op|label|lms|lsd|rms|rsd|notes)
# Uses metadata globals: SNOMED_VERSION SNOMED_CONCEPT_COUNT FHIR_PING_MS
#                        BENCH_SERVER BENCH_DB BENCH_RUNS BENCH_WARMUP BENCH_DATE

# _speedup LOCAL REMOTE — compute Nx speedup as string, or "—"
_speedup() {
  local lms="$1" rms="$2"
  if [[ "$rms" == "-" || "$lms" == "-" || "$lms" -le 0 ]]; then
    echo "—"
    return
  fi
  awk -v l="$lms" -v r="$rms" 'BEGIN {
    x = r / l
    if (x >= 10) printf "%d×", int(x)
    else          printf "%.1f×", x
  }'
}

# _time_fmt US — auto-scale microseconds for human display
#   < 1000 us  → "NNN us"   e.g. "847 us"
#   < 10000 us → "N.N ms"   e.g. "1.3 ms"
#   >= 10000   → "NNNN ms"  e.g. "131 ms"
_time_fmt() {
  local us="$1"
  [[ "$us" == "-" ]] && echo "—" && return
  if (( us < 1000 )); then
    echo "${us} us"
  elif (( us < 10000 )); then
    awk -v n="$us" 'BEGIN { printf "%.1f ms", n/1000 }'
  else
    echo "$(( us / 1000 )) ms"
  fi
}

# Keep _ms_fmt as an alias so any direct callers still work.
_ms_fmt() { _time_fmt "$@"; }

# _pm_fmt US — "±N us" / "±N.N ms" / "±NNN ms" or "—"
_pm_fmt() {
  local us="$1"
  [[ "$us" == "-" ]] && echo "—" && return
  printf '±%s' "$(_time_fmt "$us")"
}

# _footnotes — collect and de-duplicate footnote notes
_footnotes=()
_add_footnote() {
  local note="$1"
  [[ -z "$note" ]] && return
  _footnotes+=( "$note" )
}

render_table() {
  local tsv="$1"
  local remote_label="fhir (remote)"
  [[ -z "$BENCH_SERVER" ]] && remote_label="(not measured)"

  # Header
  printf '\nsct benchmark — %s\n' "$BENCH_DATE"
  printf '  local db  : %s' "$BENCH_DB"
  [[ -n "$SNOMED_CONCEPT_COUNT" && "$SNOMED_CONCEPT_COUNT" != "?" ]] && \
    printf ' (%s concepts, v%s)' \
      "$(printf '%s' "$SNOMED_CONCEPT_COUNT" | sed ':a;s/\B[0-9]\{3\}\b/,&/;ta')" \
      "${SNOMED_VERSION:-?}"
  printf '\n'
  if [[ -n "$BENCH_SERVER" ]]; then
    printf '  remote    : %s' "$BENCH_SERVER"
    [[ -n "$FHIR_PING_MS" && "$FHIR_PING_MS" != "0" ]] && \
      printf ' (ping: %s ms)' "$FHIR_PING_MS"
    printf '\n'
  fi
  printf '  timing    : %s (%s runs, %s warmup)\n\n' \
    "$(timing_tool_name)" "$BENCH_RUNS" "$BENCH_WARMUP"

  # Column widths (fixed, wide enough for all expected values)
  local w_op=34 w_l=10 w_sd=8 w_r=14 w_rsd=8 w_sp=14

  # Table header
  printf '%-*s  %*s  %*s  %*s  %*s  %*s\n' \
    "$w_op" "operation" \
    "$w_l"  "sct (local)" \
    "$w_sd" "±" \
    "$w_r"  "$remote_label" \
    "$w_rsd" "±" \
    "$w_sp" "speedup"
  printf '%s\n' "$(printf '─%.0s' $(seq 1 $(( w_op + w_l + w_sd + w_r + w_rsd + w_sp + 12 ))))"

  local total_lms=0 total_rms=0 total_rms_valid=true
  local fnidx=0
  _footnotes=()

  while IFS=$'\t' read -r op label lms lsd rms rsd notes; do
    [[ -z "$op" ]] && continue

    local sp; sp=$(_speedup "$lms" "$rms")
    local note_marker=""
    if [[ -n "$notes" ]]; then
      (( fnidx++ ))
      _add_footnote "[${fnidx}] ${notes}"
      note_marker=" [${fnidx}]"
    fi

    printf '%-*s  %*s  %*s  %*s  %*s  %*s\n' \
      "$w_op" "${label}" \
      "$w_l"  "$(_ms_fmt "$lms")" \
      "$w_sd" "$(_pm_fmt "$lsd")" \
      "$w_r"  "$(_ms_fmt "$rms")${note_marker}" \
      "$w_rsd" "$(_pm_fmt "$rsd")" \
      "$w_sp" "$sp"

    [[ "$lms" != "-" ]] && (( total_lms += lms ))
    if [[ "$rms" != "-" ]]; then
      (( total_rms += rms ))
    else
      total_rms_valid=false
    fi

  done < "$tsv"

  # Totals row
  printf '%s\n' "$(printf '─%.0s' $(seq 1 $(( w_op + w_l + w_sd + w_r + w_rsd + w_sp + 12 ))))"
  local total_sp="—"
  $total_rms_valid && total_sp=$(_speedup "$total_lms" "$total_rms")
  local total_rms_str="—"
  $total_rms_valid && total_rms_str="$(_ms_fmt "$total_rms")"
  printf '%-*s  %*s  %*s  %*s  %*s  %*s\n' \
    "$w_op" "total (sum)" \
    "$w_l"  "$(_ms_fmt "$total_lms")" \
    "$w_sd" "" \
    "$w_r"  "$total_rms_str" \
    "$w_rsd" "" \
    "$w_sp" "$total_sp"

  if (( ${#_footnotes[@]} > 0 )); then
    printf '\n'
    for fn in "${_footnotes[@]}"; do printf '%s\n' "$fn"; done
  fi
  printf '\ntimes are wall-clock median (us = microseconds); local times include sqlite3 process startup.\n'
}

render_json() {
  local tsv="$1"
  local rows="[]"
  while IFS=$'\t' read -r op label lms lsd rms rsd notes; do
    [[ -z "$op" ]] && continue
    local sp; sp=$(_speedup "$lms" "$rms")
    rows=$(printf '%s' "$rows" | jq --arg op "$op" --arg label "$label" \
      --arg lms "$lms" --arg lsd "$lsd" \
      --arg rms "$rms" --arg rsd "$rsd" \
      --arg notes "$notes" --arg speedup "$sp" \
      '. + [{op:$op,label:$label,local_us:($lms|tonumber? // null),
             local_stddev_us:($lsd|tonumber? // null),
             remote_us:($rms|tonumber? // null),
             remote_stddev_us:($rsd|tonumber? // null),
             speedup:$speedup,notes:$notes}]')
  done < "$tsv"
  jq -n \
    --arg date "$BENCH_DATE" \
    --arg db "$BENCH_DB" \
    --arg snomed_version "${SNOMED_VERSION:-?}" \
    --arg concept_count "${SNOMED_CONCEPT_COUNT:-?}" \
    --arg server "${BENCH_SERVER:-}" \
    --arg ping "${FHIR_PING_MS:-0}" \
    --arg runs "$BENCH_RUNS" \
    --arg warmup "$BENCH_WARMUP" \
    --argjson results "$rows" \
    '{date:$date,db:$db,snomed_version:$snomed_version,
      concept_count:($concept_count|tonumber? // $concept_count),
      remote_server:$server,remote_ping_ms:($ping|tonumber),
      runs:($runs|tonumber),warmup:($warmup|tonumber),
      results:$results}'
}

render_csv() {
  local tsv="$1"
  printf 'operation,label,local_us,local_stddev_us,remote_us,remote_stddev_us,speedup,notes\n'
  while IFS=$'\t' read -r op label lms lsd rms rsd notes; do
    [[ -z "$op" ]] && continue
    local sp; sp=$(_speedup "$lms" "$rms")
    printf '"%s","%s",%s,%s,%s,%s,"%s","%s"\n' \
      "$op" "$label" "$lms" "$lsd" "$rms" "$rsd" "$sp" "$notes"
  done < "$tsv"
}

# render_markdown TSV OUTPUT_FILE
# Writes a benchmarks.md file suitable for committing to the repository.
render_markdown() {
  local tsv="$1" outfile="$2"
  local remote_label="fhir (remote)"
  [[ -z "$BENCH_SERVER" ]] && remote_label="not measured"

  _footnotes=()
  local fnidx=0

  # Collect rows into arrays for two-pass (need footnotes before writing table)
  local -a row_ops=() row_labels=() row_lms=() row_lsd=()
  local -a row_rms=() row_rsd=() row_notes=() row_sp=()
  local total_lms=0 total_rms=0 total_rms_valid=true

  while IFS=$'\t' read -r op label lms lsd rms rsd notes; do
    [[ -z "$op" ]] && continue
    row_ops+=("$op"); row_labels+=("$label")
    row_lms+=("$lms"); row_lsd+=("$lsd")
    row_rms+=("$rms"); row_rsd+=("$rsd")
    row_notes+=("$notes")
    row_sp+=("$(_speedup "$lms" "$rms")")
    [[ "$lms" != "-" ]] && (( total_lms += lms ))
    if [[ "$rms" != "-" ]]; then (( total_rms += rms )); else total_rms_valid=false; fi
  done < "$tsv"

  {
    printf '# benchmarks\n\n'

    printf '## environment\n\n'
    printf '| | |\n|:---|:---|\n'
    printf '| date | %s |\n' "$BENCH_DATE"
    printf '| sct version | %s |\n' \
      "$(command -v sct >/dev/null 2>&1 && sct --version 2>/dev/null | head -1 || echo "n/a")"
    printf '| snomed version | %s |\n' "${SNOMED_VERSION:-?}"
    printf '| concept count | %s |\n' \
      "$(printf '%s' "${SNOMED_CONCEPT_COUNT:-?}" | sed ':a;s/\B[0-9]\{3\}\b/,&/;ta')"
    printf '| sqlite3 version | %s |\n' "$(sqlite3 --version 2>/dev/null | cut -d' ' -f1)"
    printf '| os | %s |\n' "$(uname -sr)"
    printf '\n'

    printf '## results\n\n'
    printf '| operation | sct (local) | ± | %s | ± | speedup |\n' "$remote_label"
    printf '|:---|---:|---:|---:|---:|:---|\n'
    local i
    for (( i=0; i<${#row_ops[@]}; i++ )); do
      local note_marker=""
      if [[ -n "${row_notes[$i]}" ]]; then
        (( fnidx++ ))
        _add_footnote "[${fnidx}] ${row_notes[$i]}"
        note_marker=" [${fnidx}]"
      fi
      local sp_cell="${row_sp[$i]}"
      [[ "$sp_cell" != "—" ]] && sp_cell="**${sp_cell} faster**"
      printf '| %s | %s | %s | %s%s | %s | %s |\n' \
        "${row_labels[$i]}" \
        "$(_ms_fmt "${row_lms[$i]}")" \
        "$(_pm_fmt "${row_lsd[$i]}")" \
        "$(_ms_fmt "${row_rms[$i]}")" \
        "$note_marker" \
        "$(_pm_fmt "${row_rsd[$i]}")" \
        "$sp_cell"
    done

    # Totals
    local total_sp="—"
    $total_rms_valid && total_sp=$(_speedup "$total_lms" "$total_rms")
    [[ "$total_sp" != "—" ]] && total_sp="**${total_sp} faster**"
    local total_rms_str="—"
    $total_rms_valid && total_rms_str="$(_ms_fmt "$total_rms")"
    printf '| **total** | **%s** | | **%s** | | %s |\n\n' \
      "$(_ms_fmt "$total_lms")" "$total_rms_str" "$total_sp"

    if [[ -n "$BENCH_SERVER" ]]; then
      printf 'remote: %s' "$BENCH_SERVER"
      [[ -n "$FHIR_PING_MS" && "$FHIR_PING_MS" != "0" ]] && \
        printf ' | ping: %s ms' "$FHIR_PING_MS"
      printf ' | %s runs (+%s warmup) | %s\n\n' \
        "$BENCH_RUNS" "$BENCH_WARMUP" "$(timing_tool_name)"
    fi

    if (( ${#_footnotes[@]} > 0 )); then
      printf '## notes\n\n'
      for fn in "${_footnotes[@]}"; do printf '%s\n' "- ${fn}"; done
      printf '\n'
    fi

    printf '_times are wall-clock median (us = microseconds); local times include sqlite3 process startup._\n'
  } > "$outfile"

  printf 'wrote %s\n' "$outfile" >&2
}
