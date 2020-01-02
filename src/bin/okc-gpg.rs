extern crate base64;
extern crate tokio;
extern crate okc_agents;

use std::error::Error;
use std::net::SocketAddr;
use std::process::{Command, Stdio};
use tokio::prelude::*;
use tokio::fs::File;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::stream::StreamExt;
use okc_agents::utils::*;

fn exit_error(e: Box<dyn Error>) -> ! {
	eprintln!("Error: {:?}", e);
	std::process::exit(1)
}

async fn read_str<T: AsyncRead + Unpin>(rx: &mut T) -> std::result::Result<String, Box<dyn Error>> {
	let mut len_buf = [0u8; 2];
	rx.read_exact(&mut len_buf).await?;
	let len = ((len_buf[0] as usize) << 8) + len_buf[1] as usize;
	let mut str_buf = vec!(0u8; len);
	rx.read_exact(&mut str_buf).await?;
	Ok(String::from_utf8(str_buf)?)
}

async fn handle_control_connection(mut stream: TcpStream) -> Result {
	loop {
		let msg = read_str(&mut stream).await?;
		match msg.is_empty() {
			true => break,
			false => eprintln!("{}", msg)
		}
	}
	let mut stat_buf = [0u8; 1];
	stream.read_exact(&mut stat_buf).await?;
	match stat_buf[0] {
		0 => Ok(()),
		_ => Err(Box::new(StringError(String::from("An error has occurred."))) as Box<dyn Error>)
	}
}

async fn handle_input_connection(mut stream: TcpStream) -> Result {
	let path = read_str(&mut stream).await?;
	if path == "-" {
		let mut stdin = io::stdin();
		io::copy(&mut stdin, &mut stream).await?;
	} else {
		let mut file = File::open(&path).await?;
		io::copy(&mut file, &mut stream).await?;
	}
	Ok(())
}

async fn handle_output_connection(mut stream: TcpStream) -> Result {
	let path = read_str(&mut stream).await?;
	if path == "-" {
		let mut stdout = io::stdout();
		io::copy(&mut stream, &mut stdout).await?;
	} else {
		let mut file = File::create(&path).await?;
		io::copy(&mut stream, &mut file).await?;
	}
	Ok(())
}

async fn handle_connection(accept_result: std::result::Result<TcpStream, tokio::io::Error>) -> Result {
	let mut stream = accept_result?;
	let mut op = [0u8];
	stream.read_exact(&mut op).await?;
	match op[0] {
		0 => match handle_control_connection(stream).await {
			Ok(_) => std::process::exit(0),
			Err(e) => Err(e)
		},
		1 => handle_input_connection(stream).await,
		2 => handle_output_connection(stream).await,
		_ => Err(Box::new(StringError(String::from("Protocol error."))) as Box<dyn Error>)
	}
}

#[tokio::main]
async fn main() -> Result {
	let addr = "127.0.0.1:0".parse::<SocketAddr>()?;
	let mut listener = TcpListener::bind(&addr).await?;
	let addr = listener.local_addr()?;
	Command::new("am").arg("broadcast")
		.arg("-n").arg("org.ddosolitary.okcagent/.GpgProxyReceiver")
		.arg("--ei").arg("org.ddosolitary.okcagent.extra.PROXY_PORT").arg(addr.port().to_string())
		.arg("--esa").arg("org.ddosolitary.okcagent.extra.GPG_ARGS")
		.arg(std::env::args().skip(1).map(|s| base64::encode(&s)).collect::<Vec<_>>().join(","))
		.stdout(Stdio::null()).stderr(Stdio::null())
		.status()?;
	let mut incoming = listener.incoming();
	while let Some(accept_result) = incoming.next().await {
		if let Err(e) = handle_connection(accept_result).await {
			exit_error(e)
		}
	};
	Ok(())
}
