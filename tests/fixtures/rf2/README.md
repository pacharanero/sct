# Synthetic RF2 fixture

A tiny, **fully synthetic, licence-free** SNOMED CT RF2 Snapshot used by the end-to-end tests (`tests/end_to_end.rs`). It is **not** real SNOMED CT content: the clinical concepts and all descriptions are hand-made. Only the structural ids (root, attribute types, language reference sets, the CTV3 map refset) reuse the real metadata SCTIDs so the pipeline behaves realistically.

- `generate.py` - the generator. It documents the little ontology (see its docstring) and writes the `.txt` files. Regenerate with `python3 generate.py` after editing.
- `SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z/Snapshot/` - the RF2 snapshot itself (Concept, Description, Relationship, Language, Simple Map, Simple, Extended Map, Complex Map, Attribute Value, and Association files).

One fixture exercises every feature: active filtering, FSN/PT/synonyms, semantic tags, GB vs US dialect preferred terms, the IS-A hierarchy (descendants / ancestors / children / TCT), typed attribute relationships with groups (ECL refinement), simple refset membership (ECL `^` and `sct refset`), CTV3 reverse maps (`sct lookup`), classified and unclassified Extended Maps, Complex Maps, Attribute Value inactivation indicators, and Association history.

Because it is committed and licence-free, the full `ndjson → sqlite → query` pipeline is tested in CI without needing a licensed SNOMED release.
