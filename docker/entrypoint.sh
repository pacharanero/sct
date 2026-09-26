#!/usr/bin/env sh
# SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
# SPDX-License-Identifier: AGPL-3.0-or-later

set -eu

if [ "${1:-serve}" != "serve" ]; then
    exec "$@"
fi

: "${SCT_DATA_HOME:=/data}"
: "${SCT_CODELISTS:=/codelists}"
: "${SCT_SERVE_HOST:=0.0.0.0}"
: "${SCT_SERVE_PORT:=8080}"
: "${SCT_FHIR_BASE:=/fhir}"
: "${SCT_TRUD_EDITION:=uk_monolith}"
: "${SCT_REFSETS:=all}"
: "${SCT_LOCALE:=en-GB}"
: "${SCT_BOOTSTRAP:=true}"

export SCT_DATA_HOME SCT_CODELISTS

find_db() {
    if [ ! -d "$SCT_DATA_HOME/data" ]; then
        return 0
    fi
    find "$SCT_DATA_HOME/data" -maxdepth 1 -type f -name '*.db' -printf '%T@ %p\n' 2>/dev/null \
        | sort -nr \
        | head -n 1 \
        | cut -d' ' -f2-
}

db="${SCT_DB:-}"
if [ -z "$db" ]; then
    db="$(find_db || true)"
fi

if [ -z "$db" ] && [ "$SCT_BOOTSTRAP" != "false" ] && [ "$SCT_BOOTSTRAP" != "0" ]; then
    if [ -z "${TRUD_API_KEY:-}" ]; then
        cat >&2 <<EOF
No SNOMED SQLite database found under $SCT_DATA_HOME/data and TRUD_API_KEY is not set.

Create a .env file next to compose.yaml:

  TRUD_API_KEY=your-trud-api-key

Then run:

  docker compose up --build

You must also be subscribed in TRUD to the configured edition:
  SCT_TRUD_EDITION=$SCT_TRUD_EDITION
EOF
        exit 1
    fi

    args="download --edition $SCT_TRUD_EDITION --skip-if-current --pipeline --refsets $SCT_REFSETS --locale $SCT_LOCALE"
    if [ "${SCT_INCLUDE_INACTIVE:-false}" = "true" ] || [ "${SCT_INCLUDE_INACTIVE:-false}" = "1" ]; then
        args="$args --include-inactive"
    fi

    echo "No database found; bootstrapping with: sct trud $args"
    # shellcheck disable=SC2086
    sct trud $args
    db="$(find_db || true)"
fi

if [ -z "$db" ]; then
    echo "No SNOMED SQLite database found. Set SCT_DB or enable bootstrap with TRUD_API_KEY." >&2
    exit 1
fi

echo "Starting sct serve with database: $db"
if [ -z "${SCT_PUBLIC_URL:-}" ] && [ -n "${DOMAIN:-}" ]; then
    public_base="$SCT_FHIR_BASE"
    while [ "${public_base%/}" != "$public_base" ]; do
        public_base="${public_base%/}"
    done
    case "$public_base" in
        ""|/*) ;;
        *) public_base="/$public_base" ;;
    esac
    SCT_PUBLIC_URL="https://${DOMAIN}${public_base}"
elif [ -z "${SCT_PUBLIC_URL:-}" ] && [ -n "${SCT_PUBLIC_URL_FALLBACK:-}" ]; then
    SCT_PUBLIC_URL="$SCT_PUBLIC_URL_FALLBACK"
fi

set -- sct serve \
    --db "$db" \
    --host "$SCT_SERVE_HOST" \
    --port "$SCT_SERVE_PORT" \
    --fhir-base "$SCT_FHIR_BASE" \
    --codelists "$SCT_CODELISTS"
if [ -n "${SCT_PUBLIC_URL:-}" ]; then
    set -- "$@" --public-url "$SCT_PUBLIC_URL"
fi
exec "$@"
