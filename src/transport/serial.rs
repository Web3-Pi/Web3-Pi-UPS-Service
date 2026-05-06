use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_serial::SerialPortBuilderExt;
use tracing::{debug, error, info, warn};

use crate::proto::{Deframer, Frame};

#[derive(Debug)]
pub struct OutboundFrame {
    pub frame: Frame,
}

pub struct SerialHandles {
    pub inbound: mpsc::Receiver<Frame>,
    pub outbound: mpsc::Sender<OutboundFrame>,
    pub reader: tokio::task::JoinHandle<()>,
    pub writer: tokio::task::JoinHandle<()>,
}

pub async fn spawn_serial_tasks(port_path: String, baud: u32) -> Result<SerialHandles> {
    info!("opening serial port: {port_path} at {baud} baud");
    let port = tokio_serial::new(&port_path, baud)
        .timeout(Duration::from_millis(100))
        .open_native_async()
        .with_context(|| format!("open serial port: {port_path}"))?;

    let (rd, wr) = tokio::io::split(port);
    let (in_tx, in_rx) = mpsc::channel::<Frame>(64);
    let (out_tx, out_rx) = mpsc::channel::<OutboundFrame>(64);

    let reader = tokio::spawn(reader_loop(rd, in_tx));
    let writer = tokio::spawn(writer_loop(wr, out_rx));

    Ok(SerialHandles {
        inbound: in_rx,
        outbound: out_tx,
        reader,
        writer,
    })
}

async fn reader_loop<R: tokio::io::AsyncRead + Unpin>(mut rd: R, sink: mpsc::Sender<Frame>) {
    let mut deframer = Deframer::new();
    let mut buf = [0u8; 256];
    loop {
        match rd.read(&mut buf).await {
            Ok(0) => {
                warn!("serial read EOF; reader exiting");
                return;
            }
            Ok(n) => {
                for &b in &buf[..n] {
                    if let Some(result) = deframer.feed(b) {
                        match result {
                            Ok(frame) => {
                                debug!(
                                    src = frame.src,
                                    dst = frame.dst,
                                    class = frame.class,
                                    op = frame.op,
                                    flags = frame.flags,
                                    seq = frame.seq,
                                    payload_len = frame.payload.len(),
                                    "rx"
                                );
                                if sink.send(frame).await.is_err() {
                                    warn!("inbound channel closed; reader exiting");
                                    return;
                                }
                            }
                            Err(e) => warn!("frame parse error: {e}"),
                        }
                    }
                }
            }
            Err(e) => {
                error!("serial read error: {e}");
                return;
            }
        }
    }
}

async fn writer_loop<W: tokio::io::AsyncWrite + Unpin>(
    mut wr: W,
    mut src: mpsc::Receiver<OutboundFrame>,
) {
    while let Some(out) = src.recv().await {
        let bytes = match out.frame.encode() {
            Ok(b) => b,
            Err(e) => {
                error!("encode outbound frame failed: {e}");
                continue;
            }
        };
        debug!(
            dst = out.frame.dst,
            class = out.frame.class,
            op = out.frame.op,
            payload_len = out.frame.payload.len(),
            "tx"
        );
        if let Err(e) = wr.write_all(&bytes).await {
            error!("serial write error: {e}");
            return;
        }
    }
}
