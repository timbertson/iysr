use std::collections::{HashMap, HashSet};
use std::process::{Command,Stdio,Child};
use std::thread;
use std::char;
use std::io;
use std::convert;
use std::sync::mpsc;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc,Mutex};
use std::io::{BufRead, BufReader};
use rustc_serialize::json;
use rustc_serialize::json::{Json};
use chrono::{DateTime,Local};
use monitor::*;
use std::thread::JoinHandle;
use util::read_all;
use systemd::RuntimeError;

type SharedRef<T> = Arc<Mutex<T>>;

pub struct Journal {
	subscribers: SharedRef<Vec<SyncSender<Arc<Update>>>>,
	thread: Option<JoinHandle<Result<(), InternalError>>>,
}

impl Journal {
	pub fn new() -> Result<Journal, InternalError> {
		let subscribers = Arc::new(Mutex::new(Vec::new()));
		let subscribers2 = subscribers.clone();
		let thread = try!(thread::Builder::new().spawn(move ||
			Self::run_thread(subscribers)
		));
		Ok(Journal {
			subscribers: subscribers2,
			thread: Some(thread),
		})
	}

	fn spawn() -> Result<Child, InternalError> {
		let child = Command::new("journalctl")
				.arg("-f")
				.arg("--output=json")
				.arg("--lines=1")
				// TODO: --cursor=<c>
				.stdout(Stdio::piped())
				.stderr(Stdio::piped())
				.spawn();

		match child {
			Ok(child) => Ok(child),
			Err(err) => Err(InternalError::new(format!("Unable to follow journal logs: {}", err)))
		}
	}

	fn send_update(
		subscribers: &mut SharedRef<Vec<SyncSender<Arc<Update>>>>,
		update: Arc<Update>)
	{
		for sub in subscribers.lock().unwrap().iter() {
			// TODO: don't lock this on every event; collect a
			// number of events
			ignore_error!(sub.try_send(update.clone()), "sending event");
		}
	}

	fn run_thread(mut subscribers: SharedRef<Vec<SyncSender<Arc<Update>>>>) -> Result<(), InternalError> {
		loop {
			match Self::follow_journal(&mut subscribers) {
				Ok(()) => (),
				Err(e) => {
					Self::send_update(&mut subscribers, Arc::new(Update {
						data: Data::Error(Failure {
							id: Some("follow".to_string()),
							error: format!("failed to follow journal logs: {}", e),
						}),
						source: "TODO".to_string(),
						typ: "journal".to_string(),
						time: Time::now(),
					}));
					// XXX make this configurable
					thread::sleep_ms(10000);
				}
			}
		}
	}

	fn follow_journal(subscribers: &mut SharedRef<Vec<SyncSender<Arc<Update>>>>) -> Result<(), InternalError> {
		let mut child = try!(Self::spawn());
		let stdout = BufReader::new(try!(child.stdout.take().ok_or(RuntimeError::ChildOutputStreamMissing)));
		let mut stderr = try!(child.stderr.take().ok_or(RuntimeError::ChildOutputStreamMissing));

		let ok_t = try!(thread::Builder::new().scoped(move|| -> Result<(), InternalError> {
			for line_r in stdout.lines() {
				let line = try!(line_r);
				debug!("got journal line: {}", line);
				let event = match Json::from_str(line.as_str()) {
					Ok(Json::Object(mut attrs)) => {
						let message = match attrs.remove("MESSAGE") {
							Some(Json::String(m)) => Some(m),
							_ => None,
						};

						
						// TODO: filter attribs via config, as most of these are totally unnecessary
						let mut _attrs = HashMap::new();
						for (k,v) in attrs {
							if k.starts_with('_') { continue; }
							_attrs.insert(k,v);
						}

						Event {
							id: None,
							severity: Some(Severity::Warning),
							message: message,
							attrs: Arc::new(_attrs),
						}
					},
					_ => {
						Event {
							id: None,
							severity: Some(Severity::Warning),
							message: Some(format!("Unparseable journal line: {}", line)),
							attrs: Arc::new(HashMap::new()),
						}
					},
				};
				{
					let update = Arc::new(Update {
						data: Data::Event(event),
						source: "TODO".to_string(),
						typ: "journal".to_string(),
						time: Time::now(),
					});
					Self::send_update(subscribers, update);
				}
			}
			Err(InternalError::new("journalctl ended".to_string()))
		}));

		let err_t = try!(thread::Builder::new().scoped(move|| {
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

impl Drop for Journal {
	fn drop(&mut self) {
		match self.thread.take() {
			Some(t) => match t.join() {
				Ok(Ok(())) => (),
				Err(e) => log_error!(e, "joining thread"),
				Ok(Err(e)) => log_error!(e, "joining thread"),
			},
			None => (),
		}
		//let _ = self.thread.join();
	}
}


impl PushDataSource for Journal {
	fn subscribe(&mut self, subscriber: SyncSender<Arc<Update>>) -> Result<(), InternalError> {
		let mut subscribers = self.subscribers.lock().unwrap();
		subscribers.push(subscriber);
		Ok(())
	}
}
