// types and helpers used during config parsing (but not useful beyond that)

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

use config::error::*;

pub type JsonMap = BTreeMap<String,Json>;

macro_rules! annotate_error {
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

pub fn type_mismatch(j:&Json, desc: &'static str) -> ConfigError {
	ConfigError::new(format!("Expected {}, got {}", desc, json_type(j)))
}

pub fn json_type(j:&Json) -> &'static str {
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

pub fn as_string(v: &Json) -> Result<String, ConfigError> {
	match *v {
		Json::String(ref s) => Ok(s.to_string()),
		ref v => Err(type_mismatch(v, "String")),
	}
}

pub fn as_boolean(v: &Json) -> Result<bool, ConfigError> {
	match *v {
		Json::Boolean(ref s) => Ok(*s),
		ref v => Err(type_mismatch(v, "Boolean")),
	}
}

pub fn as_string_opt(v: Option<&Json>) -> Result<Option<String>, ConfigError> {
	v.map_m(|v| as_string(&v))
}

pub fn mandatory(v: Option<&Json>) -> Result<&Json, ConfigError> {
	v.ok_or(ConfigError::missing())
}

pub fn as_i32(v: &Json) -> Result<i32, ConfigError> {
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

pub fn as_object(j:&Json) -> Result<JsonMap, ConfigError> {
	match *j {
		Json::Object(ref attrs) => Ok(attrs.clone()),
		ref j => Err(type_mismatch(j, "Object")),
	}
}

pub fn as_config(j:&Json) -> Result<ConfigCheck, ConfigError> {
	match as_object(j) {
		Ok(attrs) => Ok(ConfigCheck::new(attrs)),
		Err(e) => Err(e),
	}
}


pub fn as_array(j:&Json) -> Result<Vec<Json>, ConfigError> {
	match *j {
		Json::Array(ref rv) => Ok(rv.clone()),
		ref j => Err(type_mismatch(j, "Array")),
	}
}


// a wrapper around a JSON object which requires
// you to consume all of its keys
pub struct ConfigCheck {
	attrs: ConfigMap,
	checked: bool,
}


impl ConfigCheck {
	fn new(attrs: JsonMap) -> ConfigCheck {
		ConfigCheck {
			attrs: ConfigMap(attrs),
			checked: false,
		}
	}

	pub fn consume_new<F,R>(attrs: JsonMap, f: F) -> Result<R, ConfigError>
		where F: FnOnce(&mut ConfigMap) -> Result<R, ConfigError>
	{
		Self::consume(Self::new(attrs), f)
	}

	pub fn consume_new_opt<F,R>(attrs: Option<JsonMap>, f: F) -> Result<R, ConfigError>
		where F: FnOnce(Option<&mut ConfigMap>) -> Result<R, ConfigError>
	{
		Self::consume_opt(attrs.map(Self::new), f)
	}

	// Implemented as classmethod so that `self` gets moved
	pub fn consume<F,R>(mut slf: Self, f: F) -> Result<R, ConfigError>
		where F: FnOnce(&mut ConfigMap) -> Result<R, ConfigError>
	{
		let rv = f(&mut slf.attrs);

		slf.checked = true;
		let attrs = slf.attrs._deref();
		let consumed = if attrs.is_empty() {
			Ok(())
		} else {
			let keys : Vec<String> = attrs.keys().cloned().collect();
			Err(ConfigError::new(format!(
				"Unused config key(s): {}", keys.as_slice().connect(", ")
			)))
		};

		match (rv, consumed) {
			(Ok(_), Err(e)) => Err(e),
			(rv, _) => rv,
		}
	}

	pub fn consume_opt<F,R>(slf: Option<Self>, f: F) -> Result<R, ConfigError>
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

#[derive(Debug)]
pub struct ConfigMap(BTreeMap<String,Json>);
impl ConfigMap {
	fn _remove(&mut self, key:&str) -> Option<Json> {
		self._deref().remove(key)
	}

	fn _deref<'a>(&'a mut self) -> &'a mut JsonMap {
		match *self {
			ConfigMap(ref mut attrs) => attrs
		}
	}

	pub fn descend_json_mut<F,R>(&mut self, key: &'static str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&mut Json>) -> Result<R,ConfigError>
	{
		annotate_error!(key,
			f(self._remove(key).as_mut())
		)
	}

	pub fn consume<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&mut ConfigMap>) -> Result<R,ConfigError>
	{
		annotate_error!(key,
			self._remove(key)
			.map_m(as_config)
			.and_then(|conf| ConfigCheck::consume_opt(conf, f))
		)
	}
}



//Boo! requires HKT?
//trait ResultM<T> {
//	fn map_m<F,R,E>(&self, f:F) -> Result<M<R>,E>
//		where F:Fn(&T) -> Result<R,E>;
//}

pub trait OptionResultM<T> {
	fn map_m<F,R,E>(&self, f:F) -> Result<Option<R>,E>
		where F:FnOnce(&T) -> Result<R,E>;

	fn map_m_mut<F,R,E>(&mut self, f:F) -> Result<Option<R>,E>
		where F:FnOnce(&mut T) -> Result<R,E>;
}

pub trait CloneOptionResultM<T> {
	fn clone_map_m<F,R,E>(&self, f:F) -> Result<Option<R>,E>
		where F:FnOnce(T) -> Result<R,E>;
}

pub trait VecResultM<T> {
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



// XXX these impls are very repetitive
pub trait AnnotatedDescentJsonIter {
	type Inner;
	fn descend_map_json<F,R>(&self, f: F) -> Result<Vec<R>,ConfigError>
		where F: Fn(&Json) -> Result<R,ConfigError>;
}

pub trait AnnotatedDescentJson {
	fn descend_json<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&Json>) -> Result<R,ConfigError>;
}

// XXX make this AnnotatedDescentJson
impl AnnotatedDescentJson for JsonMap {
	fn descend_json<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&Json>) -> Result<R,ConfigError>
	{
		let val = self.remove(key);
		annotate_error!(key, f(val.as_ref()))
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
			annotate_error!(idx, f(entry))
		})
	}
}

impl<'a, T:AnnotatedDescentJson> AnnotatedDescentJson for Option<&'a mut T> {
	fn descend_json<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&Json>) -> Result<R,ConfigError>
	{
		match *self {
			Some(ref mut inner) => inner.descend_json(key, f),
			None => annotate_error!(key, f(None)),
		}
	}
}


impl AnnotatedDescentJson for ConfigMap {
	fn descend_json<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<&Json>) -> Result<R,ConfigError>
	{
		annotate_error!(key, f(self._remove(key).as_ref()))
	}
}
