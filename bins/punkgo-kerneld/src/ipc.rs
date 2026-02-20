use std::sync::Arc;

use punkgo_core::protocol::{RequestEnvelope, ResponseEnvelope};
use punkgo_runtime::Kernel;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tracing::{error, info, warn};

#[cfg(unix)]
pub async fn run_ipc_server(kernel: Arc<Kernel>, endpoint: &str) -> std::io::Result<()> {
    use tokio::net::UnixListener;

    let socket_path = std::path::Path::new(endpoint);
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if socket_path.exists() {
        std::fs::remove_file(socket_path)?;
    }

    let listener = UnixListener::bind(socket_path)?;
    info!(endpoint = endpoint, "unix IPC server listening");

    loop {
        let (stream, _) = listener.accept().await?;
        let kernel = Arc::clone(&kernel);
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, kernel).await {
                warn!(error = %err, "unix IPC client disconnected with error");
            }
        });
    }
}

#[cfg(windows)]
pub async fn run_ipc_server(kernel: Arc<Kernel>, endpoint: &str) -> std::io::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    info!(endpoint = endpoint, "named pipe IPC server listening");
    loop {
        let server = ServerOptions::new().create(endpoint)?;
        server.connect().await?;
        let kernel = Arc::clone(&kernel);
        tokio::spawn(async move {
            if let Err(err) = handle_connection(server, kernel).await {
                warn!(error = %err, "named pipe IPC client disconnected with error");
            }
        });
    }
}

async fn handle_connection<S>(stream: S, kernel: Arc<Kernel>) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<RequestEnvelope>(&line) {
            Ok(req) => req,
            Err(err) => {
                let response = ResponseEnvelope::err(
                    "unknown".to_string(),
                    format!("invalid request envelope: {err}"),
                );
                write_response(&mut writer, &response).await?;
                continue;
            }
        };
        info!(request_id = %request.request_id, "ipc request received");

        let response = kernel.handle_request(request).await;
        if let Err(err) = write_response(&mut writer, &response).await {
            error!(error = %err, "failed to write IPC response");
            return Err(err);
        }
    }
    Ok(())
}

async fn write_response<W>(writer: &mut W, response: &ResponseEnvelope) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut json = serde_json::to_vec(response)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    json.push(b'\n');
    writer.write_all(&json).await?;
    writer.flush().await?;
    Ok(())
}
