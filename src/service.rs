extern crate hyper;

use std::io;
use std::io::{Write};
use std::sync::{Mutex};

use monitor::InternalError;
use system_monitor::SystemMonitor;
use hyper::server::{Request,Response,Handler};
use hyper::net::{Fresh,Streaming};
use hyper::header;
use schedule_recv;
use rustc_serialize::json;

struct Server {
	monitor: Mutex<Box<SystemMonitor>>,
}

fn _write_sse(dest: &mut io::Write, prefix: &str, data: &str, end: bool) -> io::Result<()> {
	for line in data.lines() {
		debug!("writing line: {}", line);
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
fn write_sse_part(dest: &mut io::Write, prefix: &str, data: &str) -> io::Result<()> {
	_write_sse(dest, prefix, data, false)
}
fn write_sse_keepalive(dest: &mut io::Write) -> io::Result<()> {
	try!(write!(dest, ":\n"));
	dest.flush()
}

impl Server {
	fn try_handle(&self, response: &mut Response<Streaming>) -> Result<(), InternalError> {
		let receiver = {
			try!(self.monitor.lock().unwrap().subscribe())
		};

		loop {
			let timer = schedule_recv::oneshot_ms(1500);
			select!(
				data = receiver.recv() => {
					// TODO: lazy (asJson) encoding?
					let data = try!(data);
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

pub fn main(monitor: SystemMonitor) {
	let server = Server { monitor: Mutex::new(Box::new(monitor)), };
	hyper::Server::http(server).listen("127.0.0.1:3000").unwrap();
}
