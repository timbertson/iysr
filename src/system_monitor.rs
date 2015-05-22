extern crate chrono;

use chrono::{DateTime,UTC};
use std::collections::HashMap;
use std::collections::hash_map::{Entry};
use std::sync::mpsc;
use std::sync::{Arc,Mutex};
use std::thread;
use std::ops::Deref;
use monitor::*;



#[derive(Debug)]
pub struct SystemState {
	pub time: DateTime<UTC>,
	pub sources: HashMap<String, Result<HashMap<String, PollResult>, InternalError>>,
}
//impl Clone for SystemState {
//	fn clone(&self) -> Self {
//		SystemState {
//			time: self.time,
//			sources: self.sources.clone(),
//		}
//	}
//}
//
type Listeners = HashMap<u32, mpsc::SyncSender<Arc<SystemState>>>;

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
			let time = UTC::now();
			let mut sources = HashMap::new();
			let monitors = {
				let shared = shared.lock().unwrap();
				shared.monitors.clone()
			};
			for (name, monitor) in monitors.iter() {
				// XXX can we not clone this?
				sources.insert(name.clone(), monitor.scan());
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
