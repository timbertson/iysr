extern crate chrono;

use chrono::{DateTime,UTC};
use std::collections::HashMap;
use std::error::{Error};
use std::fmt;
use std::string;
use std::io;
use std::convert;
use std::sync::mpsc;

#[derive(Debug)]
pub enum State {
	Running,
	Stopped,
	Missing,
	Error,
}

#[derive(Debug)]
pub enum Severity {
	Emergency,
	Alert,
	Critical,
	Error,
	Warning,
	Notice,
	Info,
	Debug,
}

#[derive(Debug)]
pub struct PollResult {
	pub state: State,
	pub time: DateTime<UTC>,
}

//#[derive(Debug)]
//pub struct Tags<'a> {
//	  pub tags: &'a [&'a str],
//}

#[derive(Debug)]
pub struct Service<'a> {
	pub name: &'a str,
}

#[derive(Debug)]
pub struct Status<'a> {
	pub state: State,
	pub service: &'a Service<'a>,
	pub time: DateTime<UTC>,
}

#[derive(Debug)]
pub struct Message<'a> {
	pub content: &'a str,
}

#[derive(Debug)]
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

// TODO: these impls are all the same. can't we use generics?
impl    convert::From<io::Error>             for InternalError { fn from(err: io::Error            ) -> InternalError { InternalError::wrap(&err) } }
impl    convert::From<mpsc::RecvError>       for InternalError { fn from(err: mpsc::RecvError      ) -> InternalError { InternalError::wrap(&err) } }
impl    convert::From<string::FromUtf8Error> for InternalError { fn from(err: string::FromUtf8Error) -> InternalError { InternalError::wrap(&err) } }
impl<T> convert::From<mpsc::SendError<T>>    for InternalError { fn from(err: mpsc::SendError<T>   ) -> InternalError { InternalError::new(format!("{}", err)) } }

impl Error for InternalError {
	fn description(&self) -> &str {
		self.reason.as_str()
	}
}

#[derive(Debug)]
pub enum Event <'a> {
	ServiceStateChange(&'a Service<'a>, State),
	Message(&'a Service<'a>, Message<'a>),
}

pub trait PollMonitor {
	type T;
	fn refresh(&self) { return }
	fn poll(&self, &Self::T) -> PollResult;
}

pub trait Monitor: Send + Sync {
	fn scan(&self) -> Result<HashMap<String, PollResult>, InternalError>;
}
