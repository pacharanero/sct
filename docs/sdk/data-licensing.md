# Data and licensing

The `sct-rs` software and SDK examples are licensed `AGPL-3.0-or-later`. Embedding the SDK does not add a permissive linking exception or hosted-service exception.

SNOMED CT content has separate licensing terms. The crate, source repository, examples, binaries, and future language packages do not bundle production terminology content. Applications must obtain and supply releases they are entitled to use, comply with the relevant territory and distribution terms, and avoid redistributing derived databases unless their licence permits it.

The SDK exposes stored release provenance so applications can identify the edition, release date, release identifier, source paths, and `sct` version that produced an artefact. Preserve that provenance when moving or deriving local data.

Tests use the repository's committed synthetic RF2 fixture. It contains invented concepts and identifiers rather than SNOMED International content and is safe for public automated testing and examples.
