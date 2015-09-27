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
use super::errors::InternalError;

#[derive(Debug, RustcEncodable, Clone)]
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

#[derive(Debug,Eq,PartialEq,Clone)]
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

enum_json!(Severity);

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

enum_json!(State);

pub type Attributes = HashMap<String, Json>;

#[derive(Debug, RustcEncodable,ToJson, Clone)]
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

impl ToJson for ComputedMetricValue {
	fn to_json(&self) -> Json {
		match *self {
			ComputedMetricValue::Int(ref n) => n.to_json(),
			ComputedMetricValue::Float(ref n) => n.to_json(),
			ComputedMetricValue::Duration(ref n) => n.to_json(),
		}
	}
}

#[derive(Debug)]
pub struct Duration(chrono::Duration);

impl ToJson for Duration {
	fn to_json(&self) -> Json {
		let mut attrs = BTreeMap::new();
		match *self {
			Duration(d) => {
				attrs.insert(String::from_str("ms"), Json::I64(d.num_milliseconds()));
			}
		}
		Json::Object(attrs)
	}
}

#[derive(Debug,ToJson)]
pub struct ComputedMetric {
	pub id: String,
	pub value: ComputedMetricValue,
}

#[derive(Debug,ToJson)]
pub struct Metrics {
	values: Vec<ComputedMetric>,
	span: Duration,
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
	Metrics(Metrics),
	Error(Failure),
}

#[derive(Debug,ToJson)]
pub struct Failure {
	// For ongoing / recurring errors, we use the same ID so that the UI can roll them up.
	// Ephemeral errors don't need an ID.
	pub id: Option<String>,
	pub error: String,
}

impl ToJson for Data {
	fn to_json(&self) -> Json {
		let (key, val) = match *self {
			Data::State(ref x) => ("State", x.to_json()),
			Data::Event(ref x) => ("Event", x.to_json()),
			Data::Metrics(ref x) => ("Metrics", x.to_json()),
			Data::Error(ref x) => ("Error", x.to_json()),
		};
		let mut pair = Vec::new();
		pair.push(key.to_json());
		pair.push(val);
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

#[derive(Debug)]
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

pub trait PushSubscription : Send {
}

pub trait PushDataSource: Send + Sync {
	fn subscribe(&self, mpsc::SyncSender<Arc<Update>>) -> Result<Box<PushSubscription>, InternalError>;
}
