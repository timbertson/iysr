use std::collections::HashMap;
use std::process::{Command,Stdio,Child};
use std::thread;
use std::char;
use std::io;
use std::io::{BufReader};
use monitor::*;
use chrono::{DateTime,UTC};

#[derive(Debug)]
pub struct Unit<'a> {
	pub service: &'a Service<'a>,
	pub unit: &'a str,
}

pub struct SystemdMonitor;

fn read_all(source: &io::Read) -> io::Result<String> {
	let mut buf = Vec::new();
	try!(source.read_to_end(&buf));
	try!(String::from_utf8(buf))
}

impl SystemdMonitor {
	fn spawn(&self) -> Result<InternalError, Child> {
		let child = Command::new("systemctl")
				.arg("list-units")
				.arg("--no-pager")
				.arg("--no-legend")
				.arg("--full")
				.stdout(Stdio::Piped())
				.stderr(Stdio::Piped())
				.spawn();

		match child {
			Ok(child) => child,
			Err(err) => InternalError::new(format!("Unable to list systemd units: {}", err))
		}
	}

	fn get_unit_status(&self, list_line: &str) -> Result<InternalError,(str, PollResult)> {
		list_line = list_line.trim();
		let parts = list_line.split(char::is_whitespace);
		let unit = try!(parts.next().expect("blank line from systemctl list-units"));
		match Command::new("systemctl")
			.arg("show")
			.arg("--property=LoadState,ActiveState,SubState,Result")
			.arg(unit)
			.stdout(Stdio::Piped())
			.stderr(Stdio::Piped())
			.spawn()
		{
			Err(err) => InternalError::new(format!("Unable to get status for unit {}: {}", unit, err)),
			Ok(child) => {
				// XXX reuse a thread or two for this
				let ok_t = thread::Builder::scoped(|| {
					// TODO: actual status!
					(unit, PollResult {
						state: State::OK,
						time: UTC::now(),
					})
				});

				let err_t = thread::Builder::scoped(|| {
					let msg = try!(read_all(child.stderr));
					InternalError::new(format!("`systemctl show` failed: {}", msg));
				});

				if !child.wait().success() {
					return Err(try!(err_t.join()));
				}
				Ok(try!(ok_t))
			}
		}
	}

	fn parse(&self, child: Child) -> Result<InternalError,HashMap<&str, PollResult>> {
		let mut rv = HashMap::new();
		let stdout = BufReader::new(child.stdout);
		let stderr = child.stderr;

		let ok_t = thread::Builder::scoped(|| {
			let mut footer = false;
			for line in child.stdout {
				let (unit, status) = try!(self.get_unit_status(line));
				rv.insert(unit, status)
			}
		});

		let err_t = thread::Builder::scoped(|| {
			let msg = try!(read_all(child.stderr));
			InternalError::new(format!("`systemctl show` failed: {}", msg));
		});

		let status = try!(child.wait());
		if !status.success() {
			let error = try!(err_t.join());
			return Err(error);
		}

		try!(ok_t);
		rv
	}
}

impl <'a> Monitor<> for SystemdMonitor {
	fn scan(&self) -> Result<InternalError, HashMap<&str, PollResult>> {
		let child = try!(self.spawn());
		self.parse(child)
	}
}
