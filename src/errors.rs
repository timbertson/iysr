extern crate chrono;
extern crate hyper;

use std::collections::{HashMap,BTreeMap};
use std::error::{Error};
use std::fmt;
use std::string;
use std::io;
use std::convert;
use std::ops::Deref;
use std::sync::mpsc;
use std::sync::{Arc};
use std::cmp::Ordering;
use rustc_serialize::{json,Encodable,Encoder};
use chrono::{DateTime,UTC};
use chrono::Timelike;
use worker::{WorkerError,TickError};

#[derive(Debug)]
pub struct InternalError {
	pub reason: String,
}

impl Encodable for InternalError {
	fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
		self.reason.encode(s)
	}
}

impl InternalError {
	pub fn new(reason: String) -> InternalError {
		InternalError { reason: reason }
	}

	pub fn wrap(err: &Error) -> InternalError {
		InternalError { reason: err.description().to_string() }
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
	, hyper::error::Error
);
coerce_to_internal_error!(
	  generic(mpsc::SendError<T>, T)
);
coerce_to_internal_error!(
	  generic(mpsc::TrySendError<T>, T)
);

impl Error for InternalError {
	fn description(&self) -> &str {
		self.reason.deref()
	}
}

impl convert::From<WorkerError<InternalError>> for InternalError {
	fn from(err: WorkerError<InternalError>) -> InternalError {
		match err {
			WorkerError::Cancelled => InternalError::new(String::from("worker cancelled")),
			WorkerError::Failed(e) => e,
			WorkerError::Aborted(reason) => InternalError::new(reason),
		}
	}
}

impl convert::From<TickError> for InternalError {
	fn from(err: TickError) -> InternalError {
		match err {
			TickError::Cancelled => InternalError::new(String::from("worker cancelled")),
		}
	}
}
