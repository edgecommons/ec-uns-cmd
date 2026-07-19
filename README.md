# uns-cmd — EdgeCommons UNS command tool

`uns-cmd` sends a command to any EdgeCommons component's `cmd` inbox over the Unified Namespace
(UNS) and prints the correlated reply. It is a general ecosystem utility: every EdgeCommons
component has a library-owned `commands()` inbox, so one tool drives them all.

The UNS `cmd` wire is a protobuf (prost) envelope, so operators and tests cannot drive a
component's commands with raw JSON over MQTT. `uns-cmd` builds the proper protobuf `cmd/{verb}`
request envelope through the EdgeCommons library, publishes it on the component's UNS command
topic, awaits the correlated reply within a deadline, and prints the reply's `result` (or
`error`) as JSON.

## What it does

- Connects to an MQTT broker as an EdgeCommons client (HOST platform, MQTT transport) — the same
  client bring-up the edge-console gateway uses.
- Builds the request topic `ecv1/{device}/{component}[/{instance}]/cmd/{verb}` through the
  library's UNS topic builder (char-set and depth validated at build time).
- Builds the protobuf `cmd` envelope through `MessageBuilder` (header `name` = the verb, body =
  your JSON), stamped with the tool's own identity and a generated `reply_to`.
- Issues `MessagingService::request_with_timeout` and awaits the correlated reply within the
  deadline.
- Prints the reply and exits non-zero on `ok:false`, timeout, or transport failure.

It works for arbitrary verbs and arbitrary JSON bodies — the library built-ins (`ping`,
`describe`, `status`, `reload-config`, `get-configuration`) and any component verb
(`sb/status`, `sb/read`, `sb/write`, …).

## Install

`uns-cmd` is a Rust binary crate. It depends on the EdgeCommons Rust library by a pinned git
revision.

```bash
cargo build --release
# binary at target/release/uns-cmd
```

For local development against a sibling `edgecommons` checkout, create a gitignored
`.cargo/config.toml` that patches the git dependency to your local library path (this repo ships
one for the reference layout):

```toml
[patch."https://github.com/edgecommons/edgecommons"]
edgecommons = { path = "../edgecommons/core/libs/rust" }

[net]
git-fetch-with-cli = true
```

## Usage

```text
uns-cmd --broker <host:port> [--tls --ca <ca.pem> --cert <c.pem> --key <k.pem>] \
        --device <device> --component <component> [--instance <instance>] \
        <verb> [--body '<json>'] [--timeout <secs>] [--json]
```

Every flag is documented in [`docs/reference/cli.md`](docs/reference/cli.md).

### Examples (driving a component)

Ask a component if it is responsive and read its uptime:

```bash
uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter ping
# { "status": "RUNNING", "uptimeSecs": 43 }
```

Discover a component's verbs and console panels:

```bash
uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter describe
# { "commands": [ { "verb": "sb/status", "builtIn": false }, … ], "panels": { … } }
```

Query a southbound adapter's instance status:

```bash
uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter sb/status
# { "id": "filler-plc", "mode": "poll", "connected": true, "state": "ONLINE", "paused": false, … }
```

Pause and resume an adapter's telemetry production:

```bash
uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter sb/pause
# { "id": "filler-plc", "paused": true, "changed": true }
uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter sb/resume
# { "id": "filler-plc", "paused": false, "changed": true }
```

On-demand read of live values:

```bash
uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter \
        sb/read --body '{"signals":[{"name":"line-speed"},{"name":"tank-level"}]}'
```

Confirmed, allow-listed write (a non-allow-listed signal is refused inline):

```bash
uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter \
        sb/write --body '{"writes":[{"tagPath":"FILL_SETPOINT","type":"real","value":55.5}]}'
```

Target one instance of a multi-instance component by the topic instance slot:

```bash
uns-cmd --broker localhost:1883 --device gw-01 --component opcua-adapter --instance kep1 ping
```

## How it fits the ecosystem

- **UNS command surface.** EdgeCommons components expose commands as request/reply verbs on their
  `cmd` inbox at `ecv1/{device}/{component}[/{instance}]/cmd/{verb}`. The reply body is
  `{"ok": true, "result": …}` or `{"ok": false, "error": {"code", "message"}}`. `uns-cmd` speaks
  exactly that contract.
- **The same request path the console uses.** The tool mirrors the edge-console command gateway:
  build the topic with the library's UNS builder, build the envelope with `MessageBuilder`,
  `request_with_timeout`, and interpret the `{ok, result|error}` reply.
- **Scriptable and CI-friendly.** stdout carries only the reply JSON (logs go to stderr); the
  process exit code reflects the outcome, so `uns-cmd` slots into shell pipelines and end-to-end
  tests that need to drive a component's command surface over the real bus.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Reply was `{"ok": true, …}`. |
| 1 | Reply was `{"ok": false, "error": …}` (a coded command failure). |
| 2 | Usage / argument error. |
| 3 | Connection / transport / request failure (not a timeout). |
| 4 | No reply within the deadline. |
| 5 | Reply was not the `{ok, result|error}` shape. |

## Development

```bash
cargo build
cargo clippy --all-targets -- -D warnings
cargo test          # unit tests + a self-skipping live round-trip (needs a broker)
```

The live integration test (`tests/live_roundtrip.rs`) probes `UNS_CMD_TEST_BROKER` (default
`localhost:1883`) and skips when no broker is up; with one running it stands up a real component
and performs a `ping`/`describe` round-trip over the protobuf bus.

## License

Business Source License 1.1 (BUSL-1.1) — see [`LICENSE`](LICENSE). Licensed Work: *EdgeCommons UNS
Command Tool*. Converts to MPL-2.0 four years after each version's publication.
