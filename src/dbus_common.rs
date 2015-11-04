use std::collections::{HashMap, HashSet};
use std::process::{Command,Stdio,Child};
use std::thread;
use std::thread::{JoinHandle};
use std::char;
use std::ops::Deref;
use std::io;
use std::convert;
use std::hash;
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
use dbus::{Connection,BusType,Message,MessageItem,MessageType,Props,ConnectionItem,Path};
use super::errors::*;
use super::systemd_common::*;
extern crate dbus;

pub const DBUS_CALL_TIMEOUT: i32 = 1000 * 60;

impl convert::From<dbus::Error> for InternalError {
	fn from(err: dbus::Error) -> InternalError {
		let typ = match err.name() {
			Some(t) => t,
			None => "DBus error",
		};
		let message = match err.message() {
			Some(m) => m,
			None => "Failed",
		};
		InternalError::new(format!("{}: {}", typ, message))
	}
}

pub fn call_method(conn: &Connection, call: Message) -> Result<Message, dbus::Error> {
	let mut rv = try!(conn.send_with_reply_and_block(call, DBUS_CALL_TIMEOUT));
	// fail on a successful response which contains an error
	try!(rv.as_result());
	Ok(rv)
}
