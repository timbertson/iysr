use std::io;
use std::collections::{BTreeMap};
use rustc_serialize::json::{Json};
use monitor::*;
use errors::*;

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

macro_rules! errln {
	($($arg:tt)*) => (
		let _ign = writeln!(&mut ::std::io::stderr(), $($arg)* );
	)
}

macro_rules! encode_sub {
	($x:expr) => { {|s| $x.encode(s) } }
}

pub type JsonMap = BTreeMap<String,Json>;

pub fn read_all(source: &mut io::Read) -> Result<String, InternalError> {
	let mut buf = Vec::new();
	try!(source.read_to_end(&mut buf));
	Ok(try!(String::from_utf8(buf)))
}
