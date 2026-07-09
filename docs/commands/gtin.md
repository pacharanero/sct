# GTIN Codes Support

`sct` supports Global Trade Item Numbers (GTINs) for barcodes of actual medicinal packages. This enables clinical applications and devices to map scanned barcodes directly to the corresponding SNOMED CT concept ID (e.g. Actual Medicinal Product Packs/AMPPs).

---

## Schema Changes

In `ConceptRecord` (schema version `6` and above), a new array field is populated:
```json
{
  "id": "999000011000001104",
  "preferred_term": "Virtual Medicinal Product",
  "gtin_codes": [
    "5012345678901",
    "5012345678902"
  ],
  "schema_version": 6
}
```

In the SQLite database, this is represented by:
*   A `gtin_codes` text column in the `concepts` table storing a JSON array of barcode strings.
*   Cross-references in both the `concept_maps` and `crossmaps` tables under the `"gtin"` terminology mapping namespace.

---

## Querying GTIN Mappings

### 1. Concept Lookup
You can lookup a concept directly using a GTIN barcode via the `sct lookup` command. If the barcode is found in the crossmaps, `sct` returns the full concept record:
```bash
sct lookup 5012345678901
```

### 2. Terminology Mapping
You can translate barcodes to SNOMED concepts (or retrieve all barcode codes associated with a SNOMED ID) using `sct map`:

*   **Convert a barcode to its SNOMED concept ID**:
    ```bash
    sct map gtin 5012345678901
    ```
*   **List all barcode GTINs for a given concept**:
    ```bash
    sct map snomed 73211009
    ```

### 3. MCP Server Integration
The MCP server includes support for the `"gtin"` terminology in its `snomed_map` tool. Connected agents/LLMs can automatically perform forward and reverse barcode mappings.
