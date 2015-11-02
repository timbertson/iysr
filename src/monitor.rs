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
use rustc_serialize::json::{Json};
use rustc_serialize::{Encodable,Encoder};
use chrono::{DateTime,UTC};
use chrono::Timelike;
use super::errors::InternalError;

#[derive(Debug, RustcEncodable, Clone)]
pub enum State {
	Active,
	Inactive,
	Error,
	Unknown,
}

#[derive(Debug,Eq,PartialEq,Clone,RustcEncodable)]
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

impl Severity {
	pub fn to_int(&self) -> i64 {
		match *self {
			Severity::Emergency => 0,
			Severity::Alert => 1,
			Severity::Critical => 2,
			Severity::Error => 3,
			Severity::Warning => 4,
			Severity::Notice => 5,
			Severity::Info => 6,
			Severity::Debug => 7,
		}
	}

	pub fn from_syslog(i:i64) -> Result<Severity,InternalError> {
		match i {
			0 => Ok(Severity::Emergency),
			1 => Ok(Severity::Alert),
			2 => Ok(Severity::Critical),
			3 => Ok(Severity::Error),
			4 => Ok(Severity::Warning),
			5 => Ok(Severity::Notice),
			6 => Ok(Severity::Info),
			7 => Ok(Severity::Debug),
			_ => Err(InternalError::new(format!("Unknown severity: {}", i))),
		}
	}

	pub fn to_string(&self) -> String {
		format!("{:?}", self)
	}

	fn cmp(&self, other: &Self) -> Ordering {
		let a = self.to_int();
		let b = other.to_int();
		// NOTE: Debug is "less than" emergency in terms of severity, so
		// we _reverse_ the arguments (because debug is a larger number than alert)
		b.cmp(&a)
	}
}

impl Default for Severity {
	fn default() -> Self {
		Severity::Info
	}
}

impl PartialOrd for Severity {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for Severity {
	fn cmp(&self, other: &Self) -> Ordering {
		let a = self.to_int();
		let b = other.to_int();
		// NOTE: Debug is "less than" emergency in terms of severity, so
		// we _reverse_ the arguments (because debug is a larger number than alert)
		b.cmp(&a)
	}
}

pub type Attributes = HashMap<String, Json>;

#[derive(Debug, RustcEncodable,Clone)]
pub struct Status {
	pub state: State,
	pub attrs: Arc<Attributes>,
}

#[derive(Debug,RustcEncodable)]
pub struct Event {
	pub id: Option<String>,
	pub severity: Option<Severity>,
	pub message: Option<String>,
	pub attrs: Arc<Attributes>,
}

#[derive(Debug)]
pub enum GaugeValue {
	Absolute(i32),
	Difference(i32),
}

#[derive(Debug)]
pub enum MetricValue {
	Counter(i32),
	Gauge(GaugeValue),
	Timespan(Duration),
}

#[derive(Debug)]
pub struct Metric {
	pub id: String,
	pub value: MetricValue,
}

#[derive(Debug)]
pub enum ComputedMetricValue {
	Int(i64),
	Float(f64),
	Duration(Duration),
}

impl Encodable for ComputedMetricValue {
	fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
		match *self {
			ComputedMetricValue::Int(ref n) => n.encode(s),
			ComputedMetricValue::Float(ref n) => n.encode(s),
			ComputedMetricValue::Duration(ref n) => n.encode(s),
		}
	}
}

#[derive(Debug)]
pub struct Duration(chrono::Duration);

impl Encodable for Duration {
	fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
		s.emit_struct("duration", 1, {|s|
			match *self {
				Duration(d) => {
					s.emit_struct_field("ms", 0, {|s| d.num_milliseconds().encode(s) })
				},
			}
		})
	}
}

#[derive(Debug,RustcEncodable)]
pub struct ComputedMetric {
	pub id: String,
	pub value: ComputedMetricValue,
}

#[derive(Debug, RustcEncodable)]
pub struct Metrics {
	values: Vec<ComputedMetric>,
	span: Duration,
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
	Metrics(Metrics),
	Error(Failure),
}

#[derive(Debug,RustcEncodable)]
pub struct Failure {
	// For ongoing / recurring errors, we use the same ID so that the UI can roll them up.
	// Ephemeral errors don't need an ID.
	pub id: Option<String>,
	pub error: String,
}

impl Encodable for Data {
	fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
		s.emit_tuple(2, {|s| {
			fn emit_pair<S:Encoder,V:Encodable>(s: &mut S, k: &'static str, v: &V) -> Result<(),S::Error> {
				try!(s.emit_tuple_arg(0, encode_sub!(k)));
				try!(s.emit_tuple_arg(1, encode_sub!(v)));
				Ok(())
			}

			match *self {
				Data::State(ref x) => emit_pair(s, "State", x),
				Data::Event(ref x) => emit_pair(s, "Event", x),
				Data::Metrics(ref x) => emit_pair(s, "Metrics", x),
				Data::Error(ref x) => emit_pair(s, "Error", x),
			}
		}})
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

impl Encodable for Time {
	fn encode<S:Encoder>(&self, encoder: &mut S) -> Result<(), S::Error> {
		encoder.emit_struct("Time", 2, |encoder| {
			try!(encoder.emit_struct_field("sec", 0, |e| self.timestamp().encode(e)));
			try!(encoder.emit_struct_field("ms", 1, |e| (self.time().nanosecond() / 1000000).encode(e)));
			Ok(())
		})
	}
}

#[derive(Debug)]
pub enum UpdateScope {
	Snapshot, // this update represents the entire latest state
	          // (will be cached and sent to new subscribers)
	Partial   // only a partial view of the state
}

#[derive(Debug)]
pub struct Update {
	pub source: String,
	pub scope: UpdateScope,
	pub typ: String,
	pub time: Time,
	pub data: Data,
}

impl Encodable for Update {
	fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
		s.emit_struct("update", 4, {|s| {
			try!(s.emit_struct_field("source", 0, encode_sub!(self.source)));
			try!(s.emit_struct_field("type", 1, encode_sub!(self.typ)));
			try!(s.emit_struct_field("time", 2, encode_sub!(self.time)));
			try!(s.emit_struct_field("data", 3, encode_sub!(self.data)));
			Ok(())
		}})
	}
}

pub trait DataSource: Send + Sync {
	fn id(&self) -> String;
	fn typ(&self) -> String;
}

impl DataSource for MonitorDataSource {
	fn id(&self) -> String { self.id.clone() }
	fn typ(&self) -> String { self.typ.clone() }
}

pub struct MonitorDataSource {
	id: String,
	typ: String,
}

impl MonitorDataSource {
	pub fn extract(src: &DataSource) -> MonitorDataSource {
		MonitorDataSource {
			id: src.id(),
			typ: src.typ(),
		}
	}
}

pub trait PullDataSource: Send + Sync + DataSource {
	fn poll(&self) -> Result<Data, InternalError>;
}

pub trait PushSubscription : Send {
}

pub trait PushDataSource: Send + Sync {
	fn subscribe(&self, mpsc::SyncSender<Arc<Update>>) -> Result<Box<PushSubscription>, InternalError>;
}

pub struct ErrorReporter {
	id: String,
	typ: String,
}

impl ErrorReporter {
	pub fn new(src: &DataSource) -> ErrorReporter {
		ErrorReporter {
			id: src.id(),
			typ: src.typ(),
		}
	}

	pub fn process_error<T:Error>(&self, emitter: &mpsc::SyncSender<Arc<Update>>, error_msg: &str, res: Result<(), T>) -> Result<Result<(), T>,InternalError> {
		match res {
			Ok(()) => Ok(Ok(())),
			Err(e) => {
				try!(emitter.try_send(Arc::new(Update {
					data: Data::Error(Failure {
						id: None,
						error: format!("Error {}: {}", error_msg, e),
					}),
					source: self.id.clone(),
					typ: self.typ.clone(),
					scope: UpdateScope::Partial,
					time: Time::now(),
				})));
				Ok(Err(e)) // we successfully reported an error. So the emitter is OK, but the underlying process failed.
			}
		}
	}

	// like `sender.send`, but with error reporting
	pub fn report_fatal<T:Error+convert::Into<InternalError>>(&self, emitter: &mpsc::SyncSender<Arc<Update>>, error_msg: &str, res: Result<(), T>) -> Result<(),InternalError> {
		match self.process_error(emitter, error_msg, res) {
			Ok(Ok(())) => Ok(()),
			Ok(Err(e)) => Err(e.into()),
			Err(e) => Err(e),
		}
	}

	pub fn report_recoverable<T:Error>(&self, emitter: &mpsc::SyncSender<Arc<Update>>, error_msg: &str, res: Result<(), T>) -> Result<(), InternalError> {
		match self.process_error(emitter, error_msg, res) {
			// ignore Ok(Err(e)), the inner error is recoverable
			Ok(_) => Ok(()),
			// if the emitter is failed, this still fails
			Err(e) => Err(e),
		}
	}
}

