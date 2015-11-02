use std::collections::{HashMap, HashSet};
use std::process::{Command,Stdio,Child};
use std::thread;
use std::thread::{JoinHandle};
use std::char;
use std::io;
use std::convert;
use std::error::{Error};
use std::sync::mpsc;
use std::sync::{Arc};
use std::io::{BufRead, BufReader};
use std::fmt;
use rustc_serialize::json::{Json};
use chrono::{DateTime,Local};
use monitor::*;
use config::SystemdConfig;
use util::read_all;
use dbus::{Connection,BusType,Message,MessageItem,Props};
use super::errors::*;

pub const SYSTEMD_TYPE : &'static str = "systemd";

#[derive(Debug)]
pub enum RuntimeError {
	UnexpectedBlankLine,
	ChildOutputStreamMissing,
	BadServiceName(String),
	MissingProperty(&'static str),
}

impl fmt::Display for RuntimeError {
	fn fmt(&self, formatter: &mut fmt::Formatter) -> Result<(), fmt::Error> {
		match *self {
			RuntimeError::UnexpectedBlankLine => "Unexpected blank line".fmt(formatter),
			RuntimeError::ChildOutputStreamMissing => "Child output stream missing".fmt(formatter),
			RuntimeError::BadServiceName(ref name) => format!("Bad service name: {}", name).fmt(formatter),
			RuntimeError::MissingProperty(p) => format!("Missing service property: {}", p).fmt(formatter),
		}
	}
}

impl Error for RuntimeError {
	fn description(&self) -> &str {
		"RuntimeError"
	}
}

coerce_to_internal_error!(RuntimeError);

pub fn should_ignore_unit(ignored_types: &HashSet<String>, unit: &str) -> Result<bool, InternalError> {
	let unit_type = try!(unit.rsplit('.').next().ok_or(RuntimeError::BadServiceName(unit.to_string())));
	//debug!("unit type: {}, ignored_types = {:?}", unit_type, self.ignored_types);
	Ok(if ignored_types.contains(unit_type) {
		debug!("ignoring unit: {}", unit);
		true
	} else {
		false
	})
}

pub fn state_of_active_state(active_state: &str) -> State {
	match active_state {
		"active" | "reloading" | "activating" => State::Active,
		"inactive" | "deactivating" => State::Inactive,
		"failed" => State::Error,
		_ => State::Unknown,
	}
}

