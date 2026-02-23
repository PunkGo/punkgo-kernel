use std::sync::Arc;

use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, ToFsName, ToNsName,
    traits::tokio::Listener as _,
};
use punkgo_core::protocol::{RequestEnvelope, ResponseEnvelope};
use punkgo_runtime::Kernel;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tracing::{error, info, warn};

fn endpoint_to_name(endpoint: &str) -> std::io::Result<Name<'_>> {
    if endpoint.contains('/') || endpoint.contains('\\') {
        endpoint.to_fs_name::<GenericFilePath>()
    } else {
        endpoint.to_ns_name::<GenericNamespaced>()
    }
}

pub async fn run_ipc_server(kernel: Arc<Kernel>, endpoint: &str) -> std::io::Result<()> {
    let name = endpoint_to_name(endpoint)?;
    let listener = ListenerOptions::new().name(name).create_tokio()?;
    info!(endpoint = endpoint, "IPC server listening");

    loop {
        let conn = listener.accept().await?;
        let kernel = Arc::clone(&kernel);
        tokio::spawn(async move {
            if let Err(err) = handle_connection(conn, kernel).await {
                warn!(error = %err, "IPC client disconnected with error");
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
