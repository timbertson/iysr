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

	pub fn missing_key(key: &'static str) -> ConfigError {
		ConfigError {
			reason: ConfigErrorReason::MissingKey(key),
			context: Vec::new(),
		}
	}

	pub fn missing() -> ConfigError {
		ConfigError {
			reason: ConfigErrorReason::Missing,
			context: Vec::new(),
		}
	}

	pub fn annotate(&mut self, key: String) {
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
coerce_to_config_error!(ParseIntError, json::ParserError, io::Error, ::glob::PatternError, ::regex::Error);

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
