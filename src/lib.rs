//! # ec-uns-cmd — reusable EdgeCommons UNS command sender (library core)
//!
//! **One-liner purpose**: the pure, broker-free logic behind the `ec-uns-cmd` binary — CLI
//! surface, UNS `cmd/{verb}` request-topic construction, request-body parsing, and reply
//! interpretation. The [`crate`] binary ([`main`](../main.rs)) wires these to a live
//! EdgeCommons runtime; everything here is unit-testable without a broker.
//!
//! ## What the tool does
//! The EdgeCommons UNS `cmd` wire is a protobuf (prost) envelope, so operators and tests
//! cannot drive a component's commands with raw JSON over MQTT. `ec-uns-cmd` builds the proper
//! protobuf `cmd` request envelope through the library's [`MessageBuilder`], publishes it on
//! the component's UNS command topic
//! (`ecv1/{device}/{component}[/{instance}]/cmd/{verb}`, per the UNS grammar D-U28), and
//! awaits the correlated reply within a framework deadline via
//! [`MessagingService::request_with_timeout`]. Every EdgeCommons component has a
//! library-owned `commands()` inbox, so this drives ANY component: the built-ins
//! (`ping`, `describe`, `status`, `reload-config`, `get-configuration`) and any custom verb
//! (`sb/status`, `sb/read`, `sb/write`, …) with an arbitrary JSON body.
//!
//! ## Reply contract (mirrored from the inbox — `edgecommons::commands`)
//! A reply body is `{"ok": true, "result": <object>}` on success or
//! `{"ok": false, "error": {"code": <CODE>, "message": <text>}}` on a coded failure.
//! [`interpret_reply`] maps that to [`ReplyOutcome`]; the binary turns it into stdout/stderr
//! and a process exit code ([`ExitCode`]).
//!
//! [`MessageBuilder`]: edgecommons::messaging::message::MessageBuilder
//! [`MessagingService::request_with_timeout`]: edgecommons::messaging::MessagingService::request_with_timeout

use anyhow::{Context, bail};
use clap::Parser;
use edgecommons::messaging::message::{HierEntry, MessageIdentity};
use edgecommons::uns::{Uns, UnsClass};
use serde_json::{Value, json};

/// The default MQTT port for a plaintext broker (`--broker host` with no `:port`).
pub const DEFAULT_MQTT_PORT: u16 = 1883;
/// The default MQTT port for a TLS broker (`--tls` with no explicit `:port`).
pub const DEFAULT_MQTTS_PORT: u16 = 8883;
/// The default request deadline in seconds.
pub const DEFAULT_TIMEOUT_SECS: u64 = 10;
/// The tool's own default UNS component token (its client identity).
pub const DEFAULT_CLIENT_COMPONENT: &str = "ec-uns-cmd";
/// The tool's own default device (thing) identity.
pub const DEFAULT_CLIENT_DEVICE: &str = "ec-uns-cmd";

/// Send a command to an EdgeCommons component's `cmd` inbox over the UNS and print the reply.
///
/// The tool connects to an MQTT broker as an EdgeCommons client, builds the protobuf
/// `cmd/{verb}` request envelope through the library, publishes it on the target's UNS
/// command topic, awaits the correlated reply within the deadline, and prints the reply's
/// `result` (or `error`) as JSON. Works for any component and any verb.
#[derive(Debug, Parser)]
#[command(
    name = "ec-uns-cmd",
    about = "Send a command to an EdgeCommons component's cmd inbox over the UNS and print the reply.",
    version
)]
pub struct Cli {
    /// Broker as `host:port` (port defaults to 1883, or 8883 with --tls).
    #[arg(long, default_value = "localhost:1883")]
    pub broker: String,

    /// Connect over TLS. Supply --ca (and --cert/--key for mutual TLS).
    #[arg(long)]
    pub tls: bool,

    /// CA certificate path (PEM) for TLS server verification.
    #[arg(long)]
    pub ca: Option<String>,

    /// Client certificate path (PEM) for mutual TLS.
    #[arg(long)]
    pub cert: Option<String>,

    /// Client private-key path (PEM) for mutual TLS.
    #[arg(long)]
    pub key: Option<String>,

    /// Reuse an existing edgecommons messaging config file instead of synthesizing one
    /// from --broker/--tls (a `{"messaging":{"local":{...}}}` JSON document).
    #[arg(long, value_name = "FILE")]
    pub config: Option<String>,

    /// Target component's device (thing) identity — the `{device}` topic level.
    #[arg(long)]
    pub device: String,

    /// Target component token — the `{component}` topic level.
    #[arg(long)]
    pub component: String,

    /// Target instance — the optional `{instance}` topic level. Omit for a
    /// component-scoped command (`ecv1/{device}/{component}/cmd/{verb}`).
    #[arg(long)]
    pub instance: Option<String>,

    /// The command verb (the `cmd/{verb}` channel): `ping`, `describe`, `status`,
    /// `sb/status`, `sb/read`, `sb/write`, … `/`-namespaced verbs are allowed.
    pub verb: String,

    /// The request body as a JSON object (default `{}`).
    #[arg(long, value_name = "JSON")]
    pub body: Option<String>,

    /// Request deadline in seconds.
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS)]
    pub timeout: u64,

    /// Print the full reply body `{ok, result|error}` instead of just result/error.
    #[arg(long)]
    pub json: bool,

    /// The tool's own UNS component token (its client identity).
    #[arg(long, default_value = DEFAULT_CLIENT_COMPONENT)]
    pub client_component: String,

    /// The tool's own device (thing) identity.
    #[arg(long, default_value = DEFAULT_CLIENT_DEVICE)]
    pub client_device: String,

    /// Verbose library/tool logging on stderr (default: warnings only).
    #[arg(long, short = 'v')]
    pub verbose: bool,
}

/// Process exit codes (documented in `docs/reference/cli.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitCode {
    /// The reply was `{"ok": true, ...}`.
    Success = 0,
    /// The reply was `{"ok": false, "error": {...}}` (a coded command failure).
    CommandError = 1,
    /// A usage/argument error (also clap's own exit code).
    Usage = 2,
    /// A connection/transport/request failure that was not a timeout.
    RequestFailed = 3,
    /// No reply arrived within the deadline.
    Timeout = 4,
    /// A reply arrived but was not the `{ok, result|error}` shape.
    MalformedReply = 5,
}

impl ExitCode {
    /// The integer this code passes to the OS.
    pub const fn code(self) -> i32 {
        self as i32
    }
}

/// A `host:port` broker endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broker {
    /// The broker hostname.
    pub host: String,
    /// The broker TCP port.
    pub port: u16,
}

/// Split a `host:port` (or bare `host`) broker string, defaulting the port by TLS.
///
/// The port is taken after the LAST `:` so IPv6-free `host:port` forms parse; a bare `host`
/// with no `:` takes the TLS-appropriate default. An empty host, empty port, or unparsable
/// port is an error.
///
/// # Errors
/// Returns an error when the host is empty or the port is not a `u16`.
pub fn parse_broker(raw: &str, tls: bool) -> anyhow::Result<Broker> {
    let default_port = if tls {
        DEFAULT_MQTTS_PORT
    } else {
        DEFAULT_MQTT_PORT
    };
    let (host, port) = match raw.rsplit_once(':') {
        Some((host, port)) => (
            host,
            port.parse::<u16>()
                .with_context(|| format!("broker port '{port}' is not a valid TCP port"))?,
        ),
        None => (raw, default_port),
    };
    if host.is_empty() {
        bail!("broker host is empty in '{raw}'");
    }
    Ok(Broker {
        host: host.to_string(),
        port,
    })
}

/// Build the concrete UNS command request topic for a target component + verb.
///
/// Produces `ecv1/{device}/{component}[/{instance}]/cmd/{verb}` through the library's
/// [`Uns::topic_for`] builder — the SAME construction the edge-console command gateway uses —
/// so the char-set/depth/leaf rules (UNS §2.2) are enforced at build time and a bad
/// device/component/instance/verb token fails here rather than producing an unpublishable
/// topic. A `None` instance yields the component-scoped topic (D-U28); the component's inbox
/// subscribes both scopes, so either is delivered.
///
/// # Errors
/// [`edgecommons::EdgeCommonsError::UnsValidation`] on any token/grammar violation.
pub fn cmd_topic(
    device: &str,
    component: &str,
    instance: Option<&str>,
    verb: &str,
) -> edgecommons::Result<String> {
    let identity = MessageIdentity::new(
        vec![HierEntry {
            level: "device".to_string(),
            value: device.to_string(),
        }],
        component,
        instance.map(str::to_string),
    )?;
    // Rootless (single-level device hierarchy): includeRoot is a no-op here (D-U25).
    let uns = Uns::new(identity.clone(), false);
    uns.topic_for(&identity, UnsClass::Cmd, Some(verb))
}

/// Parse the `--body` argument into the request body object.
///
/// `None` or an all-whitespace string yields an empty object `{}`. Any other input must be a
/// JSON **object** (the command-argument contract); a JSON scalar/array/string is rejected.
///
/// # Errors
/// Returns an error when the string is not valid JSON, or is valid JSON but not an object.
pub fn parse_body(raw: Option<&str>) -> anyhow::Result<Value> {
    match raw {
        None => Ok(json!({})),
        Some(s) if s.trim().is_empty() => Ok(json!({})),
        Some(s) => {
            let value: Value =
                serde_json::from_str(s).context("--body must be a valid JSON object")?;
            if !value.is_object() {
                bail!(
                    "--body must be a JSON object, got a JSON {}",
                    json_kind(&value)
                );
            }
            Ok(value)
        }
    }
}

/// The JSON kind name for an error message.
fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The interpreted outcome of a reply envelope's body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyOutcome {
    /// `{"ok": true, "result": <object>}` — carries the result object (or `{}`).
    Ok(Value),
    /// `{"ok": false, "error": {"code", "message"}}` — a coded command failure.
    Err {
        /// The machine-readable error code.
        code: String,
        /// The human-readable error message.
        message: String,
    },
    /// The body was not the `{ok, result|error}` shape.
    Malformed(Value),
}

/// Interpret a reply envelope's body as a [`ReplyOutcome`] (mirrors the console's mapping and
/// the `edgecommons::commands` inbox reply contract).
pub fn interpret_reply(body: &Value) -> ReplyOutcome {
    let Some(obj) = body.as_object() else {
        return ReplyOutcome::Malformed(body.clone());
    };
    match obj.get("ok").and_then(Value::as_bool) {
        Some(true) => ReplyOutcome::Ok(obj.get("result").cloned().unwrap_or_else(|| json!({}))),
        Some(false) => {
            let err = obj.get("error").and_then(Value::as_object);
            let code = err
                .and_then(|e| e.get("code"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or("ERROR")
                .to_string();
            let message = err
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            ReplyOutcome::Err { code, message }
        }
        _ => ReplyOutcome::Malformed(body.clone()),
    }
}

/// Classify a request error string into the timeout vs other-failure exit code. The library
/// surfaces a request deadline as [`edgecommons::EdgeCommonsError::RequestTimeout`], whose
/// `Display` contains "RequestTimeout"/"timed out" — the same signal the console keys on.
pub fn classify_request_error(message: &str) -> ExitCode {
    if message.contains("RequestTimeout")
        || message.contains("timed out")
        || message.contains("timeout")
    {
        ExitCode::Timeout
    } else {
        ExitCode::RequestFailed
    }
}

/// Build the synthesized standalone messaging config document (`{"messaging":{"local":{…}}}`)
/// for a broker + optional TLS credentials, matching the reference adapters'
/// `standalone-messaging.json` shape.
pub fn messaging_config_doc(
    broker: &Broker,
    client_id: &str,
    tls: bool,
    ca: Option<&str>,
    cert: Option<&str>,
    key: Option<&str>,
) -> Value {
    let mut local = json!({
        "host": broker.host,
        "port": broker.port,
        "clientId": client_id,
    });
    if tls || ca.is_some() || cert.is_some() || key.is_some() {
        let mut creds = serde_json::Map::new();
        if let Some(ca) = ca {
            creds.insert("caPath".to_string(), json!(ca));
        }
        if let Some(cert) = cert {
            creds.insert("certPath".to_string(), json!(cert));
        }
        if let Some(key) = key {
            creds.insert("keyPath".to_string(), json!(key));
        }
        local["credentials"] = Value::Object(creds);
    }
    json!({ "messaging": { "local": local } })
}

/// Build the synthesized component config document for the tool's OWN client identity.
///
/// The tool is a real EdgeCommons component acting as a client: it needs an identity so the
/// library stamps the request envelope's `identity`/`reply_to`. Heartbeat is disabled and
/// metrics go to the log so the tool stays quiet on the bus — it sends one request and exits.
pub fn client_config_doc(client_component: &str, level: &str) -> Value {
    json!({
        "logging": { "level": level },
        "heartbeat": { "enabled": false },
        "metricEmission": { "target": "log", "namespace": "edgecommons" },
        "component": { "token": client_component }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    // ---- parse_broker ----
    #[test]
    fn broker_host_and_port_split() {
        let b = parse_broker("emqx.local:1884", false).unwrap();
        assert_eq!(b.host, "emqx.local");
        assert_eq!(b.port, 1884);
    }

    #[test]
    fn broker_defaults_port_by_tls() {
        assert_eq!(parse_broker("localhost", false).unwrap().port, 1883);
        assert_eq!(parse_broker("localhost", true).unwrap().port, 8883);
    }

    #[test]
    fn broker_rejects_empty_host_and_bad_port() {
        assert!(parse_broker(":1883", false).is_err());
        assert!(parse_broker("host:notaport", false).is_err());
        assert!(parse_broker("host:99999999", false).is_err());
    }

    // ---- cmd_topic ----
    #[test]
    fn component_scope_topic_omits_instance() {
        let t = cmd_topic("gw-01", "ethernet-ip-adapter", None, "sb/status").unwrap();
        assert_eq!(t, "ecv1/gw-01/ethernet-ip-adapter/cmd/sb/status");
    }

    #[test]
    fn instance_scope_topic_includes_instance() {
        let t = cmd_topic("gw-01", "opcua-adapter", Some("kep1"), "ping").unwrap();
        assert_eq!(t, "ecv1/gw-01/opcua-adapter/kep1/cmd/ping");
    }

    #[test]
    fn builtin_and_namespaced_verbs_build() {
        assert_eq!(
            cmd_topic("d", "c", None, "describe").unwrap(),
            "ecv1/d/c/cmd/describe"
        );
        assert_eq!(
            cmd_topic("d", "c", None, "sb/read").unwrap(),
            "ecv1/d/c/cmd/sb/read"
        );
    }

    #[test]
    fn bad_tokens_are_rejected_at_build_time() {
        // A '+' in a token violates the UNS char-set rule.
        assert!(cmd_topic("gw+1", "c", None, "ping").is_err());
        assert!(cmd_topic("d", "c", None, "sb/#bad").is_err());
        // Too many channel tokens blow the IoT-Core 7-slash depth budget.
        assert!(cmd_topic("d", "c", None, "a/b/c/d/e").is_err());
    }

    // ---- parse_body ----
    #[test]
    fn body_defaults_to_empty_object() {
        assert_eq!(parse_body(None).unwrap(), json!({}));
        assert_eq!(parse_body(Some("   ")).unwrap(), json!({}));
    }

    #[test]
    fn body_parses_json_object() {
        let v = parse_body(Some(r#"{"instance":"filler-plc","max":50}"#)).unwrap();
        assert_eq!(v["instance"], "filler-plc");
        assert_eq!(v["max"], 50);
    }

    #[test]
    fn body_rejects_non_object_and_bad_json() {
        assert!(parse_body(Some("[1,2,3]")).is_err());
        assert!(parse_body(Some("42")).is_err());
        assert!(parse_body(Some("\"hi\"")).is_err());
        assert!(parse_body(Some("{not json")).is_err());
    }

    // ---- interpret_reply ----
    #[test]
    fn reply_ok_carries_result() {
        let out = interpret_reply(&json!({"ok": true, "result": {"status": "RUNNING"}}));
        assert_eq!(out, ReplyOutcome::Ok(json!({"status": "RUNNING"})));
    }

    #[test]
    fn reply_ok_without_result_is_empty_object() {
        assert_eq!(
            interpret_reply(&json!({"ok": true})),
            ReplyOutcome::Ok(json!({}))
        );
    }

    #[test]
    fn reply_error_carries_code_and_message() {
        let out = interpret_reply(&json!({
            "ok": false,
            "error": {"code": "WRITE_NOT_ALLOWED", "message": "not in writes.allow"}
        }));
        assert_eq!(
            out,
            ReplyOutcome::Err {
                code: "WRITE_NOT_ALLOWED".to_string(),
                message: "not in writes.allow".to_string()
            }
        );
    }

    #[test]
    fn reply_error_defaults_code_when_missing() {
        let out = interpret_reply(&json!({"ok": false}));
        assert_eq!(
            out,
            ReplyOutcome::Err {
                code: "ERROR".to_string(),
                message: String::new()
            }
        );
    }

    #[test]
    fn reply_without_ok_flag_is_malformed() {
        assert!(matches!(
            interpret_reply(&json!({"result": {}})),
            ReplyOutcome::Malformed(_)
        ));
        assert!(matches!(
            interpret_reply(&json!("nope")),
            ReplyOutcome::Malformed(_)
        ));
    }

    // ---- classify_request_error ----
    #[test]
    fn timeout_errors_map_to_timeout_code() {
        assert_eq!(
            classify_request_error("RequestTimeout { topic: ..., secs: 10.0 }"),
            ExitCode::Timeout
        );
        assert_eq!(
            classify_request_error("the request timed out"),
            ExitCode::Timeout
        );
        assert_eq!(
            classify_request_error("connection refused"),
            ExitCode::RequestFailed
        );
    }

    #[test]
    fn exit_codes_are_stable() {
        assert_eq!(ExitCode::Success.code(), 0);
        assert_eq!(ExitCode::CommandError.code(), 1);
        assert_eq!(ExitCode::Usage.code(), 2);
        assert_eq!(ExitCode::RequestFailed.code(), 3);
        assert_eq!(ExitCode::Timeout.code(), 4);
        assert_eq!(ExitCode::MalformedReply.code(), 5);
    }

    // ---- config docs ----
    #[test]
    fn messaging_config_plaintext_has_no_credentials() {
        let b = Broker {
            host: "localhost".into(),
            port: 1883,
        };
        let doc = messaging_config_doc(&b, "ec-uns-cmd-1", false, None, None, None);
        assert_eq!(doc["messaging"]["local"]["host"], "localhost");
        assert_eq!(doc["messaging"]["local"]["port"], 1883);
        assert_eq!(doc["messaging"]["local"]["clientId"], "ec-uns-cmd-1");
        assert!(doc["messaging"]["local"].get("credentials").is_none());
    }

    #[test]
    fn messaging_config_tls_carries_credential_paths() {
        let b = Broker {
            host: "iot.example".into(),
            port: 8883,
        };
        let doc = messaging_config_doc(
            &b,
            "ec-uns-cmd-2",
            true,
            Some("ca.pem"),
            Some("c.pem"),
            Some("k.pem"),
        );
        let creds = &doc["messaging"]["local"]["credentials"];
        assert_eq!(creds["caPath"], "ca.pem");
        assert_eq!(creds["certPath"], "c.pem");
        assert_eq!(creds["keyPath"], "k.pem");
    }

    #[test]
    fn client_config_disables_heartbeat_and_sets_token() {
        let doc = client_config_doc("ec-uns-cmd", "WARN");
        assert_eq!(doc["heartbeat"]["enabled"], false);
        assert_eq!(doc["component"]["token"], "ec-uns-cmd");
        assert_eq!(doc["logging"]["level"], "WARN");
    }

    // ---- clap wiring ----
    #[test]
    fn cli_parses_minimal_invocation() {
        let cli = Cli::try_parse_from([
            "ec-uns-cmd",
            "--broker",
            "localhost:1883",
            "--device",
            "gw-01",
            "--component",
            "ethernet-ip-adapter",
            "sb/status",
        ])
        .unwrap();
        assert_eq!(cli.device, "gw-01");
        assert_eq!(cli.component, "ethernet-ip-adapter");
        assert_eq!(cli.verb, "sb/status");
        assert_eq!(cli.instance, None);
        assert_eq!(cli.timeout, DEFAULT_TIMEOUT_SECS);
        assert!(!cli.json);
    }

    #[test]
    fn cli_parses_full_invocation() {
        let cli = Cli::try_parse_from([
            "ec-uns-cmd",
            "--broker",
            "emqx:8883",
            "--tls",
            "--ca",
            "ca.pem",
            "--device",
            "gw-01",
            "--component",
            "ethernet-ip-adapter",
            "--instance",
            "filler-plc",
            "--body",
            r#"{"instance":"filler-plc"}"#,
            "--timeout",
            "30",
            "--json",
            "sb/write",
        ])
        .unwrap();
        assert!(cli.tls);
        assert_eq!(cli.ca.as_deref(), Some("ca.pem"));
        assert_eq!(cli.instance.as_deref(), Some("filler-plc"));
        assert_eq!(cli.timeout, 30);
        assert!(cli.json);
        assert_eq!(cli.verb, "sb/write");
        assert_eq!(cli.body.as_deref(), Some(r#"{"instance":"filler-plc"}"#));
    }

    #[test]
    fn cli_requires_device_component_and_verb() {
        assert!(Cli::try_parse_from(["ec-uns-cmd", "ping"]).is_err()); // no device/component
        assert!(
            Cli::try_parse_from(["ec-uns-cmd", "--device", "d", "--component", "c"]).is_err()
        ); // no verb
    }
}
