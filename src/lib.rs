#[macro_use]
extern crate lazy_static;
extern crate slog_async;
extern crate tokio;

pub mod utils {
	use std::error::Error;
	use std::fmt::{Display, Formatter};
	use std::sync::Mutex;
	use slog_async::AsyncGuard;

	pub type Result = std::result::Result<(), Box<dyn Error>>;

	#[derive(Debug)]
	pub struct StringError(pub String);

	impl Display for StringError {
		fn fmt(&self, f: &mut Formatter) -> std::result::Result<(), std::fmt::Error> {
			self.0.fmt(f)
		}
	}

	impl Error for StringError {}


	lazy_static! {
		pub static ref LOG_GUARD: Mutex<Option<AsyncGuard>> = Mutex::new(None);
	}

	pub fn exit_process(code: i32) -> ! {
		if let Some(guard) = LOG_GUARD.lock().unwrap().take() {
			std::mem::drop(guard);
		}
		std::process::exit(code)
	}
}
