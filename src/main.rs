mod cli;
mod commands;
mod config;
mod dispatcher;
mod host_metrics;
mod ipc;
mod logging;
mod proto;
mod shutdown_sm;
mod state;
mod transport;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info, warn};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(name = "w3p-ups", version = VERSION, about = "Web3 Pi UPS agent")]
struct Cli {
    /// Path to TOML config file.
    #[arg(short, long, global = true, default_value = config::DEFAULT_CONFIG_PATH)]
    config: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print one snapshot from the running daemon and exit.
    Status,
    /// Stream snapshots from the running daemon (Ctrl-C to stop).
    Watch,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg_path = cli.config.to_string_lossy().to_string();

    let config_present = Path::new(&cfg_path).exists();
    let cfg = config::load(&cfg_path).with_context(|| format!("loading {cfg_path}"))?;

    match cli.command {
        Some(Command::Status) => return cli::run_status(&cfg.ipc).await,
        Some(Command::Watch) => return cli::run_watch(&cfg.ipc).await,
        None => {} // fall through to daemon mode
    }

    logging::init(&cfg.logging)?;
    info!("w3p-ups v{VERSION} starting");
    if !config_present {
        warn!("config not found at {cfg_path}; using defaults");
    } else {
        info!("config loaded from {cfg_path}");
    }

    run_daemon(cfg).await
}

async fn run_daemon(cfg: config::Config) -> Result<()> {
    let state = state::State::new();
    let commands_handler = std::sync::Arc::new(commands::CommandsHandler::new(
        state.clone(),
        cfg.commands.clone(),
        cfg.shutdown.clone(),
    ));

    // Start the IPC server up front; clients can connect even before the
    // serial transport comes up (snapshot will be empty until then).
    let ipc_handle = match ipc::spawn_ipc(
        cfg.ipc.socket_path.clone(),
        state.clone(),
        cfg.battery.voltage_at_zero_pct,
        cfg.battery.voltage_at_full_pct,
        cfg.battery.input_min_valid_mv,
        cfg.battery.input_max_valid_mv,
    )
    .await
    {
        Ok(h) => Some(h),
        Err(e) => {
            error!("IPC server failed to start: {e}; continuing without it");
            None
        }
    };

    let mut sigterm = signal(SignalKind::terminate()).context("install SIGTERM handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("install SIGINT handler")?;

    'reconnect: loop {
        let port_path = match transport::resolve_port(&cfg.serial.port) {
            Ok(p) => p,
            Err(e) => {
                error!("port detection failed: {e}; retrying in 5 s");
                if wait_or_signal(Duration::from_secs(5), &mut sigterm, &mut sigint).await {
                    break 'reconnect;
                }
                continue 'reconnect;
            }
        };

        let handles = match transport::spawn_serial_tasks(port_path, cfg.serial.baud_rate).await {
            Ok(h) => h,
            Err(e) => {
                error!("open serial: {e}; retrying in 5 s");
                if wait_or_signal(Duration::from_secs(5), &mut sigterm, &mut sigint).await {
                    break 'reconnect;
                }
                continue 'reconnect;
            }
        };

        let mut reader = handles.reader;
        let mut writer = handles.writer;
        let mut dispatcher = tokio::spawn(dispatcher::dispatch_loop(
            state.clone(),
            handles.inbound,
            handles.outbound.clone(),
            commands_handler.clone(),
        ));
        let mut sm = tokio::spawn(shutdown_sm::shutdown_sm_loop(
            state.clone(),
            cfg.battery.clone(),
            cfg.shutdown.clone(),
            handles.outbound.clone(),
        ));
        let mut metrics = tokio::spawn(host_metrics::host_metrics_loop(
            state.clone(),
            cfg.host_metrics.clone(),
            handles.outbound.clone(),
        ));

        info!("transport tasks running; entering supervisor loop");

        let cause = tokio::select! {
            _ = sigterm.recv() => Cause::Signal("SIGTERM"),
            _ = sigint.recv()  => Cause::Signal("SIGINT"),
            r = &mut reader    => Cause::Reader(format_join(r)),
            w = &mut writer    => Cause::Writer(format_join(w)),
            d = &mut dispatcher => Cause::Dispatcher(format_join(d)),
            s = &mut sm         => Cause::Sm(format_join(s)),
            m = &mut metrics    => Cause::Metrics(format_join(m)),
        };

        reader.abort();
        writer.abort();
        dispatcher.abort();
        sm.abort();
        metrics.abort();
        let _ = reader.await;
        let _ = writer.await;
        let _ = dispatcher.await;
        let _ = sm.await;
        let _ = metrics.await;

        match cause {
            Cause::Signal(s) => {
                info!("{s} received; shutting down");
                break 'reconnect;
            }
            Cause::Reader(why) | Cause::Writer(why) => {
                warn!("transport task exited ({why}); restarting in 5 s");
                if wait_or_signal(Duration::from_secs(5), &mut sigterm, &mut sigint).await {
                    break 'reconnect;
                }
            }
            Cause::Dispatcher(why) | Cause::Sm(why) | Cause::Metrics(why) => {
                error!("supervisor task exited unexpectedly ({why}); restarting in 5 s");
                if wait_or_signal(Duration::from_secs(5), &mut sigterm, &mut sigint).await {
                    break 'reconnect;
                }
            }
        }
    }

    if let Some(h) = ipc_handle {
        h.abort();
        let _ = h.await;
    }
    let _ = tokio::fs::remove_file(&cfg.ipc.socket_path).await;
    Ok(())
}

enum Cause {
    Signal(&'static str),
    Reader(String),
    Writer(String),
    Dispatcher(String),
    Sm(String),
    Metrics(String),
}

fn format_join<T: std::fmt::Debug>(r: Result<T, tokio::task::JoinError>) -> String {
    match r {
        Ok(v) => format!("clean: {v:?}"),
        Err(e) if e.is_cancelled() => "cancelled".into(),
        Err(e) => format!("error: {e}"),
    }
}

async fn wait_or_signal(
    dur: Duration,
    sigterm: &mut tokio::signal::unix::Signal,
    sigint: &mut tokio::signal::unix::Signal,
) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(dur) => false,
        _ = sigterm.recv() => { info!("SIGTERM during backoff; shutting down"); true }
        _ = sigint.recv()  => { info!("SIGINT during backoff; shutting down"); true }
    }
}
