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

coerce_to_internal_error!(dbus::Error);


const SYSTEMD_DBUS_PATH: &'static str = "/org/freedesktop/systemd1";
const SYSTEMD_DBUS_DEST: &'static str = "org.freedesktop.systemd1";
const SYSTEMD_MANAGER_IFACE: &'static str = "org.freedesktop.systemd1.Manager";
const DBUS_PROPERTIES_IFACE: &'static str = "org.freedesktop.DBus.Properties";
const DBUS_ROOT_IFACE: &'static str = "org.freedesktop.DBus";
const SYSTEMD_UNIT_IFACE: &'static str = "org.freedesktop.systemd1.Unit";
const DBUS_CALL_TIMEOUT: i32 = 1000 * 60;
const UNIT_ADDED: &'static str = "UnitNew";
const UNIT_REMOVED: &'static str = "UnitRemoved";
const RELOADING: &'static str = "Reloading";
const PROPERTIES_CHANGED: &'static str = "PropertiesChanged";


fn safe_remove<T>(vec: &mut Vec<T>, idx: usize) -> Option<T> {
	if vec.len() > idx {
		Some(vec.remove(idx))
	} else {
		None
	}
}

fn method_call(name: &str) -> Result<Message,InternalError> {
		Message::new_method_call(SYSTEMD_DBUS_DEST, SYSTEMD_DBUS_PATH, SYSTEMD_MANAGER_IFACE, name)
			.or(Err(InternalError::new(format!("Failed to create method call"))))
}

fn call_method(conn: &Connection, call: Message) -> Result<Message, dbus::Error> {
	let mut rv = try!(conn.send_with_reply_and_block(call, DBUS_CALL_TIMEOUT));
	// fail on a successful response which contains an error
	try!(rv.as_result());
	Ok(rv)
}

// A bit lame. headers contains Option<String>, but we can't match on those
// directly, so instead we convert each to Option<&str> (with the same
// lifetime as the original)
fn matchable_headers<'a>(
	headers: &'a (MessageType, Option<String>, Option<String>, Option<String>))
	-> (MessageType, Option<&'a str>, Option<&'a str>, Option<&'a str>)
{
	fn conv<'a>(o: &'a Option<String>) -> Option<&'a str> {
		o.as_ref().map(|x| x.deref())
	}
	match *headers {
		(typ, ref path, ref iface, ref name) =>
			(typ, conv(path), conv(iface), conv(name))
	}
}

pub fn watch_units(
	sender: &mpsc::SyncSender<Arc<Update>>,
	bus: BusType,
	ignored_types: HashSet<String>,
	error_reporter: ErrorReporter
	) -> Result<(), InternalError>
{
	use dbus::MessageItem::*;
	debug!("Connecting to {:?} bus", bus);
	let conn = try!(Connection::get_private(bus));
	let mut dbus_state = DBusState::new(&conn, sender, &ignored_types, error_reporter);

	debug!("Subscribing to {}", SYSTEMD_DBUS_DEST);
	let _:Message = try!(call_method(&conn, try!(method_call("Subscribe"))));

	// let _msgid_list_units = conn.send(try!(method_call("ListUnits")));
	// TODO: use these
	try!(conn.add_match(match_rule(UNIT_ADDED, None).deref()));
	try!(conn.add_match(match_rule(UNIT_REMOVED, None).deref()));
	// try!(conn.add_match(match_rule(RELOADING, None).deref()));

	let unit_listing = try!(conn.send_with_reply_and_block(try!(method_call("ListUnits")), DBUS_CALL_TIMEOUT));

	for item in unit_listing.get_items() {
		match item {
			Array(items, sig) => {
				debug!("sig: {}", sig);
				for item in items {
					try!(dbus_state.process_unit_tuple(item));
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
			ignore_error!(dbus_state.process_message(message), "dbus message");
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
		SYSTEMD_MANAGER_IFACE,
		path,
		name
	);
	debug!("Adding DBus match: {}", rv);
	rv
	// "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',member='Foo',path='/bar/foo',destination=':452345.34',arg2='bar'" 
}

fn property_match_rule(path: &str) -> String {
	let rv = format!("type='signal',sender='{}',interface='{}',path='{}',member='{}'",
		SYSTEMD_DBUS_DEST, 
		DBUS_PROPERTIES_IFACE,
		path,
		PROPERTIES_CHANGED,
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


#[derive(Debug,Clone)]
struct DBusUnit {
	status: Status,
	name: String,
	path: String,
}

struct DBusState<'a> {
	conn: &'a Connection,
	sender: &'a mpsc::SyncSender<Arc<Update>>,
	ignored_types: &'a HashSet<String>,
	error_reporter: ErrorReporter,
	// XXX these should be keyed as `Path`, but that's not hashable
	units: HashMap<String,DBusUnit>,
	state: HashMap<String,Status>,
}

fn get_unit_prop(conn: &Connection, path: &str, name: &str) -> Result<MessageItem,InternalError> {
	Ok(try!(Props::new(conn, SYSTEMD_DBUS_DEST, path, SYSTEMD_UNIT_IFACE, DBUS_CALL_TIMEOUT).get(name)))
}

impl<'a> DBusState<'a> {
	fn new(
		conn: &'a Connection,
		sender: &'a mpsc::SyncSender<Arc<Update>>,
		ignored_types: &'a HashSet<String>,
		error_reporter: ErrorReporter
	) -> DBusState<'a>
	{
		DBusState {
			conn: conn,
			sender: sender,
			ignored_types: ignored_types,
			error_reporter: error_reporter,
			units: HashMap::new(),
			state: HashMap::new(),
		}
	}

	fn process_unit_tuple(&mut self, item: dbus::MessageItem) -> Result<(), InternalError> {
		use dbus::MessageItem::*;
		fn fail<T>(subject: &fmt::Debug) -> Result<T,InternalError> {
			Err(InternalError::new(format!("Unexpected sbud response: {:?}", subject)))
		}

		match item {
			Struct(mut values) => {
				// XXX this would be much nicer with destructuring
				let unit_path = safe_remove(&mut values, 6);
				let unit_name = safe_remove(&mut values, 0);
				match (unit_name, unit_path) {
					// XXX can we do this without the `ref` and later `copy()`?
					(
						// The primary unit name as string
						Some(Str(ref unit_name)),

						// The human readable description string
						// The load state (i.e. whether the unit file has been loaded successfully)
						// The active state (i.e. whether the unit is currently started or not)
						// The sub state (a more fine-grained version of the active state that is specific to the unit type, which the active state is not)
						// A unit that is being followed in its state by this unit, if there is any, otherwise the empty string.
						
						// The unit object path
						Some(ObjectPath(ref unit_path)),

						// If there is a job queued for the job unit the numeric job id, 0 otherwise
						// The job type as string
						// The job object path
					) => {
						try!(self.add_unit(unit_name, unit_path));
						Ok(())
					},
					other => fail(&other)
				}
			},
			other => fail(&other)
		}
	}


	fn get_unit_status(&mut self, path: &String) -> Result<Status, InternalError> {
		use dbus::MessageItem::*;
		let active_state = match try!(get_unit_prop(self.conn, path, "ActiveState")) {
			Str(s) => state_of_active_state(s.deref()),
			other => return Err(InternalError::new(format!("Invalid ActiveState: {:?}", other))),
		};

		// ActiveState,SubState,Result,ExecMainExitTimestamp,ExecMainStartTimestamp,StatusText
		let attrs = HashMap::new(); // XXX populate

		Ok(Status {
			state: active_state,
			attrs: Arc::new(attrs),
		})
	}

	fn check_unit(&mut self, path: &str) -> Result<(), InternalError> {
		// XXX this seems a bit inefficient...
		let unit: Option<DBusUnit> = self.units.get(path).map(|x| (*x).clone());
		match unit {
			Some(mut unit) => {
				let status = try!(self.get_unit_status(&unit.path));
				unit.status = status;
				self.unit_changed(unit)
			},
			None => Ok(()),
		}
	}

	fn _update_unit(&mut self, unit: DBusUnit) {
		let _:Option<Status> = self.state.insert(unit.name.clone(), unit.status.clone());
		let _:Option<DBusUnit> = self.units.insert(unit.path.clone(), unit);
	}

	fn emit(&self) -> Result<(), InternalError> {
		try!(self.sender.send(Arc::new(Update {
			source: format!("TODO"),
			scope: UpdateScope::Snapshot,
			typ: format!("TODO"),
			time: Time::now(),
			data: Data::State(self.state.clone()),
		})));
		Ok(())
	}

	fn unit_changed(&mut self, unit: DBusUnit) -> Result<(), InternalError> {
		self._update_unit(unit);
		// NOTE: this sends the whole state - can we simplify this to an individual Update?
		self.emit()
	}

	fn add_unit(&mut self, name: &String, path: &Path) -> Result<(),InternalError> {
		let _path = path.to_string();
		let path = &_path;
		if self.units.contains_key(path) || try!(should_ignore_unit(self.ignored_types, name)) {
			return Ok(())
		}

		debug!("Adding unit {} with path {}", name, path);

		try!(self.conn.add_match(property_match_rule(path).deref()));
		let status = try!(self.get_unit_status(path));
		let unit = DBusUnit { status: status, name: name.clone(), path: path.clone() };
		self._update_unit(unit);
		Ok(())
	}

	fn remove_unit(&mut self, path: &Path) -> Result<(),InternalError> {
		let _path = path.to_string();
		let path = &_path;
		match self.units.remove(path) {
			None => {
				debug!("Remove_unit saw unknown path {}", path);
				Ok(())
			},
			Some(unit) => {
				debug!("Removing unit {}", unit.name);
				let _:Option<Status> = self.state.remove(unit.name.deref());
				self.emit()
			},
		}
	}

	fn process_message(&mut self, message: ConnectionItem) -> Result<(), InternalError> {
		let result = self._process_message(message);
		// dbus message processing errors shouldn't cause invalid states, so we can ignore individual failures
		// XXX after failed message; re-scan all units
		self.error_reporter.report_recoverable(self.sender, "processing DBus message", result)
	}

	fn _process_message(&mut self, message: ConnectionItem) -> Result<(), InternalError> {
		use dbus::MessageItem::*;
		debug!("MSG: {:?}", message);
		fn unexpected_message<T>(m: &fmt::Debug) -> Result<T,InternalError> {
			Err(InternalError::new(format!("Unknown DBus message: {:?}", m)))
		}

		match message {
			ConnectionItem::Nothing => Ok(()),
			ConnectionItem::WatchFd(_) => Ok(()),
			ConnectionItem::Signal(msg) => {
				// debug!("MSG HEADERS: {:?}", msg.headers());
				let headers = msg.headers();
				match matchable_headers(&headers) {
					(MessageType::Signal, Some(SYSTEMD_DBUS_PATH), Some(SYSTEMD_MANAGER_IFACE), signal_name) => {
						let mut items = msg.get_items();
						let path = safe_remove(&mut items, 1);
						let name = safe_remove(&mut items, 0);
						let unit_id = match (name, path) {
							(Some(Str(name)), Some(ObjectPath(path))) => Some((name, path)),
							_ => None,
						};
						match (signal_name, unit_id) {
							(Some(UNIT_ADDED), Some((name, path))) => (self.add_unit(&name, &path)),
							(Some(UNIT_REMOVED), Some((_name, path))) => (self.remove_unit(&path)),
							// (Some(RELOADING), None) => /* ... */,
							other => unexpected_message(&other),
						}
					},
					(MessageType::Signal,unit_path,Some(DBUS_PROPERTIES_IFACE),Some(PROPERTIES_CHANGED)) => {
						// warn!("TODO: process changed properties for unit {:?}, {:?}", unit_path, msg.get_items());
						// items is an array of [iface_name, changed, invalidated]. We could potentially
						// use these to update state, but let's just do a fresh poll for simplicity...
						match unit_path {
							Some(path) => self.check_unit(path),
							None => unexpected_message(&"no path given to PropertiesChanged"),
						}
					},
					(MessageType::Signal, unit_path, Some(DBUS_ROOT_IFACE), signal) => {
						debug!("Ignoring protocol signal from {:?}: {:?}", unit_path, signal);
						Ok(())
					},
					other => unexpected_message(&other),
				}
			},
			other => unexpected_message(&other),
		}
	}
}
