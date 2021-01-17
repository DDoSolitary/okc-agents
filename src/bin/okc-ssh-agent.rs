extern crate clap;
#[macro_use]
extern crate lazy_static;
extern crate libc;
#[macro_use]
extern crate slog;
extern crate tokio;
extern crate tokio_stream;
extern crate okc_agents;

use std::ffi::{CString, OsStr, OsString};
use std::fs;
use std::net::SocketAddr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use futures_util::{StreamExt, TryFutureExt};
use futures_util::future;
use slog::Logger;
use tokio::io::{self, AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::process::Command;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::time;
use tokio_stream::wrappers::{TcpListenerStream, UnixListenerStream};
use okc_agents::utils::*;

type StdUnixListener = std::os::unix::net::UnixListener;

const PROTO_VER: i32 = 0;
const SOCKET_DIR_TEMPLATE: &str = "okc-ssh-XXXXXXXXXX";
const AUTH_SOCK_ENV: &str = "SSH_AUTH_SOCK";
const AGENT_PID_ENV: &str = "SSH_AGENT_PID";

async fn do_copy<T1, T2>(rx: &mut T1, tx: &mut T2) -> std::result::Result<(), io::Error>
	where T1: AsyncRead + Unpin, T2: AsyncWrite + Unpin
{
	io::copy(rx, tx).await?;
	tx.shutdown().await?;
	Ok(())
}

async fn handle_connection(accept_result: std::result::Result<UnixStream, io::Error>, logger: Logger) -> Result {
	let mut client_stream = accept_result?;
	info!(logger, "connected to client");
	let (mut crx, mut ctx) = client_stream.split();
	let addr = "127.0.0.1:0".parse::<SocketAddr>()?;
	let app_listener = TcpListener::bind(&addr).await?;
	let addr = app_listener.local_addr()?;
	info!(logger, "listening on port {}", addr.port());
	Command::new("am").arg("broadcast")
		.arg("-n").arg("org.ddosolitary.okcagent/.SshProxyReceiver")
		.arg("--ei").arg("org.ddosolitary.okcagent.extra.SSH_PROTO_VER").arg(PROTO_VER.to_string())
		.arg("--ei").arg("org.ddosolitary.okcagent.extra.PROXY_PORT").arg(addr.port().to_string())
		.stdout(Stdio::null()).stderr(Stdio::null())
		.status().await?;
	info!(logger, "broadcast sent, waiting for app to connect");
	let mut app_stream = time::timeout(Duration::from_secs(10), TcpListenerStream::new(app_listener).next()).await
		.map_err(|_| StringError::new("timed out waiting for app to connect"))?.unwrap()?;
	info!(logger, "app connected, start forwarding"; "remote_port" => app_stream.peer_addr()?.port());
	let (mut arx, mut atx) = app_stream.split();
	let (r1, r2) = future::join(do_copy(&mut crx, &mut atx), do_copy(&mut arx, &mut ctx)).await;
	r1?;
	r2?;
	info!(logger, "connection finished");
	Ok(())
}

lazy_static! {
	pub static ref SOCKET_FILE: RwLock<Option<OsString>> = RwLock::new(None);
	pub static ref SOCKET_DIR: RwLock<Option<OsString>> = RwLock::new(None);
}

async fn run(listener: StdUnixListener, logger: Logger) -> Result {
	info!(logger, "okc-ssh-agent"; "version" => env!("CARGO_PKG_VERSION"), "protocol_version" => PROTO_VER);

	let listener = UnixListener::from_std(listener)?;

	let counter = AtomicU64::new(0);
	UnixListenerStream::new(listener).for_each_concurrent(Some(4), |accept_result| async {
		let logger = logger.new(o!("id" => counter.fetch_add(1, Ordering::Relaxed)));
		debug!(logger, "new incoming connection");
		if let Err(e) = handle_connection(accept_result, logger.clone()).await {
			error!(logger, "failed to accept the connection: {:?}", e);
		}
	}).await;

	Ok(())
}

fn cleanup(logger: Option<Logger>) {
	if let Some(ref socket_file) = *SOCKET_FILE.read().unwrap() {
		fs::remove_file(socket_file).unwrap_or_else(|e| {
			if let Some(ref logger) = logger {
				warn!(logger, "failed to delete the socket file: {:?}", e;
					"path" => &*socket_file.to_string_lossy()
				);
			}
		});
	}
	if let Some(ref socket_dir) = *SOCKET_DIR.read().unwrap() {
		fs::remove_dir(socket_dir).unwrap_or_else(|e| {
			if let Some(ref logger) = logger {
				warn!(logger, "failed to delete the temporary directory: {:?}", e;
					"path" => &*socket_dir.to_string_lossy()
				);
			}
		});
	}
}

async fn handle_signals(mut signal: Signal, logger: Logger) {
	signal.recv().await;
	cleanup(Some(logger));
	exit_process(1);
}

async fn run_wrapper(listener: StdUnixListener, cmd: Option<Command>, logger: Logger) -> Result {
	tokio::spawn(future::join3(
		handle_signals(signal(SignalKind::hangup())?, logger.clone()),
		handle_signals(signal(SignalKind::interrupt())?, logger.clone()),
		handle_signals(signal(SignalKind::terminate())?, logger.clone()),
	));
	let run_future = run(listener, logger.clone());
	if let Some(mut cmd) = cmd {
		let err_logger = logger.clone();
		tokio::spawn(run_future.map_err(move |e| {
			error!(err_logger, "{:?}", e);
			future::pending::<()>()
		}));
		let stat = cmd.status().await;
		cleanup(Some(logger.clone()));
		exit_process(stat?.code().unwrap_or(1))
	} else {
		let res = run_future.await;
		cleanup(Some(logger.clone()));
		res
	}
}

unsafe fn redirect_null(fd: i32, write: bool) {
	let null_path = CString::new("/dev/null").unwrap();
	let null_fd = libc::open(null_path.as_ptr(), if write { libc::O_RDWR } else { libc::O_RDONLY });
	if null_fd == -1 || libc::dup2(null_fd, fd) == -1 {
		std::process::exit(1);
	}
}

fn main() {
	let matches = clap::App::new("okc-ssh-agent")
		.version(env!("CARGO_PKG_VERSION"))
		.author(env!("CARGO_PKG_AUTHORS"))
		.arg(clap::Arg::with_name("addr")
			.short("a")
			.value_name("bind_address")
			.takes_value(true)
			.help(&format!("Bind the agent to the UNIX-domain socket bind_address. \
				The default is $TMPDIR/{}/agent.<ppid>.", SOCKET_DIR_TEMPLATE)))
		.arg(clap::Arg::with_name("csh")
			.short("c")
			.conflicts_with("bash")
			.help("Generate C-shell commands on stdout. \
				This is the default if SHELL looks like it's a csh style of shell."))
		.arg(clap::Arg::with_name("bash")
			.short("s")
			.conflicts_with("csh")
			.help("Generate Bourne shell commands on stdout. \
				This is the default if SHELL does not look like it's a csh style of shell."))
		.arg(clap::Arg::with_name("foreground")
			.short("D")
			.help("Foreground mode. \
				When this option is specified okc-ssh-agent will not fork."))
		.arg(clap::Arg::with_name("debug")
			.short("d")
			.help("Debug mode. \
				When this option is specified okc-ssh-agent will not fork \
				and will write debug information to standard error."))
		.arg(clap::Arg::with_name("cmd")
			.value_name("COMMAND")
			.index(1)
			.multiple(true)
			.conflicts_with_all(&["csh", "bash", "foreground", "debug"])
			.help("If a command (and optional arguments) is given, this is executed as a subprocess of the agent.\
				The agent exits automatically when the command given on the command line terminates."))
		.arg(clap::Arg::with_name("kill")
			.short("k")
			.conflicts_with_all(&["addr", "foreground", "debug", "cmd"])
			.help("Kill the current agent (given by the SSH_AGENT_PID environment variable)."))
		.get_matches();

	let is_csh = matches.is_present("csh") ||
		!matches.is_present("bash") &&
			std::env::var("SHELL").map_or(false, |s| s.ends_with("csh"));

	if matches.is_present("debug") {
		std::env::set_var("RUST_LOG", "trace");
	}

	let mut pid = std::process::id() as i32;

	if matches.is_present("kill") {
		let env_pid = std::env::var(AGENT_PID_ENV).ok()
			.and_then(|s| s.parse::<i32>().ok())
			.unwrap_or_else(|| {
				eprintln!("failed to read environment variable {}", AGENT_PID_ENV);
				std::process::exit(1)
			});
		unsafe {
			if libc::kill(env_pid, libc::SIGTERM) != 0 {
				eprintln!("failed to kill the process {}: {:?}", env_pid, io::Error::last_os_error());
				std::process::exit(1);
			}
		}

		if is_csh {
			println!("unsetenv {};", AUTH_SOCK_ENV);
			println!("unsetenv {};", AGENT_PID_ENV);
		} else {
			println!("unset {};", AUTH_SOCK_ENV);
			println!("unset {};", AGENT_PID_ENV);
		}
		println!("echo Agent pid {} killed;", env_pid);
		return;
	}

	let socket_file = matches.value_of_os("addr")
		.map(|s| s.to_owned())
		.unwrap_or_else(|| {
			let tmpdir = if let Some(tmpdir) = std::env::var_os("TMPDIR") {
				tmpdir
			} else if let Ok(prefix) = std::env::var("PREFIX") {
				Path::new(&prefix).join("tmp").into_os_string()
			} else {
				OsString::from("/tmp")
			};
			let mut template = CString::new(
				Path::new(&tmpdir).join(SOCKET_DIR_TEMPLATE).as_os_str().as_bytes()
			).unwrap();
			unsafe {
				let template_ptr = template.into_raw();
				if libc::mkdtemp(template_ptr).is_null() {
					eprintln!("failed to create temporary directory: {:?}", io::Error::last_os_error());
					std::process::exit(1);
				}
				template = CString::from_raw(template_ptr);
			}
			let socket_dir = OsStr::from_bytes(template.as_bytes());
			*SOCKET_DIR.write().unwrap() = Some(socket_dir.to_owned());
			Path::new(socket_dir).join(format!("agent.{}", pid)).into_os_string()
		});

	// Create the socket now so that we can report errors and run specified commands.
	let listener = StdUnixListener::bind(&socket_file).unwrap_or_else(|e| {
		eprintln!("failed to create Unix socket {:?}: {:?}", socket_file.to_string_lossy(), e);
		std::process::exit(1)
	});
	*SOCKET_FILE.write().unwrap() = Some(socket_file.clone());

	let cmd = matches.values_of_os("cmd");
	let is_foreground = cmd.is_some() || matches.is_present("foreground") || matches.is_present("debug");

	if !is_foreground {
		unsafe {
			match libc::fork() {
				-1 => {
					eprintln!("failed to fork: {:?}", io::Error::last_os_error());
					cleanup(None);
					std::process::exit(1);
				}
				0 => {
					redirect_null(libc::STDIN_FILENO, false);
					redirect_null(libc::STDOUT_FILENO, true);
					redirect_null(libc::STDERR_FILENO, true);
					lib_main(|logger| run_wrapper(listener, None, logger));
					return;
				}
				child_pid => pid = child_pid,
			}
		}
	}

	if cmd.is_none() {
		if is_csh {
			println!("setenv {} {:?};", AUTH_SOCK_ENV, socket_file.to_string_lossy());
			if !is_foreground {
				println!("setenv {} {};", AGENT_PID_ENV, pid);
			}
		} else {
			println!("{}={:?}; export {0};", AUTH_SOCK_ENV, socket_file.to_string_lossy());
			if !is_foreground {
				println!("{}={}; export {0};", AGENT_PID_ENV, pid);
			}
		}
		println!("echo Agent pid {};", pid);
	}

	if is_foreground {
		lib_main(|logger| {
			let tokio_cmd = cmd.map(|mut cmd| {
				let mut tokio_cmd = Command::new(cmd.next().unwrap());
				tokio_cmd
					.args(cmd)
					.env(AUTH_SOCK_ENV, socket_file)
					.env(AGENT_PID_ENV, pid.to_string());
				tokio_cmd
			});
			run_wrapper(listener, tokio_cmd, logger)
		});
	}
}
