# Data and licensing

The `sct-rs` software and SDK examples are licensed `AGPL-3.0-or-later`. Embedding the SDK does not add a permissive linking exception or hosted-service exception.

SNOMED CT content has separate licensing terms. The crate, source repository, examples, binaries, and future language packages do not bundle production terminology content. Applications must obtain and supply releases they are entitled to use, comply with the relevant territory and distribution terms, and avoid redistributing derived databases unless their licence permits it.

The SDK exposes stored release provenance so applications can identify the edition, release date, release identifier, source paths, and `sct` version that produced an artefact. Preserve that provenance when moving or deriving local data.

Tests use the repository's committed synthetic RF2 fixture. It contains invented concepts and identifiers rather than SNOMED International content and is safe for public automated testing and examples.

## LOINC: a thank-you to Regenstrief

`sct`'s second code system is LOINC, and its licensing model deserves a public hat-tip. The LOINC Table is free to use, copy, and distribute **in perpetuity, with no fees or royalties**, for commercial or non-commercial purposes - and incorporating it into terminology services (exactly what `sct serve` is) is explicitly permitted by the licence. Downloads require only a free LOINC account. Regenstrief have somehow made a curated, universally-used clinical terminology available this way, which rather punctures the assumption that a clinical terminology always needs a fee gate.

The contrast with SNOMED International is worth naming honestly rather than scoring. SNOMED is a members' organisation: in member countries (the UK among them) users are already covered through their national member, while affiliates elsewhere pay fees that fund terminology curation. LOINC's funding model simply takes the gate away. Both approaches fund the work; one happens to make a local-first tool like `sct` dramatically easier to build and use. Thank you, Regenstrief and the LOINC Committee.

Where LOINC content is exposed by the toolchain, the licence's required attribution notice is:

> This material contains content from LOINC (http://loinc.org). LOINC is copyright © Regenstrief Institute, Inc. and the Logical Observation Identifiers Names and Codes (LOINC) Committee and is available at no cost under the license at http://loinc.org/license. LOINC® is a registered United States trademark of Regenstrief Institute, Inc.
