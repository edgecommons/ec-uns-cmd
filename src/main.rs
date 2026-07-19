//! # ec-uns-cmd — binary entry point
//!
//! Wires the pure logic in [`ec_uns_cmd`] to a live EdgeCommons client runtime: it stands up an
//! `EdgeCommons` HOST/MQTT runtime (the same client bring-up the edge-console gateway uses),
//! builds the protobuf `cmd/{verb}` request envelope through the library's `MessageBuilder`,
//! publishes it on the target's UNS command topic, and awaits the correlated reply via
//! `MessagingService::request_with_timeout`. The reply's `result`/`error` is printed as JSON
//! and mapped to a process exit code.
//!
//! The tool synthesizes two throwaway config files for its OWN client identity — a standalone
//! messaging config (broker host/port/TLS) and a minimal component config (heartbeat off) —
//! then feeds them to `EdgeCommonsBuilder` through the standard CLI contract
//! (`--platform HOST --transport MQTT <msg> -c FILE <cfg> -t <device>`). This is the real,
//! supported client path — no bespoke MQTT plumbing.

use std::io::Write;
use std::process::ExitCode as ProcExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use edgecommons::EdgeCommonsBuilder;
use edgecommons::messaging::message::MessageBuilder;
use serde_json::{Value, json};
use tempfile::NamedTempFile;

use ec_uns_cmd::{
    Cli, ExitCode, ReplyOutcome, classify_request_error, client_config_doc, cmd_topic,
    interpret_reply, messaging_config_doc, parse_body, parse_broker,
};

fn main() -> ProcExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("ec-uns-cmd: failed to start async runtime: {e}");
            return ProcExitCode::from(ExitCode::RequestFailed.code() as u8);
        }
    };

    let exit = runtime.block_on(run(cli));
    // Map our exit code onto the process exit status.
    ProcExitCode::from(exit.code() as u8)
}

/// The async body: connect, send, await, print, and return the exit code.
async fn run(cli: Cli) -> ExitCode {
    match try_run(&cli).await {
        Ok(exit) => exit,
        Err(e) => {
            // Argument/topic/body construction failures land here (broker-independent).
            eprintln!("ec-uns-cmd: {e:#}");
            ExitCode::RequestFailed
        }
    }
}

async fn try_run(cli: &Cli) -> anyhow::Result<ExitCode> {
    // 1. Resolve the target UNS command topic FIRST — a bad verb/target token fails before we
    //    ever touch the network.
    let topic = cmd_topic(
        &cli.device,
        &cli.component,
        cli.instance.as_deref(),
        &cli.verb,
    )
    .with_context(|| {
        format!(
            "invalid command target ecv1/{}/{}{}/cmd/{}",
            cli.device,
            cli.component,
            cli.instance
                .as_deref()
                .map(|i| format!("/{i}"))
                .unwrap_or_default(),
            cli.verb
        )
    })?;
    let body = parse_body(cli.body.as_deref())?;

    // 2. Synthesize the tool's own client config files (kept alive until after build()).
    let level = if cli.verbose { "DEBUG" } else { "WARN" };
    let client_cfg = write_temp_json(
        "ec-uns-cmd-config",
        &client_config_doc(&cli.client_component, level),
    )?;

    let messaging_path = match &cli.config {
        // Reuse an existing edgecommons messaging config file verbatim.
        Some(path) => TempOrPath::Path(path.clone()),
        None => {
            let broker = parse_broker(&cli.broker, cli.tls)?;
            let client_id = format!("{}-{}", cli.client_component, std::process::id());
            let doc = messaging_config_doc(
                &broker,
                &client_id,
                cli.tls,
                cli.ca.as_deref(),
                cli.cert.as_deref(),
                cli.key.as_deref(),
            );
            TempOrPath::Temp(write_temp_json("ec-uns-cmd-messaging", &doc)?)
        }
    };

    // 3. Stand up the EdgeCommons client runtime via the standard CLI contract.
    let argv: Vec<String> = vec![
        "ec-uns-cmd".to_string(),
        "--platform".to_string(),
        "HOST".to_string(),
        "--transport".to_string(),
        "MQTT".to_string(),
        messaging_path.path().to_string(),
        "-c".to_string(),
        "FILE".to_string(),
        client_cfg.path().display().to_string(),
        "-t".to_string(),
        cli.client_device.clone(),
    ];

    tracing::debug!(topic = %topic, broker = %cli.broker, "connecting to broker");
    let gg = match EdgeCommonsBuilder::new("ec-uns-cmd").args(argv).build().await {
        Ok(gg) => Arc::new(gg),
        Err(e) => {
            eprintln!(
                "ec-uns-cmd: could not connect / bring up the client runtime: {e}\n\
                 (is the broker reachable at {}?)",
                cli.broker
            );
            return Ok(ExitCode::RequestFailed);
        }
    };

    let messaging = gg.messaging().context("messaging service unavailable")?;
    let config = gg.config();

    // 4. Build the protobuf cmd request envelope through the library builder (the console's
    //    exact construction): header.name = verb, command body, stamped with our identity.
    let message = MessageBuilder::new(cli.verb.clone(), "1.0")
        .from_config(&config)
        .command(body)
        .build();

    // 5. Publish + await the correlated reply within the deadline.
    let timeout = Duration::from_secs(cli.timeout.max(1));
    tracing::debug!(topic = %topic, timeout_secs = cli.timeout, "sending request");
    let reply_result = async {
        let fut = messaging
            .request_with_timeout(&topic, message, Some(timeout))
            .await?;
        fut.await
    }
    .await;

    let exit = match reply_result {
        Ok(reply) => emit_reply(cli, &reply.body),
        Err(e) => {
            let code = classify_request_error(&e.to_string());
            match code {
                ExitCode::Timeout => eprintln!(
                    "ec-uns-cmd: no reply from ecv1/{}/{} for '{}' within {}s",
                    cli.device, cli.component, cli.verb, cli.timeout
                ),
                _ => eprintln!("ec-uns-cmd: request failed: {e}"),
            }
            code
        }
    };

    // 6. Drop the runtime (RAII unsubscribes the client's own inbox before exit).
    drop(gg);
    Ok(exit)
}

/// Print the reply per the interpreted outcome and return the matching exit code.
fn emit_reply(cli: &Cli, body: &Value) -> ExitCode {
    if cli.json {
        // Full reply body, whatever its shape.
        print_json(body);
    }
    match interpret_reply(body) {
        ReplyOutcome::Ok(result) => {
            if !cli.json {
                print_json(&result);
            }
            ExitCode::Success
        }
        ReplyOutcome::Err { code, message } => {
            if !cli.json {
                print_json_err(&json!({ "code": code, "message": message }));
            }
            eprintln!("ec-uns-cmd: command '{}' failed: {code}: {message}", cli.verb);
            ExitCode::CommandError
        }
        ReplyOutcome::Malformed(value) => {
            if !cli.json {
                print_json_err(&value);
            }
            eprintln!(
                "ec-uns-cmd: reply was not the {{ok, result|error}} shape (component '{}')",
                cli.component
            );
            ExitCode::MalformedReply
        }
    }
}

/// A synthesized temp file OR a caller-supplied path to reuse.
enum TempOrPath {
    Temp(NamedTempFile),
    Path(String),
}

impl TempOrPath {
    fn path(&self) -> std::borrow::Cow<'_, str> {
        match self {
            TempOrPath::Temp(f) => f.path().display().to_string().into(),
            TempOrPath::Path(p) => p.as_str().into(),
        }
    }
}

/// Write a JSON document to a named temp file (kept alive by the returned handle).
fn write_temp_json(prefix: &str, doc: &Value) -> anyhow::Result<NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(".json")
        .tempfile()
        .context("could not create a temporary config file")?;
    let bytes = serde_json::to_vec_pretty(doc).context("serialize config")?;
    file.write_all(&bytes).context("write temp config")?;
    file.flush().ok();
    Ok(file)
}

/// Pretty-print a JSON value to stdout.
fn print_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => println!("{s}"),
        Err(_) => println!("{value}"),
    }
}

/// Pretty-print a JSON value to stderr (error path).
fn print_json_err(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => eprintln!("{s}"),
        Err(_) => eprintln!("{value}"),
    }
}

/// Initialize stderr logging (default WARN; --verbose lifts to DEBUG). stdout is reserved for
/// the reply JSON so the tool is pipe-friendly.
fn init_tracing(verbose: bool) {
    let default = if verbose { "debug" } else { "warn" };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
