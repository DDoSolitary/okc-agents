extern crate base64;
#[macro_use]
extern crate slog;
extern crate tokio;
extern crate okc_agents;

use std::error::Error;
use std::net::SocketAddr;
use std::process::{Command, Stdio};
use futures_util::StreamExt;
use slog::Logger;
use tokio::prelude::*;
use tokio::fs::File;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use okc_agents::utils::*;

const PROTO_VER: i32 = 0;

async fn read_str<T: AsyncRead + Unpin>(rx: &mut T) -> std::result::Result<String, Box<dyn Error>> {
	let mut len_buf = [0u8; 2];
	rx.read_exact(&mut len_buf).await?;
	let len = ((len_buf[0] as usize) << 8) + len_buf[1] as usize;
	let mut str_buf = vec!(0u8; len);
	rx.read_exact(&mut str_buf).await?;
	Ok(String::from_utf8(str_buf)?)
}

async fn handle_control_connection(mut stream: TcpStream, logger: Logger) -> Result {
	info!(logger, "control connection established");
	loop {
		let msg = read_str(&mut stream).await?;
		debug!(logger, "new warning message received"; "length" => msg.len());
		if msg.is_empty() {
			break;
		}
		if msg.starts_with("[E] ") {
			error!(logger, "{}", &msg[4..]);
		} else if msg.starts_with("[W] ") {
			warn!(logger, "{}", &msg[4..]);
		} else {
			eprintln!("{}", msg);
		}
	}
	debug!(logger, "all messages processed, waiting for status code");
	let mut stat_buf = [0u8; 1];
	stream.read_exact(&mut stat_buf).await?;
	info!(logger, "control connection finished"; "status_code" => stat_buf[0]);
	match stat_buf[0] {
		0 => Ok(()),
		_ => Err(Box::new(StringError(String::from("an error has occurred in the app"))) as Box<dyn Error>)
	}
}

async fn handle_input_connection(mut stream: TcpStream, logger: Logger) -> Result {
	let path = read_str(&mut stream).await?;
	info!(logger, "input connection established"; "path" => &path);
	if &path == "-" {
		let mut stdin = io::stdin();
		debug!(logger, "reading from stdin");
		io::copy(&mut stdin, &mut stream).await?;
	} else {
		let mut file = File::open(&path).await?;
		debug!(logger, "reading from file");
		io::copy(&mut file, &mut stream).await?;
	}
	info!(logger, "input connection finished");
	Ok(())
}

async fn handle_output_connection(mut stream: TcpStream, logger: Logger) -> Result {
	let path = read_str(&mut stream).await?;
	info!(logger, "output connection established"; "path" => &path);
	if &path == "-" {
		let mut stdout = io::stdout();
		debug!(logger, "writing to stdout");
		io::copy(&mut stream, &mut stdout).await?;
	} else {
		let mut file = File::create(&path).await?;
		debug!(logger, "writing to file");
		io::copy(&mut stream, &mut file).await?;
	}
	info!(logger, "output connection finished");
	Ok(())
}

async fn handle_connection(accept_result: std::result::Result<TcpStream, tokio::io::Error>, logger: Logger) -> Result {
	let mut stream = accept_result?;
	let logger = logger.new(o!("remote_port" => stream.peer_addr()?.port()));
	debug!(logger, "connection accepted");
	let mut op = [0u8];
	stream.read_exact(&mut op).await?;
	debug!(logger, "connection type is {}", op[0]);
	let res = match op[0] {
		0 => match handle_control_connection(stream, logger.clone()).await {
			Ok(_) => exit_process(0),
			Err(e) => {
				error!(logger, "{:?}", e);
				exit_process(1);
			}
		},
		1 => handle_input_connection(stream, logger.clone()).await,
		2 => handle_output_connection(stream, logger.clone()).await,
		_ => Err(Box::new(StringError(String::from("protocol error: invalid connection type"))) as Box<dyn Error>)
	};
	if let Err(e) = res {
		error!(logger, "{:?}", e);
	}
	Ok(())
}

async fn run(logger: Logger) -> Result {
	let addr = "127.0.0.1:0".parse::<SocketAddr>()?;
	let mut listener = TcpListener::bind(&addr).await?;
	let addr = listener.local_addr()?;
	info!(logger, "listening on port {}", addr.port());
	let mut cmd = Command::new("am");
	cmd.arg("broadcast")
		.arg("-n").arg("org.ddosolitary.okcagent/.GpgProxyReceiver")
		.arg("--ei").arg("org.ddosolitary.okcagent.extra.GPG_PROTO_VER").arg(PROTO_VER.to_string())
		.arg("--ei").arg("org.ddosolitary.okcagent.extra.PROXY_PORT").arg(addr.port().to_string())
		.stdout(Stdio::null()).stderr(Stdio::null());
	if std::env::args().len() > 1 {
		cmd.arg("--esa").arg("org.ddosolitary.okcagent.extra.GPG_ARGS")
			.arg(std::env::args().skip(1).map(|s| base64::encode(&s)).collect::<Vec<_>>().join(","));
	} else {
		debug!(logger, "no arguments specified, GPG_ARGS won't be sent")
	}
	cmd.status()?;
	info!(logger, "broadcast sent, waiting for app to connect");
	listener.incoming().for_each_concurrent(Some(3), |accept_result| async {
		debug!(logger, "new incoming connection");
		if let Err(e) = handle_connection(accept_result, logger.clone()).await {
			error!(logger, "{:?}", e);
			exit_process(1);
		}
	}).await;
	Ok(())
}

#[tokio::main]
async fn main() {
	lib_main(run).await;
}
