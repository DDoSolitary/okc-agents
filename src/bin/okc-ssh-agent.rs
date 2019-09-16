extern crate ctrlc;
extern crate tokio;
extern crate okc_agents;

use std::net::SocketAddr;
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::prelude::*;
use tokio::net::TcpListener;
use tokio::io::{AsyncRead, AsyncWrite};
use okc_agents::utils::*;

#[cfg(unix)]
type ClientStream = tokio::net::UnixStream;
#[cfg(not(unix))]
type ClientStream = tokio::net::TcpStream;

async fn do_copy<T1: AsyncRead + Unpin, T2: AsyncWrite + Unpin>(rx: &mut T1, tx: &mut T2) -> Result {
	rx.copy(tx).await?;
	tx.shutdown().await?;
	Ok(())
}

async fn handle_connection(client_stream: std::result::Result<ClientStream, tokio::io::Error>) -> Result {
	let (mut crx, mut ctx) = client_stream?.split();
	let addr = "127.0.0.1:0".parse::<SocketAddr>()?;
	let app_listener = TcpListener::bind(&addr).await?;
	let addr = app_listener.local_addr()?;
	Command::new("am").arg("broadcast")
		.arg("-n").arg("org.ddosolitary.okcagent/.SshProxyReceiver")
		.arg("--ei").arg("org.ddosolitary.okcagent.extra.PROXY_PORT").arg(addr.port().to_string())
		.stdout(Stdio::null()).stderr(Stdio::null())
		.status()?;
	let (mut arx, mut atx) = app_listener.incoming().take(1).collect::<Vec<_>>()
		.timeout(Duration::from_secs(10)).await
		.map_err(|_| StringError(String::from("Timed out waiting for app to connect.")))?
		.pop().unwrap()?.split();
	let (r1, r2) = futures_util::future::join(do_copy(&mut crx, &mut atx), do_copy(&mut arx, &mut ctx)).await;
	r1?;
	r2?;
	Ok(())
}

#[tokio::main]
async fn main() -> Result {
	let args = std::env::args().collect::<Vec<_>>();
	let path = args.get(1)
		.ok_or(StringError(String::from("Please specify path of the agent socket.")))?;

	#[cfg(unix)]
	let listener = tokio::net::UnixListener::bind(&path)?;
	#[cfg(not(unix))]
	let listener = TcpListener::bind(path.parse::<SocketAddr>()?).await?;

	#[cfg(unix)] {
		let path = path.clone();
		ctrlc::set_handler(move || {
			if let Err(e) = std::fs::remove_file(&path) {
				eprintln!("Error: {:?}", e);
				std::process::exit(1);
			}
			std::process::exit(0);
		}).unwrap();
	}

	listener.incoming().for_each_concurrent(Some(4), |stream| async {
		if let Err(e) = handle_connection(stream).await {
			eprintln!("Error: {:?}", e);
		}
	}).await;

	#[cfg(unix)]
	std::fs::remove_file(&path)?;

	Ok(())
}
