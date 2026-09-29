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

Use a database built with `sct ndjson --include-inactive --refsets all` followed by `sct sqlite`. Without `--include-inactive`, retired concepts are absent.

Without `--refsets all` the command **refuses to run**, because the Association reference sets it reports were never ingested:

```text
$ sct history 9468002 --db simple.db
Error: sct history needs historical association data, which this database did not load
(its Association reference sets were never ingested). Rebuild with `sct ndjson --refsets all`
then `sct sqlite`.
```

This is deliberate. An inactive concept printed with no replacements reads as "no replacement exists", and an empty `historical_associations` array cannot be distinguished by a script from a genuinely unassociated concept. A build whose Association files were present but empty is valid and simply reports no associations. If you only need a concept's active status, [`sct lookup`](lookup.md) works against any build.

## Structured output

`--format json` and `--format yaml` output `id`, `preferred_term`, `active`, `effective_time`, `inactivation_reason`, and `historical_associations`. Active concepts are returned with `active: true`, a `null` reason, and an empty association list, so scripts can distinguish a known active code from a missing one.
