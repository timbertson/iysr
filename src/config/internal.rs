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
use util::*;

use config::error::*;

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

pub fn as_string(v: Json) -> Result<String, ConfigError> {
	match v {
		Json::String(s) => Ok(s.to_string()),
		v => Err(type_mismatch(&v, "String")),
	}
}

pub fn as_boolean(v: Json) -> Result<bool, ConfigError> {
	match v {
		Json::Boolean(s) => Ok(s),
		v => Err(type_mismatch(&v, "Boolean")),
	}
}

pub fn as_string_opt(v: Option<Json>) -> Result<Option<String>, ConfigError> {
	v.map_m(as_string)
}

pub fn mandatory(v: Option<Json>) -> Result<Json, ConfigError> {
	v.ok_or(ConfigError::missing())
}

pub fn as_i32(v: Json) -> Result<i32, ConfigError> {
	match v {
		Json::I64(n) => Ok(n as i32),
		Json::U64(n) => Ok(n as i32),

		Json::F64(_) |
		Json::Object(_) |
		Json::Array(_) |
		Json::String(_) |
		Json::Boolean(_) |
		Json::Null => Err(type_mismatch(&v, "Integer"))
	}
}

pub fn as_object(j:Json) -> Result<JsonMap, ConfigError> {
	match j {
		Json::Object(attrs) => Ok(attrs),
		j => Err(type_mismatch(&j, "Object")),
	}
}

pub fn as_config(j:Json) -> Result<ConfigCheck, ConfigError> {
	match as_object(j) {
		Ok(attrs) => Ok(ConfigCheck::new(attrs)),
		Err(e) => Err(e),
	}
}


pub fn as_array(j:Json) -> Result<Vec<Json>, ConfigError> {
	match j {
		Json::Array(rv) => Ok(rv),
		j => Err(type_mismatch(&j, "Array")),
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



// A trait which can "map" over the Result type.
// An Err() result in _any_ of the elements is promoted to
// an early Err() return, while an input of all Ok(T) elements
// is wrapped up as a single Ok(Collection<T>) result.
//
// This API consumes the subject, which is appropriate
// for the config API. ResultRefM will let you map over references,
// but there's currently no use of it.

pub trait ResultRefM<I,O> {
	type OutputWrapper;
	fn map_ref_m<F,E>(&self, f:F) -> Result<Self::OutputWrapper,E>
		where F:Fn(&I) -> Result<O,E>;
}
impl<I,O> ResultRefM<I,O> for Vec<I> {
	type OutputWrapper = Vec<O>;
	fn map_ref_m<F,E>(&self, f:F) -> Result<Self::OutputWrapper,E>
		where F:Fn(&I) -> Result<O,E>
	{
		Self::OutputWrapper::run_m(self.iter().map(f))
	}
}
impl<I,O> ResultRefM<I,O> for Option<I> {
	type OutputWrapper = Option<O>;
	fn map_ref_m<F,E>(&self, f:F) -> Result<Self::OutputWrapper,E>
		where F:Fn(&I) -> Result<O,E>
	{
		Self::OutputWrapper::run_m(self.iter().map(f))
	}
}

pub trait ConsumeResultM<I,O> {
	type OutputWrapper;
	fn map_m<F,E>(self, f:F) -> Result<Self::OutputWrapper,E>
		where F:Fn(I) -> Result<O,E>;
}

impl<I,O,IT:Iterator<Item=I>> ConsumeResultM<(usize,I),O> for ::std::iter::Enumerate<IT> {
	type OutputWrapper = Vec<O>;
	fn map_m<F,E>(self, f:F) -> Result<Self::OutputWrapper,E>
		where F:Fn((usize,I)) -> Result<O,E>
	{
		Self::OutputWrapper::run_m(self.map(f))
	}
}

// Unlike the default Iterator for Option<T>, this
// yields the item by-value (and is therefore single-shot)
struct ConsumeOption<T>(Option<T>);
impl<T> Iterator for ConsumeOption<T> {
	type Item = T;
	fn next(&mut self) -> Option<Self::Item> {
		match *self {
			ConsumeOption(ref mut x) => x.take()
		}
	}
}

impl<I,O> ConsumeResultM<I,O> for Option<I> {
	type OutputWrapper = Option<O>;
	fn map_m<F,E>(self, f:F) -> Result<Self::OutputWrapper,E>
		where F:Fn(I) -> Result<O,E>
	{
		Self::OutputWrapper::run_m(&mut (ConsumeOption(self.map(f))))
	}
}

struct ConsumeVec<T>(Vec<T>);
impl<T> ConsumeVec<T> {
	fn new(mut v:Vec<T>) -> ConsumeVec<T> {
		// vec needs to be reversed so that pop() returns
		// elements in order
		v.reverse();
		ConsumeVec(v)
	}
}
impl<T> Iterator for ConsumeVec<T> {
	type Item = T;
	fn next(&mut self) -> Option<Self::Item> {
		match *self {
			ConsumeVec(ref mut inner) => inner.pop()
		}
	}
}

impl<I,O> ConsumeResultM<I,O> for Vec<I> {
	type OutputWrapper = Vec<O>;
	fn map_m<F,E>(self, f:F) -> Result<Self::OutputWrapper,E>
		where F:Fn(I) -> Result<O,E>
	{
		let iter = ConsumeVec::new(self);
		Self::OutputWrapper::run_m(iter.map(f))
	}
}

// An internal trait for output collections to
// consume an Iterator<Result<T,E>> iterator and produce a Result<Self<T>,E>>.
trait RunM<T,E,IT:Iterator<Item=Result<T,E>>>
{
	type Output;
	fn run_m(it: IT) -> Result<Self::Output,E>;
}

impl<T,E,IT: Iterator<Item=Result<T,E>>> RunM<T,E,IT> for Option<T> {
	type Output = Option<T>;
	fn run_m(mut it: IT) -> Result<Self::Output, E> {
		match it.next() {
			None => Ok(None),
			Some(res) => {
				// We only ever call Option<T>::run_m from
				// Option<T>.map_m, so this should never occur
				debug_assert!(it.next().is_none(), "more than one item in iterator");
				match res {
					Ok(ok) => Ok(Some(ok)),
					Err(e) => Err(e),
				}
			}
		}
	}
}

impl<T,E,IT: Iterator<Item=Result<T,E>>> RunM<T,E,IT> for Vec<T> {
	type Output = Vec<T>;
	fn run_m(mut it: IT) -> Result<Self::Output, E> {
		let mut rv = Vec::new();
		loop {
			match it.next() {
				None => { return Ok(rv); },
				Some(res) => match res {
					Ok(ok) => { rv.push(ok); },
					Err(e) => { return Err(e); },
				}
			}
		}
	}
}

// Traits for descending into (& consuming) an attribute
// as either Json data or a sub-ConfigMap object.
// Use of these methods will ensure that Error results will be correctly
// annotated with the attribute path in which the problem occurred.
pub trait AnnotatedDescentJson {
	fn descend_json<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<Json>) -> Result<R,ConfigError>;
}

impl AnnotatedDescentJson for JsonMap {
	fn descend_json<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<Json>) -> Result<R,ConfigError>
	{
		let val = self.remove(key);
		annotate_error!(key, f(val))
	}
}

// Treat descent over a None<T> as if it were a T with no properties
impl<'a, T:AnnotatedDescentJson> AnnotatedDescentJson for Option<&'a mut T> {
	fn descend_json<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<Json>) -> Result<R,ConfigError>
	{
		match *self {
			Some(ref mut inner) => inner.descend_json(key, f),
			None => annotate_error!(key, f(None)),
		}
	}
}

impl AnnotatedDescentJson for ConfigMap {
	fn descend_json<F,R>(&mut self, key: &str, f: F) -> Result<R,ConfigError>
		where F: FnOnce(Option<Json>) -> Result<R,ConfigError>
	{
		annotate_error!(key, f(self._remove(key)))
	}
}

// Consumes an enture array and descends into each index.
pub trait AnnotatedDescentJsonIter {
	fn descend_map_json<F,R>(self, f: F) -> Result<Vec<R>,ConfigError>
		where F: Fn(Json) -> Result<R,ConfigError>;
}

impl AnnotatedDescentJsonIter for Json {
	fn descend_map_json<F,R>(self, f: F) -> Result<Vec<R>,ConfigError>
		where F: Fn(Json) -> Result<R,ConfigError>
	{
		let arr = try!(as_array(self));
		ConsumeVec::new(arr).enumerate().map_m(|(idx, entry)| {
			annotate_error!(idx, f(entry))
		})
	}
}

