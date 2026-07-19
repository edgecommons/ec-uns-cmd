# uns-cmd documentation

`uns-cmd` sends a command to any EdgeCommons component's `cmd` inbox over the Unified Namespace and
prints the correlated reply. Every EdgeCommons component exposes a library-owned `commands()` inbox,
so one tool drives them all — the built-ins (`ping`, `describe`, `status`, `reload-config`,
`get-configuration`) and any component verb (`sb/status`, `sb/read`, `sb/write`, …).

The UNS `cmd` wire is a protobuf envelope. `uns-cmd` builds that envelope through the EdgeCommons
library, publishes it on the component's command topic
(`ecv1/{device}/{component}[/{instance}]/cmd/{verb}`), awaits the correlated reply within a
deadline, and prints the reply's `result` or `error` as JSON.

## Pages

- **[Tutorial](tutorial.md)** — a guided first session: bring up a broker and a component, then send
  `ping`, `describe`, and `sb/status`.
- **[How-to guides](how-to-guides.md)** — task recipes: read and write signals, pause and resume an
  adapter, target one instance, connect over TLS, and use `uns-cmd` in a test or pipeline.
- **[CLI reference](reference/cli.md)** — every flag, argument, exit code, and the request/reply
  contract.

## The reply contract

A reply body is one of:

```json
{ "ok": true, "result": { … } }
```

```json
{ "ok": false, "error": { "code": "…", "message": "…" } }
```

`uns-cmd` prints the `result` object on success and the `error` object on failure, and its process
exit code reflects the outcome (see the [CLI reference](reference/cli.md)).
