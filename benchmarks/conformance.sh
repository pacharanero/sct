#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
# SPDX-License-Identifier: AGPL-3.0-or-later

# conformance.sh - FHIR R4 terminology server conformance smoke/regression suite.
#
# This is intentionally stricter than a connectivity check and broader than the
# small timing benchmark fixtures. It asserts FHIR response shape and semantics
# before any performance comparison is considered meaningful.

set -uo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIXTURE_DIR="${BENCH_DIR}/fixtures/conformance"
SERVER=""
TIMEOUT=30
FORMAT="table"
OPERATIONS="metadata,lookup,validate-code,expand,subsumes,valueset-validate,translate,errors"
STRICT=false
OUTPUT_FILE=""

_die() { printf 'error: %s\n' "$*" >&2; exit 1; }

_show_help() {
  cat <<'USAGE'
Usage:
  benchmarks/conformance.sh --server URL [OPTIONS]

Options:
  --server URL        FHIR R4 terminology server base URL, e.g. http://localhost:8080/fhir
  --fixtures DIR      Fixture directory (default: benchmarks/fixtures/conformance)
  --operations LIST   Comma-separated subset:
                      metadata,lookup,validate-code,expand,subsumes,
                      valueset-validate,translate,errors
  --timeout SECS      Per-request curl timeout (default: 30)
  --format FORMAT     table (default) | jsonl
  --output FILE       Write result rows to FILE as JSONL
  --strict            Treat optional unsupported operations as failures
  --help              Show this help
USAGE
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --server)      SERVER="${2%/}"; shift 2 ;;
    --fixtures)    FIXTURE_DIR="$2"; shift 2 ;;
    --operations)  OPERATIONS="$2"; shift 2 ;;
    --timeout)     TIMEOUT="$2"; shift 2 ;;
    --format)      FORMAT="$2"; shift 2 ;;
    --output)      OUTPUT_FILE="$2"; shift 2 ;;
    --strict)      STRICT=true; shift ;;
    --help|-h)     _show_help ;;
    *)             _die "unknown option: $1" ;;
  esac
done

[[ -n "$SERVER" ]] || _die "--server is required"
[[ -d "$FIXTURE_DIR" ]] || _die "fixture directory not found: $FIXTURE_DIR"
[[ "$FORMAT" == "table" || "$FORMAT" == "jsonl" ]] || _die "--format must be table or jsonl"

for cmd in curl jq awk; do
  command -v "$cmd" >/dev/null 2>&1 || _die "missing required tool: $cmd"
done

# shellcheck source=lib/fhir.sh
source "${BENCH_DIR}/lib/fhir.sh"

TMPDIR=$(mktemp -d /tmp/sct_conformance_XXXXXX)
trap 'rm -rf "$TMPDIR"' EXIT
RESULTS_JSONL="${TMPDIR}/results.jsonl"
BODY_FILE="${TMPDIR}/body.json"
STATUS_FILE="${TMPDIR}/status.txt"
CAPS_FILE="${TMPDIR}/metadata.json"

pass_count=0
fail_count=0
skip_count=0

_json_escape() {
  jq -Rn --arg s "$1" '$s'
}

_record() {
  local status="$1" op="$2" name="$3" message="${4:-}"
  case "$status" in
    pass) (( pass_count++ )) ;;
    fail) (( fail_count++ )) ;;
    skip) (( skip_count++ )) ;;
  esac
  jq -cn \
    --arg status "$status" \
    --arg operation "$op" \
    --arg name "$name" \
    --arg message "$message" \
    '{status:$status,operation:$operation,name:$name,message:$message}' \
    >> "$RESULTS_JSONL"
  if [[ "$FORMAT" == "table" ]]; then
    printf '%-5s %-18s %s' "$status" "$op" "$name"
    [[ -n "$message" ]] && printf ' - %s' "$message"
    printf '\n'
  fi
}

_get() {
  local path="$1"
  local url="${SERVER}${path}"
  local status
  status=$(curl -sS \
    --max-time "$TIMEOUT" \
    -H "Accept: application/fhir+json" \
    -o "$BODY_FILE" \
    -w '%{http_code}' \
    "$url" 2>"${TMPDIR}/curl.err")
  printf '%s' "$status" > "$STATUS_FILE"
}

_status() {
  cat "$STATUS_FILE"
}

_body_resource_type() {
  jq -r '.resourceType // ""' "$BODY_FILE" 2>/dev/null
}

_has_operation() {
  local name="$1"
  jq -e --arg name "$name" '
    [.rest[]?.resource[]?.operation[]?.name, .rest[]?.operation[]?.name]
    | flatten
    | index($name) != null
  ' "$CAPS_FILE" >/dev/null 2>&1
}

_contains_text_ci() {
  local text="$1" needle="$2"
  awk -v text="$text" -v needle="$needle" 'BEGIN {
    text=tolower(text); needle=tolower(needle);
    exit(index(text, needle) ? 0 : 1)
  }'
}

_parameter_string() {
  local name="$1"
  jq -r --arg name "$name" '
    .parameter[]? | select(.name == $name)
    | if has("valueBoolean") then .valueBoolean
      else (.valueString // .valueCode // "")
      end
  ' "$BODY_FILE" 2>/dev/null | head -1
}

_lookup_property_values() {
  local prop="$1"
  jq -r --arg prop "$prop" '
    .parameter[]?
    | select(.name == "property")
    | select((.part // [])[] | .name == "code" and .valueCode == $prop)
    | (.part[] | select(.name == "value") | (.valueCode // .valueString // ""))
  ' "$BODY_FILE" 2>/dev/null
}

_expand_codes() {
  jq -r '.expansion.contains[]?.code // empty' "$BODY_FILE" 2>/dev/null
}

_skip_header() {
  local line="$1"
  [[ -z "$line" || "${line:0:1}" == "#" ]]
}

_field() {
  local value="${1:-}"
  [[ "$value" == "-" ]] && value=""
  printf '%s' "$value"
}

_require_200_resource() {
  local op="$1" name="$2" resource="$3"
  local status rt
  status=$(_status)
  rt=$(_body_resource_type)
  if [[ "$status" != "200" ]]; then
    _record fail "$op" "$name" "HTTP $status"
    return 1
  fi
  if [[ "$rt" != "$resource" ]]; then
    _record fail "$op" "$name" "expected $resource, got ${rt:-empty}"
    return 1
  fi
  return 0
}

run_metadata() {
  _get "/metadata"
  if ! _require_200_resource metadata "CapabilityStatement" "CapabilityStatement"; then
    return
  fi
  cp "$BODY_FILE" "$CAPS_FILE"

  local version
  version=$(jq -r '.fhirVersion // ""' "$CAPS_FILE")
  if [[ "$version" != 4.* ]]; then
    _record fail metadata "FHIR version" "expected R4, got ${version:-empty}"
  else
    _record pass metadata "FHIR version" "$version"
  fi

  for op in lookup validate-code subsumes expand; do
    if _has_operation "$op"; then
      _record pass metadata "advertises $op"
    else
      _record fail metadata "advertises $op" "missing from CapabilityStatement"
    fi
  done
  if _has_operation translate; then
    _record pass metadata "advertises translate"
  else
    _record skip metadata "advertises translate" "optional operation not advertised"
  fi
}

run_lookup() {
  local file="${FIXTURE_DIR}/lookup.tsv"
  while IFS=$'\t' read -r name code display_contains expected_parent; do
    _skip_header "$name" && continue
    display_contains=$(_field "$display_contains")
    expected_parent=$(_field "$expected_parent")
    _get "/CodeSystem/\$lookup?system=http://snomed.info/sct&code=${code}&property=display&property=designation&property=parent"
    _require_200_resource lookup "$name" "Parameters" || continue

    local display
    display=$(_parameter_string display)
    if [[ -n "$display_contains" ]] && ! _contains_text_ci "$display" "$display_contains"; then
      _record fail lookup "$name" "display '$display' does not contain '$display_contains'"
      continue
    fi

    if [[ -n "$expected_parent" ]]; then
      if ! _lookup_property_values parent | grep -qx "$expected_parent"; then
        _record fail lookup "$name" "missing parent $expected_parent"
        continue
      fi
    fi

    _record pass lookup "$name"
  done < "$file"
}

run_validate_code() {
  local file="${FIXTURE_DIR}/validate-code.tsv"
  while IFS=$'\t' read -r name code expected display; do
    _skip_header "$name" && continue
    display=$(_field "$display")
    local path="/CodeSystem/\$validate-code?url=http://snomed.info/sct&code=${code}"
    [[ -n "$display" ]] && path="${path}&display=$(_urlencode "$display")"
    _get "$path"
    _require_200_resource validate-code "$name" "Parameters" || continue
    local actual
    actual=$(_parameter_string result)
    if [[ "$actual" == "$expected" ]]; then
      _record pass validate-code "$name" "result=$actual"
    else
      _record fail validate-code "$name" "expected result=$expected, got ${actual:-empty}"
    fi
  done < "$file"
}

run_expand() {
  local file="${FIXTURE_DIR}/expand.tsv"
  while IFS=$'\t' read -r name valueset_url filter count min_total contains_csv; do
    _skip_header "$name" && continue
    filter=$(_field "$filter")
    contains_csv=$(_field "$contains_csv")
    local path="/ValueSet/\$expand?url=$(_urlencode "$valueset_url")&count=${count:-20}"
    [[ -n "$filter" ]] && path="${path}&filter=$(_urlencode "$filter")"
    _get "$path"
    _require_200_resource expand "$name" "ValueSet" || continue

    local total
    total=$(jq -r '.expansion.total // (.expansion.contains | length) // 0' "$BODY_FILE")
    if (( total < min_total )); then
      _record fail expand "$name" "expected at least $min_total results, got $total"
      continue
    fi

    local missing=()
    IFS=',' read -ra expected_codes <<< "$contains_csv"
    for code in "${expected_codes[@]}"; do
      [[ -z "$code" ]] && continue
      if ! _expand_codes | grep -qx "$code"; then
        missing+=("$code")
      fi
    done
    if (( ${#missing[@]} > 0 )); then
      _record fail expand "$name" "missing expected code(s): ${missing[*]}"
    else
      _record pass expand "$name" "total=$total"
    fi
  done < "$file"
}

run_subsumes() {
  local file="${FIXTURE_DIR}/subsumes.tsv"
  while IFS=$'\t' read -r name code_a code_b expected; do
    _skip_header "$name" && continue
    _get "/CodeSystem/\$subsumes?system=http://snomed.info/sct&codeA=${code_a}&codeB=${code_b}"
    _require_200_resource subsumes "$name" "Parameters" || continue
    local actual
    actual=$(_parameter_string outcome)
    if [[ "$actual" == "$expected" ]]; then
      _record pass subsumes "$name" "$actual"
    else
      _record fail subsumes "$name" "expected $expected, got ${actual:-empty}"
    fi
  done < "$file"
}

run_valueset_validate() {
  local file="${FIXTURE_DIR}/valueset-validate.tsv"
  while IFS=$'\t' read -r name valueset_url code expected; do
    _skip_header "$name" && continue
    _get "/ValueSet/\$validate-code?url=$(_urlencode "$valueset_url")&system=http://snomed.info/sct&code=${code}"
    _require_200_resource valueset-validate "$name" "Parameters" || continue
    local actual
    actual=$(_parameter_string result)
    if [[ "$actual" == "$expected" ]]; then
      _record pass valueset-validate "$name" "result=$actual"
    else
      _record fail valueset-validate "$name" "expected result=$expected, got ${actual:-empty}"
    fi
  done < "$file"
}

run_translate() {
  if ! [[ -f "$CAPS_FILE" ]] || ! _has_operation translate; then
    local msg="ConceptMap/\$translate not advertised"
    if $STRICT; then
      _record fail translate "capability" "$msg"
    else
      _record skip translate "capability" "$msg"
    fi
    return
  fi

  local file="${FIXTURE_DIR}/translate.tsv"
  while IFS=$'\t' read -r name system code targetsystem expected_result expected_code; do
    _skip_header "$name" && continue
    expected_code=$(_field "$expected_code")
    _get "/ConceptMap/\$translate?system=$(_urlencode "$system")&code=$(_urlencode "$code")&targetsystem=$(_urlencode "$targetsystem")"
    _require_200_resource translate "$name" "Parameters" || continue
    local result
    result=$(_parameter_string result)
    if [[ "$result" != "$expected_result" ]]; then
      _record fail translate "$name" "expected result=$expected_result, got ${result:-empty}"
      continue
    fi
    if [[ -n "$expected_code" ]]; then
      # Collect all match codes into an array and test membership with `any`.
      # A bare per-match `.valueCoding.code == $code` makes `jq -e` read only the
      # *last* emitted boolean, so a present-but-not-last target (e.g. a reverse
      # ICD-10 -> SNOMED translate returns many matches) is wrongly reported
      # missing. See issue #30.
      if ! jq -e --arg code "$expected_code" '
        [ .parameter[]?
          | select(.name == "match")
          | .part[]?
          | select(.name == "concept")
          | .valueCoding.code ]
        | any(. == $code)
      ' "$BODY_FILE" >/dev/null; then
        _record fail translate "$name" "missing target code $expected_code"
        continue
      fi
    fi
    _record pass translate "$name" "result=$result"
  done < "$file"
}

run_errors() {
  local file="${FIXTURE_DIR}/errors.tsv"
  while IFS=$'\t' read -r name path expected_status expected_resource; do
    _skip_header "$name" && continue
    _get "$path"
    local status rt
    status=$(_status)
    rt=$(_body_resource_type)
    if [[ "|$expected_status|" != *"|$status|"* ]]; then
      _record fail errors "$name" "expected HTTP $expected_status, got $status"
      continue
    fi
    if [[ -n "$expected_resource" && "$rt" != "$expected_resource" ]]; then
      _record fail errors "$name" "expected $expected_resource, got ${rt:-empty}"
      continue
    fi
    _record pass errors "$name" "HTTP $status"
  done < "$file"
}

if [[ "$FORMAT" == "table" ]]; then
  printf 'sct FHIR conformance - %s\n' "$SERVER"
  printf 'fixtures: %s\n\n' "$FIXTURE_DIR"
else
  printf 'sct FHIR conformance - %s\n' "$SERVER" >&2
  printf 'fixtures: %s\n\n' "$FIXTURE_DIR" >&2
fi

# Always run metadata first so optional operation decisions use CapabilityStatement.
run_metadata

IFS=',' read -ra requested_ops <<< "$OPERATIONS"
for op in "${requested_ops[@]}"; do
  op="${op// /}"
  [[ "$op" == "metadata" || -z "$op" ]] && continue
  case "$op" in
    lookup)             run_lookup ;;
    validate-code)      run_validate_code ;;
    expand)             run_expand ;;
    subsumes)           run_subsumes ;;
    valueset-validate)  run_valueset_validate ;;
    translate)          run_translate ;;
    errors)             run_errors ;;
    *)                  _record fail harness "$op" "unknown operation" ;;
  esac
done

if [[ "$FORMAT" == "table" ]]; then
  printf '\nsummary: %s passed, %s failed, %s skipped\n' "$pass_count" "$fail_count" "$skip_count"
else
  printf 'summary: %s passed, %s failed, %s skipped\n' "$pass_count" "$fail_count" "$skip_count" >&2
fi

if [[ -n "$OUTPUT_FILE" ]]; then
  cp "$RESULTS_JSONL" "$OUTPUT_FILE"
  if [[ "$FORMAT" == "table" ]]; then
    printf 'wrote JSONL results to %s\n' "$OUTPUT_FILE"
  else
    printf 'wrote JSONL results to %s\n' "$OUTPUT_FILE" >&2
  fi
fi

if [[ "$FORMAT" == "jsonl" ]]; then
  cat "$RESULTS_JSONL"
fi

(( fail_count == 0 ))
