# my-project

> Created from [butterflyskies/rust-template](https://github.com/butterflyskies/rust-template)

## Quick start

```bash
gh repo create butterflyskies/my-new-project --template butterflyskies/rust-template --public --clone
cd my-new-project
```

Then search-and-replace `my-project` with your actual project name in:

- `Cargo.toml` — package name
- `Dockerfile` — binary name and runtime user
- `.github/workflows/build.yml` — `IMAGE` env var

## What's included

| File | Purpose |
|------|---------|
| `.github/workflows/build.yml` | CI: fmt, clippy, nextest, cargo audit, Docker build (commented out) |
| `.github/workflows/lint-pr.yml` | Conventional commit enforcement on PR titles |
| `.github/workflows/release.yml` | release-please automation |
| `Dockerfile` | Multi-stage build (builder + runtime), Debian Trixie, BuildKit cache mounts |
| `.dockerignore` | Keeps Docker context lean |
| `release-please-config.json` | Rust release type with pre-major bump config |
| `.release-please-manifest.json` | Version tracking (starts at 0.1.0) |
| `LICENSE-MIT`, `LICENSE-APACHE` | Dual MIT/Apache-2.0 licensing |
| `docs/adr/` | Architecture Decision Records directory |

## Enabling container builds

The Docker build & push job in `build.yml` is **commented out by default** to prevent
accidentally publishing a scaffolding image. To enable it:

1. Update the `IMAGE` env var in `build.yml` to match your repo
2. Uncomment the `build` job
3. Ensure your repo has `packages: write` permissions for GITHUB_TOKEN

## CI details

All third-party GitHub Actions are **pinned to commit SHAs** (not tags) to prevent
supply chain attacks. Top-level `permissions: {}` with per-job least-privilege grants.

- **test**: `cargo fmt --check` + `cargo clippy` + `cargo nextest run`
- **audit**: `cargo audit` for known vulnerabilities
- **build** (when enabled): Docker build with SLSA provenance and SBOM attestations

## ADRs

Architecture Decision Records live in `docs/adr/`. See
[ADR-0000](docs/adr/0000-use-adrs.md) for the rationale.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT License](LICENSE-MIT) at your option.
