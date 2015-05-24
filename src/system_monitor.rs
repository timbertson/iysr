extern crate chrono;

use chrono::{DateTime,UTC};
use std::collections::HashMap;
use std::collections::hash_map::{Entry};
use std::sync::mpsc;
use std::sync::{Arc,Mutex};
use std::thread;
use std::fmt;
use std::ops::Deref;
use monitor::*;
use rustc_serialize::{Encoder,Encodable};
use rustc_serialize::json;
use rustc_serialize::json::Json;
use chrono::Timelike;


pub struct Time(DateTime<UTC>);
impl Time {
	fn timestamp(&self) -> i64 {
		let Time(t) = *self;
		t.timestamp()
	}
	fn time(&self) -> chrono::NaiveTime {
		let Time(t) = *self;
		t.time()
	}
}
impl fmt::Debug for Time {
	fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
		let Time(t) = *self;
		t.fmt(f)
	}
}

impl Encodable for Time {
	fn encode<S:Encoder>(&self, encoder: &mut S) -> Result<(), S::Error> {
		encoder.emit_struct("Time", 2, |encoder| {
			try!(encoder.emit_struct_field("sec", 0usize, |e| self.timestamp().encode(e)));
			try!(encoder.emit_struct_field("ms", 1usize, |e| (self.time().nanosecond() / 1000000).encode(e)));
			Ok(())
		})
	}
}

#[derive(Debug)]
pub struct SourceStatus {
	typ: String,
	status: Result<HashMap<String, Status>, InternalError>,
}

#[derive(Debug)]
pub struct SystemState {
	pub time: Time,
	pub sources: HashMap<String, SourceStatus>,
}
type Listeners = HashMap<u32, mpsc::SyncSender<Arc<SystemState>>>;

impl Encodable for SystemState {
	fn encode<S:Encoder>(&self, encoder: &mut S) -> Result<(), S::Error> {
		encoder.emit_struct("SystemState", 2, |encoder| {
			try!(encoder.emit_struct_field("time", 0usize, |e| self.time.encode(e)));
			try!(encoder.emit_struct_field("type", 1usize, |e| "system".encode(e)));
			try!(encoder.emit_struct_field("sources", 2usize, |encoder| {
				try!(encoder.emit_map(self.sources.len(), |encoder| {
					use monitor::Monitor;
					let mut idx = 0usize;
					for (key, source) in self.sources.iter() {
						try!(encoder.emit_map_elt_key(idx, |e| key.encode(e)));
						try!(encoder.emit_map_elt_val(idx, |encoder| {
							try!(encoder.emit_struct("Result", 2usize, |encoder| {
								match source.status {
									Ok(ref v) => {
										try!(encoder.emit_struct_field("ok", 0usize, |e| v.encode(e)));
									},
									Err(ref v) => {
										try!(encoder.emit_struct_field("err", 0usize, |e| v.encode(e)));
									},
								};
								try!(encoder.emit_struct_field("type", 1usize, |e| source.typ.encode(e)));
								Ok(())
							}));
							Ok(())
						}));
						idx += 1;
					}
					Ok(())
				}));
				Ok(())
			}));
			Ok(())
		})
	}
}


struct SharedState {
	monitors: HashMap<String, Arc<Box<Monitor>>>,
	last_state: Option<Arc<SystemState>>,
}

pub struct Receiver<T> {
	inner: mpsc::Receiver<T>,
	collection: Arc<Mutex<Listeners>>,
	id: u32,
}

impl<T> Receiver<T> {
	pub fn recv(&self) -> Result<T,mpsc::RecvError> {
		self.inner.recv()
	}
}

impl<T> Deref for Receiver<T> {
	type Target = mpsc::Receiver<T>;
	fn deref<'a>(&'a self) -> &'a Self::Target {
		&self.inner
	}
}

impl<T> Drop for Receiver<T> {
	fn drop(&mut self) {
		debug!("dropping listener {}", self.id);
		match self.collection.lock() {
			Ok(mut collection) => {
				use std::ops::DerefMut;
				// XXX why does this need to be explicit?
				let collection = collection.deref_mut();
				match collection.remove(&self.id) {
					Some(_) => (),
					None => warn!("listener not found in collection"),
				}
			},
			Err(e) => {
				warn!("Can't remove subscriber from collection: {}",e);
			}
		}
	}
}

pub struct SystemMonitor {
	poll_time_ms: u32,
	thread: Option<thread::JoinHandle<()>>,
	shared: Arc<Mutex<SharedState>>,
	listeners: Arc<Mutex<Listeners>>,
	subscriber_id: u32,
}

impl Drop for SystemMonitor {
	fn drop(&mut self) {
		match self.thread.take() {
			None => (),
			Some(t) => {
				match t.join() {
					Ok(_) => (),
					Err(e) => debug!("failed to join SystemMonitor thread: {:?}",e),
				}
			}
		}
	}
}

impl SystemMonitor {
	pub fn new(poll_time:u32) -> SystemMonitor {
		let shared_state = SharedState {
			monitors: HashMap::new(),
			last_state: None,
		};
		SystemMonitor {
			poll_time_ms: poll_time,
			// XXX can we remove these ARCs?
			shared: Arc::new(Mutex::new(shared_state)),
			listeners: Arc::new(Mutex::new(HashMap::new())),
			thread: None,
			subscriber_id: 0,
		}
	}

	pub fn add(&mut self, key: String, monitor: Box<Monitor>) -> Result<(), InternalError> {
		let mut shared = self.shared.lock().unwrap();
		// XXX shouldn't need clone?
		match shared.monitors.entry(key.clone()) {
			Entry::Vacant(entry) => {
				entry.insert(Arc::new(monitor));
				Ok(())
			},
			Entry::Occupied(_) => {
				Err(InternalError::new(format!("Multiple monitors with key {}", key)))
			},
		}
	}

	fn run_loop(sleep_ms: u32, shared: Arc<Mutex<SharedState>>, listeners: Arc<Mutex<Listeners>>) {
		// XXX stop loop when last listener deregisters
		loop {
			let time = Time(UTC::now());
			let mut sources = HashMap::new();
			let monitors = {
				let shared = shared.lock().unwrap();
				shared.monitors.clone()
			};
			for (name, monitor) in monitors.iter() {
				// XXX can we not clone this?
				sources.insert(name.clone(), SourceStatus {
					typ: monitor.typ(),
					status: monitor.scan()
				});
			}

			let state = Arc::new(SystemState {
				time: time,
				sources: sources,
			});
			{
				let listeners = listeners.lock().unwrap();
				for listener in listeners.values() {
					match listener.try_send(state.clone()) {
						Ok(()) => (),
						Err(e) => {
							debug!("SystemState try_send error (ignoring): {}",e)
						},
					}
				}
			}
			{
				let mut shared = shared.lock().unwrap();
				shared.last_state = Some(state);
			};
			thread::sleep_ms(sleep_ms);
		}
	}

	pub fn subscribe(&mut self) -> Result<Receiver<Arc<SystemState>>, InternalError> {
		let (sender, receiver) = mpsc::sync_channel(1);
		{
			let shared = self.shared.lock().unwrap();
			match shared.last_state {
				None => (),
				Some(ref state) => {
					let _:Result<(), mpsc::TrySendError<_>> = sender.try_send(state.clone());
				}
			}
		}
		let mut id;
		{
			let mut listeners = self.listeners.lock().unwrap();
			loop {
				id = self.subscriber_id;
				self.subscriber_id += 1;
				let num_listeners = listeners.len() as u32;
				if num_listeners > ::std::u32::MAX / 2 {
					return Err(InternalError::new(String::from_str("Too many listeners")))
				}
				match listeners.entry(id) {
					Entry::Vacant(entry) => {
						entry.insert(sender);
						break;
					},
					Entry::Occupied(_) => { /* continue */ }
				}
			}
			debug!(
				"inserted listener[{}], there are now {} listeners",
				id, listeners.len());
		}
		let rv = Receiver {
			inner: receiver,
			collection: self.listeners.clone(),
			id: id,
		};
		match self.thread {
			Some(_) => (),
			None => {
				debug!("Starting system monitor thread");
				let shared : Arc<Mutex<SharedState>> = self.shared.clone();;
				let listeners : Arc<Mutex<Listeners>> = self.listeners.clone();;
				let sleep_ms = self.poll_time_ms;
				let thread = try!(thread::Builder::new().spawn(move ||
					Self::run_loop(sleep_ms, shared, listeners)
				));
				self.thread = Some(thread);
			}
		}
		Ok(rv)
	}
}
