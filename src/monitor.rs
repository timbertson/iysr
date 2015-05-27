extern crate chrono;

use std::collections::{HashMap,BTreeMap};
use std::error::{Error};
use std::fmt;
use std::string;
use std::io;
use std::convert;
use std::sync::mpsc;
use std::sync::{Arc};
use rustc_serialize::json::{Json,ToJson};
use rustc_serialize::json;
use chrono::{DateTime,UTC};
use chrono::Timelike;

#[derive(Debug, RustcEncodable)]
pub enum State {
	Active,
	Inactive,
	Error,
	Unknown,
}

macro_rules! enum_json {
	($x:ty) => {
		impl ToJson for $x {
			fn to_json(&self) -> Json {
				Json::String(format!("{:?}", self))
			}
		}
	}
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

enum_json!(State);
enum_json!(Severity);

pub type Attributes = HashMap<String, Json>;

#[derive(Debug, RustcEncodable,ToJson)]
pub struct Status {
	pub state: State,
	pub attrs: Arc<Attributes>,
}

//impl ToJson for Status {
//	fn to_json(&self) -> Json {
//		// XXX this is suboptimal ;)
//		json::decode(json::encode(self));
//	}
//}

#[derive(Debug,ToJson)]
pub struct Event {
	pub id: String,
	pub severity: Option<Severity>,
	pub content: String,
	pub attrs: Arc<Attributes>,
}

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

impl ToJson for InternalError {
	fn to_json(&self) -> Json {
		self.reason.to_json()
	}
}

pub trait PollMonitor {
	type T;
	fn refresh(&self) { return }
	fn poll(&self, &Self::T) -> Status;
}

pub trait Monitor: Send + Sync {
	fn typ(&self) -> String;
	fn scan(&self) -> Result<HashMap<String, Status>, InternalError>;
}

#[derive(Debug)]
pub enum Data {
	State(HashMap<String, Status>),
	Event(Event),
	Error(InternalError),
}

impl ToJson for Data {
	fn to_json(&self) -> Json {
		let mut pair = Vec::new();
		match *self {
			Data::State(ref x) => {
				pair.push("State".to_json());
				pair.push(x.to_json());
			},
			Data::Event(ref x) => {
				pair.push("Event".to_json());
				pair.push(x.to_json());
			},
			Data::Error(ref x) => {
				pair.push("Error".to_json());
				pair.push(x.to_json());
			},
		}
		Json::Array(pair)
	}
}

#[derive(Clone)]
pub struct Time(DateTime<UTC>);
impl Time {
	pub fn now() -> Time {
		Time(UTC::now())
	}
	pub fn timestamp(&self) -> i64 {
		let Time(t) = *self;
		t.timestamp()
	}
	pub fn time(&self) -> chrono::NaiveTime {
		let Time(t) = *self;
		t.time()
	}
}
impl fmt::Debug for Time {
	fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
		let Time(t) = *self;
		t.fmt(f)
	}
}

//impl Encodable for Time {
//	fn encode<S:Encoder>(&self, encoder: &mut S) -> Result<(), S::Error> {
//		encoder.emit_struct("Time", 2, |encoder| {
//			try!(encoder.emit_struct_field("sec", 0usize, |e| self.timestamp().encode(e)));
//			try!(encoder.emit_struct_field("ms", 1usize, |e| (self.time().nanosecond() / 1000000).encode(e)));
//			Ok(())
//		})
//	}
//}

impl ToJson for Time {
	fn to_json(&self) -> Json {
		let mut attrs = BTreeMap::new();
		attrs.insert(String::from_str("sec"), Json::I64(self.timestamp()));
		attrs.insert(String::from_str("ms"), Json::U64((self.time().nanosecond() / 1000000) as u64));
		Json::Object(attrs)
	}
}

pub struct Update {
	pub source: String,
	pub typ: String,
	pub time: Time,
	pub data: Data,
}

impl ToJson for Update {
	fn to_json(&self) -> Json {
		let mut attrs = BTreeMap::new();
		attrs.insert(String::from_str("source"), self.source.to_json());
		attrs.insert(String::from_str("type"), self.typ.to_json());
		attrs.insert(String::from_str("time"), self.time.to_json());
		attrs.insert(String::from_str("data"), self.data.to_json());
		Json::Object(attrs)
	}
}

pub trait PullDataSource: Send + Sync {
	fn id(&self) -> String;
	fn typ(&self) -> String;
	fn poll(&self) -> Result<Data, InternalError>;
}

pub trait PushDataSource: Send + Sync {
	fn subscribe(&mut self, mpsc::SyncSender<Arc<Update>>) -> Result<(), InternalError>;
}
