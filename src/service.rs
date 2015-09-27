extern crate hyper;

use std::io;
use std::io::{Write};
use std::collections::BTreeMap;
use std::sync::{Arc,Mutex};

use super::errors::InternalError;
use super::monitor::{Update};
use system_monitor::{SystemMonitor,Receiver};
use hyper::server::{Request,Response,Handler};
use hyper::net::{Fresh,Streaming};
use hyper::header;
use schedule_recv;
use rustc_serialize::json;
use rustc_serialize::json::{Json,ToJson};
use rustc_serialize::{Encodable};

struct Server {
	monitor: Mutex<Box<SystemMonitor>>,
}

fn _write_sse(dest: &mut io::Write, prefix: &str, data: &str, end: bool) -> io::Result<()> {
	for line in data.lines() {
		trace!("writing line: {}", line);
		try!(write!(dest, "{}: {}\n", prefix, line));
	}
	if end {
		try!(write!(dest, "\n"));
		try!(dest.flush());
	}
	Ok(())
}

fn write_sse(dest: &mut io::Write, prefix: &str, data: &str) -> io::Result<()> {
	_write_sse(dest, prefix, data, true)
}
//fn write_sse_part(dest: &mut io::Write, prefix: &str, data: &str) -> io::Result<()> {
//	_write_sse(dest, prefix, data, false)
//}
fn write_sse_keepalive(dest: &mut io::Write) -> io::Result<()> {
	try!(write!(dest, ":\n"));
	dest.flush()
}

impl Server {
	fn try_handle(&self, response: &mut Response<Streaming>) -> Result<(), InternalError> {
		let receiver = {
			try!(self.monitor.lock().unwrap().subscribe())
		};

		let mut last_state = None;
		loop {
			let timer = schedule_recv::oneshot_ms(10000);
			select!(
				data = receiver.recv() => {
					let data = try!(data);
					let json = data.to_json();
					let mut attrs = BTreeMap::new();
					attrs.insert(String::from_str("key"), "TODO_UNIQUE_KEY".to_json());
					let overlay = String::from_str("overlay");
					//let next_state = Some(json.clone());
					let (next_state, update) = match last_state {
						None => {
							attrs.insert(overlay, "replace".to_json());
							(json.clone(), json)
						},
						Some(_old_data) => {
							/* TODO: diffing! */
							attrs.insert(overlay, "diff".to_json());
							(json.clone(), json)
						}
					};
					last_state = Some(next_state);
					attrs.insert(String::from_str("data"), update);

					let data = Json::Object(attrs);
					// TODO: lazy (asJson) encoding?
					let data = try!(json::encode(&data));
					//try!(write_sse_part(response, "event", &"state"));
					try!(write_sse(response, "data", &data));
				},

				_ = timer.recv() => {
					try!(write_sse_keepalive(response));
				}
			);
		}
	}

	fn try_report_exception(&self, e: &InternalError, response: &mut Response<Streaming>) -> Result<(), io::Error> {
		let desc = format!("{}", e);
		try!(write_sse(response, "error", &desc.as_str()));
		Ok(())
	}
}

impl Handler for Server {
	fn handle<'a, 'k>(&'a self, _: Request<'a, 'k>, mut response: Response<'a, Fresh>) {
		{
			use hyper::header::*;
			use hyper::mime::*;
			let headers = response.headers_mut();
			headers.set(AccessControlAllowOrigin::Any);
			headers.set(ContentType(
				Mime(
					TopLevel::Text,
					SubLevel::Ext(String::from_str("event-stream")),
					Vec::new()
				)
			));
		}
		match response.start() {
			Err(e) => debug!("Unable to start response: {}", e),
			Ok(mut response) => {
				match self.try_handle(&mut response) {
					Ok(()) => (),
					Err(e) => {
						// TODO: don't bother trying to report this exception
						// if it's a pipe error
						match self.try_report_exception(&e, &mut response) {
							Ok(_) => (),
							Err(_) => debug!("Unable to report exception: {}", e),
						}
					},
				}
				match response.end() {
					Ok(()) => (),
					Err(e) => debug!("Unable to close stream: {}",e),
				}
			}
		}
	}
}

pub fn main(monitor: SystemMonitor) -> Result<(),InternalError> {
	let server = Server { monitor: Mutex::new(Box::new(monitor)), };
	match hyper::Server::http(server).listen("127.0.0.1:3000") {
		Ok(_) => {
			errln!("Server listening on port 3000");
			Ok(())
		},
		Err(e) => Err(InternalError::new(format!("Server failed to start: {}", e))),
	}
}
