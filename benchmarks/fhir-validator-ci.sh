#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Independent FHIR R4 structural validation gate for CI and local use.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

SCT="${SCT_BIN:-target/debug/sct}"
PORT="${FHIR_VALIDATOR_PORT:-8198}"
FIXTURES="benchmarks/fixtures/fhir-validator-ci"
OUTPUT_DIR="${FHIR_VALIDATOR_OUTPUT_DIR:-target/fhir-validator}"
DOWNLOAD_CACHE="${FHIR_VALIDATOR_DOWNLOAD_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/sct/fhir-validator}"

for command in curl java jq tar awk sort diff; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'error: missing required tool: %s\n' "$command" >&2
    exit 1
  }
done

if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
else
  printf 'error: missing required tool: sha256sum or shasum\n' >&2
  exit 1
fi

[[ -x "$SCT" ]] || {
  printf "error: sct binary not found at '%s' (build it, or set SCT_BIN)\n" "$SCT" >&2
  exit 1
}

mkdir -p "$OUTPUT_DIR" "$DOWNLOAD_CACHE"
OUTPUT_DIR=$(cd "$OUTPUT_DIR" && pwd)
DOWNLOAD_CACHE=$(cd "$DOWNLOAD_CACHE" && pwd)
rm -rf "$OUTPUT_DIR/responses"
rm -f "$OUTPUT_DIR/validator.log" "$OUTPUT_DIR/serve.log" \
  "$OUTPUT_DIR/warnings.actual.tsv" "$OUTPUT_DIR/warnings.expected.tsv" \
  "$OUTPUT_DIR/warnings.diff" "$OUTPUT_DIR/errors.actual.tsv" \
  "$OUTPUT_DIR/downloads.tsv" "$OUTPUT_DIR/responses.tsv" \
  "$OUTPUT_DIR/run-manifest.json"
mkdir -p "$OUTPUT_DIR/responses"

tmp=$(mktemp -d)
serve_pid=""
cleanup() {
  [[ -n "$serve_pid" ]] && kill "$serve_pid" 2>/dev/null || true
  rm -rf "$tmp"
}
trap cleanup EXIT

package_cache="$tmp/home/.fhir/packages"
mkdir -p "$package_cache"
validator_jar=""
validator_sha=""
package_count=0

printf 'preparing checksum-pinned FHIR Validator inputs ...\n'
while IFS=$'\t' read -r kind identifier filename url expected_sha; do
  [[ -z "$kind" || "${kind:0:1}" == "#" ]] && continue
  [[ "$kind" == "jar" || "$kind" == "package" ]] || {
    printf "error: unsupported download kind '%s'\n" "$kind" >&2
    exit 1
  }
  [[ "$filename" =~ ^[A-Za-z0-9._-]+$ ]] || {
    printf "error: unsafe download filename '%s'\n" "$filename" >&2
    exit 1
  }
  [[ "$expected_sha" =~ ^[0-9a-f]{64}$ ]] || {
    printf "error: invalid SHA-256 for '%s'\n" "$identifier" >&2
    exit 1
  }

  archive="$DOWNLOAD_CACHE/$filename"
  if [[ ! -f "$archive" ]] || [[ "$(sha256 "$archive")" != "$expected_sha" ]]; then
    rm -f "$archive.part"
    curl --fail --location --silent --show-error "$url" --output "$archive.part"
    actual_sha=$(sha256 "$archive.part")
    [[ "$actual_sha" == "$expected_sha" ]] || {
      printf "error: checksum mismatch for '%s': expected %s, got %s\n" \
        "$identifier" "$expected_sha" "$actual_sha" >&2
      exit 1
    }
    mv "$archive.part" "$archive"
  fi
  actual_sha=$(sha256 "$archive")
  [[ "$actual_sha" == "$expected_sha" ]] || {
    printf "error: cached checksum mismatch for '%s'\n" "$identifier" >&2
    exit 1
  }

  if [[ "$kind" == "jar" ]]; then
    validator_jar="$archive"
    validator_sha="$actual_sha"
    continue
  fi

  [[ "$identifier" =~ ^[a-z0-9.-]+#[A-Za-z0-9.-]+$ ]] || {
    printf "error: unsafe FHIR package coordinate '%s'\n" "$identifier" >&2
    exit 1
  }
  package_name=${identifier%%#*}
  package_version=${identifier#*#}
  package_dir="$package_cache/$identifier"
  mkdir -p "$package_dir"
  tar -xzf "$archive" -C "$package_dir"
  package_json="$package_dir/package/package.json"
  [[ -f "$package_json" ]] || {
    printf "error: package '%s' has no package/package.json\n" "$identifier" >&2
    exit 1
  }
  jq -e --arg name "$package_name" --arg version "$package_version" \
    '.name == $name and .version == $version' "$package_json" >/dev/null || {
    printf "error: package identity does not match '%s'\n" "$identifier" >&2
    exit 1
  }
  ((package_count += 1))
done < "$FIXTURES/downloads.tsv"

[[ -n "$validator_jar" && "$package_count" -eq 8 ]] || {
  printf 'error: download manifest must contain one Validator JAR and eight packages\n' >&2
  exit 1
}

printf 'building the committed synthetic fixture ...\n'
"$SCT" ndjson --rf2 tests/fixtures/rf2 --output "$tmp/synthetic.ndjson" >/dev/null
"$SCT" sqlite --input "$tmp/synthetic.ndjson" --output "$tmp/synthetic.db" \
  --transitive-closure >/dev/null

base_url="http://127.0.0.1:$PORT/fhir"
if curl -fsS --max-time 1 "$base_url/metadata" >/dev/null 2>&1; then
  printf 'error: another FHIR server is already responding on %s\n' "$base_url" >&2
  exit 1
fi

printf 'starting sct serve on 127.0.0.1:%s ...\n' "$PORT"
"$SCT" serve --db "$tmp/synthetic.db" --host 127.0.0.1 --port "$PORT" \
  --fhir-base /fhir >"$OUTPUT_DIR/serve.log" 2>&1 &
serve_pid=$!

for attempt in $(seq 1 30); do
  if ! kill -0 "$serve_pid" 2>/dev/null; then
    wait "$serve_pid" 2>/dev/null || true
    serve_pid=""
    printf 'error: sct serve exited before becoming healthy; see %s/serve.log\n' "$OUTPUT_DIR" >&2
    exit 1
  fi
  if curl -fsS "$base_url/metadata" >/dev/null 2>&1; then
    break
  fi
  if [[ "$attempt" -eq 30 ]]; then
    printf 'error: sct serve did not become healthy; see %s/serve.log\n' "$OUTPUT_DIR" >&2
    exit 1
  fi
  sleep 1
done

printf 'capturing the declared response matrix ...\n'
response_count=0
while IFS=$'\t' read -r filename path expected_resource; do
  [[ -z "$filename" || "${filename:0:1}" == "#" ]] && continue
  kill -0 "$serve_pid" 2>/dev/null || {
    printf 'error: sct serve exited during response capture; see %s/serve.log\n' "$OUTPUT_DIR" >&2
    exit 1
  }
  [[ "$filename" =~ ^[0-9]{2}-[a-z0-9-]+\.json$ ]] || {
    printf "error: unsafe response filename '%s'\n" "$filename" >&2
    exit 1
  }
  response="$OUTPUT_DIR/responses/$filename"
  curl --fail --silent --show-error --max-time 30 \
    -H 'Accept: application/fhir+json' "$base_url$path" --output "$response"
  jq -e --arg expected "$expected_resource" '.resourceType == $expected' \
    "$response" >/dev/null || {
    printf "error: response '%s' is not a %s\n" "$filename" "$expected_resource" >&2
    exit 1
  }
  ((response_count += 1))
done < "$FIXTURES/responses.tsv"

[[ "$response_count" -eq 8 ]] || {
  printf 'error: response manifest must contain exactly eight resources\n' >&2
  exit 1
}

kill "$serve_pid" 2>/dev/null || true
wait "$serve_pid" 2>/dev/null || true
serve_pid=""

cp "$FIXTURES/downloads.tsv" "$OUTPUT_DIR/downloads.tsv"
cp "$FIXTURES/responses.tsv" "$OUTPUT_DIR/responses.tsv"
cp "$FIXTURES/warnings.tsv" "$OUTPUT_DIR/warnings.expected.tsv"

sct_version=$("$SCT" --version)
git_sha=$(git rev-parse HEAD)
jq -n \
  --arg git_sha "$git_sha" \
  --arg sct_version "$sct_version" \
  --arg validator_sha256 "$validator_sha" \
  --argjson response_count "$response_count" \
  --argjson package_count "$package_count" \
  '{
    schema_version: 1,
    git_sha: $git_sha,
    sct_version: $sct_version,
    validator: {version: "6.10.4", sha256: $validator_sha256},
    fhir_version: "4.0.1",
    terminology_server: null,
    http_access: false,
    response_count: $response_count,
    package_count: $package_count
  }' > "$OUTPUT_DIR/run-manifest.json"

printf 'validating captured resources with FHIR Validator 6.10.4 ...\n'
responses=("$OUTPUT_DIR/responses/"*.json)
set +e
java -Duser.home="$tmp/home" -Duser.language=en -Duser.country=GB \
  -jar "$validator_jar" "${responses[@]}" \
  -version 4.0.1 \
  -ig 'hl7.terminology#7.4.0' \
  -ig 'hl7.fhir.uv.extensions#5.3.0' \
  -tx n/a \
  -txCache n/a \
  -no-http-access \
  -best-practice warning \
  -show-message-ids \
  -output-style compact 2>&1 | tee "$OUTPUT_DIR/validator.log"
validator_status=${PIPESTATUS[0]}
set -e

extract_messages() {
  local severity_pattern="$1"
  awk -v severity_pattern="$severity_pattern" '
    /\/responses\/[^ ]+\.json [0-9][0-9]:/ {
      current = $0
      sub(/^.*\/responses\//, "", current)
      sub(/ [0-9][0-9]:.*$/, "", current)
      next
    }
    $0 ~ "^\\[[^]]+\\] .*: (" severity_pattern ") - " {
      line = $0
      sub(/^\[[^]]+\] /, "", line)
      print current "\t" line
    }
  ' "$OUTPUT_DIR/validator.log" | LC_ALL=C sort
}

{
  awk 'NR <= 3 { print }' "$FIXTURES/warnings.tsv"
  extract_messages 'Warning'
} > "$OUTPUT_DIR/warnings.actual.tsv"
extract_messages 'Error|Fatal' > "$OUTPUT_DIR/errors.actual.tsv"

if [[ "$validator_status" -ne 0 ]]; then
  printf 'error: FHIR Validator exited with status %s\n' "$validator_status" >&2
  exit 1
fi
if [[ -s "$OUTPUT_DIR/errors.actual.tsv" ]]; then
  printf 'error: FHIR Validator reported structural errors; see %s/errors.actual.tsv\n' \
    "$OUTPUT_DIR" >&2
  exit 1
fi
if ! diff -u "$OUTPUT_DIR/warnings.expected.tsv" "$OUTPUT_DIR/warnings.actual.tsv" \
  | tee "$OUTPUT_DIR/warnings.diff"; then
  printf 'error: FHIR Validator warning baseline changed\n' >&2
  exit 1
fi

printf 'FHIR Validator gate passed: %s resources, zero errors, warning baseline unchanged\n' \
  "$response_count"
printf 'evidence: %s\n' "$OUTPUT_DIR"
