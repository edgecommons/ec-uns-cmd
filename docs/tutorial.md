# Tutorial: your first command over the bus

This tutorial drives a live EdgeCommons component with `uns-cmd`. By the end you send `ping`,
discover a component's verbs with `describe`, and read a southbound adapter's status — all over the
real protobuf UNS command wire.

You need a Rust toolchain, an MQTT broker, and one EdgeCommons component to talk to.

## 1. Build uns-cmd

```bash
cargo build --release
# binary at target/release/uns-cmd
```

## 2. Start a broker

Any MQTT broker works. A local EMQX in Docker listens on `1883`:

```bash
docker run -d --name emqx -p 1883:1883 emqx/emqx:latest
```

## 3. Start a component

Bring up any EdgeCommons component against the same broker. Using the reference
`ethernet-ip-adapter` with its hardware-free `sim` backend:

```bash
ethernet-ip-adapter --platform HOST --transport MQTT standalone-messaging.json \
                    -c FILE config.json -t plant-line1
```

The adapter's device (thing) identity is `plant-line1`, its component token is
`ethernet-ip-adapter`, and it configures one instance, `filler-plc`.

## 4. Ping it

`ping` asks the component whether it is responsive and returns its uptime:

```bash
uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter ping
```

```json
{
  "status": "RUNNING",
  "uptimeSecs": 43
}
```

`uns-cmd` built the topic `ecv1/plant-line1/ethernet-ip-adapter/cmd/ping`, published a protobuf
`cmd` envelope with a generated reply topic, and printed the correlated reply. The exit code is `0`.

## 5. Discover its verbs

`describe` returns the component's command verbs and its console panels:

```bash
uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter describe
```

```json
{
  "commands": [
    { "verb": "ping", "builtIn": true },
    { "verb": "sb/status", "builtIn": false },
    { "verb": "sb/read", "builtIn": false },
    { "verb": "sb/write", "builtIn": false },
    { "verb": "sb/pause", "builtIn": false }
  ],
  "panels": { "views": [ … ] }
}
```

Every verb listed here is one you can send with `uns-cmd`.

## 6. Ask a southbound adapter for status

`sb/status` returns the adapter's per-instance connectivity and metrics:

```bash
uns-cmd --broker localhost:1883 --device plant-line1 --component ethernet-ip-adapter sb/status
```

```json
{
  "id": "filler-plc",
  "mode": "poll",
  "connected": true,
  "state": "ONLINE",
  "paused": false,
  "endpoint": "127.0.0.1:44818",
  "metrics": { "read": { "total": 150 }, "write": { "total": 0 } }
}
```

## Where to go next

- The [how-to guides](how-to-guides.md) cover reading and writing signals, pausing and resuming an
  adapter, targeting a single instance, and TLS.
- The [CLI reference](reference/cli.md) documents every flag and exit code.
