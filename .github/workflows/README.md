# Workflows

## ci.yml — Lint, Check & Test

Runs formatting, clippy, cargo check, docs, native tests, wasm-pack tests, and integration tests.

- **Trigger**: push/PR to `dev` / `stage` / `master`
- **What it does**: fmt → clippy → check → doc → native tests → wasm tests → integration tests (Docker)

## release.yml — Production Release

Builds WASM package and publishes to npm as `@utexo/rgb-lib-wasm`.

- **Trigger**: manual (`workflow_dispatch`)
- **Input**: `version` (e.g. `0.3.0-beta.4`)
- **What it does**: `wasm-pack build` → set version → `npm publish` → GitHub Release
- **Intended branch**: `master`
- **npm tag**: `latest` (default)

## release-dev.yml — Dev Release

Same as production but with a custom suffix for testing.

- **Trigger**: manual (`workflow_dispatch`)
- **Inputs**: `version` (e.g. `0.3.0-beta.4`) + `suffix` (e.g. `test1`)
- **Published version**: `0.3.0-beta.4.test1`
- **npm tag**: value of `suffix` (not `latest`)
- **Intended branch**: `dev` or any feature branch

### Example

Run from `dev` with version `0.3.0-beta.4` and suffix `wasm-idb`:

```bash
npm install @utexo/rgb-lib-wasm@0.3.0-beta.4.wasm.idb
```

## Required secrets

- `NPM_TOKEN` — npm access token for publishing `@utexo/rgb-lib-wasm`
