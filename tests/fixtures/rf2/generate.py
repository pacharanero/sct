#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
# SPDX-License-Identifier: AGPL-3.0-or-later

"""Generate a tiny, fully-synthetic SNOMED CT RF2 Snapshot for end-to-end tests.

This is NOT real SNOMED CT content. The structural ids (root, attribute types,
language reference sets, CTV3 map refset) are the real metadata SCTIDs so the
pipeline behaves realistically, but the clinical concepts and all descriptions
are hand-made and licence-free. Re-run this script to regenerate the .txt files.

The little ontology is designed so one fixture exercises every feature:

  138875005 SNOMED CT Concept (root)
  |- 404684003 Clinical finding
  |   |- 73211009 Diabetes mellitus
  |   |   |- 46635009 Type 1 diabetes mellitus      (member of example refset)
  |   |   |- 44054006 Type 2 diabetes mellitus      (member of example refset)
  |   |- 22298006 Myocardial infarction             (CTV3 map; finding site = Myocardium,
  |   |                                               associated morphology = Infarct, group 1;
  |   |                                               synonym "Heart attack")
  |   |- 195967001 Asthma
  |   |- 9468002 Inactive example disorder          (INACTIVE - filtered out)
  |- 71388002 Procedure
  |   |- 80146002 Excision of appendix              (dialect: GB "Appendicectomy" /
  |                                                  US "Appendectomy")
  |- 123037004 Body structure
  |   |- 74281007 Myocardium structure
  |   |- 55641003 Infarct (morphologic abnormality)
  |- 105590001 Substance
  |   |- 387517004 Paracetamol
  |- 410662002 Concept model attribute
      |- 116680003 Is a / 363698007 Finding site / 116676008 Associated morphology

Features covered: active filtering, FSN/PT/synonyms, semantic tags, GB vs US
dialect selection, IS-A hierarchy (descendants/ancestors/children/tct), typed
attribute relationships with groups (ECL refinement), simple refset membership
(ECL ^ and `sct refset`), CTV3 reverse map (`sct lookup`), payload-bearing map
refsets, Attribute Value inactivation indicators, and historical associations.
"""

import os

ET = "20260101"
MODULE = "900000000000207008"          # SNOMED CT core module
PRIMITIVE = "900000000000074008"       # primitive definition status
FSN = "900000000000003001"
SYN = "900000000000013009"
ISA = "116680003"
INFERRED = "900000000000011006"
EXISTENTIAL = "900000000000451002"
PREFERRED = "900000000000548007"
ACCEPTABLE = "900000000000549004"
CIS = "900000000000448009"             # case insignificant
GB = "900000000000508004"              # GB English language reference set
US = "900000000000509007"              # US English language reference set
CTV3_MAP = "900000000000497000"        # CTV3 simple map reference set
EXAMPLE_REFSET = "991381000000107"     # our synthetic simple refset

# (id, active, fsn, parent, [extra synonyms])
CONCEPTS = [
    ("138875005", 1, "SNOMED CT Concept (SNOMED RT+CTV3)", None, []),
    ("404684003", 1, "Clinical finding (finding)", "138875005", []),
    ("71388002", 1, "Procedure (procedure)", "138875005", []),
    ("123037004", 1, "Body structure (body structure)", "138875005", []),
    ("105590001", 1, "Substance (substance)", "138875005", []),
    ("410662002", 1, "Concept model attribute (attribute)", "138875005", []),
    ("116680003", 1, "Is a (attribute)", "410662002", []),
    ("363698007", 1, "Finding site (attribute)", "410662002", []),
    ("116676008", 1, "Associated morphology (attribute)", "410662002", []),
    ("73211009", 1, "Diabetes mellitus (disorder)", "404684003", ["Sugar diabetes"]),
    ("46635009", 1, "Type 1 diabetes mellitus (disorder)", "73211009", []),
    ("44054006", 1, "Type 2 diabetes mellitus (disorder)", "73211009", []),
    ("22298006", 1, "Myocardial infarction (disorder)", "404684003", ["Heart attack"]),
    ("195967001", 1, "Asthma (disorder)", "404684003", []),
    ("9468002", 0, "Inactive example disorder (disorder)", "404684003", []),
    ("74281007", 1, "Myocardium structure (body structure)", "123037004", []),
    ("55641003", 1, "Infarct (morphologic abnormality)", "123037004", []),
    ("387517004", 1, "Paracetamol (substance)", "105590001", []),
    ("900000000000508004", 1,
     "Great Britain English language reference set (foundation metadata concept)", "138875005", []),
    ("900000000000509007", 1,
     "United States English language reference set (foundation metadata concept)", "138875005", []),
    ("900000000000497000", 1,
     "CTV3 simple map reference set (foundation metadata concept)", "138875005", []),
    ("991381000000107", 1,
     "Example clinical reference set (foundation metadata concept)", "138875005", []),
]

# Concept 80146002 is special: two dialect synonyms, no single PT in the table above.
DIALECT_CONCEPT = ("80146002", 1, "Excision of appendix (procedure)", "71388002")

# Non-IS-A attribute relationships: (source, type, destination, group)
ATTRIBUTES = [
    ("22298006", "363698007", "74281007", "1"),   # finding site = Myocardium
    ("22298006", "116676008", "55641003", "1"),   # associated morphology = Infarct
]

# Simple refset (EXAMPLE_REFSET) members.
REFSET_MEMBERS = ["46635009", "44054006"]

# CTV3 simple map: (concept, ctv3 code)
CTV3_MAPS = [("22298006", "X200"), ("73211009", "C10..")]

# Extended map (SNOMED CT -> ICD-10 / OPCS-4): real UK map refset SCTIDs so the
# loader's extended_map_system() classifies them. Columns per row:
#   (refsetId, concept, mapGroup, mapPriority, mapRule, mapAdvice, mapTarget)
ICD10_MAP = "999002271000000101"       # UK SNOMED CT -> ICD-10
OPCS4_MAP = "1126441000000105"         # UK SNOMED CT -> OPCS-4
CORRELATION = "447561005"              # correlation: 'not specified'
UNKNOWN_MAP = "991391000000109"        # synthetic, deliberately unclassified map refset
EXTENDED_MAPS = [
    (ICD10_MAP, "22298006", 1, 1, "", "ALWAYS I21.9", "I219"),
    (ICD10_MAP, "22298006", 1, 2, "", "SECOND I21.9 ALTERNATIVE", "I219"),
    (ICD10_MAP, "73211009", 1, 1, "", "ALWAYS E14.9", "E149"),
    (ICD10_MAP, "46635009", 1, 1, "", "ALWAYS E10.9", "E109"),
    (ICD10_MAP, "44054006", 1, 1, "", "ALWAYS E11.9", "E119"),
    (OPCS4_MAP, "80146002", 1, 1, "", "", "H011"),
    (UNKNOWN_MAP, "195967001", 1, 1, "", "PRESERVE UNKNOWN TARGET", "X-UNKNOWN"),
    (ICD10_MAP, "195967001", 2, 1, "", "NULL MAP", ""),
]

# International Extended Map variant with mapCategoryId rather than UK mapBlock.
MAP_CATEGORY = "447637006"
EXTENDED_MAP_CATEGORIES = [
    ("30000000-0000-4000-8000-000000000001", 1, ICD10_MAP, "387517004", 3, 1,
     "OTHERWISE TRUE", "CATEGORY VARIANT", "Z998", CORRELATION, MAP_CATEGORY),
    ("30000000-0000-4000-8000-000000000002", 0, ICD10_MAP, "195967001", 4, 1,
     "OTHERWISE TRUE", "INACTIVE CATEGORY VARIANT", "I221", CORRELATION, MAP_CATEGORY),
]

# Complex Map members use the older iisssc shape. The synthetic refset is not
# assigned a target system, so these rows must be preserved without projection.
COMPLEX_MAP = "991401000000101"
COMPLEX_MAPS = [
    ("10000000-0000-4000-8000-000000000001", 1, COMPLEX_MAP, "22298006", 1, 1,
     "IFA 246075003", "CHECK LEGACY CLASSIFICATION", "LEGACY-01", CORRELATION),
    ("10000000-0000-4000-8000-000000000002", 0, COMPLEX_MAP, "195967001", 1, 2,
     "OTHERWISE TRUE", "INACTIVE ALTERNATIVE", "LEGACY-02", CORRELATION),
]

# Attribute Value members. The active row supplies the concept inactivation
# reason needed by R11; the inactive row proves Snapshot member status survives.
CONCEPT_INACTIVATION_INDICATOR = "900000000000489007"
DESCRIPTION_INACTIVATION_INDICATOR = "900000000000490003"
DUPLICATE = "900000000000482003"
OUTDATED = "900000000000483008"
ATTRIBUTE_VALUES = [
    ("20000000-0000-4000-8000-000000000001", 1, CONCEPT_INACTIVATION_INDICATOR,
     "9468002", DUPLICATE),
    ("20000000-0000-4000-8000-000000000002", 0, CONCEPT_INACTIVATION_INDICATOR,
     "195967001", OUTDATED),
    ("20000000-0000-4000-8000-000000000003", 1, DESCRIPTION_INACTIVATION_INDICATOR,
     "5000001", DUPLICATE),
]

# Historical associations (inactive-concept forwarding):
#   (associationType refsetId, inactiveSource, target)
SAME_AS = "900000000000527005"
REPLACED_BY = "900000000000526001"
ASSOCIATIONS = [
    (SAME_AS, "9468002", "195967001"),     # inactive disorder SAME AS Asthma
    (REPLACED_BY, "9468002", "22298006"),  # also REPLACED BY MI (multi-association)
]


def fsn_to_pt(fsn):
    """Strip the trailing ' (tag)' to get a preferred-term string."""
    i = fsn.rfind(" (")
    return fsn[:i] if i != -1 else fsn


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    root = os.path.join(
        here, "SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z", "Snapshot"
    )
    term_dir = os.path.join(root, "Terminology")
    lang_dir = os.path.join(root, "Refset", "Language")
    map_dir = os.path.join(root, "Refset", "Map")
    content_dir = os.path.join(root, "Refset", "Content")
    for d in (term_dir, lang_dir, map_dir, content_dir):
        os.makedirs(d, exist_ok=True)

    concept_rows = []
    description_rows = []   # (id, conceptId, typeId, term)
    lang_rows = []          # (refsetId, descId, acceptability)
    relationship_rows = []  # (source, dest, group, type)

    did = 5000001          # description id counter
    rid = 7000001          # relationship id counter
    lid = 9000001          # language refset uuid counter

    def add_concept(cid, active, fsn, parent, extra_syns):
        nonlocal did, rid, lid
        concept_rows.append((cid, active, MODULE, PRIMITIVE))
        # FSN
        description_rows.append((str(did), cid, FSN, fsn))
        lang_rows.append((GB, str(did), PREFERRED))
        lang_rows.append((US, str(did), PREFERRED))
        did += 1
        # Preferred synonym (same term for both dialects)
        pt = fsn_to_pt(fsn)
        description_rows.append((str(did), cid, SYN, pt))
        lang_rows.append((GB, str(did), PREFERRED))
        lang_rows.append((US, str(did), PREFERRED))
        did += 1
        # Extra acceptable synonyms
        for syn in extra_syns:
            description_rows.append((str(did), cid, SYN, syn))
            lang_rows.append((GB, str(did), ACCEPTABLE))
            lang_rows.append((US, str(did), ACCEPTABLE))
            did += 1
        # IS-A relationship
        if parent is not None:
            relationship_rows.append((cid, parent, "0", ISA))

    for (cid, active, fsn, parent, extra) in CONCEPTS:
        add_concept(cid, active, fsn, parent, extra)

    # The dialect concept: GB prefers "Appendicectomy", US prefers "Appendectomy".
    dcid, dactive, dfsn, dparent = DIALECT_CONCEPT
    concept_rows.append((dcid, dactive, MODULE, PRIMITIVE))
    description_rows.append((str(did), dcid, FSN, dfsn))
    lang_rows.append((GB, str(did), PREFERRED))
    lang_rows.append((US, str(did), PREFERRED))
    did += 1
    gb_desc = str(did)
    description_rows.append((gb_desc, dcid, SYN, "Appendicectomy"))
    did += 1
    us_desc = str(did)
    description_rows.append((us_desc, dcid, SYN, "Appendectomy"))
    did += 1
    lang_rows.append((GB, gb_desc, PREFERRED))
    lang_rows.append((US, gb_desc, ACCEPTABLE))
    lang_rows.append((GB, us_desc, ACCEPTABLE))
    lang_rows.append((US, us_desc, PREFERRED))
    relationship_rows.append((dcid, dparent, "0", ISA))

    # Attribute relationships
    for (src, typ, dest, group) in ATTRIBUTES:
        relationship_rows.append((src, dest, group, typ))

    def write_tsv(path, header, rows):
        with open(path, "w", encoding="utf-8") as f:
            f.write("\t".join(header) + "\n")
            for row in rows:
                f.write("\t".join(str(c) for c in row) + "\n")

    # --- Concept ---
    write_tsv(
        os.path.join(term_dir, "sct2_Concept_Snapshot_SYN_20260101.txt"),
        ["id", "effectiveTime", "active", "moduleId", "definitionStatusId"],
        [(cid, ET, active, module, defstat) for (cid, active, module, defstat) in concept_rows],
    )

    # --- Description ---
    write_tsv(
        os.path.join(term_dir, "sct2_Description_Snapshot-en_SYN_20260101.txt"),
        ["id", "effectiveTime", "active", "moduleId", "conceptId",
         "languageCode", "typeId", "term", "caseSignificanceId"],
        [(d, ET, 1, MODULE, cid, "en", typ, term, CIS)
         for (d, cid, typ, term) in description_rows],
    )

    # --- Relationship ---
    rel_out = []
    for (src, dest, group, typ) in relationship_rows:
        rel_out.append((str(rid), ET, 1, MODULE, src, dest, group, typ, INFERRED, EXISTENTIAL))
        rid += 1
    write_tsv(
        os.path.join(term_dir, "sct2_Relationship_Snapshot_SYN_20260101.txt"),
        ["id", "effectiveTime", "active", "moduleId", "sourceId", "destinationId",
         "relationshipGroup", "typeId", "characteristicTypeId", "modifierId"],
        rel_out,
    )

    # --- Language refset ---
    lang_out = []
    for (refset, desc, accept) in lang_rows:
        lang_out.append((str(lid), ET, 1, MODULE, refset, desc, accept))
        lid += 1
    write_tsv(
        os.path.join(lang_dir, "der2_cRefset_LanguageSnapshot-en_SYN_20260101.txt"),
        ["id", "effectiveTime", "active", "moduleId", "refsetId",
         "referencedComponentId", "acceptabilityId"],
        lang_out,
    )

    # --- Simple map (CTV3) ---
    map_out = []
    for i, (cid, code) in enumerate(CTV3_MAPS):
        map_out.append((f"m{i+1}", ET, 1, MODULE, CTV3_MAP, cid, code))
    write_tsv(
        os.path.join(map_dir, "der2_sRefset_SimpleMapSnapshot_SYN_20260101.txt"),
        ["id", "effectiveTime", "active", "moduleId", "refsetId",
         "referencedComponentId", "mapTarget"],
        map_out,
    )

    # --- Simple refset (membership) ---
    refset_out = []
    for i, cid in enumerate(REFSET_MEMBERS):
        refset_out.append((f"r{i+1}", ET, 1, MODULE, EXAMPLE_REFSET, cid))
    write_tsv(
        os.path.join(content_dir, "der2_Refset_SimpleSnapshot_SYN_20260101.txt"),
        ["id", "effectiveTime", "active", "moduleId", "refsetId", "referencedComponentId"],
        refset_out,
    )

    # --- Extended map (SNOMED CT -> ICD-10 / OPCS-4) ---
    em_out = []
    for i, (rset, cid, grp, pri, rule, advice, target) in enumerate(EXTENDED_MAPS):
        em_out.append((f"em{i+1}", ET, 1, MODULE, rset, cid, grp, pri,
                       rule, advice, target, CORRELATION, 1))
    write_tsv(
        os.path.join(map_dir, "der2_iisssciRefset_ExtendedMapSnapshot_SYN_20260101.txt"),
        ["id", "effectiveTime", "active", "moduleId", "refsetId", "referencedComponentId",
         "mapGroup", "mapPriority", "mapRule", "mapAdvice", "mapTarget", "correlationId", "mapBlock"],
        em_out,
    )

    em_category_out = [
        (member_id, ET, active, MODULE, rset, cid, group, priority,
         rule, advice, target, correlation, category)
        for (member_id, active, rset, cid, group, priority, rule, advice, target, correlation, category)
        in EXTENDED_MAP_CATEGORIES
    ]
    write_tsv(
        os.path.join(map_dir, "der2_iisssccRefset_ExtendedMapSnapshot_SYN_20260101.txt"),
        ["id", "effectiveTime", "active", "moduleId", "refsetId", "referencedComponentId",
         "mapGroup", "mapPriority", "mapRule", "mapAdvice", "mapTarget", "correlationId",
         "mapCategoryId"],
        em_category_out,
    )

    # --- Complex map (lossless payload, no guessed target system) ---
    complex_out = [
        (member_id, ET, active, MODULE, rset, cid, group, priority,
         rule, advice, target, correlation)
        for (member_id, active, rset, cid, group, priority, rule, advice, target, correlation)
        in COMPLEX_MAPS
    ]
    write_tsv(
        os.path.join(map_dir, "der2_iissscRefset_ComplexMapSnapshot_SYN_20260101.txt"),
        ["id", "effectiveTime", "active", "moduleId", "refsetId", "referencedComponentId",
         "mapGroup", "mapPriority", "mapRule", "mapAdvice", "mapTarget", "correlationId"],
        complex_out,
    )

    # --- Attribute Value refset (concept inactivation indicator) ---
    attribute_value_out = [
        (member_id, ET, active, MODULE, rset, component, value)
        for (member_id, active, rset, component, value) in ATTRIBUTE_VALUES
    ]
    write_tsv(
        os.path.join(content_dir, "der2_cRefset_AttributeValueSnapshot_SYN_20260101.txt"),
        ["id", "effectiveTime", "active", "moduleId", "refsetId",
         "referencedComponentId", "valueId"],
        attribute_value_out,
    )

    # --- Historical associations (concept history) ---
    assoc_out = []
    for i, (rset, src, tgt) in enumerate(ASSOCIATIONS):
        assoc_out.append((f"a{i+1}", ET, 1, MODULE, rset, src, tgt))
    write_tsv(
        os.path.join(content_dir, "der2_cRefset_AssociationSnapshot_SYN_20260101.txt"),
        ["id", "effectiveTime", "active", "moduleId", "refsetId",
         "referencedComponentId", "targetComponentId"],
        assoc_out,
    )

    print(f"Wrote synthetic RF2 snapshot under {root}")
    print(f"  {len(concept_rows)} concepts, {len(description_rows)} descriptions, "
           f"{len(relationship_rows)} relationships, {len(lang_out)} language rows, "
           f"{len(map_out)} CTV3 maps, {len(refset_out)} simple refset members, "
           f"{len(em_out) + len(em_category_out)} extended maps, {len(complex_out)} complex maps, "
           f"{len(attribute_value_out)} attribute values")


if __name__ == "__main__":
    main()
