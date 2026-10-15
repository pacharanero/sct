#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
# SPDX-License-Identifier: AGPL-3.0-or-later

# lib/report.sh - render benchmark results as table, json, csv, or markdown.
#
# Reads from BENCH_RESULTS_TSV (tab-separated: op|label|lms|lsd|rms|rsd|notes)
# Uses metadata globals: SNOMED_VERSION SNOMED_CONCEPT_COUNT FHIR_PING_MS
#                        BENCH_SERVER BENCH_DB SCT_FHIR BENCH_RUNS BENCH_WARMUP BENCH_DATE

# _speedup LOCAL REMOTE - compute Nx speedup as string, or "-"
_speedup() {
  local lms="$1" rms="$2"
  if [[ "$rms" == "-" || "$lms" == "-" || "$lms" -le 0 ]]; then
    echo "-"
    return
  fi
  awk -v l="$lms" -v r="$rms" 'BEGIN {
    x = r / l
    if (x >= 10) printf "%d×", int(x)
    else          printf "%.1f×", x
  }'
}

# _time_fmt US - auto-scale microseconds for human display
#   < 1000 us  → "NNN us"   e.g. "847 us"
#   < 10000 us → "N.N ms"   e.g. "1.3 ms"
#   >= 10000   → "NNNN ms"  e.g. "131 ms"
_time_fmt() {
  local us="$1"
  [[ "$us" == "-" ]] && echo "-" && return
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

# _pm_fmt US - "±N us" / "±N.N ms" / "±NNN ms" or "-"
_pm_fmt() {
  local us="$1"
  [[ "$us" == "-" ]] && echo "-" && return
  printf '±%s' "$(_time_fmt "$us")"
}

# _verdict LOCAL REMOTE - human "N× faster" / "N× slower" / "about the same" / "-"
_verdict() {
  local lms="$1" rms="$2"
  if [[ "$rms" == "-" || "$lms" == "-" || "$lms" -le 0 || "$rms" -le 0 ]]; then
    echo "-"; return
  fi
  awk -v l="$lms" -v r="$rms" 'BEGIN {
    if (r >= l) { x = r/l; dir="faster" } else { x = l/r; dir="slower" }
    if (x < 1.05)     { print "about the same"; exit }
    if (x >= 10)       printf "%d× %s", int(x + 0.5), dir
    else               printf "%.1f× %s", x, dir
  }'
}

# _bar VALUE MAXVAL WIDTH - a unicode block bar with eighth-block resolution,
# so even a tiny value shows a visible sliver.
_bar() {
  local val="$1" maxval="$2" width="$3"
  [[ "$val" == "-" || "$maxval" -le 0 ]] && return
  local eighths=$(( val * width * 8 / maxval ))
  (( eighths < 0 )) && eighths=0
  local full=$(( eighths / 8 )) rem=$(( eighths % 8 ))
  local parts=(▏ ▎ ▍ ▌ ▋ ▊ ▉)   # 1/8 .. 7/8
  local bar="" i
  for (( i = 0; i < full; i++ )); do bar+="█"; done
  (( rem > 0 )) && bar+="${parts[rem-1]}"
  [[ -z "$bar" && "$val" -gt 0 ]] && bar="▏"
  printf '%s' "$bar"
}

# _footnotes - collect and de-duplicate footnote notes
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
  printf '\nsct benchmark - %s\n' "$BENCH_DATE"
  if [[ -n "$SCT_FHIR" ]]; then
    printf '  sct       : %s (FHIR)\n' "$SCT_FHIR"
  else
    printf '  sct       : %s' "$BENCH_DB"
    [[ -n "$SNOMED_CONCEPT_COUNT" && "$SNOMED_CONCEPT_COUNT" != "?" ]] && \
      printf ' (%s concepts, v%s)' \
        "$(printf '%s' "$SNOMED_CONCEPT_COUNT" | sed ':a;s/\B[0-9]\{3\}\b/,&/;ta')" \
        "${SNOMED_VERSION:-?}"
    printf '\n'
  fi
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
    "$w_l"  "${BENCH_SCT_LABEL:-sct (local)}" \
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

  # Totals row - a sum of heterogeneous operations. We deliberately do NOT show
  # a speedup ratio here: it would over-state the result (it's dominated by the
  # multi-round-trip ops). The per-operation rows are the real comparison.
  printf '%s\n' "$(printf '─%.0s' $(seq 1 $(( w_op + w_l + w_sd + w_r + w_rsd + w_sp + 12 ))))"
  local total_rms_str="-"
  $total_rms_valid && total_rms_str="$(_ms_fmt "$total_rms")"
  printf '%-*s  %*s  %*s  %*s  %*s  %*s\n' \
    "$w_op" "total (sum)" \
    "$w_l"  "$(_ms_fmt "$total_lms")" \
    "$w_sd" "" \
    "$w_r"  "$total_rms_str" \
    "$w_rsd" "" \
    "$w_sp" "-"

  if (( ${#_footnotes[@]} > 0 )); then
    printf '\n'
    for fn in "${_footnotes[@]}"; do printf '%s\n' "$fn"; done
  fi
  if [[ -n "$SCT_FHIR" ]]; then
    printf '\ntimes are wall-clock median (us = microseconds); both targets include loopback HTTP and server processing.\n'
  else
    printf '\ntimes are wall-clock median (us = microseconds); local times include sqlite3 process startup.\n'
  fi
  printf 'the single-query rows (lookup, search, children) are the like-for-like comparison. ancestor-chain\n'
  printf 'and bulk lookup also count the sequential FHIR calls a client must issue (see notes), so their\n'
  printf 'ratios reflect round-trip count as well as per-query speed; the total is a sum, not an overall ratio.\n'
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
    --arg local_server "${SCT_FHIR:-}" \
    --arg snomed_version "${SNOMED_VERSION:-?}" \
    --arg concept_count "${SNOMED_CONCEPT_COUNT:-?}" \
    --arg server "${BENCH_SERVER:-}" \
    --arg ping "${FHIR_PING_MS:-0}" \
    --arg runs "$BENCH_RUNS" \
    --arg warmup "$BENCH_WARMUP" \
    --argjson results "$rows" \
    '{date:$date,
      db:(if $local_server == "" then $db else null end),
      local_server:(if $local_server == "" then null else $local_server end),
      snomed_version:$snomed_version,
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

render_chart() {
  local tsv="$1"
  local width=36
  local rlabel="fhir"
  [[ -z "$BENCH_SERVER" ]] && rlabel="remote"

  printf '\nsct vs %s - median latency (each pair scaled to the slower of the two)\n' "$rlabel"
  printf 'single-query ops (lookup, search, children) are the like-for-like number; ancestor/bulk\n'
  printf 'ratios also reflect the N sequential FHIR calls a client must make (see the detail notes).\n\n'

  while IFS=$'\t' read -r op label lms lsd rms rsd notes; do
    [[ -z "$op" ]] && continue
    local verdict; verdict=$(_verdict "$lms" "$rms")
    # Scale both bars to the slower (longer) of the pair so the ratio is visible.
    local maxval="$lms"
    [[ "$rms" != "-" ]] && (( rms > lms )) && maxval="$rms"

    printf '%s' "$label"
    [[ "$verdict" != "-" ]] && printf '   (%s)' "$verdict"
    printf '\n'
    printf '  %-6s %8s  %s\n' "sct" "$(_ms_fmt "$lms")" "$(_bar "$lms" "$maxval" "$width")"
    [[ "$rms" != "-" ]] && \
      printf '  %-6s %8s  %s\n' "$rlabel" "$(_ms_fmt "$rms")" "$(_bar "$rms" "$maxval" "$width")"
    printf '\n'
  done < "$tsv"
}

# render_markdown TSV OUTPUT_FILE
# Writes a report leading with the speed-comparison bar chart, followed by the
# aligned detail table - both code-fenced so the raw file is readable and it
# still renders cleanly on GitHub.
render_markdown() {
  local tsv="$1" outfile="$2"

  {
    printf '# sct benchmark - %s\n\n' "$BENCH_DATE"

    # One-line environment summary.
    printf '**Environment:** %s' \
      "$(command -v sct >/dev/null 2>&1 && sct --version 2>/dev/null | head -1 || echo "sct n/a")"
    [[ -n "$SNOMED_VERSION" && "$SNOMED_VERSION" != "?" ]] && printf ' · SNOMED %s' "$SNOMED_VERSION"
    [[ -n "$SNOMED_CONCEPT_COUNT" && "$SNOMED_CONCEPT_COUNT" != "?" ]] && \
      printf ' · %s concepts' \
        "$(printf '%s' "$SNOMED_CONCEPT_COUNT" | sed ':a;s/\B[0-9]\{3\}\b/,&/;ta')"
    printf ' · %s\n\n' "$(uname -sr)"

    # Headline: the bar chart. Code-fenced so alignment survives raw and rendered.
    printf '## Speed comparison\n\n'
    printf '```text'
    render_chart "$tsv"
    printf '```\n\n'

    # Detail: the aligned monospace table (its own header carries db/remote/timing,
    # footnotes, and the caveat line).
    printf '## Detail\n\n'
    printf '```text'
    render_table "$tsv"
    printf '\n```\n'
  } > "$outfile"

  printf 'wrote %s\n' "$outfile" >&2
}
