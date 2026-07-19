//! Self-skipping LIVE integration test: a real protobuf `cmd` request/reply round-trip over
//! a running MQTT broker.
//!
//! It probes `UNS_CMD_TEST_BROKER` (default `localhost:1883`) and **skips** (prints and
//! returns) when no broker is listening — so `cargo test` is green on a machine with no
//! broker, and actually exercises the wire when one is up (`docker start edgecommons-emqx`).
//!
//! When a broker is up it stands up ONE EdgeCommons runtime (a real component with the
//! library-owned `commands()` inbox — `ping`/`describe`/`status` answer for free) and, using
//! the SAME request path the `uns-cmd` binary uses, sends `ping` and `describe` to that
//! component's OWN inbox and asserts the `{ok:true, result}` reply. `receiveOwnMessages`
//! defaults true, so a single runtime is both requester and responder — a self-contained,
//! broker-only proof of the protobuf request/reply wire and of `uns_cmd`'s topic + reply
//! logic.

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use edgecommons::EdgeCommonsBuilder;
use edgecommons::messaging::message::MessageBuilder;
use serde_json::{Value, json};
use tempfile::NamedTempFile;

use uns_cmd::{ReplyOutcome, client_config_doc, cmd_topic, interpret_reply, messaging_config_doc};

const TEST_DEVICE: &str = "uns-cmd-test";
const TEST_COMPONENT: &str = "uns-cmd-selftest";

fn broker_addr() -> String {
    std::env::var("UNS_CMD_TEST_BROKER").unwrap_or_else(|_| "localhost:1883".to_string())
}

/// True when a TCP connection to the broker succeeds within a short probe window.
fn broker_up(addr: &str) -> bool {
    let Ok(mut addrs) = addr.to_socket_addrs() else {
        return false;
    };
    addrs.any(|sa| TcpStream::connect_timeout(&sa, Duration::from_millis(500)).is_ok())
}

fn write_temp(doc: &Value) -> NamedTempFile {
    use std::io::Write;
    let mut f = tempfile::Builder::new()
        .suffix(".json")
        .tempfile()
        .expect("temp file");
    f.write_all(&serde_json::to_vec(doc).unwrap()).unwrap();
    f.flush().ok();
    f
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_and_describe_roundtrip_over_the_bus() {
    let addr = broker_addr();
    if !broker_up(&addr) {
        eprintln!(
            "SKIP live_roundtrip: no MQTT broker at {addr} \
             (start one, e.g. `docker start edgecommons-emqx`, or set UNS_CMD_TEST_BROKER)"
        );
        return;
    }

    let host_port: Vec<&str> = addr.rsplitn(2, ':').collect();
    let (port, host) = (
        host_port[0].parse::<u16>().unwrap_or(1883),
        host_port.get(1).copied().unwrap_or("localhost"),
    );
    let broker = uns_cmd::Broker {
        host: host.to_string(),
        port,
    };

    let messaging_doc = messaging_config_doc(
        &broker,
        &format!("uns-cmd-selftest-{}", std::process::id()),
        false,
        None,
        None,
        None,
    );
    let client_doc = client_config_doc(TEST_COMPONENT, "WARN");
    let messaging_cfg = write_temp(&messaging_doc);
    let client_cfg = write_temp(&client_doc);

    let argv: Vec<String> = vec![
        "uns-cmd-selftest".into(),
        "--platform".into(),
        "HOST".into(),
        "--transport".into(),
        "MQTT".into(),
        messaging_cfg.path().display().to_string(),
        "-c".into(),
        "FILE".into(),
        client_cfg.path().display().to_string(),
        "-t".into(),
        TEST_DEVICE.into(),
    ];

    let gg = Arc::new(
        EdgeCommonsBuilder::new("uns-cmd-selftest")
            .args(argv)
            .build()
            .await
            .expect("bring up the responder runtime against the live broker"),
    );
    let messaging = gg.messaging().expect("messaging");
    let config = gg.config();

    // Give the auto-started command inbox a moment to acknowledge its subscription.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ---- ping ----
    let ping_topic = cmd_topic(TEST_DEVICE, TEST_COMPONENT, None, "ping").unwrap();
    let ping_msg = MessageBuilder::new("ping", "1.0")
        .from_config(&config)
        .command(json!({}))
        .build();
    let ping_reply = messaging
        .request_with_timeout(&ping_topic, ping_msg, Some(Duration::from_secs(5)))
        .await
        .expect("ping request submitted")
        .await
        .expect("ping reply within deadline");
    match interpret_reply(&ping_reply.body) {
        ReplyOutcome::Ok(result) => {
            assert_eq!(result["status"], "RUNNING", "ping result: {result}");
        }
        other => panic!("ping did not return ok: {other:?}"),
    }

    // ---- describe ----
    let describe_topic = cmd_topic(TEST_DEVICE, TEST_COMPONENT, None, "describe").unwrap();
    let describe_msg = MessageBuilder::new("describe", "1.0")
        .from_config(&config)
        .command(json!({}))
        .build();
    let describe_reply = messaging
        .request_with_timeout(&describe_topic, describe_msg, Some(Duration::from_secs(5)))
        .await
        .expect("describe request submitted")
        .await
        .expect("describe reply within deadline");
    match interpret_reply(&describe_reply.body) {
        ReplyOutcome::Ok(result) => {
            // describe advertises the built-in verbs.
            let commands = result["commands"].as_array().expect("commands array");
            let verbs: Vec<&str> = commands
                .iter()
                .filter_map(|c| c["verb"].as_str())
                .collect();
            assert!(verbs.contains(&"ping"), "describe verbs: {verbs:?}");
            assert!(verbs.contains(&"describe"), "describe verbs: {verbs:?}");
        }
        other => panic!("describe did not return ok: {other:?}"),
    }

    drop(gg);
}
