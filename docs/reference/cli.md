# CLI reference

```text
ec-uns-cmd [OPTIONS] --device <DEVICE> --component <COMPONENT> <VERB>
```

`ec-uns-cmd` connects to an MQTT broker as an EdgeCommons client, builds the protobuf `cmd/{verb}`
request envelope, publishes it on the target's UNS command topic, awaits the correlated reply within
the deadline, and prints the reply's `result` (or `error`) as JSON.

## Argument

| Argument | Meaning |
|----------|---------|
| `<VERB>` | The command verb — the `cmd/{verb}` channel. A library built-in (`ping`, `describe`, `status`, `reload-config`, `get-configuration`) or any component verb (`sb/status`, `sb/read`, `sb/write`, …). `/`-namespaced verbs (`sb/read`) are allowed; each `/`-separated token must satisfy the UNS token rule (non-empty; no `/ + # \` or control characters; no `..`). |

## Options

| Option | Default | Meaning |
|--------|---------|---------|
| `--broker <HOST:PORT>` | `localhost:1883` | The MQTT broker. A bare `HOST` takes the default port (`1883`, or `8883` with `--tls`). |
| `--tls` | off | Connect over TLS. Supply `--ca` for server verification and `--cert`/`--key` for mutual TLS. |
| `--ca <FILE>` | — | CA certificate (PEM) for TLS server verification. |
| `--cert <FILE>` | — | Client certificate (PEM) for mutual TLS. |
| `--key <FILE>` | — | Client private key (PEM) for mutual TLS. |
| `--config <FILE>` | — | Reuse an existing edgecommons messaging config file (a `{"messaging":{"local":{…}}}` document) instead of synthesizing one from `--broker`/`--tls`. |
| `--device <DEVICE>` | required | The target component's device (thing) identity — the `{device}` topic level. |
| `--component <COMPONENT>` | required | The target component token — the `{component}` topic level. |
| `--instance <INSTANCE>` | — | The optional `{instance}` topic level. Omit for a component-scoped command (`ecv1/{device}/{component}/cmd/{verb}`). Present, the topic is instance-scoped (`ecv1/{device}/{component}/{instance}/cmd/{verb}`). |
| `--body <JSON>` | `{}` | The request body as a JSON **object**. A non-object (array, scalar, string) or invalid JSON is a usage error. |
| `--timeout <SECS>` | `10` | The request deadline in seconds. No reply within it exits `4`. |
| `--json` | off | Print the full reply body `{ok, result|error}` instead of just the `result`/`error` object. |
| `--client-component <TOKEN>` | `ec-uns-cmd` | The tool's own UNS component token (its client identity — the sender stamped on the request). |
| `--client-device <NAME>` | `ec-uns-cmd` | The tool's own device (thing) identity. |
| `-v`, `--verbose` | off | Verbose library/tool logging on stderr (default: warnings only). |
| `-h`, `--help` | — | Print help. |
| `-V`, `--version` | — | Print the version. |

The `RUST_LOG` environment variable overrides the log filter when set.

## The request topic

`ec-uns-cmd` builds `ecv1/{device}/{component}[/{instance}]/cmd/{verb}` through the EdgeCommons UNS
topic builder. The instance slot is optional (UNS D-U28): omit `--instance` for a component-scoped
command, which every component's inbox also serves. The topic is validated at build time — a bad
device/component/instance/verb token fails before any network I/O.

## The request envelope

The request is a protobuf `cmd` envelope built through the library's `MessageBuilder`: `header.name`
is the verb, `header.version` is `1.0`, the body is your `--body` object, and the envelope carries
the tool's own `identity` plus a generated `reply_to` reply topic. The library stamps the
`correlation_id` and enforces the deadline.

## The reply

The reply body is the inbox's structured shape:

- Success — `{"ok": true, "result": <object>}`. `ec-uns-cmd` prints `result` (or the full body with
  `--json`) and exits `0`.
- Coded failure — `{"ok": false, "error": {"code": <CODE>, "message": <text>}}`. `ec-uns-cmd` prints
  `error` to stderr and exits `1`.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Reply was `{"ok": true, …}`. |
| `1` | Reply was `{"ok": false, "error": …}` (a coded command failure — e.g. `UNKNOWN_VERB`, `WRITE_NOT_ALLOWED`). |
| `2` | Usage / argument error (missing required flag, non-object `--body`, invalid JSON). |
| `3` | Connection / transport / request failure that was not a timeout (e.g. the broker is unreachable). |
| `4` | No reply arrived within `--timeout`. |
| `5` | A reply arrived but was not the `{ok, result|error}` shape. |

## Output streams

stdout carries only the reply JSON, so the tool composes in pipelines. Logs, diagnostics, and error
summaries go to stderr.
