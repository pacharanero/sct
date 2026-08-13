# sct history

Show a concept's current historical status in the loaded SNOMED CT Snapshot: whether it is active, its latest effective time, its inactivation reason, and all active RF2 historical associations to related or replacement concepts.

**When to use:** you need a focused retirement/replacement view for an identifier from an old record. For the complete current concept record, use [`sct lookup`](lookup.md). A chronological lifecycle, including a concept's birth date and every prior state, requires Full RF2 and is planned in `R25`.

## Usage

```
sct history <SCTID> [--db <FILE>] [-f text|json|yaml]
```

## Example

```text
$ sct history 9468002 --db snomed.db
  [9468002] Inactive example disorder
  INACTIVE - Duplicate
    Replaced by: [22298006] Myocardial infarction
    Same as: [195967001] Asthma
  Snapshot effective: 20260101
  Note: chronological history requires Full RF2 (R25).
```

Use a database built with `sct ndjson --include-inactive --refsets all` followed by `sct sqlite`. Without `--include-inactive`, retired concepts are absent. Without `--refsets all`, the concept's inactive status remains available but its reason and associations are unavailable.

## Structured output

`--format json` and `--format yaml` output `id`, `preferred_term`, `active`, `effective_time`, `inactivation_reason`, and `historical_associations`. Active concepts are returned with `active: true`, a `null` reason, and an empty association list, so scripts can distinguish a known active code from a missing one.
