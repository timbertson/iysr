extern crate chrono;

use chrono::{DateTime,UTC};
use std::collections::HashMap;

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
	fn new(reason: &str) -> InternalError {
		InternalError { reason: reason }
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

pub trait Monitor {
	fn scan(&self) -> Result<InternalError, HashMap<&str, PollResult>>;
}


