use crate::ups_data::UpsData;
use anyhow::{Context, Result};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

/// IPC Server for the daemon to broadcast UPS data to clients
pub struct IpcServer {
    listener: UnixListener,
    clients: Vec<UnixStream>,
    socket_path: String,
}

impl IpcServer {
    pub fn new(socket_path: &str) -> Result<Self> {
        // Remove old socket file if exists
        if Path::new(socket_path).exists() {
            fs::remove_file(socket_path)
                .with_context(|| format!("Failed to remove old socket: {}", socket_path))?;
        }

        // Ensure parent directory exists
        if let Some(parent) = Path::new(socket_path).parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create socket directory: {:?}", parent))?;
        }

        let listener = UnixListener::bind(socket_path)
            .with_context(|| format!("Failed to bind Unix socket: {}", socket_path))?;

        // Set non-blocking mode for accept
        listener
            .set_nonblocking(true)
            .context("Failed to set socket non-blocking")?;

        Ok(IpcServer {
            listener,
            clients: Vec::new(),
            socket_path: socket_path.to_string(),
        })
    }

    /// Accept new client connections (non-blocking)
    pub fn accept_clients(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    // Set short timeout for writes to avoid blocking
                    let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
                    self.clients.push(stream);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No more pending connections
                    break;
                }
                Err(_) => {
                    // Ignore other errors
                    break;
                }
            }
        }
    }

    /// Broadcast UPS data to all connected clients
    pub fn broadcast(&mut self, data: &UpsData) {
        let json = match serde_json::to_string(data) {
            Ok(j) => j,
            Err(_) => return,
        };
        let message = format!("{}\n", json);

        // Send to all clients, removing disconnected ones
        self.clients
            .retain_mut(|client| client.write_all(message.as_bytes()).is_ok());
    }

    pub fn client_count(&self) -> usize {
        self.clients.len()
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        // Clean up socket file
        let _ = fs::remove_file(&self.socket_path);
    }
}

/// Connect to the daemon's IPC socket
pub fn connect_to_daemon(socket_path: &str) -> Result<BufReader<UnixStream>> {
    let stream = UnixStream::connect(socket_path).with_context(|| {
        format!(
            "Cannot connect to daemon socket: {}\n\
             Is w3p-ups service running?\n\
             Try: sudo systemctl start w3p-ups",
            socket_path
        )
    })?;

    // Set read timeout
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .context("Failed to set socket timeout")?;

    Ok(BufReader::new(stream))
}

/// Read one UPS data sample from the daemon
pub fn read_ups_data(reader: &mut BufReader<UnixStream>) -> Result<UpsData> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("Failed to read from daemon")?;

    serde_json::from_str(line.trim()).context("Failed to parse UPS data from daemon")
}
