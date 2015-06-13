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
use util::*;
use config::{Pattern, Match,FilterCommon,JournalFilter};

fn drill<'a>(path: &str, obj: &'a JsonMap) -> Option<&'a str> {
	//TODO: process a path, not just a single key
	match obj.get(path) {
		Some(&Json::String(ref s)) => Some(s.as_str()),
		// note: we're silently ignoring non-string attributes
		_ => None,
		}
}

fn test(s: &str, pat: &Pattern) -> bool {
	match *pat {
		Pattern::Glob(ref p) => p.matches(s),
		Pattern::Regex(ref p) => p.is_match(s),
		Pattern::Literal(ref p) => p == s,
	}
}

fn test_match(m: &Match, id: &str, attribs: &JsonMap) -> bool {
	let subject = match m.attr {
		Some(ref attr) => drill(attr.as_str(), attribs),
		None => Some(id),
	};
	match subject {
		None => false,
		Some(s) => test(s, &m.pattern),
	}
}

fn matches_common(common: &FilterCommon, id: &str, payload: &JsonMap) -> bool {
	let matches = |m| test_match(m, id, payload);
	if !common.include.is_empty() {
		if !common.include.iter().any(&matches) {
			return false;
		}
	}
	!common.exclude.iter().any(&matches)
}

pub trait Filter {
	fn matches(&self, id: &str, payload: &JsonMap) -> bool;
	fn mutate(&self, x: JsonMap) -> JsonMap;
}

impl Filter for JournalFilter {
	fn matches(&self, id: &str, payload: &JsonMap) -> bool {
		matches_common(&self.common, id, &payload)
	}

	fn mutate(&self, mut orig: JsonMap) -> JsonMap {
		match self.attr_extend {
			Some(ref extra) => orig.extend(extra.clone()),
			None => (),
		};
		orig
	}
}

pub fn filter<T:Filter>(id: &str, filters: &Vec<T>, payload: JsonMap)
	-> Option<JsonMap>
{
	if filters.is_empty() {
		return Some(payload);
	}
	for filter in filters {
		if filter.matches(id, &payload) {
			return Some(filter.mutate(payload))
		}
	}
	None
}
