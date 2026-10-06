# `sct cite` - cite the source, derived artefact and software

Status: proposed, 2026-10-01. Delivery is tracked by `R96` in [the roadmap](roadmap.md); provenance round-trip repair is tracked separately by `R97`.

## Motivation and attribution

A researcher should be able to turn the database they actually queried into a useful citation and a precise data-availability statement. A citation to the tool alone does not identify the terminology release or the local derived data used in an analysis.

This design is inspired by Oli Evans's [`ods`](https://github.com/olizilla/ods), reviewed at commit `7f95000`, particularly [`ods cite`](https://github.com/olizilla/ods/blob/7f95000/src/commands/cite.rs). Its key idea is to distinguish three things: the upstream source, the derived data artefact, and the software that produced it. Make that distinction part of normal CLI use rather than leaving provenance hidden in metadata.

`ods cite` reads embedded provenance from the actual files, reconstructs a manifest identity, distinguishes published datasets from local or mismatching builds, supplies publication links only when supported by its release index, and reports withdrawals and known source issues. It works offline. Borrow the evidence discipline and user experience; `sct` does not currently have an equivalent public dataset index or publication service.

## Current foundation

`src/provenance.rs` already records edition label, primary release identifier/date, builder version, canonical concept-content fingerprint, companion fingerprints, original input paths and build time. SQLite metadata and canonical NDJSON carry provenance; derived formats consume it. Read commands already open SQLite read-only through the shared opener.

Important limits of that evidence:

- Source edition/date identification is currently best-effort, derived from input-path naming. An inferred label is not publisher verification.
- Composite builds primarily identify the first source; source paths alone are not a complete, verified manifest of constituent releases.
- The canonical concept fingerprint hashes length-delimited canonical record bytes. It is neither the original TRUD archive checksum nor a hash of the SQLite file, and companion streams have separate identities.
- The recorded `sct_version` is the producing version. The version currently printing a citation can differ.
- `to_arrow_metadata()` currently omits `companions`, and `from_arrow_metadata()` constructs an empty list, despite the helper's lossless-round-trip documentation. Confirm and repair this under `R97` before promising equivalent citation coverage for Arrow-derived artefacts.

## Proposed first surface

```sh
sct cite --db snomed.db
sct cite --db snomed.db --format bibtex
sct cite --db snomed.db --format csljson
```

Start with the existing SQLite provenance reader and normal `--db` discovery/tilde expansion. The command is offline and read-only: it creates no files, builds no indexes, updates no caches and fetches no bibliography metadata. Invocation without `--db` selects the same database as other read commands.

Use a typed citation-specific output enum: `text`, `bibtex`, `csljson`, with `csl-json` and `json` aliases for CSL-JSON. Document that JSON here is a standard CSL bibliography array rather than a general diagnostic envelope. Unknown formats fail during argument parsing, before database discovery. Follow [adding a command](adding-a-command.md), including help, completion and shared output-boundary rules.

APA can follow if there is a concrete need. Before adding it, decide how a bibliography-only renderer carries or accompanies exact artefact identity; do not promise digest coverage that the output omits.

## Citation model

Build one reusable citation model from the selected artefact's provenance, then render every format from it. Keep evidence collection separate from bibliography formatting so later SDK or export consumers can reuse it without invoking a CLI or reading global state.

| Component | Content | Evidence boundary |
|---|---|---|
| Terminology source | Edition, release identifier/date, publisher and applicable licence/reference | Use recorded facts and a documented publisher mapping only where justified; identify unknown or inferred values explicitly. |
| Local derived artefact | Canonical concept fingerprint, available companion fingerprints, and release context | Label each digest by what it identifies. Describe local data as local, without inventing a DOI, public download URL or published-dataset status. |
| Software | `sct` repository and recorded producer version | Distinguish the builder from the currently running reader; do not substitute today's version for missing historical provenance. |

The default text output should have separate source, local artefact, software and data-availability sections. BibTeX and CSL-JSON should preserve the same identity facts in suitable fields or notes, with stable distinct entry identifiers. Escape bibliography text for its actual target syntax; arbitrary metadata must not become BibTeX syntax or terminal control sequences.

The data-availability statement explains which local artefact was used and how access to its source is governed. SNOMED content remains subject to its applicable licence; this command does not publish or redistribute that content. Publisher/source links describe access, not proof that the selected local bytes are an official published dataset.

## Evidence and error behaviour

- Keep local filesystem paths out of exported citations and availability statements. They are not durable identifiers and can reveal private workstation information.
- Treat an absent source date, producer version or fingerprint as unavailable evidence, never fill it from the current clock, directory guess or running executable without an explicit label.
- For a legacy database with no usable provenance, fail with a clear explanation and guidance to rebuild from known inputs. For partial provenance, render only supportable facts and disclose the missing fields, including in machine-readable citation notes; do not imply exact artefact identification when its fingerprint is absent.
- Report malformed or contradictory metadata as an error. Distinguish an identity recorded in metadata from independently verified current content: merely reading a stored digest is not verification of every SQLite row.
- Cite composite releases exactly only when independently recorded metadata supplies their identities. Otherwise disclose that only the primary source is identified.
- Keep bibliographic results on stdout and diagnostics on stderr. Uncertainty needed to interpret a saved citation also belongs in its fields/notes, so redirecting stdout does not lose it.
- Do not assert upstream withdrawal/freshness checks without a mechanism that supplies those facts. A future local advisory manifest can be designed separately if required.

Software publication dates must come from software metadata when known, not from the terminology release year. Exact bibliography field choices and how partial entries are represented should be settled against the chosen formats during implementation.

## Delivery stages

1. **SQLite vertical slice (`R96`).** Read existing provenance, build the citation model, implement text/BibTeX/CSL-JSON, document limitations, add tests and attribution. This stage is useful without an OCI registry or a new provenance schema.
2. **Round-trip integrity (`R97`).** Preserve companion identity through supported Arrow metadata paths, with compatibility for older files and explicit handling of malformed companion metadata. This can ship independently of the command.
3. **Later evidence expansion.** Consider NDJSON/Parquet/Arrow inputs and richer constituent-source manifests once lossless evidence carriage is established. Keep any source-checksum capture or independently verified artefact mode explicit. Broader reproducibility work is tracked in [ods-inspired assurance](ods-inspired-assurance.md).

## Acceptance

- Use the committed synthetic RF2 pipeline to build a real database and assert that citation source/version/fingerprints agree with its recorded provenance.
- Exercise a recorded builder version different from the running tool; all formats cite the producer correctly and never infer its publication year from the data release.
- Exercise missing/partial/invalid metadata and composite-source limitations. Missing identity cannot silently become a claim of exact reproducibility.
- Check exact fingerprint values across every supported format, including companion identity when present. Parse CSL-JSON and validate BibTeX escaping with adversarial text.
- Assert that exported text contains no source paths, terminal escapes or invented DOI/publication claims.
- Verify read-only/no-network operation and normal redirected/broken-pipe behaviour. A citation must work after its database has been moved away from the original RF2 directory.
- Add root `CITATION.cff` for the software, keeping its metadata aligned with software citation output and the release process. It complements, rather than replaces, artefact-specific citations.

## Credit and licensing

Include this acknowledgement in the README or relevant user documentation when the command ships:

> The `sct cite` command was inspired by Oli Evans's [`ods`](https://github.com/olizilla/ods), particularly its separation of source, derived-data, and software citations.

If substantial code is copied or adapted, retain Oli Evans's copyright and MIT permission notice and add appropriate REUSE annotations. An acknowledgement is sufficient for crediting the design inspiration but does not replace licence notices for copied implementation. Keep software licensing distinct from the terminology's own rights and attribution.
