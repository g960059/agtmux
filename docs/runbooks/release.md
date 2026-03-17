# Release Runbook

## Channels

- Homebrew tap is the primary install path.
- Shell installer is the secondary path for Linux or non-Homebrew setups.
- `cargo install --locked agtmux` remains available for Rust users.

## Preconditions

- Run `just verify` from a clean worktree before tagging.
- Keep `agtmux --version` working without tmux so package smoke tests stay valid.
- Keep install and uninstall commands in `README.md` aligned with the shipped artifacts.

## Release Flow

1. Update the release version from a clean tree.
2. Run the local verification gate.
3. Push the tag and let CI build artifacts, checksums, attestations, and tap updates.

## Guardrails

- Do not add self-update behavior that fights the package manager.
- Prefer small, reversible release process changes and document operator-impacting updates here.
