# Project governance

## Stewardship and transition

`sct` is a candidate for adoption by the [Open Health Technology Foundation (OHTF)](https://openhealthtechnology.org), legally **OPEN HEALTH TECHNOLOGY CIC**. Marcus Baw approved adoption and the intended transfer on 13 November 2026. The repository remains at `pacharanero/sct`; the agreed eventual destination is the `ohtf` GitHub organisation. This records approval and direction, not a claim that the repository, publishing accounts or copyright have already transferred.

Marcus Baw remains the project maintainer, with the same day-to-day technical and release responsibilities. Changes are discussed through issues and pull requests; significant design decisions belong in `spec/`. The current validation and release rules in [AGENTS.md](AGENTS.md) continue to apply. Automated agents do not acquire transfer or release authority through this transition.

## Commitments

- Keep `sct` local-first, open source and clinically focused. Network-dependent features remain explicit and opt-in; this does not claim that every experimental surface is already offline-only.
- Preserve the canonical NDJSON pipeline and shared terminology semantics across interfaces.
- Keep the existing AGPL-3.0-or-later licence and copyright attribution. Adoption does not silently relicense work or assign contributors' copyright.
- Follow OHTF's [open-source commitment](https://openhealthtechnology.org/governance/open-source-commitment/) as the agreed basis for adoption.
- Keep SNOMED CT and other third-party content subject to its own licences. Foundation stewardship grants no new right to redistribute terminology data, and patient data is outside the transfer.
- Preserve continuity for existing users: keep package names and installation channels stable unless a separately reviewed migration is necessary.

## Contribution and reporting

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution terms and DCO sign-off, [CODE-OF-CONDUCT.md](CODE-OF-CONDUCT.md) for community behaviour, and [SECURITY.md](SECURITY.md) for private vulnerability reporting. The Foundation's contribution, conduct and security requirements are part of onboarding; do not describe `sct` as fully onboarded until the operational checks are complete.

The [transition record](spec/foundation-transition.md) owns the remaining handover work. Account changes, repository transfer, secret changes and public announcements require an explicit maintainer instruction; a checklist entry is not permission to perform them.
