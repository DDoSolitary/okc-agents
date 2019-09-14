extern crate tokio;

pub mod utils {
	use std::error::Error;
	use std::fmt::{Display, Formatter};
	use tokio::prelude::*;
	use tokio::io::{AsyncRead, AsyncWrite};

	pub type Result = std::result::Result<(), Box<dyn Error>>;

	#[derive(Debug)]
	pub struct StringError(pub String);

	impl Display for StringError {
		fn fmt(&self, f: &mut Formatter) -> std::result::Result<(), std::fmt::Error> {
			self.0.fmt(f)
		}
	}

	impl Error for StringError {}

	pub async fn do_copy<T1: AsyncRead + Unpin, T2: AsyncWrite + Unpin>(rx: &mut T1, tx: &mut T2) -> Result {
		rx.copy(tx).await?;
		tx.shutdown().await?;
		Ok(())
	}
}
