# Contributing to sct

Thank you for your interest in contributing! This project is licensed under the **GNU Affero General Public License v3.0** — by contributing, you agree that your contributions will be licensed under the same terms.

## Code of Conduct

This project follows the [Contributor Covenant Code of Conduct v3.0](https://www.contributor-covenant.org/version/3/0/). See [CODE-OF-CONDUCT.md](CODE-OF-CONDUCT.md) for the full text.

To report a Code of Conduct violation, please open a confidential issue or email the maintainers directly.

## How to Contribute

### Reporting Issues

- Search existing issues before opening a new one.
- Include a clear title, description, and steps to reproduce.
- Attach any relevant logs or error output.

### Submitting Changes

1. Fork the repository and create a branch from `main`.
2. Make your changes, keeping commits focused and atomic.
3. Ensure the test suite passes before submitting.
4. Open a pull request with a clear description of what and why.

### Commit Style

Prefer concise, imperative commit messages:

```
fix: handle empty SNOMED release directory
feat: add parquet export for concept hierarchy
docs: clarify embedding API usage
```

### Code Style

Follow the conventions already present in the codebase. Where in doubt, favour clarity over cleverness.

### Local hooks

To catch formatting and lint issues before they hit CI, install the repo-tracked git hook once per clone:

```bash
git config core.hooksPath .githooks
```

The `pre-commit` hook runs `cargo fmt --check` and `cargo clippy -- -D warnings` — the same fast checks CI runs. It only triggers when Rust-relevant files (`*.rs`, `*.toml`, `Cargo.lock`) are staged, so doc-only commits are never blocked. Tests are not run in the hook (too slow for a commit gate) — run `cargo test` yourself or let CI do it.

Bypass the hook for a single commit with `git commit --no-verify` — use sparingly, since CI will still reject what you bypass.

## Licensing Note

This is a **copyleft** project. Contributions must be compatible with AGPLv3. If you include third-party code or libraries, ensure their licenses are compatible.

Commercial use of this software requires compliance with the AGPLv3, including making source available to users of any network-accessible deployment.
