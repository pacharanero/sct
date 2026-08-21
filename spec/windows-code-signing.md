# Windows code-signing for `sct` (roadmap `R35`, unblocks `R37`)

How to get `sct`'s Windows binaries digitally signed, why it matters, the realistic options in 2026, and step-by-step setup with cost and time estimates. This is the design/decision doc referenced by `R35` (Authenticode signing) and `R37` (winget) in [`roadmap.md`](roadmap.md).

## Why sign at all

Unsigned Windows executables show an "unknown publisher" prompt, and Microsoft SmartScreen warns on first run until a file's hash has accumulated download reputation. For a clinical tool this is exactly the friction non-technical users hit before they ever reach a terminal. Signing does two things: it binds every release to a verified identity (so the publisher is named, not "unknown"), and it lets SmartScreen reputation accrue against that identity rather than per-file-hash, so warnings fade far sooner. Signing is also a practical precondition for a smooth `winget` submission (`R37`) and for any future double-click `.msi` installer.

Signing does **not** instantly silence SmartScreen. Reputation still builds with download volume; a brand-new signing identity can still trigger a warning for a while. An EV certificate used to grant instant reputation, but that advantage has largely eroded. Plan for "warnings clear within weeks of real downloads", not "gone on day one".

## The 2026 landscape (what changed)

- **Plain-file OV certificates no longer exist.** Since the June 2023 CA/Browser Forum change, all publicly-trusted code-signing private keys must live on FIPS 140-2 Level 2 hardware - a physical USB token or a cloud HSM. You can no longer download a `.pfx` and sign with it directly.
- **Azure Artifact Signing** (formerly "Trusted Signing", formerly "Azure Code Signing") is Microsoft's managed signing service. The key is generated and held inside the service (no token to mail around, no HSM to run), and you sign through `SignTool`/a GitHub Action. It is the cheapest and most CI-friendly path, and it is the recommended option below.
- **EV certificates** remain available from CAs but are the most expensive and still require hardware/HSM, for a benefit that no longer justifies the cost here.

## Recommendation for `sct`

Use **Azure Artifact Signing**, integrated into the existing `release.yml` Windows job. It is ~US$10/month, needs no physical token, holds the key in the service, and has a first-party GitHub Action. Baw Medical Ltd is UK-based, and the UK is an eligible region.

**The one decision to make first - Organization vs Individual identity validation:**

- **Organization validation** issues a certificate whose publisher is `Baw Medical Ltd`. It requires the organization to have a **verifiable tax history of three or more years**. **Action: confirm Baw Medical Ltd's incorporation/tax age.** If it is under three years old, organization validation will be declined (Microsoft has rejected sub-3-year entities), and there is no documented exception path.
- **Individual validation** issues a certificate whose publisher is your validated personal name (Marcus Baw). It has **no organization-age requirement** - it validates a government ID plus a Microsoft Verified ID (via the Authenticator app / AU10TIX facial verification). This is the fallback if the company is too young, and it can be completed faster.

Either way the technical integration is identical; only the certificate's subject name and the validation flow differ.

## At a glance

| Option | Up-front cost | Ongoing cost | Hardware | Calendar time | Hands-on effort | Verdict |
|---|---|---|---|---|---|---|
| **Azure Artifact Signing** | none | ~US$10/mo + paid Azure subscription | none (key in service) | ~2-10 business days (identity validation is the wait) | ~0.5-1 day | **Recommended** |
| OV cert + Azure Key Vault HSM | cert ~US$200-400/yr | HSM ~US$5/mo + per-op | cloud HSM | ~1-3 weeks | ~1-2 days | Only if you specifically need a named CA |
| EV cert | ~US$300-600/yr | renewal | token/HSM | ~1-3 weeks | ~1-2 days | Not worth it here |
| Microsoft Store signs on submission | none | none | none | Store review | packaging rework | Poor fit for a CLI |

Cost/quota figures move; confirm the current price on the [Artifact Signing pricing page](https://azure.microsoft.com/pricing/details/artifact-signing/) before budgeting. Artifact Signing requires a **paid** (pay-as-you-go or EA) Azure subscription - free/trial/sponsored subscriptions are rejected.

## Step by step - Azure Artifact Signing

Estimated effort ~0.5-1 day of work, but the identity-validation review is a wait of several business days, so **start step 3 first**.

1. **Prerequisites.** A paid Azure subscription (pay-as-you-go is fine) and an account with permission to create resources and to be assigned the *Artifact Signing Identity Verifier* and *Certificate Profile Signer* roles.
2. **Register the resource provider.** In the subscription, register `Microsoft.CodeSigning` (Subscription → Resource providers).
3. **Start identity validation immediately** (the long pole). Create an Artifact Signing account, then start an identity validation - **Organization** (if Baw Medical Ltd is 3+ years old) or **Individual** (otherwise). Watch for the email verification link; it expires after 7 days and cannot be resent - a missed link means starting over. Organization review typically takes several business days; Individual validation via Verified ID can be quicker.
4. **Create a certificate profile** once validation shows *Completed*. The profile's CN/O is fixed to your validated name (no custom CN/O allowed).
5. **Grant the signing role.** Assign the identity that CI will use (a Microsoft Entra app registration / federated credential) the *Certificate Profile Signer* role on the account or resource group.
6. **Integrate into `release.yml`.** In the Windows job, after building `sct-windows-x86_64.exe`, add a signing step using the official Artifact Signing GitHub Action, authenticating to Azure with OIDC federated credentials (no long-lived secret). Sign the `.exe` (and, later, any `.msi`) with an RFC-3161 timestamp so signatures stay valid after the certificate rotates. Then compute checksums and upload as today.
7. **Verify.** `signtool verify /v /pa sct.exe` should report a valid chain; the file's Properties → Digital Signatures tab should name the publisher.

Order the workflow so signing happens **before** `SHA256SUMS` is computed, so the published checksums match the signed artefacts.

## Alternatives, briefly

- **OV certificate from a CA (Sectigo/DigiCert) + Azure Key Vault HSM.** Choose this only if you specifically need the certificate issued by a named commercial CA. It costs more, needs a cloud HSM (or a shipped USB token, which does not work in CI), and delivers no reputation advantage over Artifact Signing.
- **EV certificate.** Most expensive, still needs hardware, no longer buys instant SmartScreen reputation. Skip.
- **Microsoft Store.** The Store signs on submission, but shipping a developer CLI through the Store is an awkward fit and does not help the direct-download or `winget` paths.

## After signing lands

- **`R37` (winget).** A signed installer/binary is expected for a smooth `winget-pkgs` submission; do `R37` once signing is operational.
- **Double-click installer.** With signing in place, a signed `.msi` (e.g. via `cargo-wix`) becomes the genuinely non-technical, GUI, auto-PATH install path - the biggest usability win for non-technical Windows users. Worth scoping as a follow-on once `R35` is done.

## Open items to confirm before committing budget/time

- **Baw Medical Ltd's verifiable tax-history age** - decides Organization vs Individual validation (the only real branch above).
- **Current Artifact Signing price and included signing quota** - confirm on the pricing page.
- **Which Azure subscription** to bill this to (must be a paid type).

## References

- [Code-signing options for Windows app developers](https://learn.microsoft.com/windows/apps/package-and-deploy/code-signing-options)
- [Quickstart: set up Artifact Signing](https://learn.microsoft.com/azure/artifact-signing/quickstart)
- [Artifact Signing FAQ](https://learn.microsoft.com/azure/artifact-signing/faq) (identity validation, EV stance, error codes)
- [Artifact Signing pricing](https://azure.microsoft.com/pricing/details/artifact-signing/)
- [SmartScreen reputation for Windows app developers](https://learn.microsoft.com/windows/apps/package-and-deploy/smartscreen-reputation)
