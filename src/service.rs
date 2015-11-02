extern crate hyper;
extern crate worker;

use std::io;
use std::fmt;
use std::thread;
use std::ops::Deref;
use std::io::{Write};
use std::collections::BTreeMap;
use std::sync::{Arc,Mutex};
use std::sync::mpsc::{sync_channel};

use super::errors::InternalError;
use super::monitor::{Update};
use system_monitor::{SystemMonitor,Receiver};
use hyper::server::{Request,Response,Handler};
use hyper::net::{Fresh,Streaming};
use hyper::header;
use schedule_recv;
use rustc_serialize::json;
use rustc_serialize::json::Json;
use rustc_serialize::{Encodable, Encoder};

struct Server {
	monitor: Mutex<Box<SystemMonitor>>,
}

fn write_sse_end(dest: &mut io::Write) -> io::Result<()> {
	try!(write!(dest, "\n"));
	try!(dest.flush());
	Ok(())
}

fn _write_sse(dest: &mut io::Write, prefix: &str, data: &str, end: bool) -> io::Result<()> {
	for line in data.lines() {
		trace!("writing line: {}", line);
		try!(write!(dest, "{}: {}\n", prefix, line));
	}
	if end {
		try!(write_sse_end(dest));
	}
	Ok(())
}

// Awkward wrapper around Response that is SSE-aware, mostly
// so we can use `Encodable` rather than ToJson
struct WriteSSE<'a, 'stream: 'a> {
	response: &'a mut Response<'stream, Streaming>,
	prefix: &'a str,
	buffer: String,
}

impl<'a, 'stream> WriteSSE<'a, 'stream> {
	fn end_msg(&mut self) -> io::Result<()> {
		try!(self.flush());
		write_sse_end(self.response)
	}

	fn keepalive(&mut self) -> io::Result<()> {
		write_sse_keepalive(self.response)
	}

	fn emit_json<F>(&mut self, f: F) -> Result<(), json::EncoderError>
		where F: FnOnce(&mut json::Encoder) -> Result<(), json::EncoderError>
	{
		let mut encoder = json::Encoder::new_pretty(self);

		match encoder.set_indent(0) {
			Ok(()) => (),
			Err(()) => {
				// this is dumb, set_indent should return a compatible error
				return Err(json::EncoderError::BadHashmapKey);
				// return json::EncoderError::FmtError("couldn't set indent");
			},
		};

		f(&mut encoder)
	}

	fn flush(&mut self) -> io::Result<()> {
		if self.buffer.len() > 0 {
			try!(_write_sse(self.response, self.prefix, self.buffer.deref(), false));
			self.buffer.truncate(0);
		}
		Ok(())
	}
}

impl<'a, 'stream> fmt::Write for WriteSSE<'a, 'stream> {
	// XXX pretty lame buffer impl
	fn write_str(&mut self, mut s: &str) -> fmt::Result {
		let new_len = self.buffer.len() + s.len();
		if new_len > 80 {
			let mut parts = s.splitn(2, '\n');
			let first = parts.next();
			let second = parts.next();
			match (first, second) {
				(Some(line), Some(remainder)) => {
					s = remainder;
					self.buffer.push_str(line);
					try!(match self.flush() {
						Ok(()) => Ok(()),
						Err(_) => Err(fmt::Error),
					})
				},
				_ => (),
			};
		}
		self.buffer.push_str(s.replace("\n", "").deref());
		Ok(())
	}
}

fn write_sse(dest: &mut io::Write, prefix: &str, data: &str) -> io::Result<()> {
	_write_sse(dest, prefix, data, true)
}

fn write_sse_keepalive(dest: &mut io::Write) -> io::Result<()> {
	try!(write!(dest, ":\n"));
	dest.flush()
}

impl Server {
	// 'a: 'stream means "'a outlives 'stream" - i.e. the reference
	// lives less long than the data it references
	fn try_handle<'a, 'stream: 'a>(&self, response: &'a mut Response<'stream, Streaming>) -> Result<(), InternalError> {
		let receiver = {
			try!(self.monitor.lock().unwrap().subscribe())
		};

		let mut writer = WriteSSE {
			response: response,
			prefix: "data",
			buffer: String::with_capacity(100),
		};
		// let mut encoder = json::Encoder::new(&mut writer);

		// XXX it'd be convenient to use select! here, but that's unavailable in stable

		let (emit, combined_data) = sync_channel(0);

		let timeout_emit = emit.clone();
		let _t = try!(worker::spawn_anon(move |t| -> Result<(),InternalError> {
			// let keepalive_writer = writer.clone();
			let _keepalive_thread = try!(t.spawn_anon(move |t| -> Result<(),InternalError> {
				loop {
					thread::sleep_ms(10000);
					try!(t.tick());
					try!(timeout_emit.send(None));
				}
			}));

			loop {
				let data = try!(receiver.recv());
				try!(t.tick());
				try!(emit.send(Some(data)));
			}
		}));


		let mut last_state = None;
		loop {
			match try!(combined_data.recv()) {
				None => try!(writer.keepalive()),
				Some(data) => {
					let old_state = last_state;
					last_state = Some(data.clone());
					try!(writer.emit_json(|s| {
						s.emit_struct("data", 3, {|s| {
							fn emit_pair<S:Encoder,V:Encodable>(s: &mut S, k: &'static str, v: &V) -> Result<(),S::Error> {
								try!(s.emit_struct_field("key", 0, encode_sub!("TODO_UNIQUE_KEY")));
								try!(s.emit_struct_field("overlay", 1, encode_sub!(k)));
								try!(s.emit_struct_field("data", 2, encode_sub!(v)));
								Ok(())
							}

							//let next_state = Some(json.clone());
							match old_state {
								None => {
									try!(emit_pair(s, "replace", &*data));
								},
								Some(ref _old_data) => {
									/* TODO: diffing! */
									try!(emit_pair(s, "diff", &*data));
								}
							};
							Ok(())
						}})
					}));
					try!(writer.end_msg());
				}
			}
		}
	}

	fn try_report_exception(&self, e: &InternalError, response: &mut Response<Streaming>) -> Result<(), io::Error> {
		let desc = format!("{}", e);
		try!(write_sse(response, "error", &desc));
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
					SubLevel::Ext(String::from("event-stream")),
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
	match hyper::Server::http("127.0.0.1:3000").and_then(|s| s.handle(server)) {
		Ok(_) => {
			errln!("Server listening on port 3000");
			Ok(())
		},
		Err(e) => Err(InternalError::new(format!("Server failed to start: {}", e))),
	}
}
