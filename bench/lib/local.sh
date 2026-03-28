#!/usr/bin/env bash
# lib/local.sh — sqlite3 wrappers for each benchmark operation.
#
# Each function times BENCH_RUNS executions (+ BENCH_WARMUP warmup) of the
# relevant SQLite query. Sets TIMING_MEDIAN / TIMING_STDDEV globals and
# echoes the median ms to stdout.
#
# Requires: BENCH_DB  (path to snomed.db)
#           time_cmd  (from lib/timing.sh, already sourced by bench.sh)

# local_time_lookup CODE
local_time_lookup() {
  local code="$1"
  local sql="SELECT id, preferred_term, fsn, hierarchy
             FROM concepts WHERE id = '${code}' LIMIT 1"
  time_cmd "$BENCH_RUNS" "$BENCH_WARMUP" sqlite3 "$BENCH_DB" "$sql"
}

# local_time_search TERM [LIMIT]
local_time_search() {
  local term="$1" limit="${2:-10}"
  # Escape any single quotes in the search term (safety for controlled fixtures)
  local escaped_term="${term//\'/\'\'}"
  local sql="SELECT id, preferred_term
             FROM concepts_fts
             WHERE concepts_fts MATCH '${escaped_term}'
             LIMIT ${limit}"
  time_cmd "$BENCH_RUNS" "$BENCH_WARMUP" sqlite3 "$BENCH_DB" "$sql"
}

# local_time_children PARENT_ID
local_time_children() {
  local parent="$1"
  local sql="SELECT c.id, c.preferred_term
             FROM concept_isa ci
             JOIN concepts c ON ci.child_id = c.id
             WHERE ci.parent_id = '${parent}'"
  time_cmd "$BENCH_RUNS" "$BENCH_WARMUP" sqlite3 "$BENCH_DB" "$sql"
}

# local_time_ancestors CODE
# Uses a recursive CTE to walk the full IS-A chain to root in a single query.
local_time_ancestors() {
  local code="$1"
  local sql="WITH RECURSIVE anc(id) AS (
               SELECT parent_id FROM concept_isa WHERE child_id='${code}'
               UNION ALL
               SELECT ci.parent_id FROM concept_isa ci
               JOIN anc ON ci.child_id = anc.id
             )
             SELECT c.id, c.preferred_term
             FROM concepts c
             WHERE c.id IN (SELECT id FROM anc)"
  time_cmd "$BENCH_RUNS" "$BENCH_WARMUP" sqlite3 "$BENCH_DB" "$sql"
}

# local_time_subsumes CHILD_ID PARENT_ID
# Returns 1 if CHILD is subsumed by PARENT (i.e. PARENT is an ancestor of CHILD).
local_time_subsumes() {
  local child="$1" parent="$2"
  local sql="WITH RECURSIVE anc(id) AS (
               SELECT parent_id FROM concept_isa WHERE child_id='${child}'
               UNION ALL
               SELECT ci.parent_id FROM concept_isa ci
               JOIN anc ON ci.child_id = anc.id
             )
             SELECT CASE WHEN EXISTS(SELECT 1 FROM anc WHERE id='${parent}')
                    THEN 1 ELSE 0 END"
  time_cmd "$BENCH_RUNS" "$BENCH_WARMUP" sqlite3 "$BENCH_DB" "$sql"
}

# local_time_bulk ID1 ID2 ...
# Resolves all given SCTIDs in a single IN-clause query.
local_time_bulk() {
  local ids_csv
  ids_csv=$(printf "'%s'," "$@")
  ids_csv="${ids_csv%,}"
  local sql="SELECT id, preferred_term, fsn
             FROM concepts
             WHERE id IN (${ids_csv})"
  time_cmd "$BENCH_RUNS" "$BENCH_WARMUP" sqlite3 "$BENCH_DB" "$sql"
}

# local_concept_depth CODE
# Returns the depth of a concept in the IS-A hierarchy (number of ancestors).
local_concept_depth() {
  local code="$1"
  sqlite3 "$BENCH_DB" \
    "WITH RECURSIVE anc(id) AS (
       SELECT parent_id FROM concept_isa WHERE child_id='${code}'
       UNION ALL
       SELECT ci.parent_id FROM concept_isa ci
       JOIN anc ON ci.child_id = anc.id
     ) SELECT COUNT(*) FROM anc" 2>/dev/null
}

# local_snomed_info
# Sets SNOMED_VERSION and SNOMED_CONCEPT_COUNT globals from the database.
local_snomed_info() {
  SNOMED_CONCEPT_COUNT=$(sqlite3 "$BENCH_DB" \
    "SELECT COUNT(*) FROM concepts WHERE active=1" 2>/dev/null || echo "?")
  # Derive version from the max effective_time stored in the DB
  SNOMED_VERSION=$(sqlite3 "$BENCH_DB" \
    "SELECT MAX(effective_time) FROM concepts" 2>/dev/null || echo "?")
}
