// XXX disable these when things get less prototypey
#![allow(unused_imports)]
#![allow(dead_code)]

use std::thread;
use std::fmt;
use std::convert;
use std::thread::{Thread,JoinHandle};
use std::error::Error;
use std::sync::mpsc;
use std::io;
use std::time::Duration;
use std::sync::{Arc,Mutex,PoisonError,MutexGuard};

extern crate env_logger;

#[macro_use]
extern crate log;

fn mutex_io_error() -> io::Error {
	io::Error::new(io::ErrorKind::Other, "failed to acquire mutex")
}

fn mutex_worker_error() -> io::Error {
	WorkerError::Aborted("could not unlock child state".into())
}

macro_rules! try_mutex {
	($e: expr) => {
		match $e {
			Ok(x) => x,
			Err(_) => { return Err(mutex_io_error()); },
		}
	}
}


trait OptionExt<T> {
	// fn may<T,F,R>(&self, fn: F) -> Option<R>
	fn bind<F,R>(&self, f: F) -> Option<R>
		where F: FnOnce(&T) -> Option<R>;
}

impl<T> OptionExt<T> for Option<T> {
	fn bind<F,R>(&self, f: F) -> Option<R>
		where F: FnOnce(&T) -> Option<R>
	{
		match *self {
			Some(ref x) => f(x),
			None => None,
		}
	}
}

// XXX LinkedSpawn is not object-safe, which makes it a bit useless.
// pub trait LinkedSpawn {
// 	fn spawn<F,E>(&self, name: String, work: F) -> Result<Worker<E>, io::Error>
// 		where F: FnOnce(WorkerSelf<E>) -> Result<(),E>, F: Send + 'static, E: Send + 'static;
//
// 	fn spawn_anon<F,E>(&self, work: F) -> Result<Worker<E>, io::Error>
// 		where F: FnOnce(WorkerSelf<E>) -> Result<(),E>, F: Send + 'static, E: Send + 'static;
// }

struct ChildWorker<E> {
	child_id: u32,
	shared: Arc<Mutex<WorkerShared<E>>>,
}

impl<E> ChildWorker<E> {
	// NOTE: child_id is only unique within the given
	// parent - you cannot compare children from different
	// parents
	fn eq(&self, other: &WorkerShared<E>) -> bool {
		match other.child_id {
			Some(id) => id == self.child_id,
			None => false,
		}
	}
}

struct WorkerShared<E> {
	// XXX check deadlocks with parent/child interactions
	signal: mpsc::Sender<()>,
	children: Vec<Arc<ChildWorker<E>>>,
	thread: Option<JoinHandle<Result<(),E>>>,
	// XXX if T is clone(), we wouldn't need Arc
	result: Option<Result<(),WorkerError<E>>>,
	parent: Option<Arc<Mutex<WorkerShared<E>>>>,
	next_child_id: u32,
	child_id: Option<u32>,
	detached: bool,
}

impl<E> WorkerShared<E> {
	fn add_child(&mut self, child: Arc<Mutex<WorkerShared<E>>>) -> Result<(), io::Error> {
		// XXX do we really need to unlock here?
		let id = self.next_child_id;
		{
			let mut child_struct = try_mutex!(child.lock());
			child_struct.child_id = Some(id);
		}
		self.next_child_id += 1;
		self.children.push(Arc::new(ChildWorker {
			child_id: id,
			shared: child
		}));
		Ok(())
	}

	fn detach(&mut self) -> Result<(), TickError> {
		self.detached = true;
		match self.parent {
			None => (),
			Some(ref parent) => {
				let mut found = None;
				let mut idx = 0;
				let mut parent = try!(parent.lock());
				// XXX each_with_index?
				for candidate in parent.children.iter() {
					if candidate.eq(self) {
						found = Some(idx);
						break;
					}
					idx += 1;
				}
				match found {
					Some(i) => {
						let _:Arc<ChildWorker<E>> = parent.children.remove(i);
					},
					None => (),
				}
			},
		};
		Ok(())
	}

	fn wait(&mut self) -> Result<(),WorkerError<E>> {
		match self.result {
			// check for already done
			Some(ref r) => { return r.clone(); },
			None => (),
		};

		let mut low_priority_error = Ok(());

		// try to signal all children
		self.children.retain(|child| {
			match child.shared.lock() {
				Ok(child) =>
					match child.signal.send(()) {
						Ok(()) => true,
						Err(_) => {
							low_priority_error = mutex_worker_error();
							false
						},
					},
				Err(_) => {
					low_priority_error = mutex_worker_error();
					false
				},
			}
		});

		// check our _own_ state
		let mut state = match self.thread.take() {
			None => Err(
				WorkerError::Aborted("worker.wait() called multiple times".into())
			),
			Some(t) => {
				match t.join() {
					Ok(Ok(())) => Ok(()),
					Ok(Err(e)) => Err(WorkerError::Failed(Arc::new(e))),
					Err(ref e) => {
						let desc = format!("Thread panic: {:?}", e);
						Err(WorkerError::Aborted(desc))
					},
				}
			}
		};

		// then wait on the children we signalled
		loop {
			// XXX use drain() when stable
			match self.children.pop() {
				Some(child) => {
					match child.shared.lock() {
						Ok(mut child) => {
							state = state.and(child.wait());
						},
						Err(_) => cannot_unlock(),
					}
				},
				None => { break; }
			}
		};

		state = state.and(low_priority_error);
		self.result = Some(state.clone());
		state
	}
}

#[must_use = "worker will be immediately joined if `Worker` is not used"]
pub struct Worker<E:Send+Sync+'static> {
	work_ended: mpsc::Receiver<()>,
	shared: Arc<Mutex<WorkerShared<E>>>,
	name: Option<String>,
}

pub struct WorkerSelf<E> {
	receiver: mpsc::Receiver<()>,
	name: Option<String>,
	shared: Arc<Mutex<WorkerShared<E>>>,
}

pub enum WorkerError<E> {
	Cancelled,
	Failed(Arc<E>),
	Aborted(String),
}

pub enum TickError {
	Cancelled,
	Aborted(String),
}

impl<E> Clone for WorkerError<E> {
	fn clone(&self) -> WorkerError<E> {
		match *self {
			WorkerError::Cancelled => WorkerError::Cancelled,
			WorkerError::Aborted(s) => WorkerError::Aborted(s.clone()),
			WorkerError::Failed(e) => WorkerError::Failed(e.clone()),
		}
	}
}

impl<'a, T> convert::From<PoisonError<MutexGuard<'a, T>>> for TickError {
	fn from(err: PoisonError<MutexGuard<T>>) -> TickError {
		TickError::Aborted("failed to acquire lock".into())
	}
}

impl<'a, T, E> convert::From<PoisonError<MutexGuard<'a, T>>> for WorkerError<E> {
	fn from(err: PoisonError<MutexGuard<T>>) -> WorkerError<E> {
		WorkerError::Aborted("failed to acquire lock".into())
	}
}


impl<E:fmt::Debug> convert::From<WorkerError<E>> for io::Error {
	fn from(err: WorkerError<E>) -> io::Error {
		io::Error::new(io::ErrorKind::Other, format!("{:?}", err))
	}
}

// impl convert::From<WorkerError<io::Error>> for io::Error {
// 	fn from(err: WorkerError<io::Error>) -> io::Error {
// 		match err {
// 			WorkerError::Failed(e) => e,
// 			WorkerError::Aborted(s) => io::Error::new(io::ErrorKind::Other, s),
// 			WorkerError::Cancelled =>
// 				io::Error::new(io::ErrorKind::Other, "thread cancelled"),
// 		}
// 	}
// }

impl convert::From<TickError> for io::Error {
	fn from(err: TickError) -> io::Error {
		match err {
			TickError::Cancelled => io::Error::new(io::ErrorKind::Other, "thread cancelled"),
			TickError::Aborted(e) => io::Error::new(io::ErrorKind::Other, e),
		}
	}
}

impl<E> convert::From<TickError> for WorkerError<E> {
	fn from(err: TickError) -> WorkerError<E> {
		match err {
			TickError::Cancelled => WorkerError::Cancelled,
			TickError::Aborted(e) => WorkerError::Aborted(e),
		}
	}
}

impl<E:fmt::Debug> fmt::Debug for WorkerError<E> {
	fn fmt(&self, formatter: &mut fmt::Formatter) -> Result<(), fmt::Error> {
		match *self {
			WorkerError::Cancelled => "Worker cancelled".fmt(formatter),
			WorkerError::Failed(ref e) => format!("Worker failed: {:?}", e).fmt(formatter),
			WorkerError::Aborted(ref e) => format!("Worker aborted: {}", e).fmt(formatter),
		}
	}
}

impl<E> WorkerSelf<E> {
	pub fn spawn<F>(&self, name: String, work: F) -> Result<Worker<E>, io::Error>
		where F: FnOnce(WorkerSelf<E>) -> Result<(),E>, F: Send + 'static, E: Send + Sync + 'static
	{
		_spawn(Some(self.shared.clone()), Some(name), work)
	}

	pub fn spawn_anon<F>(&self, work: F) -> Result<Worker<E>, io::Error>
		where F: FnOnce(WorkerSelf<E>) -> Result<(),E>, F: Send + 'static, E: Send + Sync + 'static
	{
		_spawn(Some(self.shared.clone()), None, work)
	}

	pub fn tick(&self) -> Result<(),TickError> {
		println!("debug: {}: tick()", self.name());
		match self.receiver.try_recv() {
			Err(mpsc::TryRecvError::Empty) => Ok(()),
			Err(mpsc::TryRecvError::Disconnected) => Err(TickError::Cancelled),
			Ok(()) => Err(TickError::Cancelled),
		}
	}

	pub fn await_cancel(&self) -> () {
		println!("debug: {}: await_cancel()", self.name());
		match self.receiver.recv() {
			Ok(()) => (),
			Err(mpsc::RecvError) => (),
		}
	}


	fn name(&self) -> String {
		// XXX do this without copying
		match self.name {
			Some(ref n) => n.clone(),
			None => "<unnamed>".to_string(),
		}
	}
}

fn _spawn<E, F>(parent: Option<Arc<Mutex<WorkerShared<E>>>>, name: Option<String>, work: F) -> Result<Worker<E>, io::Error>
	where F: FnOnce(WorkerSelf<E>) -> Result<(),E>, F: Send + 'static, E: Send+Sync+'static
{
	let (sender, receiver) = mpsc::channel();
	let (done_sender, done_receiver) = mpsc::channel();
	let (self_thread_sender, self_thread_receiver) = mpsc::channel();

	let builder = thread::Builder::new();
	let builder = match name {
		None => builder,
		Some(name) => builder.name(name),
	};

	let thread_parent = parent.clone();
	let thread = try!(builder.spawn(move || {
		let self_thread : WorkerSelf<E> = self_thread_receiver.recv().unwrap();
		let result = work(self_thread);
		// XXX check race between end.send() and sendError?
		let _:Result<(), mpsc::SendError<()>> = done_sender.send(());
		match result {
			Ok(()) => Ok(()),
			Err(e) => {
				match thread_parent {
					Some(parent) => {
						// ignore send or unlock failure - it just means the parent has already ended
						match parent.lock() {
							Ok(ref mut parent) => {
								let _:Result<(),mpsc::SendError<()>> = parent.signal.send(());
								loop {
									// XXX use drain when stable
									match parent.children.pop() {
										Some(child) => {
											match child.shared.lock() {
												Ok(child) => {
													let _:Result<(),mpsc::SendError<()>> = child.signal.send(());
												},
												Err(_) => (),
											}
										},
										None => { break; }
									}
								}
							},
							Err(_) => (),
						}
					},
					None => (),
				};
				Err(e)
			},
		}
	}));

	let shared = Arc::new(Mutex::new(WorkerShared {
		children: Vec::new(),
		next_child_id: 0,
		child_id: None,
		signal: sender.clone(),
		result: None,
		thread: Some(thread),
		parent: parent,
		detached: false,
	}));

	let self_thread = WorkerSelf {
		shared: shared.clone(),
		name: name.clone(),
		receiver: receiver,
	};
	self_thread_sender.send(self_thread);

	match parent {
		Some(ref parent) => {
			match parent.lock() {
				Ok(ref mut parent) => {
					parent.add_child(shared.clone());
				},
				Err(_) => {
					return Err(io::Error::new(io::ErrorKind::Other, "failed to acquire mutex"));
				}
			};
		},
		None => (),
	}

	Ok(Worker {
		name: name.clone(),
		shared: shared,
		work_ended: done_receiver,
	})
}

pub fn spawn<F,E>(name: String, work: F) -> Result<Worker<E>, io::Error>
	where F: FnOnce(WorkerSelf<E>) -> Result<(),E>, F: Send + 'static, E: Send+ Sync +'static
{
	_spawn(None, Some(name), work)
}

pub fn spawn_anon<F,E>(work: F) -> Result<Worker<E>, io::Error>
	where F: FnOnce(WorkerSelf<E>) -> Result<(),E>, F: Send + 'static, E: Send+ Sync +'static
{
	_spawn(None, None, work)
}

impl<E:Send + Sync + 'static> Worker<E> {
	pub fn spawn<F>(&self, name: String, work: F) -> Result<Worker<E>, io::Error>
		where F: FnOnce(WorkerSelf<E>) -> Result<(),E>, F: Send + 'static
	{
		_spawn(Some(self.shared.clone()), Some(name), work)
	}

	pub fn spawn_anon<F>(&self, work: F) -> Result<Worker<E>, io::Error>
		where F: FnOnce(WorkerSelf<E>) -> Result<(),E>, F: Send + 'static
	{
		_spawn(Some(self.shared.clone()), None, work)
	}

	pub fn wait(&mut self) -> Result<(),WorkerError<E>> {
		match self.shared.lock() {
			Ok(shared) => shared.wait(),
			Err(_) => Err(WorkerError::Aborted(
					"failed to unlock shared state".into())),
		}
	}

	fn name(&self) -> String {
		// XXX do this without copying :(
		match self.name.map(|s|s.to_string()) {
			Some(n) => n,
			None => "<unnamed>".to_string(),
		}
	}

	pub fn poll(&mut self) -> Result<(),WorkerError<E>> {
		// checks error channel for event. If received
		// then wait(), otherwise return Ok(None)
		println!("debug: {}: poll()", self.name());
		let ended = match self.work_ended.try_recv() {
			Ok(()) => true,
			Err(mpsc::TryRecvError::Empty) => false,
			Err(e) => {
				return Err(WorkerError::Aborted(format!("poll() failed: {:?}", e)));
			},
		};
		if ended {
			self.wait()
		} else {
			Ok(())
		}
	}

	pub fn detach(&mut self) -> Result<(), TickError> {
		// TODO: detach will ignore errors, they should probably panic!
		let shared = try!(self.shared.lock());
		shared.detach()
	}

	pub fn terminate(&mut self) -> Result<(),WorkerError<E>> {
		fn err<E>() -> WorkerError<E> {
			WorkerError::Aborted("failed to send terminate()".to_string())
		}
		match self.shared.lock() {
			Ok(shared) => match shared.signal.send(()) {
				Ok(()) => self.wait(),
				Err(_) => Err(err()),
			},
			Err(_) => Err(err()),
		}
	}
}

impl<E:Send+Sync+'static> Drop for Worker<E> {
	fn drop(&mut self) {
		match self.shared.lock() {
			Ok(shared) => {
				if (!shared.detached) {
					match shared.result {
						Some(_) => (),
						None => {
							warn!("thread dropped without `detach` or `wait()`");
							match self.terminate() {
								Err(_) => (), // already dead
								Ok(()) => match self.wait() {
									Ok(()) => (),
									Err(_) => error!("thread failed during drop()"),
								},
							}
						}
					}
				}
			},
			Err(_) => { /* nothing we can do */ () },
		}

	}
}

// impl<T:Send> LinkedSpawn for Worker<T> {
// 	fn spawn<F,E>(&self, name: String, work: F) -> Result<Worker<E>, io::Error>
// 		where F: FnOnce(WorkerSelf<E>) -> Result<(),E>, F: Send + 'static, E: Send + 'static
// 	{
// 		_spawn(Some(self.shared.clone()), Some(name), work)
// 	}
//
// 	fn spawn_anon<F,E>(&self, work: F) -> Result<Worker<E>, io::Error>
// 		where F: FnOnce(WorkerSelf<E>) -> Result<(),E>, F: Send + 'static, E: Send + 'static
// 	{
// 		_spawn(Some(self.shared.clone()), None, work)
// 	}
// }
//
// impl LinkedSpawn for WorkerSelf<E> {
// 	fn spawn<F,E>(&self, name: String, work: F) -> Result<Worker<E>, io::Error>
// 		where F: FnOnce(WorkerSelf<E>) -> Result<(),E>, F: Send + 'static, E: Send + 'static
// 	{
// 		_spawn(Some(self.shared.clone()), Some(name), work)
// 	}
//
// 	fn spawn_anon<F,E>(&self, work: F) -> Result<Worker<E>, io::Error>
// 		where F: FnOnce(WorkerSelf<E>) -> Result<(),E>, F: Send + 'static, E: Send + 'static
// 	{
// 		_spawn(Some(self.shared.clone()), None, work)
// 	}
// }


pub fn test() {
	let mut t = spawn("t".to_string(), |t| -> Result<(), io::Error> {
		let mut ch = try!(t.spawn("ch".to_string(), |t| {
			println!("ch: sleeping 2");
			try!(t.tick());
			thread::sleep_ms(2000);
			println!("ch: slept 2");
			// panic!("ch: end panic");
			Err(io::Error::new(io::ErrorKind::Other, "tet"))
			// Ok(())
		}));
		loop {
			thread::sleep_ms(2000);
			println!("t: slept 1, polling...");
			try!(ch.poll()); // fail if thread has been cancelled
		}
	}).unwrap();

	// Every thread must do self.check() to see if
	//  - it has been cancelled from outside
	//  - it has been cancelled via attachment propagation
	//
	// Every _spawner_ must either check threads that it creates manually
	// (if it wants to be able to handle errors), or if it only
	// spawns attached threads then it may get by with self.tick()

	// let mut t2 = (WorkerStatic::spawn("t2".to_string(), |_t| {
	// 	thread::sleep(Duration::from_secs(1));
	// 	panic!("t2: oh dear...");
	// })).unwrap();
	// t2.detach();
	// println!("Nobody is listening to t2; error will be fatal");

	for _ in 0..10 {
		// do_something();
		t.poll().unwrap();
		thread::sleep_ms(1000);
	}
	println!("dropping t!");
	drop(t);
}
