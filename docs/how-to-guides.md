# How-to guides

Task recipes for `ec-uns-cmd`. Each assumes a broker at `localhost:1883` and a target component. Adjust
`--device` and `--component` to your target.

## Send a verb with a JSON body

Pass the request arguments as a JSON object with `--body`:

```bash
ec-uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter \
        sb/read --body '{"signals":[{"name":"line-speed"},{"name":"tank-level"}]}'
```

The body must be a JSON object. Verbs that take no arguments need no `--body` (it defaults to `{}`).

## Read live signal values

```bash
ec-uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter \
        sb/read --body '{"signals":[{"name":"line-speed"}]}'
```

```json
{
  "id": "filler-plc",
  "reads": [
    { "signal": { "id": "LINE_SPEED", "address": { "tagPath": "LINE_SPEED", "type": "real" } },
      "value": 23.97, "quality": "GOOD", "qualityRaw": "0x00", "serverTs": "2026-07-19T12:58:59Z" }
  ]
}
```

## Write a signal (confirmed, allow-listed)

A write to an allow-listed signal is confirmed; a write to a signal outside the adapter's
`writes.allow` list is refused inline:

```bash
ec-uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter \
        sb/write --body '{"writes":[
          {"tagPath":"FILL_SETPOINT","type":"real","value":55.5},
          {"tagPath":"LINE_SPEED","type":"real","value":10.0}
        ]}'
```

```json
{
  "id": "filler-plc",
  "written": 1,
  "results": [
    { "signal": "FILL_SETPOINT", "value": 55.5, "ok": true },
    { "signal": "LINE_SPEED", "ok": false, "error": "not in writes.allow" }
  ]
}
```

When every entry is refused, the whole command fails with error code `WRITE_NOT_ALLOWED` and
`ec-uns-cmd` exits `1`.

## Pause and resume an adapter

Pausing an adapter suspends its telemetry production; its `data` topics go quiet, its status reads
`PAUSED`, and it publishes an `adapter-paused` event. Both verbs are idempotent (`changed` reports
whether the state actually changed):

```bash
ec-uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter sb/pause
# { "id": "filler-plc", "paused": true, "changed": true }

ec-uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter sb/resume
# { "id": "filler-plc", "paused": false, "changed": true }
```

## Target a single instance

`--instance` addresses the topic's instance slot, producing
`ecv1/{device}/{component}/{instance}/cmd/{verb}`:

```bash
ec-uns-cmd --broker localhost:1883 --device gw-01 --component opcua-adapter --instance kep1 ping
```

Some adapters (including `ethernet-ip-adapter`) instead route by an `"instance"` field in the body
and accept commands on the component-scoped topic. Consult the component's own command docs; use
`--instance` for the topic slot and `--body` for body-routed instances.

## Connect over TLS

Supply the CA (and, for mutual TLS, the client cert and key). The port defaults to `8883` with
`--tls`:

```bash
ec-uns-cmd --broker mybroker:8883 --tls --ca ca.pem --cert client.pem --key client.key \
        --device plant-line1 --component ethernet-ip-adapter ping
```

## Reuse an existing messaging config

Instead of `--broker`/`--tls`, point at an existing edgecommons messaging config file (a
`{"messaging":{"local":{…}}}` document):

```bash
ec-uns-cmd --config ./standalone-messaging.json \
        --device plant-line1 --component ethernet-ip-adapter ping
```

## Use it in a test or pipeline

stdout carries only the reply JSON (logs go to stderr), and the exit code reflects the outcome, so
`ec-uns-cmd` composes with `jq` and shell conditionals:

```bash
state=$(ec-uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter \
                sb/status | jq -r '.state')
[ "$state" = "ONLINE" ] || { echo "adapter not online: $state"; exit 1; }
```

Print the full `{ok, result|error}` reply body (not just `result`/`error`) with `--json`, and raise
the deadline for slow commands with `--timeout <secs>`.
