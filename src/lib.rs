extern crate tokio;

pub mod utils {
	use std::error::Error;
	use std::fmt::{Display, Formatter};

	pub type Result = std::result::Result<(), Box<dyn Error>>;

	#[derive(Debug)]
	pub struct StringError(pub String);

	impl Display for StringError {
		fn fmt(&self, f: &mut Formatter) -> std::result::Result<(), std::fmt::Error> {
			self.0.fmt(f)
		}
	}

	impl Error for StringError {}
}
