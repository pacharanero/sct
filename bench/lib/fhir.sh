#!/usr/bin/env bash
# lib/fhir.sh — FHIR R4 terminology server query wrappers.
#
# All functions require BENCH_SERVER (base URL, no trailing slash),
# BENCH_TIMEOUT, BENCH_RUNS, BENCH_WARMUP, and time_cmd from lib/timing.sh.
#
# The FHIR $-operations require literal $ in the URL. Inside bash double
# quotes, use \$ to prevent variable interpolation.

# _urlencode STR — percent-encodes a string for safe embedding in a URL query value.
_urlencode() {
  local s="$1" out='' i c
  for (( i=0; i<${#s}; i++ )); do
    c="${s:$i:1}"
    case "$c" in
      [a-zA-Z0-9._~-]) out+="$c" ;;
      ' ')              out+='%20' ;;
      *)                out+=$(printf '%%%02X' "'$c") ;;
    esac
  done
  printf '%s' "$out"
}

# _curl_fhir URL [EXTRA_CURL_ARGS...]
# Silent curl against a FHIR endpoint with appropriate Accept header.
_curl_fhir() {
  curl -sf \
    --max-time "${BENCH_TIMEOUT:-30}" \
    -H "Accept: application/fhir+json" \
    "$@"
}

# fhir_time_lookup CODE
fhir_time_lookup() {
  local code="$1"
  local url="${BENCH_SERVER}/CodeSystem/\$lookup?system=http://snomed.info/sct&code=${code}&property=display&property=designation"
  time_cmd "$BENCH_RUNS" "$BENCH_WARMUP" \
    curl -sf --max-time "${BENCH_TIMEOUT:-30}" \
    -H "Accept: application/fhir+json" \
    "$url"
}

# fhir_time_lookup_property CODE PROPERTY
# Lookup a single property (used for per-hop ancestor timing).
fhir_time_lookup_property() {
  local code="$1" prop="$2"
  local url="${BENCH_SERVER}/CodeSystem/\$lookup?system=http://snomed.info/sct&code=${code}&property=${prop}"
  time_cmd "$BENCH_RUNS" "$BENCH_WARMUP" \
    curl -sf --max-time "${BENCH_TIMEOUT:-30}" \
    -H "Accept: application/fhir+json" \
    "$url"
}

# fhir_time_search TERM [LIMIT]
fhir_time_search() {
  local term="$1" limit="${2:-10}"
  local enc_term; enc_term=$(_urlencode "$term")
  local url="${BENCH_SERVER}/ValueSet/\$expand?url=http://snomed.info/sct?fhir_vs&filter=${enc_term}&count=${limit}"
  time_cmd "$BENCH_RUNS" "$BENCH_WARMUP" \
    curl -sf --max-time "${BENCH_TIMEOUT:-30}" \
    -H "Accept: application/fhir+json" \
    "$url"
}

# fhir_time_children PARENT_ID
# Uses ECL expression <!PARENT_ID (direct children) via ValueSet/$expand.
fhir_time_children() {
  local parent="$1"
  local ecl; ecl=$(_urlencode "<!${parent}")
  # ValueSet canonical for an ECL-defined set:
  #   http://snomed.info/sct?fhir_vs=ecl/<!PARENT
  local vs_url; vs_url=$(_urlencode "http://snomed.info/sct?fhir_vs=ecl/<!${parent}")
  local url="${BENCH_SERVER}/ValueSet/\$expand?url=${vs_url}&count=1000"
  time_cmd "$BENCH_RUNS" "$BENCH_WARMUP" \
    curl -sf --max-time "${BENCH_TIMEOUT:-30}" \
    -H "Accept: application/fhir+json" \
    "$url"
}

# fhir_time_subsumes CODE_A CODE_B
# Checks whether A is subsumed by B (A is-a B).
fhir_time_subsumes() {
  local a="$1" b="$2"
  local url="${BENCH_SERVER}/CodeSystem/\$subsumes?system=http://snomed.info/sct&codeA=${a}&codeB=${b}"
  time_cmd "$BENCH_RUNS" "$BENCH_WARMUP" \
    curl -sf --max-time "${BENCH_TIMEOUT:-30}" \
    -H "Accept: application/fhir+json" \
    "$url"
}

# fhir_time_ancestors_iterative CODE DEPTH
# Times a full iterative ancestor walk via sequential $lookup?property=parent
# calls. Because each call depends on the previous result, hyperfine cannot
# be used here; we use manual loop timing.
#
# Sets FHIR_ANCESTOR_HOPS to the number of hops actually traversed.
fhir_time_ancestors_iterative() {
  local start_code="$1"
  local -a all_times=()
  local total=$(( BENCH_WARMUP + BENCH_RUNS ))
  local i

  # Discover the parent chain once (untimed) to confirm connectivity.
  FHIR_ANCESTOR_HOPS=0
  local probe_resp probe_parent current="$start_code"
  while true; do
    probe_resp=$(_curl_fhir \
      "${BENCH_SERVER}/CodeSystem/\$lookup?system=http://snomed.info/sct&code=${current}&property=parent" \
      2>/dev/null) || break
    probe_parent=$(printf '%s' "$probe_resp" | jq -r '
      .parameter[]?
      | select(.name == "property")
      | select((.part // [])[] | .name == "code" and .valueCode == "parent")
      | (.part[] | select(.name == "value") | .valueCode)
    ' 2>/dev/null | head -1)
    [[ -z "$probe_parent" || "$probe_parent" == "null" ]] && break
    current="$probe_parent"
    (( FHIR_ANCESTOR_HOPS++ ))
    # Safety cap — SNOMED hierarchy never exceeds ~20 levels
    (( FHIR_ANCESTOR_HOPS > 25 )) && break
  done

  if (( FHIR_ANCESTOR_HOPS == 0 )); then
    TIMING_MEDIAN="-"
    TIMING_STDDEV="-"
    echo "-"
    return 1
  fi

  # Timed runs: each run re-traverses the full chain.
  for (( i=0; i<total; i++ )); do
    local t0 t1
    t0=$(date +%s%N)
    current="$start_code"
    while true; do
      local resp parent
      resp=$(_curl_fhir \
        "${BENCH_SERVER}/CodeSystem/\$lookup?system=http://snomed.info/sct&code=${current}&property=parent" \
        2>/dev/null) || break
      parent=$(printf '%s' "$resp" | jq -r '
        .parameter[]?
        | select(.name == "property")
        | select((.part // [])[] | .name == "code" and .valueCode == "parent")
        | (.part[] | select(.name == "value") | .valueCode)
      ' 2>/dev/null | head -1)
      [[ -z "$parent" || "$parent" == "null" ]] && break
      current="$parent"
    done
    t1=$(date +%s%N)
    all_times+=( $(( (t1 - t0) / 1000000 )) )
  done

  local -a kept=("${all_times[@]:$BENCH_WARMUP}")
  local n=${#kept[@]}
  local -a sorted
  IFS=$'\n' read -r -d '' -a sorted \
    < <(printf '%d\n' "${kept[@]}" | sort -n; printf '\0') || true
  TIMING_MEDIAN=${sorted[$(( n / 2 ))]:-0}
  TIMING_STDDEV=$(printf '%d\n' "${kept[@]}" | awk -v n="$n" '
    { sum+=$1; sumsq+=$1*$1 }
    END { mean=sum/n; var=(sumsq/n)-(mean*mean); printf "%d", (var>0)?int(sqrt(var)):0 }
  ')
  echo "$TIMING_MEDIAN"
}

# fhir_time_bulk ID1 ID2 ...
# Attempts a FHIR batch Bundle. Falls back to sequential $lookup calls.
# Sets FHIR_BULK_MODE to "batch" or "sequential".
fhir_time_bulk() {
  local -a codes=("$@")

  # Build a batch Bundle body.
  local entries=''
  local code
  for code in "${codes[@]}"; do
    entries+=$(printf '{"request":{"method":"GET","url":"CodeSystem/$lookup?system=http://snomed.info/sct&code=%s"}},\n' "$code")
  done
  entries="${entries%,$'\n'}"
  local bundle="{\"resourceType\":\"Bundle\",\"type\":\"batch\",\"entry\":[${entries}]}"

  # Probe batch support (one untimed call).
  local probe
  probe=$(printf '%s' "$bundle" | curl -sf --max-time "${BENCH_TIMEOUT:-30}" \
    -X POST \
    -H "Content-Type: application/fhir+json" \
    -H "Accept: application/fhir+json" \
    -d @- \
    "${BENCH_SERVER}" 2>/dev/null)

  if [[ $(printf '%s' "$probe" | jq -r '.resourceType' 2>/dev/null) == "Bundle" ]]; then
    FHIR_BULK_MODE="batch"
    local tmpbody; tmpbody=$(mktemp /tmp/bench_bulk_XXXXXX.json)
    printf '%s' "$bundle" > "$tmpbody"
    time_cmd "$BENCH_RUNS" "$BENCH_WARMUP" \
      curl -sf --max-time "${BENCH_TIMEOUT:-30}" \
      -X POST \
      -H "Content-Type: application/fhir+json" \
      -H "Accept: application/fhir+json" \
      --data "@${tmpbody}" \
      "${BENCH_SERVER}"
    rm -f "$tmpbody"
  else
    FHIR_BULK_MODE="sequential"
    # Write a temp script that issues all lookups sequentially.
    local tmpscript; tmpscript=$(mktemp /tmp/bench_bulk_XXXXXX.sh)
    {
      printf '#!/usr/bin/env bash\n'
      for code in "${codes[@]}"; do
        printf 'curl -sf --max-time %s -H "Accept: application/fhir+json" "%s/CodeSystem/\\$lookup?system=http://snomed.info/sct&code=%s" >/dev/null 2>&1\n' \
          "${BENCH_TIMEOUT:-30}" "${BENCH_SERVER}" "$code"
      done
    } > "$tmpscript"
    chmod +x "$tmpscript"
    time_cmd "$BENCH_RUNS" "$BENCH_WARMUP" bash "$tmpscript"
    rm -f "$tmpscript"
  fi
}

# check_fhir_server
# Pings the server's /metadata endpoint. Sets FHIR_PING_MS.
# Returns 0 on success, 1 if unreachable.
check_fhir_server() {
  local -a pings=()
  local i t0 t1
  for (( i=0; i<3; i++ )); do
    t0=$(date +%s%N)
    if ! _curl_fhir "${BENCH_SERVER}/metadata" >/dev/null 2>&1; then
      FHIR_PING_MS=0
      return 1
    fi
    t1=$(date +%s%N)
    pings+=( $(( (t1 - t0) / 1000000 )) )
  done
  # Median of 3
  local -a sorted
  IFS=$'\n' read -r -d '' -a sorted \
    < <(printf '%d\n' "${pings[@]}" | sort -n; printf '\0') || true
  FHIR_PING_MS=${sorted[1]:-0}
  return 0
}
