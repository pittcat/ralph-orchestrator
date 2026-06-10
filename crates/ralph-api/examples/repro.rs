// Repro: ralph-api tests fail with "connection closed before message completed"
use anyhow::Result;
use ralph_api::{ApiConfig, RpcRuntime, serve_with_listener};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    let workspace = TempDir::new()?;
    let mut config = ApiConfig::default();
    config.workspace_root = workspace.path().to_path_buf();

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    println!("[repro] listening on http://{addr}");

    let runtime = RpcRuntime::new(config)?;
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        serve_with_listener(listener, runtime, async move {
            let _ = rx.await;
        })
        .await
    });

    // Give the server a moment to start.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Test 1: GET /health
    println!("\n[repro] TEST 1: GET /health");
    let mut stream = tokio::net::TcpStream::connect(addr).await?;
    let req = b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    stream.write_all(req).await?;
    stream.shutdown().await?;
    let mut buf = Vec::new();
    let n = stream.read_to_end(&mut buf).await?;
    println!(
        "[repro] /health response ({} bytes):\n{}",
        n,
        String::from_utf8_lossy(&buf)
    );

    // Test 2: POST /rpc/v1 with system.health
    println!("\n[repro] TEST 2: POST /rpc/v1 system.health");
    let body = br#"{"apiVersion":"v1","id":"r1","method":"system.health","params":{}}"#;
    let mut stream = tokio::net::TcpStream::connect(addr).await?;
    let header = format!(
        "POST /rpc/v1 HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.shutdown().await?;
    let mut buf = Vec::new();
    let n = stream.read_to_end(&mut buf).await?;
    println!(
        "[repro] /rpc/v1 response ({} bytes):\n{}",
        n,
        String::from_utf8_lossy(&buf)
    );

    let _ = tx.send(());
    let _ = server.await;
    Ok(())
}
