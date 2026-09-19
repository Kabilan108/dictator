//! Isolated IPC/OSD stress probe using private temporary sockets.
//! Run with `cargo run --release --example stress_ipc`.
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use dictator::ipc::{Client, CommandHandler, DaemonState, Server, StatusData};
use dictator::visual::SocketSink;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;
use tokio::task::JoinSet;

struct Handler;
impl CommandHandler for Handler {
    fn handle_start(&self) -> Result<()> {
        Ok(())
    }
    fn handle_stop(&self) -> Result<()> {
        Ok(())
    }
    fn handle_toggle(&self) -> Result<()> {
        Ok(())
    }
    fn handle_cancel(&self) -> Result<()> {
        Ok(())
    }
    fn get_status(&self) -> StatusData {
        StatusData {
            state: DaemonState::Idle,
            recording_duration: None,
            last_error: None,
            uptime: Duration::ZERO,
        }
    }
}

fn resources() -> serde_json::Value {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap_or("")
            .trim()
            .to_owned()
    };
    serde_json::json!({
        "rss": field("VmRSS:"),
        "threads": field("Threads:"),
        "fds": std::fs::read_dir("/proc/self/fd").unwrap().count(),
    })
}

async fn churn_osd(sink: &SocketSink, count: usize) -> Result<()> {
    for _ in 0..count {
        let stream = UnixStream::connect(sink.socket_path()).await?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        let read =
            tokio::time::timeout(Duration::from_secs(1), reader.read_line(&mut line)).await??;
        anyhow::ensure!(read > 0, "OSD client slot was not reclaimed");
        drop(reader);
        tokio::task::yield_now().await;
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("ipc.sock");
    let server = Server::with_path(Arc::new(Handler), path.clone());
    server.start().await?;
    let sink = SocketSink::with_path(None, dir.path().join("osd.sock"))?;
    let client = Client::with_path(path.clone());
    for _ in 0..1000 {
        client.status().await?;
    }
    churn_osd(&sink, 100).await?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let warmed = resources();
    let mut rounds = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        let mut tasks = JoinSet::new();
        for _ in 0..16 {
            let path = path.clone();
            tasks.spawn(async move {
                let client = Client::with_path(path);
                let mut latencies = Vec::with_capacity(625);
                for _ in 0..625 {
                    let started = Instant::now();
                    let response = client.status().await?;
                    anyhow::ensure!(response.success, "status failed");
                    latencies.push(started.elapsed().as_micros());
                }
                Ok::<_, anyhow::Error>(latencies)
            });
        }
        let mut latencies = Vec::new();
        while let Some(result) = tasks.join_next().await {
            latencies.extend(result??);
        }
        let ipc_elapsed = started.elapsed();
        latencies.sort_unstable();
        churn_osd(&sink, 1000).await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        rounds.push(serde_json::json!({
            "ipc_requests": latencies.len(),
            "ipc_elapsed_ms": ipc_elapsed.as_millis(),
            "median_us": latencies[latencies.len()/2],
            "p95_us": latencies[latencies.len()*95/100],
            "osd_reconnects": 1000,
            "resources": resources(),
        }));
    }
    server.stop().await?;
    sink.close().await?;
    anyhow::ensure!(
        !path.exists() && !sink.socket_path().exists(),
        "socket cleanup failed"
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "warmed": warmed, "rounds": rounds, "after_close": resources(),
        }))?
    );
    Ok(())
}
