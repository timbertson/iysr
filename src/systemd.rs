use std::collections::{HashMap, HashSet};
use std::process::{Command,Stdio,Child};
use std::thread;
use std::char;
use std::io;
use std::sync::mpsc;
use std::io::{BufRead, BufReader};
use monitor::*;
use chrono::{DateTime,UTC};

pub struct SystemdMonitor {
	ignored_types: HashSet<String>,
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
}
impl ::std::convert::From<RuntimeError> for InternalError {
	fn from(err: RuntimeError) -> InternalError {
		// TODO: nice error messages
		InternalError::new(format!("{:?}", err))
	}
}

impl SystemdMonitor {
	pub fn new() -> SystemdMonitor {
		let ignored = HashSet::new();
		SystemdMonitor {
			ignored_types: ignored,
		}
	}

	fn spawn(&self) -> Result<Child, InternalError> {
		let child = Command::new("systemctl")
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

	fn get_unit_statuses(&self, units: &Vec<String>) -> Result<Vec<PollResult>, InternalError> {
		assert!(units.len() > 0);
		let check_time = UTC::now();
		match Command::new("systemctl")
			.arg("show")
			.arg("--property=LoadState,ActiveState,SubState,Result")
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

				fn process_props(time:DateTime<UTC>, props: &mut HashMap<String,String>) -> Result<PollResult, InternalError> {
					// TODO: actual status from current_props
					props.clear();
					Ok(PollResult {
						state: State::Running,
						time: time,
					})
				}

				let ok_t = try!(thread::Builder::new().scoped(|| {
					let mut rv : Vec<PollResult> = Vec::new();
					let mut current_props = HashMap::new();
					for line_r in stdout.lines() {
						let line = try!(line_r);
						if line.len() == 0 {
							// separator line - start new properties
							rv.push(try!(process_props(check_time, &mut current_props)));
							current_props = HashMap::new();
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
						rv.push(try!(process_props(check_time, &mut current_props)));
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

	fn process_unit_statuses(&self, rv: &mut HashMap<String,PollResult>, units: &mut Vec<String>)
		-> Result<(), InternalError>
	{
		//println!("running execv with {} units", units.len());
		let mut statuses = try!(self.get_unit_statuses(units));
		for (unit, status) in units.drain(0..).zip(statuses.drain(0..)) {
			rv.insert(unit, status);
		}
		Ok(())
	}

	fn parse(&self, child: &mut Child) -> Result<HashMap<String, PollResult>, InternalError> {
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
				let unit_type = try!(line.rsplit('.').next().ok_or(RuntimeError::BadServiceName));
				if self.ignored_types.contains(unit_type) {
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

impl <'a> Monitor<> for SystemdMonitor {
	fn scan(&self) -> Result<HashMap<String, PollResult>, InternalError> {
		let mut child = try!(self.spawn());
		self.parse(&mut child)
	}
}
