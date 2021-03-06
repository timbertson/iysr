use chrono::{DateTime,UTC};
use std::collections::{HashMap,BTreeMap};
use std::collections::hash_map::{Entry};
use std::sync::mpsc;
use std::sync::{Arc,Mutex};
use std::thread;
use std::fmt;
use std::mem;
use std::error::Error;
use std::ops::Deref;
use monitor::*;
use errors::*;
use rustc_serialize::{Encoder,Encodable};

#[derive(Debug)]
pub struct SourceStatus {
	typ: String,
	status: Result<HashMap<String, Status>, InternalError>,
}

impl Encodable for SourceStatus {
	fn encode<S: Encoder>(&self, s: &mut S) -> Result<(), S::Error> {
		s.emit_struct("status", 2, {|s| {
			try!(s.emit_struct_field("type", 0, encode_sub!(self.typ)));
			try!(match self.status {
				Ok(ref v) => s.emit_struct_field("ok", 1, encode_sub!(v)),
				Err(ref v) => s.emit_struct_field("err", 1, encode_sub!(v)),
			});
			Ok(())
		}})
	}
}

type Listeners = HashMap<u32, mpsc::SyncSender<Arc<Update>>>;

type SharedRef<T> = Arc<Mutex<T>>;

pub struct Receiver<T> {
	inner: mpsc::Receiver<T>,
	collection: SharedRef<Listeners>,
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

enum ThreadState {
	NotRunning(mpsc::Receiver<Arc<Update>>, Vec<Box<PullDataSource>>),
	Running(thread::JoinHandle<Result<(),InternalError>>, thread::JoinHandle<()>, Vec<Box<PushSubscription>>),
	Ended,
}

impl ThreadState {
	fn _take(&mut self) -> ThreadState {
		// atomically swap out this value for `ended` (temporarily, used by `map`)
		mem::replace(self, ThreadState::Ended)
	}

	fn bind<F>(&mut self, f: F) -> ()
		where F: FnOnce(ThreadState) -> ThreadState
	{
		*self = f(self._take());
	}

	// A bit awkward: try_bind function must return threadstate even for an error,
	// because the borrow checker won't let us keep it in the case of an error!
	fn try_bind<F, E:Sized>(&mut self, f: F) -> Result<(), E>
		where F: FnOnce(ThreadState) -> Result<ThreadState,(ThreadState, E)>
	{
		let current = self._take();
		match f(current) {
			Ok(new) => { *self = new; Ok(()) },
			Err((new, e)) => { warn!("Error in try_bind"); *self = new; Err(e) },
		}
	}
}

pub struct StateSnapshot {
	state: SharedRef<HashMap<String, Arc<Update>>>,
}

impl StateSnapshot {
	pub fn new() -> StateSnapshot {
		StateSnapshot { state: Arc::new(Mutex::new(HashMap::new())) }
	}

	pub fn update(&self, update: &Arc<Update>) -> bool {
		match update.scope {
			UpdateScope::Partial => false,
			UpdateScope::Snapshot => {
				let mut state = self.state.lock().unwrap();
				state.insert(update.source.id.clone(), update.clone());
				true
			},
		}
	}

	pub fn values(&self) -> Vec<Arc<Update>> {
		let state = self.state.lock().unwrap();
		state.values().map(|update| update.clone()).collect()
	}
}

impl Clone for StateSnapshot {
	fn clone(&self) -> StateSnapshot {
		StateSnapshot { state : self.state.clone() }
	}
}

pub struct SystemMonitor {
	poll_time_ms: u32,
	thread_state: ThreadState,
	event_writable: mpsc::SyncSender<Arc<Update>>,
	listeners: SharedRef<Listeners>,
	push_sources: Vec<Box<PushDataSource>>,
	last_state: StateSnapshot,
	subscriber_id: u32,
}

impl Drop for SystemMonitor {
	fn drop(&mut self) {
		self.thread_state.bind(|state| match state {
			ThreadState::Running(t1, t2, resources) => {
				debug!("Joining system monitor thread");
				drop(resources);
				match t1.join() {
					Ok(Ok(())) => (),
					Err(e) => log_error!(e, "joining thread"),
					Ok(Err(e)) => log_error!(e, "joining thread"),
				};
				ignore_error!(t2.join(), "joining thread");
				ThreadState::Ended
			},
			r@ThreadState::NotRunning(_,_) | r@ThreadState::Ended => r,
		});
	}
}

impl SystemMonitor {
	pub fn new(
		poll_time:u32,
		event_buffer:usize,
		pull_sources: Vec<Box<PullDataSource>>,
		push_sources: Vec<Box<PushDataSource>>,
	) -> Result<SystemMonitor, InternalError> {
		let (w,r) = mpsc::sync_channel(event_buffer);
		Ok(SystemMonitor {
			poll_time_ms: poll_time,
			// XXX can we remove these ARCs? They could at least be Boxes, I think
			listeners: Arc::new(Mutex::new(HashMap::new())),
			push_sources: push_sources,
			last_state: StateSnapshot::new(),
			event_writable: w,
			thread_state: ThreadState::NotRunning(r, pull_sources),
			subscriber_id: 0,
		})
	}

	fn poll_loop(
			sleep_ms: u32,
			pull_sources: Vec<Box<PullDataSource>>,
			last_state: StateSnapshot,
			event_writable: mpsc::SyncSender<Arc<Update>>)
	{
		// XXX stop loop when last listener deregisters
		loop {
			let mut state = Vec::with_capacity(pull_sources.len());
			for source in pull_sources.iter() {
				// XXX can we not clone this?
				let time = Time::now();
				let data = match source.poll() {
					Ok(data) => data,
					Err(e) => Data::Error(Failure {
						error: format!("{}", e),
						id: Some("poll".to_string()),
					}),
				};
				let data = Arc::new(Update {
					time: time,
					source: source.source(),
					data: data,
					scope: UpdateScope::Snapshot,
				});
				last_state.update(&data);
				ignore_error!(event_writable.try_send(data.clone()), "sending poll result");
				state.push(data);
			}
			thread::sleep_ms(sleep_ms);
		}
	}

	fn run_loop(
			event_readable: mpsc::Receiver<Arc<Update>>,
			last_state: StateSnapshot,
			listeners: SharedRef<Listeners>) -> Result<(), InternalError>
	{
		// XXX stop loop when last listener deregisters
		loop {
			let data : Arc<Update> = try!(event_readable.recv());
			{
				last_state.update(&data);
				let listeners = listeners.lock().unwrap();
				for listener in listeners.values() {
					ignore_error!(listener.try_send(data.clone()), "sending data to listener");
				}
			}
		}
	}

	pub fn subscribe(&mut self) -> Result<Receiver<Arc<Update>>, InternalError> {
		// XXX make this react to self.pull_sources.len()
		let (sender, receiver) = mpsc::sync_channel(10);
		{
			let initial_states = self.last_state.values();
			debug!("sending {} initial updates from last_state", initial_states.len());
			debug!("initial_states: {:?}", initial_states);
			for update in initial_states.iter() {
				ignore_error!(sender.try_send(update.clone()), "sending initial state");
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
					return Err(InternalError::new(String::from("Too many listeners")))
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

		// we need to make local references here, because
		// we can't use `self` in the closure below while
		// self.thread_state is mutably borrowed
		let last_state = &self.last_state;
		let push_sources = &mut self.push_sources;
		let listeners = &self.listeners;
		let event_writable = &self.event_writable;
		let sleep_ms = self.poll_time_ms;

		try!(self.thread_state.try_bind(|state| match state {
			r@ThreadState::Running(_,_,_) => Ok(r),
			s@ThreadState::Ended => Err(
				(s, InternalError::new("cannot subscribe monitor, it has already ended".to_string()))
			),
			ThreadState::NotRunning(event_readable, pull_sources) => {
				// OK, here we go. We need to spawn two threads, and _only_ if they both succeed do we then proceed
				// to send `event_readable` and `pull_sources` to their respective threads. We can't just let the
				// threads own these variables, as then we can't return the same NotRunning state in the case of failure.
				debug!("Starting system monitor thread");
				let mut subscriptions = Vec::new();
				for source in push_sources.iter_mut() {
					match source.subscribe(event_writable.clone()) {
						Ok(subscription) => {
							subscriptions.push(subscription);
						},
						Err(e) => return Err(
							(ThreadState::NotRunning(event_readable, pull_sources), InternalError::from(e))
						),
					}
				}

				// kick off the first thread
				let (t1_send, t1_recv) = mpsc::sync_channel(0);
				let poll_thread = match thread::Builder::new().spawn(move || {
					let (pull_sources, last_state, event_writable) = t1_recv.recv().unwrap();
					Self::poll_loop(sleep_ms, pull_sources, last_state, event_writable)
				}) {
					Err(e) => return Err((ThreadState::NotRunning(event_readable, pull_sources), InternalError::from(e))),
					Ok(poll_thread) => poll_thread
				};

				// now the second
				let (t2_send, t2_recv) = mpsc::sync_channel(0);
				let event_thread = match thread::Builder::new().spawn(move || {
					let (event_readable, last_state, listeners) = t2_recv.recv().unwrap();
					Self::run_loop(event_readable, last_state, listeners)
				}) {
					Err(e) => {
						// we spawned the first thread, but not the second!
						drop(t1_send);
						drop(t2_send);
						ignore_error!(poll_thread.join(), "joining thread");
						return Err((ThreadState::NotRunning(event_readable, pull_sources), InternalError::from(e)))
					},
					Ok(event_thread) => event_thread
				};
				
				// Both of the threads are now succesfully started and therefore waiting on our queue.
				// So `unwap()` is safe, as there's no way those threads could have died.
				t1_send.send((pull_sources, last_state.clone(), event_writable.clone())).unwrap();
				t2_send.send((event_readable, last_state.clone(), listeners.clone())).unwrap();
				Ok(ThreadState::Running(event_thread, poll_thread, subscriptions))
			}
		}));
		Ok(rv)
	}
}
