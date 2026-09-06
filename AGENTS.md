# ec-uns-cmd — tool notes

EdgeCommons **UNS command-line tool** (Rust). Repo/crate/bin `ec-uns-cmd`. Depends on the
`edgecommons` Rust library. Read the org umbrella `../AGENTS.md` first (platform matrix, validation
infra, local-dev sibling override).

## What it is

A standalone ecosystem utility that sends a command to any EdgeCommons component's `cmd` inbox over
the UNS and prints the correlated reply. The UNS `cmd` wire is a protobuf (prost) envelope, so this
tool builds the proper `cmd/{verb}` request envelope through the library, publishes it on the
component's UNS command topic, and awaits the correlated reply. It is general: every component has a
library-owned `commands()` inbox, so `ec-uns-cmd` drives the built-ins (`ping`, `describe`, `status`,
`reload-config`, `get-configuration`) and any component verb (`sb/*`, …) with an arbitrary JSON body.

## How it works (the real library APIs it uses)

It reuses the exact client request path the edge-console command gateway uses:

- **Client runtime.** `EdgeCommonsBuilder::new("ec-uns-cmd").args([...]).build().await` stands up a
  HOST/MQTT `EdgeCommons` runtime. The tool synthesizes two throwaway config files for its own
  client identity — a standalone messaging config (`{"messaging":{"local":{host,port,clientId}}}`,
  optional TLS `credentials`) and a minimal component config (heartbeat disabled) — and feeds them
  via the standard CLI contract `--platform HOST --transport MQTT <msg> -c FILE <cfg> -t <device>`.
- **Topic.** `Uns::topic_for(&target_identity, UnsClass::Cmd, Some(verb))` builds
  `ecv1/{device}/{component}[/{instance}]/cmd/{verb}` (D-U28: the instance slot is optional). The
  target identity is a `MessageIdentity::new([HierEntry{level:"device",value:device}], component,
  instance)`.
- **Envelope.** `MessageBuilder::new(verb, "1.0").from_config(&config).command(body).build()` — the
  protobuf command envelope, header `name` = the verb, stamped with the tool's identity.
- **Request/reply.** `MessagingService::request_with_timeout(&topic, msg, Some(timeout))` returns a
  `ReplyFuture`; awaiting it yields the reply `Message`. The library stamps `reply_to` +
  `correlation_id` and enforces the deadline.
- **Reply contract.** The reply body is `{"ok":true,"result":…}` or
  `{"ok":false,"error":{"code","message"}}` (from `edgecommons::commands`). The tool prints
  `result`/`error` and maps the outcome to an exit code (0/1/3/4/5).

## Layout

- `src/lib.rs` — the pure, broker-free logic (clap `Cli`, `cmd_topic`, `parse_body`,
  `interpret_reply`, `parse_broker`, config-doc synthesis, `ExitCode`) — fully unit-tested.
- `src/main.rs` — wires the logic to the live `EdgeCommons` runtime.
- `tests/live_roundtrip.rs` — self-skipping live `ping`/`describe` round-trip over a broker.

## Conventions

- **Depends on `edgecommons` by pinned git `rev`** (same rev the reference components pin). A
  gitignored `.cargo/config.toml` patches it to `../core/libs/rust` for dev; CI uses the
  pinned rev. Do NOT edit `.cargo/config.toml` or the pin as part of feature work.
- **Default feature `standalone`** (dual-broker MQTT). The tool is always an MQTT client.
- **stdout is the reply JSON only**; logs go to stderr (pipe-friendly).
- **BUSL-1.1**, `publish = false`.

## Validation

```bash
cargo build
cargo clippy --all-targets -- -D warnings
cargo test          # 23 unit tests + 1 self-skipping live round-trip
```

Live command-surface E2E: bring up a component (e.g. the `ethernet-ip-adapter` sim) against a local
EMQX (`docker start edgecommons-emqx`), then drive it with `ec-uns-cmd` (`sb/status`, `sb/pause`/
`sb/resume`, `sb/read`, `sb/write`, `describe`).
