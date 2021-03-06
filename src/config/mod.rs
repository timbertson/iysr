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
use rustc_serialize::json::Json;
use rustc_serialize::json;
use chrono::{Duration};
use monitor::Severity;
use util::*;
use regex::Regex;


#[macro_use]
mod internal;
mod error;
pub use config::error::*;

use config::internal::*;

#[derive(Clone)]
pub enum Pattern {
	Glob(::glob::Pattern),
	Regex(Regex),
	Literal(String),
}

#[derive(Clone)]
pub struct Match {
	pub attr: Option<String>,
	pub pattern: Pattern,
}

pub struct CommonConfig<T> {
	pub filters: Vec<T>,
	pub id:String,
}
impl<T:Clone> Clone for CommonConfig<T> {
	fn clone(&self) -> CommonConfig<T> {
		CommonConfig {
			filters: self.filters.clone(),
			id: self.id.clone(),
		}
	}
}

#[derive(Clone)]
pub struct FilterCommon {
	pub include: Vec<Match>,
	pub exclude: Vec<Match>,
}

impl FilterCommon {
	fn parse_matcher(matcher: Json) -> Result<Match, ConfigError> {
		match matcher {
			Json::String(lit) => {
				Ok(Match {
					attr: None,
					pattern: Pattern::Literal(lit),
				})
			},
			Json::Object(attrs) => {
				ConfigCheck::consume_new(attrs, |attrs| {
					let attr = try!(attrs.descend_json("attr", as_string_opt));
					let typ = try!(attrs.descend_json("type", |t| mandatory(t).and_then(as_string)));
					let pat = try!(attrs.descend_json("pattern", |t| mandatory(t).and_then(as_string)));
					let pat = match typ.deref() {
						"glob" => Pattern::Glob(try!(::glob::Pattern::new(pat.deref()))),
						"regex" => Pattern::Regex(try!(Regex::new(pat.deref()))),
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

	fn parse_matchers(json: Option<Json>) -> Result<Vec<Match>, ConfigError> {
		match json {
			Some(json) => json.descend_map_json(Self::parse_matcher),
			None => Ok(Vec::new()),
		}
	}

	fn parse(attrs: &mut ConfigMap) -> Result<FilterCommon, ConfigError> {
		Ok(FilterCommon {
			include: try!(attrs.descend_json("include", Self::parse_matchers)),
			exclude: try!(attrs.descend_json("exclude", Self::parse_matchers)),
		})
	}
}

#[derive(Clone)]
pub struct JournalConfig {
	pub common: CommonConfig<JournalFilter>,
	pub backlog: Option<i32>,
}

#[derive(Clone)]
pub struct JournalFilter {
	pub common: FilterCommon,
	pub level: Option<Severity>,
	pub attr_extend: Option<JsonMap>,
}

pub struct SystemdConfig {
	pub common: CommonConfig<()>,
	pub user: Option<bool>,
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
			|b| b.map_m(as_i32))
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
			l.map_m(|l| as_string(l).and_then(as_severity))));

		let attr_extend = try!(config.descend_json("attr_extend", |a|
			a.map_m(as_object)));

		Ok(JournalFilter {
			common: common,
			level: level,
			attr_extend: attr_extend,
		})
	}
}

fn as_severity(s:String) -> Result<Severity, ConfigError> {
	match s.deref() {
		"Emergency" => Ok(Severity::Emergency),
		"Alert"     => Ok(Severity::Alert),
		"Critical"  => Ok(Severity::Critical),
		"Error"     => Ok(Severity::Error),
		"Warning"   => Ok(Severity::Warning),
		"Notice"    => Ok(Severity::Notice),
		"Info"      => Ok(Severity::Info),
		"Debug"     => Ok(Severity::Debug),
		other       => Err(ConfigError::new(format!("Unknown severity: {}", other))),
	}
}

impl ModuleConfig for SystemdConfig {
	type Filter = ();
	fn parse(
		common: CommonConfig<Self::Filter>,
		mut config: Option<&mut ConfigMap>)
		-> Result<Self, ConfigError>
	{
		let user = try!(config.descend_json("user",
			|u| u.map_m(as_boolean)
		));
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
			let module = try!(attrs.descend_json("module", as_string_opt));
			(module, Some(attrs))
		},
		ref other => {
			return Err(type_mismatch(other, "Object or `true`"));
		},
	};
	ConfigCheck::consume_new_opt(conf, |conf| {
		let id = id.clone();
		Ok(match module.unwrap_or(id.clone()).deref() {
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
				filters.map_m(|filters|
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

	fn parse(c:&mut ConfigMap) -> Result<PollConfig, ConfigError> {
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
		let config = try!(as_config(config));
		ConfigCheck::consume(config, |config| {
			let poll = try!(config.consume("poll", |p| match p {
				None => Ok(PollConfig::default()),
				Some(c) => PollConfig::parse(c),
			}));
			let sources = try!(config.descend_json("sources", |sources| match sources {
				Some(json) => {
					let conf = try!(as_object(json));
					let mut rv = Vec::new();
					for (id, module_conf) in conf {
						let module_conf = try!(
							annotate_error!(id, parse_source_config(&id, module_conf))
						);
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
										common: FilterCommon { include: Vec::new(), exclude: Vec::new(), },
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
