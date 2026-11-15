# Clinical safety

**Status: initial safety record, prepared 15 November 2026, awaiting maintainer and clinical-safety review.** This is not an approved clinical safety case, a risk-acceptance decision, regulatory certification or a declaration of DCB0129/DCB0160 compliance. Foundation adoption and passing CI do not confer those approvals.

## Purpose, users and environment

`sct` is a local-first clinical terminology toolchain for developers, terminology specialists, informaticians and clinicians who can review its outputs. It builds queryable artefacts from licensed RF2 releases and supports terminology lookup, search, hierarchy and ECL queries, mappings and codelist authoring through CLI, SDK, MCP, FHIR and experimental user interfaces. See the [README](README.md) for the implemented surfaces.

The documented purpose is terminology processing, exploration and integration, not independent diagnosis, prescribing, treatment selection or emergency decision-making. There is no patient-specific population or care pathway validated by this record. A concept match, map or codelist is not a clinical recommendation, and downstream automation can still affect patients even when `sct` itself only handles terminology.

Native SQLite/FST queries run locally. Release downloads, URL codelist imports and Ollama-backed features involve explicit network access; Ollama may be configured on a remote host. The experimental GUI still fetches public CDN assets. Local-first is not a confidentiality guarantee for every surface. Patient-identifiable data is not needed for terminology builds or regression tests; do not submit it in issues, logs or safety reports, or send it to an embedding/LLM endpoint without the deployment's separate information-governance approval.

## Responsibilities and current assurance

- **Project coordination:** Marcus Baw, the existing maintainer, is the contact for reviewing this record and arranging safety follow-up. This does not appoint him as Clinical Safety Officer or record his acceptance of clinical risk.
- **Clinical Safety Officer and formal safety sign-off:** no appointment or sign-off is recorded here. These remain human decisions under the [Foundation transition](spec/foundation-transition.md).
- **Deploying and integrating organisations:** assess the actual intended use, data, users, clinical workflow, failure handling and applicable obligations before outputs affect care. Supplier and deployer responsibilities must be established explicitly; neither an open-source licence nor this document settles them.
- **Evidence limits:** the [named guarantees inventory](spec/named-guarantees.md) records partial coverage and gaps. The [FHIR/ECL conformance record](spec/fhir-conformance.md) distinguishes regression tests, structural validation and external terminology interoperability. `sct serve` implements a SNOMED-focused FHIR R4 subset, not a generally certified terminology server. The ECL subset and refusals are documented in [spec/ecl.md](spec/ecl.md).

No approved safety case, complete hazard log or deployment-specific safety plan is asserted. The concerns below are an initial identification exercise, not a completed risk assessment.

## Initial hazards and limitations

Identifiers `SCT-H01` onwards are stable and must not be reused. All entries are **open, unassessed**. Initial and residual severity, likelihood and risk are **not assessed**; no residual risk is accepted. Marcus Baw is the coordination contact, not an assigned clinical risk owner. A qualified reviewer must agree the assessment method, owners and controls for an actual use context.

| ID | Failure mechanism and potential clinical consequence | Existing controls or evidence, and remaining limits |
| --- | --- | --- |
| SCT-H01 | Wrong edition, stale release, mismatched derived artefacts or corrupted input changes the available concepts; a downstream cohort or coding decision omits or includes the wrong people. | Provenance, fingerprints and companion checks are described in [provenance guarantees](spec/named-guarantees.md#4-provenance-and-artefact-identity); companion rollback tests are in `tests/end_to_end.rs`. Coverage is not complete across formats, and older artefacts may lack identity metadata. Record software, edition, release, build options and artefact identity; independently review upgrades before use. |
| SCT-H02 | An ECL/refset interpretation or unsupported construct silently changes a result set, leading to an incorrect codelist or decision-support population. | The [ECL contract](spec/ecl.md), `tests/ecl_eval.rs`, `tests/ecl-history-binding.rs` and `tests/ecl-refset-operands.rs` cover named behaviours. Recent refset regressions show why simple and compound paths need parity tests. Fixture coverage is not proof for every query or release; clinically review the expression and representative inclusions/exclusions. |
| SCT-H03 | Inactive concepts, historical changes or unavailable evidence are mistaken for current concepts or proof that no history exists; legacy records are missed or recoded incorrectly. | [History evidence tests](tests/history_evidence.rs) protect explicit-history refusal when evidence was not loaded. Ordinary lookup remains lenient and can still conflate unavailable evidence with genuinely empty fields (`R94`). Active-only builds omit retired concepts; history supplements do not reconstruct full historical hierarchy membership. Review lifecycle, build coverage and time context rather than interpreting an empty answer as absence of disease or history. |
| SCT-H04 | A search rank, similarity score or LLM explanation is treated as clinically validated meaning, producing an incorrect selected code. | [Semantic search limitations](docs/commands/semantic.md#known-limitations) document observed failures; hybrid retrieval is experimental. Lexical matching is not proof of clinical equivalence either. Review the identifier, fully specified name, descriptions, hierarchy and relevant clinical context. Neither an embedding score nor an LLM's use of real identifiers removes the need for review. |
| SCT-H05 | A mapping projection or historical association is treated as a one-to-one clinically equivalent replacement; information is lost or the wrong code is substituted. | [Mapping semantics](spec/cross-terminology-mapping.md) and `tests/transcode.rs` retain or test specific map claims. SDK code projections do not expose all correlation claims, and deterministic ordering does not express clinical preference. Review source/target releases, direction, correlation, advice, assurance and ambiguity before recoding. |
| SCT-H06 | A network-exposed service, remote model or exported artefact leaks data or an unavailable service is mistaken for an empty clinical answer. | Read-only database openers, request bounds and deadline cleanup tests reduce specific risks; they do not provide a complete deployment security or availability case. Review [deployment guidance](spec/deployment.md), network endpoints and access controls; distinguish errors/timeouts from valid empty sets and apply a reviewed downtime procedure. See [SECURITY.md](SECURITY.md) for private reporting. |

## Checks before outputs affect care

1. Establish the intended clinical workflow, accountable organisation and appropriate clinical-safety/regulatory review; record who authorises use.
2. Pin software and terminology versions, confirm loaded capabilities and build options, and retain the input/provenance needed to reproduce the result.
3. Validate representative queries, codelists and mappings against independently reviewed expected results, including negative cases, inactive concepts and missing evidence. Synthetic fixtures alone cannot validate a local clinical use case.
4. Preserve uncertainty, map direction and lifecycle evidence through downstream interfaces. Do not interpret failed, truncated, unavailable or unreviewed results as clinical absence or equivalence.
5. Document human review, release/change control, monitoring and a recovery or downtime process. Reassess after changes to software, terminology, models, queries or integrations.

## Medical-device and DCB applicability review

This section is a **pending assessment record**, not a conclusion that the tool is or is not a medical device. Terminology infrastructure can be incorporated into software with a medical intended purpose. Actual product claims, integration and market matter; describing a component as a development tool is not an automatic exemption.

| Field | Current record |
| --- | --- |
| Intended purpose, users, setting and clinical claims | Described above from the current repository; requires maintainer confirmation and assessment for each proposed clinical integration. No patient-specific benefit, diagnostic accuracy or treatment claim is validated here. |
| Decision-maker and responsible role | Not yet recorded. Marcus Baw coordinates obtaining the appropriate human assessment; no CSO or regulatory appointment is implied. |
| Decision date | No applicability decision taken in this record. Initial document dated 15 November 2026. |
| Evidence considered | Current README, query/mapping/search specifications and the primary guidance linked below, accessed 15 November 2026. No specialist legal/regulatory advice is recorded. |
| Conclusion | Requires human assessment; no device classification, market authorisation or DCB compliance determination is made. |
| Actions and coordination | Marcus Baw to confirm intended use, responsible decision-maker and clinical-safety roles; assess medical-device and DCB0129/DCB0160 obligations for the relevant context and obtain specialist advice where needed. Record the outcome and required safety artefacts before claiming suitability for clinical deployment. |
| Next review | Due before any new patient-specific clinical claim or deployment approval and as part of Foundation onboarding; a named reviewer and calendar review date remain to be agreed. |
| Reassessment triggers | Changes to intended purpose, users, populations, clinical claims, patient-specific inputs/outputs, decision support, models, device integration, deployment setting or market. Update the record before releasing changes that could alter the conclusion. |

Primary guidance reviewed:

- [MHRA: borderline products and medical-device classification](https://www.gov.uk/guidance/borderline-products-how-to-tell-if-your-product-is-a-medical-device): intended purpose, mode of action and public claims inform the determination.
- [MHRA: regulating medical devices in the UK](https://www.gov.uk/guidance/regulating-medical-devices-in-the-uk): applicable obligations and routes depend on the product and market; this record is not regulatory advice.
- [NHS England: step-by-step DCB0129/DCB0160 applicability guidance](https://digital.nhs.uk/services/clinical-safety/applicability-of-dcb-0129-and-dcb-0160/step-by-step-guidance): assess applicability in the actual development/deployment context. Medical-device applicability and clinical risk management are separate questions.

## Reporting and maintenance

For a suspected clinically consequential wrong answer, stop relying on the affected output in the relevant workflow, retain the software/data versions and a safe reproduction, and notify the maintainer. Public issues may contain only non-sensitive synthetic reproductions. Use the private route in [SECURITY.md](SECURITY.md) for security findings, sensitive details or a safety report that cannot safely be made public. Follow the deploying organisation's incident and urgent patient-safety procedures separately; this repository is not an emergency reporting service.

Review safety-relevant changes against the hazard IDs above in the same pull request. Update this record when controls, evidence, intended use or known limitations change. Agents may maintain evidence and propose tests, but must not appoint safety officers, assign clinical risk scores, accept residual risk or assert compliance on behalf of the maintainer or Foundation.
