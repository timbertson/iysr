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
use super::systemd_dbus::*;
use super::systemd_common::*;
use super::systemd_subprocess::*;
extern crate dbus;

pub struct SystemdMonitor {
	ignored_types: HashSet<String>,
	user: bool,
	id: String,
}

pub struct SystemdPusher {
	ignored_types: HashSet<String>,
	user: bool,
	id: String,
}


impl SystemdMonitor {
	pub fn new(conf: SystemdConfig) -> SystemdMonitor {
		let common = conf.common;
		let user = conf.user.unwrap_or(false);

		// TODO: use includes instead of hard-coded stufff
		let mut ignored = HashSet::new();
		ignored.insert(String::from_str("device"));
		ignored.insert(String::from_str("target"));
		ignored.insert(String::from_str("slice"));
		ignored.insert(String::from_str("machine"));
		ignored.insert(String::from_str("mount"));

		SystemdMonitor {
			id: common.id,
			user: user,
			ignored_types: ignored,
		}
	}

	pub fn poller(&self) -> Box<SystemdPoller> {
		Box::new(SystemdPoller {
			ignored_types: self.ignored_types.clone(),
			user: self.user,
			id: self.id.clone(),
		})
	}

	pub fn pusher(&self) -> Box<SystemdPusher> {
		Box::new(SystemdPusher {
			ignored_types: self.ignored_types.clone(),
			user: self.user,
			id: self.id.clone(),
		})
	}
}

impl DataSource for SystemdPusher {
	fn id(&self) -> String { self.id.clone() }
	fn typ(&self) -> String { "systemd".to_string() }
}

impl PushDataSource for SystemdPusher {
	fn subscribe(&self, sender: mpsc::SyncSender<Arc<Update>>) -> Result<Box<PushSubscription>, InternalError> {
		let which = if self.user { BusType::Session } else { BusType::System };
		let ignored_types = self.ignored_types.clone();

		let error_reporter = ErrorReporter::new(self);
		let thread = try!(thread::Builder::new().spawn(move|| -> Result<(), InternalError> {
			let rv = watch_units(&sender, which, ignored_types, error_reporter);
			match rv {
				Ok(()) => Ok(()),
				Err(e) => {
					ignore_error!(sender.try_send(Arc::new(Update {
						data: Data::Error(Failure {
							id: Some("sytemd".to_string()),
							error: format!("failed to monitor systemd units: {}", e),
						}),
						source: "TODO".to_string(),
						typ: "TODO".to_string(),
						time: Time::now(),
					})), "Sending error event");
					Err(e)
				}
			}
		}));
		Ok(Box::new(SystemdDbusSubscription::new(thread)))
	}
}
