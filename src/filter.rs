use std::collections::{HashMap,BTreeMap};
use std::collections::btree_map;
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
use util::*;
use monitor::Severity;
use config::{Pattern, Match,FilterCommon,JournalFilter};

const PRIORITY : &'static str = "PRIORITY";

pub fn get_severity(attrs: &JsonMap) -> Option<Severity> {
	// it is a logic error to call this on a message _before_ JournalFilter::pre_mutate().
	// Once that has been called, this key is guaranteed to be either absent or a
	// valid Severity level
	match attrs.get(PRIORITY) {
		Some(p) => {
			let p = match *p {
				Json::I64(p) => p,
				_ => panic!("PRIORITY not an int"),
			};
			Severity::from_syslog(p).ok()
		},
		None => None,
	}
}

fn drill<'a>(path: &str, obj: &'a JsonMap) -> Option<&'a str> {
	//TODO: process a path, not just a single key
	match obj.get(path) {
		Some(&Json::String(ref s)) => Some(s.deref()),
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
		Some(ref attr) => drill(attr.deref(), attribs),
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
	fn matches(&self, id: &str, payload: &mut JsonMap) -> bool;
	fn pre_mutate(x: &mut JsonMap);
	fn mutate(&self, x: &mut JsonMap);
	fn post_mutate(x: &mut JsonMap);
}

impl Filter for JournalFilter {

	// global pre-mutation (for items that may be required by filters)
	fn pre_mutate(attrs: &mut JsonMap) {
		// process severity
		let severity = match attrs.entry(PRIORITY.to_string()) {
			btree_map::Entry::Vacant(_) => {
				debug!("Message has no priority");
				None
			},
			btree_map::Entry::Occupied(mut entry) => {
				let priority = match *entry.get() {
					Json::String(ref p) => i64::from_str(p).ok(),
					Json::I64(p) => Some(p),
					Json::U64(p) => Some(p as i64),
					_ => None,
				};

				match priority {
					None => {
						debug!("Non-int priority found: {}", entry.get());
						entry.remove();
						None
					},
					Some(i) => {
						entry.insert(Json::I64(i));
						match Severity::from_syslog(i) {
							Ok(sev) => {
								Some(sev)
							},
							Err(e) => {
								debug!("{}", e);
								None
							},
						}
					},
				}
			},
		};

		match severity {
			None => (),
			Some(s) => {
				attrs.insert("SEVERITY".to_string(), Json::String(s.to_string()));
			}
		};
	}

	fn matches(&self, id: &str, payload: &mut JsonMap) -> bool {
		if !matches_common(&self.common, id, payload) {
			return false;
		}
		match self.level {
			None => (),
			Some(ref target) => {
				match get_severity(payload) {
					None => warn!("entry is missing SEVERITY"),
					Some(lvl) => {
						trace!("Comparing target level {:?} to {:?}", target, lvl);
						if lvl < *target {
							return false;
						}
					},
				}
			},
		}
		true
	}

	// instance level mutation
	fn mutate(&self, orig: &mut JsonMap) {
		match self.attr_extend {
			Some(ref extra) => orig.extend(extra.clone()),
			None => (),
		}
	}

	// global mutation (only for filtered items)
	fn post_mutate(attrs: &mut JsonMap) {
		let mut bad_keys : Vec<String> = Vec::new();
		for key in attrs.keys() {
			if key.starts_with('_') {
				bad_keys.push(key.clone());
			}
		}

		for key in bad_keys {
			attrs.remove(&key);
		}
	}

}

pub fn filter<T:Filter>(id: &str, filters: &Vec<T>, mut payload: JsonMap)
	-> Option<JsonMap>
{
	T::pre_mutate(&mut payload);
	if filters.is_empty() {
		T::post_mutate(&mut payload);
		return Some(payload);
	}

	for f in filters {
		if f.matches(id, &mut payload) {
			f.mutate(&mut payload);
			T::post_mutate(&mut payload);
			return Some(payload);
		}
	}
	None
}
