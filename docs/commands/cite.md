# sct cite

Generate citations for the terminology source, the exact local derived artefact, and the `sct` version that produced it.

`sct cite` reads provenance from the SQLite database you actually queried. It works offline, opens the database query-only, and does not build artefacts, indexes, caches, or network requests. SQLite may create its own `-wal`/`-shm` coordination sidecars to provide a concurrency-safe snapshot of a WAL-mode database; the logical database is not modified.

---

## Usage

```text
sct cite [--db <PATH>] [--format text|bibtex|csljson]
```

`--db` follows the normal [database discovery order](../path-resolution.md) when omitted. `--format` (`-f`) defaults to `text`. CSL-JSON also accepts `csl-json` and `json`; this JSON is a standard bibliography array, not a diagnostic envelope.

```bash
sct cite --db snomed.db
sct cite --db snomed.db --format bibtex > references.bib
sct cite --db snomed.db --format csljson > references.json
```

---

## What is cited

Every format derives from one validated model and keeps three identities separate:

- **Terminology source:** recorded edition, release identifier and release date, plus a publisher/access mapping only for known International and UK editions. These values were inferred during ingestion from RF2 input naming; they are not independent publisher verification.
- **Local derived artefact:** the canonical concept fingerprint and every recorded companion stream's kind, schema version, record count, and fingerprint. These digests identify length-delimited canonical records, not the original RF2 archive or the SQLite file.
- **Software:** `sct`, its repository, the recorded producing version, and the currently running reader version. The reader version never substitutes for missing producer provenance.

Text output adds a separate data-availability statement. BibTeX emits three stable `@misc` entries. CSL-JSON emits three entries with the types `dataset`, `dataset`, and `software`.

---

## Evidence limits

The command does not invent a DOI, public artefact URL, publication status, archive checksum, or software publication date. The local database remains local, and source access is governed by the applicable SNOMED CT licence and publisher requirements.

Filesystem source paths are never exported. A composite build is disclosed without exposing those paths because current provenance identifies only its primary release, not an independently verified manifest of every constituent source.

Partial provenance is rendered with explicit missing-evidence notes. A legacy database with no usable provenance fails with rebuild guidance. Malformed fingerprints, dates, timestamps, collection metadata, duplicate companion kinds, or a release date that contradicts the release identifier fail rather than producing a misleading citation.

Stored fingerprints identify the content recorded during the build. Reading them does not independently reverify every current SQLite row.

---

## Software citation

The repository's [`CITATION.cff`](https://github.com/pacharanero/sct/blob/main/CITATION.cff) describes the `sct` software release. It complements the database-specific output from `sct cite`; it does not identify the terminology release or local artefact used in an analysis.

The `sct cite` command was inspired by Oli Evans's [`ods`](https://github.com/olizilla/ods), particularly its separation of source, derived-data, and software citations.

---

## See also

- [`sct info`](info.md) - inspect artefact structure and row counts
- [`sct ndjson`](ndjson.md) - create the canonical provenance-bearing artefact
- [`sct sqlite`](sqlite.md) - derive the citation-bearing SQLite database
