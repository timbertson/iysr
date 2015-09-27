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
use super::errors::*;
use std::thread::JoinHandle;
use util::read_all;
use systemd_common::RuntimeError;
use config::{JournalConfig};
use filter::{filter,get_severity};

type SharedRef<T> = Arc<Mutex<T>>;

pub struct Journal {
	config: JournalConfig,
}

fn as_string(j: &json::Json) -> Option<String> {
	match *j {
		Json::String(ref s) => Some(s.clone()),
		_ => None,
	}
}

fn as_int(j: &json::Json) -> Option<i64> {
	match *j {
		Json::I64(s) => Some(s),
		Json::U64(s) => Some(s as i64),
		_ => None,
	}
}

impl Journal {
	pub fn new(config: JournalConfig) -> Result<Journal, InternalError> {
		// TODO: use backlog
		Ok(Journal { config: config })
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
		subscriber: &SyncSender<Arc<Update>>,
		update: Arc<Update>)
	{
		ignore_error!(subscriber.try_send(update), "sending event");
	}

	fn run_thread(
		config: JournalConfig,
		subscriber: SyncSender<Arc<Update>>
	) -> Result<(), InternalError>
	{
		let ref id = config.common.id;
		let subscriber = Arc::new(Mutex::new(subscriber));
		loop {
			match Self::follow_journal(&config, &subscriber) {
				Ok(()) => (),
				Err(e) => {
					let subscriber = subscriber.lock().unwrap();
					Self::send_update(&subscriber, Arc::new(Update {
						data: Data::Error(Failure {
							id: Some("follow".to_string()),
							error: format!("failed to follow journal logs: {}", e),
						}),
						source: id.clone(),
						typ: "journal".to_string(),
						time: Time::now(),
					}));
					// XXX make this configurable
					thread::sleep_ms(10000);
				}
			}
		}
	}

	fn follow_journal(
		config: &JournalConfig,
		subscriber: &Arc<Mutex<SyncSender<Arc<Update>>>>
	) -> Result<(), InternalError>
	{
		let ref id = config.common.id;
		let mut child = try!(Self::spawn());
		let stdout = BufReader::new(try!(child.stdout.take().ok_or(RuntimeError::ChildOutputStreamMissing)));
		let mut stderr = try!(child.stderr.take().ok_or(RuntimeError::ChildOutputStreamMissing));
		let source_keys = vec!("_SYSTEMD_UNIT".to_string(), "SYSLOG_IDENTIFIER".to_string());

		let ok_t = try!(thread::Builder::new().scoped(move|| -> Result<(), InternalError> {
			let subscriber = subscriber.lock().unwrap();
			for line_r in stdout.lines() {
				let line = try!(line_r);
				trace!("got journal line: {}", line);
				let event = match Json::from_str(line.as_str()) {
					Ok(Json::Object(mut attrs)) => {
						let mut source = None;
						for key in source_keys.iter() {
							match attrs.get(key) {
								Some(&Json::String(ref s)) => {
									source = Some(s.clone());
									break;
								},
								_ => (),
							}
						}

						let source = match source {
							None => "UNKNOWN".to_string(),
							Some(s) => {
								attrs.insert("SOURCE".to_string(), Json::String(s.clone()));
								s
							}
						};

						filter(&source, &config.common.filters, attrs).map(|mut attrs| {
							let message = match attrs.remove("MESSAGE") {
								Some(Json::String(m)) => Some(m),
								_ => None,
							};

							let severity = get_severity(&attrs);
							
							// JSON objects are BTreeMap, but we need a HashMap
							let mut _attrs = HashMap::new();
							_attrs.extend(attrs);

							Event {
								id: None,
								severity: severity,
								message: message,
								attrs: Arc::new(_attrs),
							}
						})
					},
					_ => {
						Some(Event {
							id: Some("internal".to_string()),
							severity: Some(Severity::Info),
							message: Some(format!("Unparseable journal line: {}", line)),
							attrs: Arc::new(HashMap::new()),
						})
					},
				};
				match event {
					Some(event) => {
						let update = Arc::new(Update {
							data: Data::Event(event),
							source: id.clone(),
							typ: "journal".to_string(),
							time: Time::now(),
						});
						Self::send_update(&subscriber, update);
					},
					None => {
						trace!("item filtered");
					}
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


pub struct JournalSubscription {
	thread: Option<JoinHandle<Result<(), InternalError>>>,
}
impl Drop for JournalSubscription {
	fn drop(&mut self) {
		// XXX kill child process
		match self.thread.take() {
			None => (),
			Some(thread) => {
				match thread.join() {
					Ok(Ok(())) => (),
					Err(e) => log_error!(e, "joining thread"),
					Ok(Err(e)) => log_error!(e, "joining thread"),
				}
			}
		}
	}
}

impl PushSubscription for JournalSubscription {
}

impl PushDataSource for Journal {
	fn subscribe(&self, subscriber: SyncSender<Arc<Update>>) -> Result<Box<PushSubscription>, InternalError> {
		let config = self.config.clone();
		let thread = try!(thread::Builder::new().spawn(move ||
			Self::run_thread(config, subscriber)
		));
		Ok(Box::new(JournalSubscription { thread: Some(thread) }))
	}
}
