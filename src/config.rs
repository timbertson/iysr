use std::collections::{HashMap,BTreeMap};
use std::error::{Error};
use std::fmt;
use std::string;
use std::io;
use std::ops::{Deref,DerefMut,Index};
use std::convert;
use std::sync::mpsc;
use std::sync::{Arc};
use std::str::FromStr;
use std::num::ParseIntError;
use rustc_serialize::json::{Json,ToJson};
use rustc_serialize::json;
use chrono::{Duration};
use monitor::Severity;

type JsonMap = BTreeMap<String,Json>;

// TODO: there are a bunch of `clone` calls here that wouldn't be necessary with
// cleverer use of references

fn mandatory_key(key: &'static str, attrs: &mut ConfigMap) -> Result<Json, ConfigError> {
	attrs.remove(key).ok_or(ConfigError::missing_key(key))
}

// XXX is this required?
macro_rules! ann {
	($key: expr, $x: expr) => {
		match $x {
			rv@Ok(_) => rv,
			Err(mut e) => {
				e.annotate($key.to_string());
				Err(e)
			},
		}
	}
}

macro_rules! mandatory_key {
	($a: expr, $b: expr) => {
		try!(mandatory_key($a, $b))
	}
}

fn type_mismatch(j:&Json, desc: &'static str) -> ConfigError {
	ConfigError::new(format!("Expected {}, got {}", desc, json_type(j)))
}

fn json_type(j:&Json) -> &'static str {
	match *j {
		Json::Object(_) => "Object",
		Json::Array(_) => "Array",
		Json::String(_) => "String",
		Json::Boolean(_) => "Boolean",
		Json::Null => "Null",
		Json::I64(_) | Json::U64(_) => "Integer",
		Json::F64(_) => "Float",
	}
}

fn as_string(v: &Json) -> Result<String, ConfigError> {
	match *v {
		Json::String(ref s) => Ok(s.to_string()),
		ref v => Err(type_mismatch(v, "String")),
	}
}

fn as_string_opt(v: Option<&Json>) -> Result<Option<String>, ConfigError> {
	v.map_m(|v| as_string(&v))
}

fn mandatory(v: Option<&Json>) -> Result<&Json, ConfigError> {
	v.ok_or(ConfigError::missing())
}

fn as_i32(v: &Json) -> Result<i32, ConfigError> {
	match *v {
		Json::I64(n) => Ok(n as i32),
		Json::U64(n) => Ok(n as i32),

		Json::F64(_) |
		Json::Object(_) |
		Json::Array(_) |
		Json::String(_) |
		Json::Boolean(_) |
		Json::Null => Err(type_mismatch(v, "Integer"))
	}
}

macro_rules! as_string {
	($x: expr) => {
		try!(as_string($x))
	}
}

macro_rules! try_opt {
	($x: expr) => {
		match $x {
			None => None,
			Some(rv) => Some(try!(rv)),
		}
	}
}

fn as_object(j:&Json) -> Result<JsonMap, ConfigError> {
	match *j {
		Json::Object(ref attrs) => Ok(attrs.clone()),
		ref j => Err(type_mismatch(j, "Object")),
	}
}

fn as_config(j:&Json) -> Result<ConfigCheck, ConfigError> {
	match as_object(j) {
		Ok(attrs) => Ok(ConfigCheck::new(attrs)),
		Err(e) => Err(e),
	}
}


fn as_array(j:&Json) -> Result<Vec<Json>, ConfigError> {
	match *j {
		Json::Array(ref rv) => Ok(rv.clone()),
		ref j => Err(type_mismatch(j, "Array")),
	}
}


// XXX these impls are very repetitive
trait AnnotatedDescentMut {
	type Inner;
	fn descend_mut<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&mut Self::Inner>) -> Result<R,ConfigError>;
}
trait AnnotatedDescent {
	type Inner;
	fn descend<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&Self::Inner>) -> Result<R,ConfigError>;
}

trait AnnotatedDescentJsonIter {
	type Inner;
	fn descend_map_json<F,R>(&self, f: F) -> Result<Vec<R>,ConfigError>
		where F: Fn(&Json) -> Result<R,ConfigError>;
}

trait AnnotatedDescentJson {
	fn descend_json<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&Json>) -> Result<R,ConfigError>;
}

// XXX make this AnnotatedDescentJson
impl AnnotatedDescent for JsonMap {
	type Inner = Json;
	fn descend<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&Json>) -> Result<R,ConfigError>
	{
		let val = self.remove(key);
		match f(val.as_ref()) {
			rv@Ok(_) => rv,
			Err(mut e) => {
				e.annotate(key.to_string());
				Err(e)
			},
		}
	}
}

impl AnnotatedDescentJsonIter for Json {
	type Inner = ConfigMap;
	fn descend_map_json<F,R>(&self, f: F) -> Result<Vec<R>,ConfigError>
		where F: Fn(&Json) -> Result<R,ConfigError>
	{
		let arr = try!(as_array(self));
		arr.iter().enumerate().map_m(|pair| {
			let (idx, entry) = *pair;
			ann!(idx, f(entry))
		})
	}
}

impl AnnotatedDescentMut for JsonMap {
	type Inner = Json;
	fn descend_mut<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&mut Json>) -> Result<R,ConfigError>
	{
		let mut val = self.remove(key);
		match f(val.as_mut()) {
			rv@Ok(_) => rv,
			Err(mut e) => {
				e.annotate(key.to_string());
				Err(e)
			},
		}
	}
}

impl<'a, T:AnnotatedDescentJson> AnnotatedDescentJson for Option<&'a mut T> {
	fn descend_json<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&Json>) -> Result<R,ConfigError>
	{
		match *self {
			Some(ref mut inner) => inner.descend_json(key, f),
			None => ann!(key, f(None)),
		}
	}
}

#[derive(Debug)]
struct ConfigMap(BTreeMap<String,Json>);
impl ConfigMap {
	fn descend_json_mut<F,R>(&mut self, key: &'static str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&mut Json>) -> Result<R,ConfigError>
	{
		let mut val = self.remove(key);
		match f(val.as_mut()) {
			rv@Ok(_) => rv,
			Err(mut e) => {
				e.annotate(key.to_string());
				Err(e)
			},
		}
	}

}
impl AnnotatedDescentMut for ConfigMap {
	type Inner = ConfigMap;
	fn descend_mut<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&mut Self::Inner>) -> Result<R,ConfigError>
	{
		match
			self.remove(key)
			.map_m(as_config)
			.and_then(|conf| ConfigCheck::consume_opt(conf, f))
		{
			rv@Ok(_) => rv,
			Err(mut e) => {
				e.annotate(key.to_string());
				Err(e)
			},
		}
	}
}

impl AnnotatedDescentJson for ConfigMap {
	fn descend_json<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&Json>) -> Result<R,ConfigError>
	{
		let val = self.remove(key);
		match f(val.as_ref()) {
			rv@Ok(_) => rv,
			Err(mut e) => {
				e.annotate(key.to_string());
				Err(e)
			},
		}
	}
}

//Boo! requires HKT?
//trait ResultM<T> {
//	fn map_m<F,R,E>(&self, f:F) -> Result<M<R>,E>
//		where F:Fn(&T) -> Result<R,E>;
//}

trait OptionResultM<T> {
	fn map_m<F,R,E>(&self, f:F) -> Result<Option<R>,E>
		where F:FnOnce(&T) -> Result<R,E>;

	fn map_m_mut<F,R,E>(&mut self, f:F) -> Result<Option<R>,E>
		where F:FnOnce(&mut T) -> Result<R,E>;
}

trait CloneOptionResultM<T> {
	fn clone_map_m<F,R,E>(&self, f:F) -> Result<Option<R>,E>
		where F:FnOnce(T) -> Result<R,E>;
}

trait VecResultM<T> {
	fn map_m<F,R,E>(&mut self, f:F) -> Result<Vec<R>,E>
		where F:Fn(&T) -> Result<R,E>;
}


impl<T> OptionResultM<T> for Option<T> {
	fn map_m<F,R,E>(&self, f:F) -> Result<Option<R>,E>
		where F:FnOnce(&T) -> Result<R,E>
	{
		match *self {
			Some(ref x) => match f(x) {
				Ok(x) => Ok(Some(x)),
				Err(x) => Err(x),
			},
			None => Ok(None),
		}
	}

	fn map_m_mut<F,R,E>(&mut self, f:F) -> Result<Option<R>,E>
		where F:FnOnce(&mut T) -> Result<R,E>
	{
		match *self {
			Some(ref mut x) => match f(x) {
				Ok(x) => Ok(Some(x)),
				Err(x) => Err(x),
			},
			None => Ok(None),
		}
	}
}

impl<T:Clone> CloneOptionResultM<T> for Option<T> {
	fn clone_map_m<F,R,E>(&self, f:F) -> Result<Option<R>,E>
		where F:FnOnce(T) -> Result<R,E>
	{
		match *self {
			Some(ref x) => match f(x.clone()) {
				Ok(x) => Ok(Some(x)),
				Err(x) => Err(x),
			},
			None => Ok(None),
		}
	}
}

//impl<T> VecResultM<T> for Vec<T> {
//	fn map_m<F,R,E>(&self, f:F) -> Result<Vec<R>,E>
//		where F:Fn(&T) -> Result<R,E>
//	{
//		let mut rv = Vec::with_capacity(self.len());
//		for item in self.iter() {
//			match f(item) {
//				Ok(item) => rv.push(item),
//				Err(e) => { return Err(e); },
//			}
//		}
//		Ok(rv)
//	}
//}

impl<T:Iterator> VecResultM<T::Item> for T {
	// XXX if this were implemented on Vec, we wouldn't need &mut.
	// But then we couldn't apply it to e.g. vec.enumerate()
	fn map_m<F,R,E>(&mut self, f:F) -> Result<Vec<R>,E>
		where F:Fn(&T::Item) -> Result<R,E>
	{
		let mut rv = Vec::new(); // XXX use size_hint
		loop {
			let item = self.next();
			match item {
				Some(item) => match f(&item) {
					Ok(item) => rv.push(item),
					Err(e) => { return Err(e); },
				},
				None => { return Ok(rv); },
			}
		}
	}
}

enum Pattern {
	Glob(String),
	Regexp(String),
	Literal(String),
}

pub struct Match {
	attr: Option<String>,
	pattern: Pattern,
}

#[derive(Debug)]
pub enum ConfigErrorReason {
	MissingKey(&'static str),
	Missing,
	Generic(String),
}

#[derive(Debug)]
pub struct ConfigError {
	reason: ConfigErrorReason,
	context: Vec<String>,
}

impl ConfigError {
	pub fn new(message: String) -> ConfigError {
		ConfigError {
			reason: ConfigErrorReason::Generic(message),
			context: Vec::new(),
		}
	}

	fn missing_key(key: &'static str) -> ConfigError {
		ConfigError {
			reason: ConfigErrorReason::MissingKey(key),
			context: Vec::new(),
		}
	}

	fn missing() -> ConfigError {
		ConfigError {
			reason: ConfigErrorReason::Missing,
			context: Vec::new(),
		}
	}

	fn annotate(&mut self, key: String) {
		self.context.push(key);
	}
}

macro_rules! coerce_to_config_error {
	($($t:path),*) => {
		$(
		impl convert::From<$t> for ConfigError {
			fn from(err: $t) -> ConfigError {
				ConfigError::new(format!("{}", err))
			}
		}
		)*
	}
}
coerce_to_config_error!(ParseIntError, json::ParserError, io::Error);

impl fmt::Display for ConfigError {
	fn fmt(&self, formatter: &mut fmt::Formatter) -> Result<(), fmt::Error> {
		try!(match self.reason {
			ConfigErrorReason::Generic(ref msg) => msg.fmt(formatter),
			ConfigErrorReason::MissingKey(key) => format!("Missing config key `{}`", key).fmt(formatter),
			ConfigErrorReason::Missing => "Missing config value".fmt(formatter),
		});

		if !self.context.is_empty() {
			try!(" in config: `".fmt(formatter));
			let mut path = self.context.clone();
			path.reverse();
			try!(path.as_slice().connect(".").fmt(formatter));
			try!("`".fmt(formatter));
		}
		Ok(())
	}
}

impl Error for ConfigError {
	fn description(&self) -> &str {
		match self.reason {
			ConfigErrorReason::Generic(ref msg) => msg.as_str(),
			ConfigErrorReason::MissingKey(_) => "MissingKey",
			ConfigErrorReason::Missing => "Missing",
		}
	}
}

pub struct CommonConfig<T> {
	filters: Vec<T>,
	id:String,
}

pub struct FilterCommon {
	include: Option<Vec<Match>>,
	exclude: Option<Vec<Match>>,
}

impl FilterCommon {
	fn parse_matcher(matcher: &Json) -> Result<Match, ConfigError>
	{
		match *matcher {
			Json::String(ref lit) => {
				Ok(Match {
					attr: None,
					pattern: Pattern::Literal(lit.clone()),
				})
			},
			Json::Object(ref attrs) => {
				ConfigCheck::consume_new(attrs.clone(), |attrs| {
					let attr = try!(attrs.descend_json("attr", as_string_opt));
					let typ = try!(attrs.descend_json("type", |t| mandatory(t).and_then(as_string)));
					let pat = try!(attrs.descend_json("pattern", |t| mandatory(t).and_then(as_string)));
					let pat = match typ.as_str() {
						"glob" => Pattern::Glob(pat),
						"regexp" => Pattern::Regexp(pat),
						"literal" => Pattern::Literal(pat),
						other => {
							return Err(ConfigError::new(format!("Unsupported pattern type: {}", other)));
						}
					};
					Ok(Match {
						attr: attr,
						pattern: pat,
					})
				})
			},
			ref other => Err(type_mismatch(other, "String or Object")),
		}
	}

	fn parse_matchers(json: Option<&Json>) -> Result<Option<Vec<Match>>, ConfigError> {
		json.map_m(|json:&&Json|
			// XXX this is a **json, which is dumb.
			json.descend_map_json(Self::parse_matcher)
		)
	}

	fn parse(attrs: &mut ConfigMap) -> Result<FilterCommon, ConfigError> {
		Ok(FilterCommon {
			include: try!(attrs.descend_json("include", Self::parse_matchers)),
			exclude: try!(attrs.descend_json("exclude", Self::parse_matchers)),
		})
	}
}

pub struct JournalConfig {
	common: CommonConfig<JournalFilter>,
	backlog: Option<i32>,
}

pub struct SystemdConfig {
	common: CommonConfig<()>,
	user: Option<bool>,
}

pub struct JournalFilter {
	common: FilterCommon,
	level: Option<Severity>,
	attr_extend: Option<JsonMap>,
}

struct ConfigCheck {
	attrs: ConfigMap,
	checked: bool,
}


// XXX should we actually implement this?
impl Deref for ConfigMap {
	type Target = JsonMap;
	fn deref<'a>(&'a self) -> &'a Self::Target {
		match *self {
			ConfigMap(ref attrs) => attrs
		}
	}
}
impl DerefMut for ConfigMap {
	fn deref_mut<'a>(&'a mut self) -> &'a mut Self::Target {
		match *self {
			ConfigMap(ref mut attrs) => attrs
		}
	}
}

impl ConfigCheck {
	fn new(attrs: JsonMap) -> ConfigCheck {
		ConfigCheck {
			attrs: ConfigMap(attrs),
			checked: false,
		}
	}

	fn consume_new<F,R>(attrs: JsonMap, f: F) -> Result<R, ConfigError>
		where F: FnOnce(&mut ConfigMap) -> Result<R, ConfigError>
	{
		Self::consume(Self::new(attrs), f)
	}

	// Implemented as classmethod so that `self` gets moved
	fn consume<F,R>(mut slf: Self, f: F) -> Result<R, ConfigError>
		where F: FnOnce(&mut ConfigMap) -> Result<R, ConfigError>
	{
		let rv = f(&mut slf.attrs);

		slf.checked = true;
		let consumed = if slf.attrs.is_empty() {
			Ok(())
		} else {
			let keys : Vec<String> = slf.attrs.keys().cloned().collect();
			Err(ConfigError::new(format!(
				"Unused config key(s): {}", keys.as_slice().connect(", ")
			)))
		};

		match (rv, consumed) {
			(Ok(_), Err(e)) => Err(e),
			(rv, _) => rv,
		}
	}

	fn consume_opt<F,R>(slf: Option<Self>, f: F) -> Result<R, ConfigError>
		where F: FnOnce(Option<&mut ConfigMap>) -> Result<R, ConfigError>
	{
		match slf {
			Some(conf) => Self::consume(conf, |conf| f(Some(conf))),
			None => f(None),
		}
	}
}

impl Drop for ConfigCheck {
	fn drop(&mut self) {
		if !self.checked {
			panic!("Logic error: ConfigCheck not checked");
		}
	}
}

trait ModuleConfig {
	type Filter;
	fn parse(common: CommonConfig<Self::Filter>, config: Option<&mut ConfigMap>) -> Result<Self, ConfigError>;
	fn parse_filter(common: FilterCommon, config: &mut ConfigMap) -> Result<Self::Filter, ConfigError>;
}

impl ModuleConfig for JournalConfig {
	type Filter = JournalFilter;
	fn parse(
		common: CommonConfig<Self::Filter>,
		mut config: Option<&mut ConfigMap>)
		-> Result<Self, ConfigError>
	{
		let backlog = try!(config.descend_json("backlog",
			// XXX can we get rid of this double-ref?
			|b| b.map_m(|b: &&Json| as_i32(b)))
		);

		Ok(JournalConfig {
			common: common,
			backlog: backlog,
		})
	}

	fn parse_filter(
		common: FilterCommon,
		config: &mut ConfigMap)
		-> Result<Self::Filter, ConfigError>
	{
		let level = try!(config.descend_json("level", |l|
			l.map_m(|l| as_string(l).and_then(parse_severity))));

		let attr_extend = try!(config.descend("attr_extend", |a|
			a.map_m(|a| as_object(a.clone()))));

		Ok(JournalFilter {
			common: common,
			level: level,
			attr_extend: attr_extend,
		})
	}
}

fn parse_severity(s:String) -> Result<Severity, ConfigError> {
	match s.as_str() {
		"Emergency" => Ok(Severity::Emergency),
		"Alert"     => Ok(Severity::Alert),
		"Critical"  => Ok(Severity::Critical),
		"Error"     => Ok(Severity::Error),
		"Warning"   => Ok(Severity::Warning),
		"Notice"    => Ok(Severity::Notice),
		"Info"      => Ok(Severity::Info),
		"Debug"     => Ok(Severity::Debug),
		other       => Err(ConfigError::new(format!(
		                   "Unknown severity: {}", other))),
	}
}

impl ModuleConfig for SystemdConfig {
	type Filter = ();
	fn parse(
		common: CommonConfig<Self::Filter>,
		mut config: Option<&mut ConfigMap>)
		-> Result<Self, ConfigError>
	{
		let user = try!(config.descend_json("user", |u| match u {
			None => Ok(None),
			Some(&Json::Boolean(ref u)) => Ok(Some(u.clone())),
			Some(ref other) => Err(type_mismatch(other, "boolean")),
		}));
		Ok(SystemdConfig {
			common: common,
			user: user,
		})
	}

	fn parse_filter(
		_common: FilterCommon,
		_config: &mut ConfigMap)
		-> Result<Self::Filter, ConfigError>
	{
		Ok(())
	}
}


fn parse_source_config(id: &String, conf: Json) -> Result<SourceConfig, ConfigError> {
	let (module, conf) = match conf {
		Json::Boolean(true) => (None, None),
		Json::Object(mut attrs) => {
			let module = try!(attrs.descend("module", as_string_opt));
			(module, Some(ConfigCheck::new(attrs)))
		},
		ref other => {
			return Err(type_mismatch(other, "Object or `true`"));
		},
	};
	ConfigCheck::consume_opt(conf, |conf| {
		let id = id.clone();
		Ok(match module.unwrap_or(id.clone()).as_str() {
			"systemd" => {
				SourceConfig::Systemd(try!(parse_module(id, conf)))
			},
			"journal" => {
				SourceConfig::Journal(try!(parse_module(id, conf)))
			},
			other => {
				return Err(ConfigError::new(format!("Unknown module: {}", other)));
			}
		})
	})
}

fn parse_filter<T:ModuleConfig>(conf:&mut ConfigMap) -> Result<T::Filter, ConfigError> {
	let common = try!(FilterCommon::parse(conf));
	T::parse_filter(common, conf)
}

fn parse_module<T:ModuleConfig>(id: String, attrs: Option<&mut ConfigMap>) -> Result<T, ConfigError> {
	match attrs {
		None => {
			let common = CommonConfig {
				filters: Vec::new(),
				id: id,
			};
			T::parse(common, None)
		},
		Some(attrs) => {
			let filters = try!(attrs.descend_json("filters", |filters| {
				filters.map_m(|filters:&&Json|
					filters.descend_map_json(|filter|
						as_config(filter)
							.and_then(|filter| ConfigCheck::consume(filter, parse_filter::<T>))
					)
				)
			}));

			let filters = match filters {
				Some(f) => f,

				// If we have no "filters", parse filter config from toplevel
				None => vec!(try!(parse_filter::<T>(attrs))),
			};
			let common = CommonConfig {
				filters: filters,
				id: id,
			};
			T::parse(common, Some(attrs))
		},
	}
}

pub enum SourceConfig {
	Systemd(SystemdConfig),
	Journal(JournalConfig),
}

pub struct PollConfig {
	interval: Duration,
}

impl PollConfig {
	fn default() -> PollConfig {
		PollConfig { interval: Self::default_interval() }
	}

	fn default_interval() -> Duration { Duration::seconds(15) }

	fn parse(c:&Json) -> Result<PollConfig, ConfigError> {
		let c = try!(as_config(c));
		ConfigCheck::consume(c, |c| {
			let invalid_duration = |d:&String|
				ConfigError::new(format!("Invalid duration: {}", d));

			let duration = try!(c.descend_json("interval", |s | match s {
				Some(s) => {
					let s = try!(as_string(s));
					let suffix_loc = try!(
						s.find(|c:char| !c.is_numeric())
						.ok_or_else(||invalid_duration(&s))
					);
					let digits = s.index(0..suffix_loc);
					let suffix = s.index(suffix_loc..);
					let val = try!(i64::from_str(digits));
					Ok(match suffix {
						"ms" => Duration::milliseconds(val),
						"s" => Duration::seconds(val),
						"m" => Duration::minutes(val),
						"h" => Duration::hours(val),
						"d" => Duration::days(val),
						_ => {
							return Err(invalid_duration(&s));
						}
					})
				},
				None => Ok(Self::default_interval()),
			}));
			Ok(PollConfig {
				interval: duration,
			})
		})
	}
}

pub struct Config {
	pub sources: Vec<SourceConfig>,
	pub poll: PollConfig,
}

impl Config {
	pub fn load(file: &mut io::Read) -> Result<Config, ConfigError> {
		let json = try!(Json::from_reader(file));
		Self::parse(json)
	}

	pub fn parse(config:Json) -> Result<Config, ConfigError> {
		let config = try!(as_config(&config));
		ConfigCheck::consume(config, |config| {
			let poll = try!(config.descend("poll", |p| match p {
				None => Ok(PollConfig::default()),
				Some(c) => PollConfig::parse(&c),
			}));
			let sources = try!(config.descend_json_mut("sources", |sources| match sources {
				Some(json) => {
					let conf = try!(as_object(json));
					let mut rv = Vec::new();
					for (id, module_conf) in conf {
						let module_conf = try!(ann!(id, parse_source_config(&id, module_conf)));
						rv.push(module_conf);
					}
					Ok(rv)
				},
				None => {
					//TODO: is this the best place for default config?
					Ok(vec!(
						SourceConfig::Systemd(SystemdConfig {
							common: CommonConfig {
								filters: Vec::new(),
								id: "systemd.system".to_string(),
							},
							user: None,
						}),
						SourceConfig::Journal(JournalConfig {
							common: CommonConfig {
								filters: vec!(
									JournalFilter {
										common: FilterCommon { include: None, exclude: None, },
										level: Some(Severity::Warning),
										attr_extend: None,
									}
								),
								id: "journal".to_string(),
							},
							backlog: None,
						}),
					))
				},
			}));
			Ok(Config {
				poll: poll,
				sources: sources,
			})
		})
	}
}
