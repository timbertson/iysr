use std::collections::{HashMap, HashSet};
use std::process::{Command,Stdio,Child};
use std::thread;
use std::thread::{JoinHandle,JoinGuard};
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
use super::systemd_common::*;
extern crate dbus;

coerce_to_internal_error!(dbus::Error);


const SYSTEMD_DBUS_PATH: &'static str = "/org/freedesktop/systemd1";
const SYSTEMD_DBUS_DEST: &'static str = "org.freedesktop.systemd1";
const SYSTEMD_DBUS_IFACE: &'static str = "org.freedesktop.systemd1.Manager";
const SYSTEMD_UNIT_IFACE: &'static str = "org.freedesktop.systemd1.Unit";
const DBUS_CALL_TIMEOUT: i32 = 1000 * 60;


fn method_call(name: &str) -> Result<Message,InternalError> {
		Message::new_method_call(SYSTEMD_DBUS_DEST, SYSTEMD_DBUS_PATH, SYSTEMD_DBUS_IFACE, name)
			.ok_or(InternalError::new(format!("Failed to create method call")))
}

pub fn watch_units(
	sender: &mpsc::SyncSender<Arc<Update>>,
	bus: BusType,
	ignored_types: HashSet<String>
	) -> Result<(), InternalError>
{
	use dbus::MessageItem::*;
	debug!("Connecting to {:?} bus", bus);
	let conn = try!(Connection::get_private(bus));
	let mut dbus_state = DBusState::new(&conn, sender);

	debug!("Subscribing to {}", SYSTEMD_DBUS_DEST);
	let _:Message = try!(
		conn.send_with_reply_and_block(
			try!(method_call("Subscribe")),
			DBUS_CALL_TIMEOUT));

	// let _msgid_list_units = conn.send(try!(method_call("ListUnits")));
	// TODO: use these
	let _msgid_unit_added = try!(conn.add_match(match_rule("UnitNew", None).as_str()));
	let _msgid_unit_removed = try!(conn.add_match(match_rule("UnitRemoved", None).as_str()));
	let _msgid_reloading = try!(conn.add_match(match_rule("Reloading", None).as_str()));

	let unit_listing = try!(conn.send_with_reply_and_block(try!(method_call("ListUnits")), DBUS_CALL_TIMEOUT));

	for item in unit_listing.get_items() {
		match item {
			Array(items, sig) => {
				debug!("sig: {}", sig);
				for item in items {
					match item {
						Struct(values) => {
							match values.as_slice() {
								// XXX can we do this without the `ref` / `copy()`?
								[
									// The primary unit name as string
									Str(ref unit_name),

									// The human readable description string
									_,

									// The load state (i.e. whether the unit file has been loaded successfully)
									_,

									// The active state (i.e. whether the unit is currently started or not)
									_,

									// The sub state (a more fine-grained version of the active state that is specific to the unit type, which the active state is not)
									_,

									// A unit that is being followed in its state by this unit, if there is any, otherwise the empty string.
									_,

									// The unit object path
									ObjectPath(ref unit_path),

									// If there is a job queued for the job unit the numeric job id, 0 otherwise
									// The job type as string
									// The job object path
									..
								] => {
									try!(dbus_state.add_unit(&ignored_types, unit_name, unit_path))
								},
								other => panic!("bad systemd ListUnits reply: {:?}", other),
							}
							// debug!("The whole unit: {:?}", values);
						},
						other => panic!("unknown item in array: {:?}", other),
					}
				}
			},
			other => panic!("unknown return type: {:?}", other),
		}
	}
	// initial state computed - send it
	try!(dbus_state.emit());
	// TODO: subscribe to all...


	loop {
		let messages = conn.iter(1000 * 60); // 1min timeout
		for message in messages {
			println!("DBUS MESSAGE: {:?}", message);
		}
	}
}

fn match_rule(name: &str, path: Option<&str>) -> String {
	let path = match path {
		Some(p) => format!("{}/{}", SYSTEMD_DBUS_PATH, p),
		None => SYSTEMD_DBUS_PATH.to_string(),
	};
	let rv = format!("type='signal',sender='{}',interface='{}',path='{}',member='{}'",
		SYSTEMD_DBUS_DEST, 
		SYSTEMD_DBUS_IFACE,
		path,
		name
	);
	debug!("Adding DBus match: {}", rv);
	rv
	// "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='Foo',path='/bar/foo',destination=':452345.34',arg2='bar'" 
}

pub struct SystemdDbusSubscription {
	thread: Option<JoinHandle<Result<(), InternalError>>>,
}

impl SystemdDbusSubscription {
	pub fn new(thread: JoinHandle<Result<(), InternalError>>) -> SystemdDbusSubscription {
		SystemdDbusSubscription { thread: Some(thread) }
	}
}

impl PushSubscription for SystemdDbusSubscription {
}

impl Drop for SystemdDbusSubscription {
	fn drop(&mut self) {
		match self.thread.take() {
			None => (),
			Some(thread) => {
				// XXX kill the thread!
				match thread.join() {
					Ok(Ok(())) => (),
					Err(e) => log_error!(e, "joining thread"),
					Ok(Err(e)) => log_error!(e, "joining thread"),
				}
			}
		}
	}
}


struct DBusUnit {
	status: Status,
	name: String,
	path: String,
}

struct DBusState<'a> {
	conn: &'a Connection,
	sender: &'a mpsc::SyncSender<Arc<Update>>,
	units: HashMap<String,DBusUnit>,
	state: HashMap<String,Status>,
}

fn get_unit_prop(conn: &Connection, path: &str, name: &str) -> Result<MessageItem,InternalError> {
	Ok(try!(Props::new(conn, SYSTEMD_DBUS_DEST, path, SYSTEMD_UNIT_IFACE, DBUS_CALL_TIMEOUT).get(name)))
}

impl<'a> DBusState<'a> {
	fn new(conn: &'a Connection, sender: &'a mpsc::SyncSender<Arc<Update>>) -> DBusState<'a> {
		DBusState {
			conn: conn,
			sender: sender,
			units: HashMap::new(),
			state: HashMap::new(),
		}
	}

	fn check_unit(&mut self, path: &String) -> Result<Status, InternalError> {
		use dbus::MessageItem::*;
		let active_state = match try!(get_unit_prop(self.conn, path, "ActiveState")) {
			Str(s) => state_of_active_state(s.as_str()),
			other => return Err(InternalError::new(format!("Invalid ActiveState: {:?}", other))),
		};
		let attrs = HashMap::new(); // XXX populate

		Ok(Status {
			state: active_state,
			attrs: Arc::new(attrs),
		})
	}

	fn update_unit (&mut self, unit: DBusUnit) {
		let _:Option<Status> = self.state.insert(unit.name.clone(), unit.status.clone());
		let _:Option<DBusUnit> = self.units.insert(unit.path.clone(), unit);
	}

	fn emit(&self) -> Result<(), InternalError> {
		try!(self.sender.send(Arc::new(Update {
			source: format!("TODO"),
			typ: format!("TODO"),
			time: Time::now(),
			data: Data::State(self.state.clone()),
		})));
		Ok(())
	}

	fn emit_unit(&mut self, unit: DBusUnit) -> Result<(), InternalError> {
		self.update_unit(unit);
		// NOTE: this sends the whole state - can we simplify this to an individual Update?
		self.emit()
	}

	fn add_unit(&mut self, ignored_types: &HashSet<String>, name: &String, path: &String) -> Result<(),InternalError> {
		if self.units.contains_key(path) || try!(should_ignore_unit(ignored_types, name)) {
			return Ok(())
		}

		debug!("Adding unit {} with path {}", name, path);
		let status = try!(self.check_unit(path));
		let unit = DBusUnit { status: status, name: name.clone(), path: path.clone() };
		self.update_unit(unit);
		Ok(())
	}
}
