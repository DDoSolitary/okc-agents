extern crate ctrlc;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate slog;
extern crate slog_async;
extern crate slog_envlogger;
extern crate slog_term;
extern crate tokio;
extern crate okc_agents;

use std::net::SocketAddr;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use slog::{Logger, Drain};
use slog_async::{Async, AsyncGuard};
use slog_term::{FullFormat, TermDecorator};
use futures_util::future;
use tokio::prelude::*;
use tokio::net::TcpListener;
use tokio::io::{self, AsyncRead, AsyncWrite};
use tokio::stream::StreamExt;
use tokio::time;
use okc_agents::utils::*;

#[cfg(unix)]
type ClientStream = tokio::net::UnixStream;
#[cfg(not(unix))]
type ClientStream = tokio::net::TcpStream;

lazy_static! {
	static ref LOG_GUARD: Mutex<Option<AsyncGuard>> = Mutex::new(None);
	static ref CONN_COUNTER: AtomicU64 = AtomicU64::new(0);
}

fn exit_process(code: i32) -> ! {
	if let Some(guard) = LOG_GUARD.lock().unwrap().take() {
		std::mem::drop(guard);
	}
	std::process::exit(code)
}

async fn do_copy<T1: AsyncRead + Unpin, T2: AsyncWrite + Unpin>(rx: &mut T1, tx: &mut T2) -> Result {
	io::copy(rx, tx).await?;
	tx.shutdown().await?;
	Ok(())
}

async fn handle_connection(accept_result: std::result::Result<ClientStream, io::Error>, logger: Logger) -> Result {
	let mut client_stream = accept_result?;
	let logger = logger.new(o!("id" => CONN_COUNTER.fetch_add(1, Ordering::Relaxed)));
	info!(logger, "connected to client");
	let (mut crx, mut ctx) = client_stream.split();
	let addr = "127.0.0.1:0".parse::<SocketAddr>()?;
	let mut app_listener = TcpListener::bind(&addr).await?;
	let addr = app_listener.local_addr()?;
	info!(logger, "listening on port {}", addr.port());
	Command::new("am").arg("broadcast")
		.arg("-n").arg("org.ddosolitary.okcagent/.SshProxyReceiver")
		.arg("--ei").arg("org.ddosolitary.okcagent.extra.PROXY_PORT").arg(addr.port().to_string())
		.stdout(Stdio::null()).stderr(Stdio::null())
		.status()?;
	info!(logger, "broadcast sent, waiting for app to connect");
	let mut app_stream = time::timeout(Duration::from_secs(10), app_listener.incoming().next()).await
		.map_err(|_| StringError(String::from("timed out waiting for app to connect")))?.unwrap()?;
	info!(logger, "app connected, start forwarding"; "remote_port" => app_stream.peer_addr()?.port());
	let (mut arx, mut atx) = app_stream.split();
	let (r1, r2) = future::join(do_copy(&mut crx, &mut atx), do_copy(&mut arx, &mut ctx)).await;
	r1?;
	r2?;
	info!(logger, "connection finished");
	Ok(())
}

async fn run(logger: Logger) -> Result {
	let args = std::env::args().collect::<Vec<_>>();
	let path = args.get(1)
		.ok_or(StringError(String::from("please specify path of the agent socket")))?;

	#[cfg(unix)] let mut listener = tokio::net::UnixListener::bind(&path)?;
	#[cfg(not(unix))] let mut listener = TcpListener::bind(path.parse::<SocketAddr>()?).await?;
	info!(logger, "listening on the Unix socket: \"{}\"", path);

	#[cfg(unix)] {
		let path = path.clone();
		let logger = logger.clone();
		ctrlc::set_handler(move || {
			if let Err(e) = std::fs::remove_file(&path) {
				error!(logger, "failed to delete the socket file: {:?}", e);
				exit_process(1);
			}
			exit_process(0);
		}).unwrap();
	}

	let mut incoming = listener.incoming();
	while let Some(accept_result) = incoming.next().await {
		debug!(logger, "new incoming connection");
		if let Err(e) = handle_connection(accept_result, logger.clone()).await {
			error!(logger, "failed to accept the connection: {:?}", e);
		}
	}
	#[cfg(unix)] std::fs::remove_file(&path)?;

	Ok(())
}

#[tokio::main]
async fn main() {
	let drain = FullFormat::new(TermDecorator::new().stderr().build()).build().ignore_res();
	let drain = slog_envlogger::new(drain).ignore_res();
	let (drain, guard) = Async::new(drain).build_with_guard();
	*LOG_GUARD.lock().unwrap() = Some(guard);
	let logger = Logger::root(drain.ignore_res(), o!());
	if let Err(e) = run(logger.clone()).await {
		error!(logger, "{:?}", e);
		exit_process(1);
	}
}
