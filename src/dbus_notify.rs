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
use std::sync::{Arc};
use std::io::{BufRead, BufReader};
use std::fmt;
use rustc_serialize::json::{Json};
use chrono::{DateTime,Local};
use monitor::*;
use config::SystemdConfig;
use util::read_all;
use dbus::{Connection,BusType,Message,MessageItem,MessageType,Props,ConnectionItem,Path};
use super::dbus_common::*;
use super::errors::*;
use super::systemd_common::*;
extern crate dbus;

const NOTIFY_IFACE: &'static str = "org.freedesktop.Notifications";
const NOTIFY_PATH: &'static str = "/org/freedesktop/Notifications";
const NOTIFY_SHOW_METHOD: &'static str = "Notify";
const NOTIFY_HIDE_METHOD: &'static str = "CloseNotification";

struct DbusNotify {
	id: Option<u32>,
	title: String,
	body: String,
}

fn method_call(name: &str) -> Result<Message,InternalError> {
		Message::new_method_call(NOTIFY_IFACE, NOTIFY_PATH, NOTIFY_IFACE, name)
			.or(Err(InternalError::new(format!("Failed to create method call"))))
}


impl DbusNotify {
	fn new(title: String, body: String) -> DbusNotify {
		DbusNotify {
			id: None,
			title: title,
			body: body,
		}
	}

	fn set_title(&mut self, title: String) {
		self.title = title;
	}

	fn set_body(&mut self, body: String) {
		self.body = body;
	}

	fn show(&mut self, conn: &Connection) -> Result<(), InternalError> {
		let id = match self.id {
			None => 0,
			Some(i) => i,
		};
		let mut method = try!(method_call(NOTIFY_SHOW_METHOD));
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
			MessageItem::Array(vec!(), MessageItem::Str("".into()).type_sig()),

			// DICT hints;
			MessageItem::Array(vec!(), MessageItem::DictEntry(
					Box::new(MessageItem::Str("".into())),
					Box::new(MessageItem::Variant(Box::new(MessageItem::Bool(true)))) // actual variant type doesn't matter, but we need something...
				).type_sig()),

			// INT32 expire_timeout;
			MessageItem::Int32(0i32)
		]);

		let response = try!(call_method(conn, method));
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

	fn hide(&mut self, conn: &Connection) -> Result<(), InternalError> {
		match self.id {
			None => {
				debug!("hide(): no active notification");
				Ok(())
			},
			Some(id) => {
				let method = try!(method_call(NOTIFY_HIDE_METHOD))
					.append(MessageItem::UInt32(id));
				try!(call_method(conn, method));
				Ok(())
			},
		};
	}
}

pub fn test() -> Result<(),InternalError> {
	let conn = try!(Connection::get_private(BusType::Session));
	thread::sleep_ms(3000);
	let mut n = DbusNotify::new("hello there".into(), "It's me!".into());
	println!("showing notification");
	try!(n.show(&conn));
	thread::sleep_ms(3000);
	println!("updating notification");
	n.set_body("It _was_ me...".into());
	try!(n.show(&conn));
	thread::sleep_ms(3000);
	println!("hiding notification");
	try!(n.hide(&conn));
	Ok(())
}
