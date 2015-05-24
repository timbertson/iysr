use std::collections::{HashMap, HashSet};
use std::process::{Command,Stdio,Child};
use std::thread;
use std::char;
use std::io;
use std::convert;
use std::sync::mpsc;
use std::sync::{Arc};
use std::io::{BufRead, BufReader};
use rustc_serialize::json::{Json};
use chrono::{DateTime,Local};
use monitor::*;

pub struct SystemdMonitor {
	ignored_types: HashSet<String>,
	user: bool,
}

fn read_all(source: &mut io::Read) -> Result<String, InternalError> {
	let mut buf = Vec::new();
	try!(source.read_to_end(&mut buf));
	Ok(try!(String::from_utf8(buf)))
}

const MAX_EXECV_ARGLEN : usize = 4096; // conservative, actually much higher on most linux systems

#[derive(Debug)]
pub enum RuntimeError {
	UnexpectedBlankLine,
	ChildOutputStreamMissing,
	BadServiceName,
	MissingProperty(&'static str),
}
impl convert::From<RuntimeError> for InternalError {
	fn from(err: RuntimeError) -> InternalError {
		// TODO: nice error messages
		InternalError::new(format!("{:?}", err))
	}
}

impl SystemdMonitor {
	fn new(user: bool) -> SystemdMonitor {
		let mut ignored = HashSet::new();
		// XXX make configurable
		ignored.insert(String::from_str("device"));
		ignored.insert(String::from_str("target"));
		ignored.insert(String::from_str("slice"));
		ignored.insert(String::from_str("machine"));
		ignored.insert(String::from_str("mount"));
		SystemdMonitor {
			ignored_types: ignored,
			user: user,
		}
	}

	pub fn system() -> SystemdMonitor {
		Self::new(false)
	}

	pub fn user() -> SystemdMonitor {
		Self::new(true)
	}

	fn common_args<'a>(&self, cmd: &'a mut Command) -> &'a mut Command {
		if self.user {
			cmd.arg("--user")
		} else {
			cmd
		}
	}

	fn spawn(&self) -> Result<Child, InternalError> {
		let child = self.common_args(&mut Command::new("systemctl"))
				.arg("list-units")
				.arg("--no-pager")
				.arg("--no-legend")
				.arg("--full")
				.stdout(Stdio::piped())
				.stderr(Stdio::piped())
				.spawn();

		match child {
			Ok(child) => Ok(child),
			Err(err) => Err(InternalError::new(format!("Unable to list systemd units: {}", err)))
		}
	}

	fn parse_unit_name<'a>(&self, list_line: &'a str) -> Result<&'a str, RuntimeError> {
		let list_line = list_line.trim();
		let mut parts = list_line.splitn(2, char::is_whitespace);
		parts.next().ok_or(RuntimeError::UnexpectedBlankLine)
	}

	// TODO: use dbus interface to avoid lots of forking overhead, and to
	// allow notifications on signal change (rather than polling)
	fn get_unit_statuses(&self, units: &Vec<String>) -> Result<Vec<Status>, InternalError> {
		assert!(units.len() > 0);
		match self.common_args(&mut Command::new("systemctl"))
			.arg("show")
			.arg("--property=ActiveState,SubState,Result,ExecMainExitTimestamp,ExecMainStartTimestamp,StatusText")
			.arg("--")
			.args(units)
			.stdout(Stdio::piped())
			.stderr(Stdio::piped())
			.spawn()
		{
			Err(err) => Err(
				InternalError::new(format!("Unable to get status for {} unit(s): {}", units.len(), err))
			),
			Ok(mut child) => {
				// XXX reuse a thread or two for this
				let stdout = BufReader::new(try!(child.stdout.take().ok_or(RuntimeError::ChildOutputStreamMissing)));
				let mut stderr = try!(child.stderr.take().ok_or(RuntimeError::ChildOutputStreamMissing));

				fn process_props(props: &mut HashMap<String,String>) -> Result<Status, InternalError> {
					// TODO: actual status from current_props
					let state = match props.remove("ActiveState") {
						None => return Err(
							InternalError::from(RuntimeError::MissingProperty("ActiveState"))
						),
						Some(state) => match state.as_str() {
							"active" | "reloading" | "activating" => State::Active,
							"inactive" | "deactivating" => State::Inactive,
							"failed" => State::Error,
							_ => State::Unknown,
						}
					};
					let mut attrs = HashMap::with_capacity(props.len());
					for (key, val) in props.drain() {
						if val.len() == 0 {
							continue;
						}
						let _ : Option<Json> /* ensure each branch inserts the key */ = if key.ends_with("Timestamp") {
							// e.g. Sun 2015-05-24 13:59:07 AEST
							// chrono doesn't support `%Z` timezone specifier, so we
							// strip it off and assume all timestamps are local:
							let date_val = val.clone();
							let mut parts = date_val.rsplitn(2, " ");
							let tz = parts.next();
							match (tz, parts.next()) {
								// XXX check `tz` against the current local timezone abbreviation
								(Some(_), Some(date_val)) => {
									use chrono::offset::TimeZone;
									let local_time = Local::now();
									match local_time.timezone().datetime_from_str(&date_val, "%a %F %T") {
										Ok(ts) => {
											attrs.insert(key, Json::I64(ts.timestamp()))
										},
										Err(err) => {
											info!("Unable to parse timestamp [{}]: {}", date_val, err);
											attrs.insert(key, Json::String(val))
										}
									}
								},
								_ => {
									info!("timestamp doesn't have any spaces");
									attrs.insert(key, Json::String(val))
								},
							}
						} else {
							attrs.insert(key, Json::String(val))
						};
					}
					Ok(Status {
						state: state,
						attrs: Arc::new(attrs),
					})
				}

				let ok_t = try!(thread::Builder::new().scoped(|| {
					let mut rv : Vec<Status> = Vec::new();
					let mut current_props = HashMap::new();
					for line_r in stdout.lines() {
						let line = try!(line_r);
						if line.len() == 0 {
							// separator line - start new properties
							rv.push(try!(process_props(&mut current_props)));
							current_props.clear();
						} else {
							let mut parts = line.splitn(2, '=');
							let key = parts.next();
							let val = parts.next();
							assert!(parts.next().is_none());
							match (key, val) {
								(Some(key), Some(val)) => {
									current_props.insert(String::from_str(key), String::from_str(val));
								},
								_ => {
									return Err(InternalError::new(format!("Invalid property line: {}", line)));
								},
							}
						}
					}
					if !current_props.is_empty() {
						rv.push(try!(process_props(&mut current_props)));
					}
					Ok(rv)
				}));

				let err_t = try!(thread::Builder::new().scoped(|| {
					let msg = try!(read_all(&mut stderr));
					Err(InternalError::new(format!("`systemctl show` failed: {}", msg)))
				}));

				let status = try!(child.wait());
				if !status.success() {
					return err_t.join();
				}
				ok_t.join()
			}
		}
	}

	fn process_unit_statuses(&self, rv: &mut HashMap<String,Status>, units: &mut Vec<String>)
		-> Result<(), InternalError>
	{
		//println!("running execv with {} units", units.len());
		let mut statuses = try!(self.get_unit_statuses(units));
		for (unit, status) in units.drain(0..).zip(statuses.drain(0..)) {
			rv.insert(unit, status);
		}
		Ok(())
	}

	fn parse(&self, child: &mut Child) -> Result<HashMap<String, Status>, InternalError> {
		let stdout = BufReader::new(try!(child.stdout.take().ok_or(RuntimeError::ChildOutputStreamMissing)));
		let mut stderr = try!(child.stderr.take().ok_or(RuntimeError::ChildOutputStreamMissing));

		let (sender, receiver) = mpsc::sync_channel(20);

		let collector_t = try!(thread::Builder::new().scoped(move|| {
			let mut rv = HashMap::new();
			let mut units = Vec::new();
			let mut argv_len : usize = 0;

			loop {
				match try!(receiver.recv()) {
					Some(unit) => {
						let unit : String = unit; // XXX move this to a type annotation
						let len : usize = unit.len();
						argv_len += len;
						// leave 200 for leading args
						if argv_len > MAX_EXECV_ARGLEN - 200 {
							try!(self.process_unit_statuses(&mut rv, &mut units));
							argv_len = len;
						}
						units.push(unit);
					},
					None => {
						// EOF sentinel
						try!(self.process_unit_statuses(&mut rv, &mut units));
						return Ok(rv);
					}
				}
			}
		}));

		let ok_t = try!(thread::Builder::new().scoped(move|| -> Result<(), InternalError> {
			for line_r in stdout.lines() {
				let line = try!(line_r);
				let unit = try!(self.parse_unit_name(&line));
				let unit_type = try!(unit.rsplit('.').next().ok_or(RuntimeError::BadServiceName));
				//debug!("unit type: {}, ignored_types = {:?}", unit_type, self.ignored_types);
				if self.ignored_types.contains(unit_type) {
					debug!("ignoring unit: {}", unit);
					continue;
				}
				try!(sender.send(Some(String::from_str(unit))));
			}
			try!(sender.send(None));
			Ok(())
		}));

		let err_t = try!(thread::Builder::new().scoped(move|| {
			let msg = try!(read_all(&mut stderr));
			Err(InternalError::new(format!("`systemctl show` failed: {}", msg)))
		}));

		let status = try!(child.wait());
		if !status.success() {
			return err_t.join();
		}

		try!(ok_t.join());
		collector_t.join()
	}
}

impl Monitor for SystemdMonitor {
	fn typ(&self) -> String {
		return String::from_str("systemd");
	}
	fn scan(&self) -> Result<HashMap<String, Status>, InternalError> {
		let mut child = try!(self.spawn());
		self.parse(&mut child)
	}
}
