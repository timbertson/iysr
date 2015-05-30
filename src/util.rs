use std::io;
use monitor::*;

macro_rules! log_error {
	($e:expr, $m:expr) => (
		warn!("monitor ignoring error {}: {:?}", $m, $e)
	)
}

macro_rules! ignore_error {
	($r:expr, $m:expr) => (
		match $r {
			Ok(()) => (),
			Err(e) => log_error!(e, $m),
		}
	)
}


pub fn read_all(source: &mut io::Read) -> Result<String, InternalError> {
	let mut buf = Vec::new();
	try!(source.read_to_end(&mut buf));
	Ok(try!(String::from_utf8(buf)))
}

