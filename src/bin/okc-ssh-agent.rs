extern crate tokio;
extern crate okc_agents;

use std::error::Error;
use std::net::SocketAddr;
use std::process::Command;
use std::time::Duration;
use tokio::prelude::*;
use tokio::net::TcpListener;
use okc_agents::utils::*;

#[cfg(unix)]
type ClientStream = tokio::net::UnixStream;
#[cfg(not(unix))]
type ClientStream = tokio::net::TcpStream;

async fn handle_connection(client_stream: Result<ClientStream, tokio::io::Error>) -> Result<(), Box<dyn Error>> {
	let (mut crx, mut ctx) = client_stream?.split();
	let addr = "127.0.0.1:0".parse::<SocketAddr>()?;
	let app_listener = TcpListener::bind(&addr).await?;
	let addr = app_listener.local_addr()?;
	Command::new("am").arg("broadcast")
		.arg("-n").arg("org.ddosolitary.okcagent/.SshProxyReceiver")
		.arg("--ei").arg("org.ddosolitary.okcagent.extra.SSH_PROXY_PORT").arg(addr.port().to_string())
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
async fn main() -> Result<(), Box<dyn Error>> {
	let args = std::env::args().collect::<Vec<_>>();
	let path = args.get(1)
		.ok_or(StringError(String::from("Please specify path of the agent socket.")))?;

	#[cfg(unix)]
	let listener = tokio::net::UnixListener::bind(&path)?;
	#[cfg(not(unix))]
	let listener = TcpListener::bind(path.parse::<SocketAddr>()?).await?;

	listener.incoming().for_each_concurrent(Some(4), |stream| async {
		if let Err(e) = handle_connection(stream).await {
			eprintln!("Error: {:?}", e);
		}
	}).await;
	Ok(())
}
