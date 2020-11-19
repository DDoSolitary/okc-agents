extern crate ctrlc;
#[macro_use]
extern crate slog;
extern crate tokio;
extern crate okc_agents;

use std::net::SocketAddr;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use futures_util::StreamExt;
use slog::Logger;
use futures_util::future;
use tokio::prelude::*;
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::io::{self, AsyncRead, AsyncWrite};
use tokio::time;
use okc_agents::utils::*;

const PROTO_VER: i32 = 0;

async fn do_copy<T1: AsyncRead + Unpin, T2: AsyncWrite + Unpin>(rx: &mut T1, tx: &mut T2) -> Result {
	io::copy(rx, tx).await?;
	tx.shutdown().await?;
	Ok(())
}

async fn handle_connection(accept_result: std::result::Result<UnixStream, io::Error>, logger: Logger) -> Result {
	let mut client_stream = accept_result?;
	info!(logger, "connected to client");
	let (mut crx, mut ctx) = client_stream.split();
	let addr = "127.0.0.1:0".parse::<SocketAddr>()?;
	let mut app_listener = TcpListener::bind(&addr).await?;
	let addr = app_listener.local_addr()?;
	info!(logger, "listening on port {}", addr.port());
	Command::new("am").arg("broadcast")
		.arg("-n").arg("org.ddosolitary.okcagent/.SshProxyReceiver")
		.arg("--ei").arg("org.ddosolitary.okcagent.extra.SSH_PROTO_VER").arg(PROTO_VER.to_string())
		.arg("--ei").arg("org.ddosolitary.okcagent.extra.PROXY_PORT").arg(addr.port().to_string())
		.stdout(Stdio::null()).stderr(Stdio::null())
		.status()?;
	info!(logger, "broadcast sent, waiting for app to connect");
	let mut app_stream = time::timeout(Duration::from_secs(10), app_listener.incoming().next()).await
		.map_err(|_| StringError::new("timed out waiting for app to connect"))?.unwrap()?;
	info!(logger, "app connected, start forwarding"; "remote_port" => app_stream.peer_addr()?.port());
	let (mut arx, mut atx) = app_stream.split();
	let (r1, r2) = future::join(do_copy(&mut crx, &mut atx), do_copy(&mut arx, &mut ctx)).await;
	r1?;
	r2?;
	info!(logger, "connection finished");
	Ok(())
}

async fn run(logger: Logger) -> Result {
	info!(logger, "okc-ssh-agent"; "version" => env!("CARGO_PKG_VERSION"), "protocol_version" => PROTO_VER);

	let args = std::env::args().collect::<Vec<_>>();
	let path = args.get(1)
		.ok_or(StringError::new("please specify path of the agent socket"))?;

	let mut listener = UnixListener::bind(&path)?;
	info!(logger, "listening on the Unix socket: \"{}\"", path);

	let path = path.clone();
	let logger = logger.clone();
	ctrlc::set_handler(move || {
		if let Err(e) = std::fs::remove_file(&path) {
			error!(logger, "failed to delete the socket file: {:?}", e);
			exit_process(1);
		}
		exit_process(0);
	}).unwrap();

	let counter = AtomicU64::new(0);
	listener.incoming().for_each_concurrent(Some(4), |accept_result| async {
		let logger = logger.new(o!("id" => counter.fetch_add(1, Ordering::Relaxed)));
		debug!(logger, "new incoming connection");
		if let Err(e) = handle_connection(accept_result, logger.clone()).await {
			error!(logger, "failed to accept the connection: {:?}", e);
		}
	}).await;
	std::fs::remove_file(&path)?;

	Ok(())
}

#[tokio::main]
async fn main() {
	lib_main(run).await;
}
