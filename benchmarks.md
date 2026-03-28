# benchmarks

> last updated: 2026-03-28

## results

| operation | sct (local) | ± | not measured | ± | speedup |
|:---|---:|---:|---:|---:|:---|
| concept lookup | 2 ms | ±0 ms | — | — | — |
| text search (top 10) | 2 ms | ±0 ms | — | — | — |
| direct children | 2 ms | ±0 ms | — | — | — |
| ancestor chain (depth ~12) | 2 ms | ±0 ms | — | — | — |
| subsumption test | 2 ms | ±0 ms | — | — | — |
| bulk lookup (15 concepts) | 2 ms | ±0 ms | — | — | — |
| **total** | **12 ms** | | **—** | | — |

## environment

| | |
|:---|:---|
| sct version | sct 0.2.0 |
| snomed version | 20260311 |
| concept count | 831,132 |
| sqlite3 version | 3.51.2 |
| os | Linux 6.17.0-14-generic |

_times are wall-clock median; local times include sqlite3 process startup._
