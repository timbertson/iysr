// show persistent notifications for system state, plus
// transient notifications for events

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
use std::sync::{Arc,Mutex};
use std::io::{BufRead, BufReader};
use std::fmt;
use rustc_serialize::json::{Json};
use std::time;
use chrono::{DateTime,Local};
use monitor::*;
use config::SystemdConfig;
use util::read_all;
use dbus::{Connection,BusType,Message,MessageItem,MessageType,Props,ConnectionItem,Path};
use system_monitor::{SystemMonitor,Receiver};
use worker::{Worker,WorkerSelf};
use super::dbus_common::*;
use super::errors::*;
use super::systemd_common::*;
use super::system_monitor::StateSnapshot;
extern crate dbus;

const NOTIFY_IFACE: &'static str = "org.freedesktop.Notifications";
const NOTIFY_PATH: &'static str = "/org/freedesktop/Notifications";
const NOTIFY_SHOW_METHOD: &'static str = "Notify";
const NOTIFY_HIDE_METHOD: &'static str = "CloseNotification";

struct DbusNotify<'a> {
	conn: &'a Connection,
	id: Option<u32>,
	title: String,
	body: String,
	timeout: i32,
}

fn method_call(name: &str) -> Result<Message,InternalError> {
		Message::new_method_call(NOTIFY_IFACE, NOTIFY_PATH, NOTIFY_IFACE, name)
			.or(Err(InternalError::new(format!("Failed to create method call"))))
}


impl<'a> DbusNotify<'a> {
	fn convert_timeout(t: Option<time::Duration>) -> i32 {
		match t {
			Some(t) => (t.as_secs() * 1000) as i32 + (t.subsec_nanos() / 1000000) as i32,
			None => 0,
		}
	}

	fn new(conn: &'a Connection, title: String, body: String, timeout: Option<time::Duration>) -> DbusNotify {
		DbusNotify {
			conn: conn,
			id: None,
			title: title,
			body: body,
			timeout: DbusNotify::convert_timeout(timeout),
		}
	}

	fn empty(conn: &'a Connection, timeout: Option<time::Duration>) -> DbusNotify {
		DbusNotify {
			conn: conn,
			id: None,
			title: "".into(),
			body: "".into(),
			timeout: DbusNotify::convert_timeout(timeout),
		}
	}

	fn set_title(&mut self, title: String) {
		self.title = title;
	}

	fn set_body(&mut self, body: String) {
		self.body = body;
	}

	fn set_timeout(&mut self, t: Option<time::Duration>) {
		self.timeout = DbusNotify::convert_timeout(t);
	}

	fn show(&mut self) -> Result<(), InternalError> {
		let id = match self.id {
			None => 0,
			Some(i) => i,
		};
		let mut method = try!(method_call(NOTIFY_SHOW_METHOD));
		let actions: &[&str] = &[];
		debug!("DbusNotify showing notification");
		method.append_items(&[
			// STRING app_name;
			MessageItem::Str("iysr".into()),

			// UINT32 replaces_id;
			MessageItem::UInt32(id),

			// STRING app_icon;
			MessageItem::Str("".into()),

			// STRING summary;
			MessageItem::Str(self.title.clone()),

			// STRING body;
			MessageItem::Str(self.body.clone()),

			// ARRAY actions;
			// MessageItem::Array(vec!(), MessageItem::Str("".into()).type_sig()),
			actions.into(),

			// DICT hints;
			MessageItem::Array(vec!(), MessageItem::DictEntry(
					Box::new(MessageItem::Str("".into())),
					Box::new(MessageItem::Variant(Box::new(MessageItem::Bool(true)))) // actual variant type doesn't matter, but we need something...
				).type_sig()),

			// INT32 expire_timeout;
			MessageItem::Int32(0i32)
		]);

		let response = try!(call_method(self.conn, method));
		match response.get_items().iter().next() {
			Some(&MessageItem::UInt32(id)) => {
				debug!("Created notification {}", id);
				self.id = Some(id);
			},
			unexpected => {
				warn!("Received unexpected notification reply: {:?}", unexpected);
			},
		}
		Ok(())
	}

	fn hide(&mut self) -> Result<(), InternalError> {
		match self.id {
			None => {
				debug!("hide(): no active notification");
				Ok(())
			},
			Some(id) => {
				let method = try!(method_call(NOTIFY_HIDE_METHOD))
					.append(MessageItem::UInt32(id));
				try!(call_method(self.conn, method));
				self.id = None;
				Ok(())
			},
		}
	}
}

impl<'a> Drop for DbusNotify<'a> {
	fn drop(&mut self) {
		if self.timeout == 0 {
			ignore_error!(self.hide(), "closing notification");
		}
	}
}

fn notification_contents(_state: &StateSnapshot) -> Option<String> {
	Some("TODO: summarize state".into())
}

fn run<'a>(thread: WorkerSelf<InternalError>, monitor: Arc<Mutex<SystemMonitor>>) -> Result<(), InternalError> {
	info!("Starting DBus notification service ...");
	let mut monitor = try!(monitor.lock());
	let receiver = try!(monitor.subscribe());
	let conn = try!(Connection::get_private(BusType::Session));
	let mut persistent_notification = DbusNotify::empty(&conn, None);
	persistent_notification.set_title("iysr state".into());
	let snapshot = StateSnapshot::new();
	loop {
		let data = try!(receiver.recv());
		debug!("dbus_notify saw data...");
		if snapshot.update(&data) {
			match notification_contents(&snapshot) {
				None => {
					try!(persistent_notification.hide());
				},
				Some(msg) => {
					persistent_notification.set_body(msg);
					try!(persistent_notification.show());
				},
			}
		} else {
			// just an event - create a new one
			let mut notification = DbusNotify::new(
				&conn,
				"iysr event".into(),
				"TODO".into(),
				Some(time::Duration::from_secs(10)));
			try!(notification.show());
		}
		try!(thread.tick());
	}
}

pub fn main(monitor: Arc<Mutex<SystemMonitor>>, parent: &WorkerSelf<InternalError>) -> Result<Worker<InternalError>,io::Error> {
	parent.spawn("dbus notify".into(), move |t| run(t, monitor))
}

pub fn test() -> Result<(),InternalError> {
	let conn = try!(Connection::get_private(BusType::Session));
	thread::sleep_ms(3000);
	let mut n = DbusNotify::new(&conn, "hello there".into(), "It's me!".into(), None);
	println!("showing notification");
	try!(n.show());
	thread::sleep_ms(3000);
	println!("updating notification");
	n.set_body("It _was_ me...".into());
	try!(n.show());
	thread::sleep_ms(3000);
	println!("hiding notification");
	try!(n.hide());
	Ok(())
}
