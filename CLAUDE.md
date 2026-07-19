# uns-cmd — tool notes (Claude Code)

EdgeCommons **UNS command-line tool** (Rust). Repo/crate/bin `uns-cmd`. Depends on the
`edgecommons` Rust library. Read the org umbrella `../CLAUDE.md` first (platform matrix, validation
infra, local-dev sibling override).

The full guidance — what it is, the real library APIs it uses, the crate layout, conventions, and
validation — lives in `AGENTS.md` and is shared with every agent tool. It is imported here in full:

@AGENTS.md

## Claude-Code-specific setup (additive to AGENTS.md)

- **Build against the sibling library.** `.cargo/config.toml` (gitignored) patches the `edgecommons`
  git dep to the local `../edgecommons/core/libs/rust` checkout, so a plain `cargo build` /
  `cargo test` uses your working copy. CI keeps the pinned `rev` in `Cargo.toml`. Do NOT edit
  `.cargo/config.toml` or the `edgecommons` pin as part of feature work.
- **Local broker.** `docker start edgecommons-emqx` (localhost:1883 / 8883) is the broker the live
  test and the command-surface E2E use.
