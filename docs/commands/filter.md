# sct filter

Filter a SNOMED CT NDJSON file to keep a subset of concepts and automatically remap associated barcodes (GTINs) from filtered-out concepts to their nearest kept ancestor in the taxonomy.

This is a powerful tool to generate highly optimized, minimal databases (e.g. for specific drug lists or localized departments) without losing the ability to map scanned pack barcodes to their corresponding drug classes.

---

## Usage

```
sct filter --input <INPUT> --output <OUTPUT> [OPTIONS]
```

### Options

| Flag | Type | Description |
|---|---|---|
| `-i, --input` | `Path` | **Required.** Path to the original full SNOMED CT NDJSON file. |
| `-o, --output` | `Path` | **Required.** Path to write the filtered NDJSON file. |
| `--keep-ids` | `Path` | Path to a text file containing concept IDs to keep, one per line (optional). |
| `--keep-ecl` | `String` | ECL query (e.g. `<< 999000011000001104` to select VMP/VTM concepts) to keep (optional). |
| `--db` | `Path` | Path to a SQLite database. Required if using `--keep-ecl` (optional). |
| `--gtin-map` | `Path` | Path to a CSV/TSV file mapping GTIN barcodes to SNOMED pack concept IDs (optional). |

---

## How It Works

### Concept Filtering
Only concepts specified in the `--keep-ids` file and/or matching the `--keep-ecl` query are retained in the output NDJSON. All other concept records are discarded. 

### GTIN Remapping
If a `--gtin-map` file (mapping GTIN barcodes to concept IDs, e.g. Actual Medicinal Product Packs / AMPPs) is supplied:
1. `sct filter` parses the original NDJSON to understand parent-child relationships across the complete SNOMED hierarchy.
2. For each GTIN mapping:
   - If the mapped concept is retained in the kept set, the barcode remains associated with it.
   - If the mapped concept is filtered out, the tool traverses up the hierarchy (parents/ancestors) to find the closest kept concept (e.g. the Virtual Medicinal Product / VMP).
   - The GTIN is then appended to that kept concept's `gtin_codes` list.
3. The resulting database lets you query the barcodes directly, resolving them to the appropriate higher-level kept concept!

---

## Examples

### 1. Filtering with a List of Concept IDs
Keep only concepts listed in `keep_list.txt`:
```bash
sct filter --input snomed.ndjson --output filtered.ndjson --keep-ids keep_list.txt
```

### 2. Filtering with ECL
Keep only concepts matching an ECL expression (e.g., all descendants of Virtual Medicinal Product):
```bash
sct filter --input snomed.ndjson --output filtered.ndjson --keep-ecl "< 999000011000001104" --db snomed.db
```

### 3. Filtering and Remapping GTINs
Remap GTIN barcodes from a CSV mapping file (`gtin_map.csv`) to kept ancestor concepts:
```bash
sct filter --input snomed.ndjson --output filtered.ndjson --keep-ids keep_list.txt --gtin-map gtin_map.csv
```

### 4. Loading the Filtered Database
Since `sct filter` outputs standard NDJSON, you can feed it directly to the existing `sqlite` command to generate your filtered database:
```bash
sct sqlite --input filtered.ndjson --output filtered.db
```

---

## CLI Output Example

When running `sct filter`, the CLI outputs a space-savings summary indicating exactly how many concepts were pruned and how much disk space was saved:

```
Reading input NDJSON...
Loading and remapping GTINs...
Mapped 42,912 of 45,103 GTINs to kept concepts.
Filtering and writing output NDJSON...

Database Filtering Summary
==========================
Concepts kept:     41,209 / 837,930 (4.9%)
Initial file size:  1.2 GB
Filtered file size: 68.2 MB
Space saved:        1.1 GB (94.3%)
```
