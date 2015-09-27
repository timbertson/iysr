extern crate chrono;

use std::collections::{HashMap,BTreeMap};
use std::error::{Error};
use std::fmt;
use std::string;
use std::io;
use std::convert;
use std::sync::mpsc;
use std::sync::{Arc};
use std::cmp::Ordering;
use rustc_serialize::json::{Json,ToJson};
use rustc_serialize::json;
use chrono::{DateTime,UTC};
use chrono::Timelike;

#[derive(Debug, RustcEncodable)]
pub struct InternalError {
	pub reason: String,
}

impl InternalError {
	pub fn new(reason: String) -> InternalError {
		InternalError { reason: reason }
	}

	pub fn wrap(err: &Error) -> InternalError {
		InternalError { reason: String::from_str(err.description()) }
	}
}

impl fmt::Display for InternalError {
	fn fmt(&self, formatter: &mut fmt::Formatter) -> Result<(), fmt::Error> {
		self.reason.fmt(formatter)
	}
}

// TODO: It'd be nice if we could implement all these through generics,
// but a repetitive macro will do....
macro_rules! coerce_to_internal_error {
	(generic($($t:ty, $g:ident),*)) => {
		$(
		impl<$g:Send+::std::any::Any> convert::From<$t> for InternalError {
			fn from(err: $t) -> InternalError {
				InternalError::wrap(&err)
			}
		}
		)*
	};
	($($t:path),*) => {
		$(
		impl convert::From<$t> for InternalError {
			fn from(err: $t) -> InternalError {
				InternalError::wrap(&err)
			}
		}
		)*
	}
}
coerce_to_internal_error!(
	  io::Error
	, mpsc::RecvError
	, string::FromUtf8Error
	, json::EncoderError
	, chrono::format::ParseError
);
coerce_to_internal_error!(
	  generic(mpsc::SendError<T>, T)
);

impl Error for InternalError {
	fn description(&self) -> &str {
		self.reason.as_str()
	}
}
