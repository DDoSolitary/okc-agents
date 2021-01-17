extern crate base64;
#[macro_use]
extern crate slog;
extern crate tokio;
extern crate tokio_stream;
extern crate okc_agents;

use std::error::Error;
use std::net::SocketAddr;
use std::process::{Command, Stdio};
use futures_util::StreamExt;
use slog::Logger;
use tokio::fs::File;
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_stream::wrappers::TcpListenerStream;
use okc_agents::utils::*;

const PROTO_VER: i32 = 1;

async fn read_str<T: AsyncRead + Unpin>(rx: &mut T) -> std::result::Result<String, Box<dyn Error>> {
	let len = rx.read_u16().await?;
	let mut str_buf = vec!(0u8; len as usize);
	rx.read_exact(&mut str_buf).await?;
	Ok(String::from_utf8(str_buf)?)
}

async fn copy_input(rx: &mut (impl AsyncRead + Unpin), tx: &mut (impl AsyncWrite + Unpin), logger: &Logger) -> Result {
	let mut buf = vec![0u8; u16::max_value() as usize];
	loop {
		let len = rx.read(&mut buf).await?;
		debug!(logger, "sending {} bytes", len);
		if len == 0 { break; }
		tx.write_u16(len as u16).await?;
		tx.write_all(&buf[..len]).await?;
	}
	tx.write_u16(0).await?;
	Ok(())
}

async fn copy_output(rx: &mut (impl AsyncRead + Unpin), tx: &mut (impl AsyncWrite + Unpin), logger: &Logger) -> Result {
	let mut buf = vec![0u8; u16::max_value() as usize];
	loop {
		let len = rx.read_u16().await? as usize;
		debug!(logger, "{} bytes received", len);
		if len == 0 {
			return Ok(());
		}
		rx.read_exact(&mut buf[..len]).await?;
		tx.write_all(&buf[..len]).await?;
	}
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
	let stat = stream.read_u8().await?;
	info!(logger, "control connection finished"; "status_code" => stat);
	match stat {
		0 => Ok(()),
		_ => Err(Box::new(StringError::new("an error has occurred in the app")) as Box<dyn Error>)
	}
}

async fn handle_input_connection(mut stream: TcpStream, logger: Logger) -> Result {
	let path = read_str(&mut stream).await?;
	info!(logger, "input connection established"; "path" => &path);
	if &path == "-" {
		let mut stdin = io::stdin();
		debug!(logger, "reading from stdin");
		copy_input(&mut stdin, &mut stream, &logger).await?;
	} else {
		let mut file = File::open(&path).await?;
		debug!(logger, "reading from file");
		copy_input(&mut file, &mut stream, &logger).await?;
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
		copy_output(&mut stream, &mut stdout, &logger).await?;
	} else {
		let mut file = File::create(&path).await?;
		debug!(logger, "writing to file");
		copy_output(&mut stream, &mut file, &logger).await?;
	}
	info!(logger, "output connection finished");
	Ok(())
}

async fn handle_connection(accept_result: std::result::Result<TcpStream, tokio::io::Error>, logger: Logger) -> Result {
	let mut stream = accept_result?;
	let logger = logger.new(o!("remote_port" => stream.peer_addr()?.port()));
	debug!(logger, "connection accepted");
	let op = stream.read_u8().await?;
	debug!(logger, "connection type is {}", op);
	let res = match op {
		0 => match handle_control_connection(stream, logger.clone()).await {
			Ok(_) => exit_process(0),
			Err(e) => {
				error!(logger, "{:?}", e);
				exit_process(1);
			}
		},
		1 => handle_input_connection(stream, logger.clone()).await,
		2 => handle_output_connection(stream, logger.clone()).await,
		_ => Err(Box::new(StringError::new("protocol error: invalid connection type")) as Box<dyn Error>)
	};
	if let Err(e) = res {
		error!(logger, "{:?}", e);
	}
	Ok(())
}

async fn run(logger: Logger) -> Result {
	info!(logger, "okc-gpg"; "version" => env!("CARGO_PKG_VERSION"), "protocol_version" => PROTO_VER);

	let addr = "127.0.0.1:0".parse::<SocketAddr>()?;
	let listener = TcpListener::bind(&addr).await?;
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
	TcpListenerStream::new(listener).for_each_concurrent(Some(3), |accept_result| async {
		debug!(logger, "new incoming connection");
		if let Err(e) = handle_connection(accept_result, logger.clone()).await {
			error!(logger, "{:?}", e);
			exit_process(1);
		}
	}).await;
	Ok(())
}

fn main() {
	lib_main(run);
}
