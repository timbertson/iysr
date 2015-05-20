extern crate hyper;

use std::io;
use std::fmt;
use std::thread;
use std::io::{Write};

use monitor::InternalError;
use hyper::server::{Request,Response,Handler};
use hyper::net::{Fresh,Streaming};

struct Server;

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
	_write_sse(dest, "", "", false)
}

impl Server {
	fn try_handle(&self, response: &mut Response<Streaming>) -> Result<(), InternalError> {
		let some_data = "foo!\nbar!";
		loop {
			try!(write_sse(response, "data", &some_data));
			debug!("sleeping ...");
			thread::sleep_ms(1000);
			try!(write_sse_keepalive(response));
		}
	}

	fn try_report_exception(&self, e: &InternalError, response: &mut Response<Streaming>) -> Result<(), io::Error> {
		let desc = format!("{}", e);
		try!(write_sse(response, "error", &desc.as_str()));
		Ok(())
	}
}

impl Handler for Server {
	fn handle<'a, 'k>(&'a self, _: Request<'a, 'k>, response: Response<'a, Fresh>) {
		match response.start() {
			Err(e) => debug!("Unable to start response: {}", e),
			Ok(mut response) => {
				match self.try_handle(&mut response) {
					Ok(()) => (),
					Err(e) => {
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

fn stream_events(_: Request, res: Response<Fresh>) {
	let mut res = res.start().unwrap();
	res.write_all(b"Hello World!").unwrap();
	res.end().unwrap();
}

pub fn main() {
	let server = Server;
	hyper::Server::http(server).listen("127.0.0.1:3000").unwrap();
}
