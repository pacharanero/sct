# Foundation transition

Decision record: 13 November 2026. Marcus Baw, as the current project owner and on behalf of the adopting organisation, approved adoption of `sct` by the Open Health Technology Foundation, legally **OPEN HEALTH TECHNOLOGY CIC**. Website: <https://openhealthtechnology.org>. The eventual repository destination is the `ohtf` GitHub organisation. Marcus remains maintainer; the local-first, open-source and clinically focused purpose is unchanged.

## Scope and current state

Approval to begin the transition is recorded in [GOVERNANCE.md](../GOVERNANCE.md). Repository and publishing transfers have not been performed. The website continues to label `sct` a Candidate under its current rule that Foundation Projects have repositories under `ohtf`. Existing copyright holders and the AGPL-3.0-or-later licence remain unchanged. Do not rewrite historical attribution or describe this record as an executed copyright-assignment agreement.

The initial [SAFETY.md](../SAFETY.md) records intended use, open hazards and assurance limits. It awaits human review and does not establish regulatory approval, certification, an appointed Clinical Safety Officer or accepted residual risk. Confirm the clinical-safety responsibilities and applicability assessment before claiming fully onboarded status.

## Operational inventory

This inventory is based on tracked configuration, not an audit of access to the external accounts. Never record secret values here.

| Surface | Current location or configuration | Transfer check |
| --- | --- | --- |
| GitHub repository | `pacharanero/sct` | Transfer to `ohtf/sct` only after permissions, Actions and redirect checks. |
| Rust package | `sct-rs` on crates.io; authentication in `.github/workflows/release.yml` | Preserve the name; verify owners and trusted-publisher repository identity. |
| Python package | `sct-py` on PyPI; upload in `.github/workflows/auto-tag.yml`, environment `pypi` | Preserve the name; update and verify the trusted-publisher binding before release. |
| Documentation | `pacharanero.github.io/sct`, configured in `mkdocs.yml` | Plan Pages destination, old links and installation-guide continuity; do not assume GitHub repository redirects cover Pages. |
| Containers | `docker.io/pacharanero/sct`, `ghcr.io/pacharanero/sct` | Verify namespace access, existing tags, package visibility and compatibility for existing pull commands. |
| Homebrew and Scoop | `pacharanero/homebrew-tap`, `pacharanero/scoop` | Verify updater permissions and hard-coded source/download URLs. |
| AUR | `sct-rs-bin`; `.github/workflows/publish-aur.yml` and reusable release job | Verify maintainer access, SSH identity and source URLs. |
| Publishing credentials | Workflow references to `DOCKER_HUB_PAT`, `TAP_REPOS_TOKEN`, `AUR_SSH_PRIVATE_KEY`; GitHub/OIDC permissions | Verify scope and recovery with the owner; change no secret without approval. |
| Citation and project links | `Cargo.toml`, Python metadata, `CITATION.cff`, README, docs and workflows | Update current ownership links only at cutover; preserve historical citations and release records. |
| Release announcements | Open Health Hub topic 3033, posted via `dsc` | Keep the existing audience and verify Foundation-authorised posting access. |

## Handover checklist

These are human-led operational gates, not assignments for the overnight agent. Stable `OHTF-` identifiers are local to this transition; they are never renumbered or reused. Remove completed tasks and retain durable decisions in the relevant governance or operational document.

- [ ] **OHTF-1 - Confirm access and continuity.** Verify Foundation-controlled administration, named maintainers, recovery arrangements and ownership of the services above without publishing credential material. Agree which accounts stay in place during transition.
- [ ] **OHTF-2 - Complete onboarding policies.** Reconcile the existing code of conduct with the Foundation policy; introduce DCO enforcement for new contributions without rewriting historical commits; verify the private security-reporting route. Record a clinical safety position and name the responsible people without inventing appointments or assurance claims. Existing licences and attribution are preserved; any future licensing or rights transfer is a separate explicit decision.
- [ ] **OHTF-3 - Prepare repository cutover.** Review all old repository/Pages/container references, trusted publishers, environments, external updater credentials and package access. Define the exact cutover order, verification steps and recovery plan. Do not transfer the repository merely because a target name is recorded here.
- [ ] **OHTF-4 - Transfer and verify.** On explicit approval, transfer to `ohtf/sct`, update current ownership metadata and publishing bindings, and test clone, installation, documentation, CI and release publication. Existing package names and data-licensing boundaries remain unchanged.
- [ ] **OHTF-5 - Announce completed adoption.** Only after verification, update the Foundation website and any organisation profile, project status and user-facing links together. Publish a joint announcement stating the unchanged maintainer, purpose and licences, and any practical user actions.
