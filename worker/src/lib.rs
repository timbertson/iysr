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

#[macro_use]
extern crate log;

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

enum WorkerState {
	Running,
	Ended,
	Detached, // will panic! on error
}

pub trait LinkedSpawn {
	fn spawn<F,E>(&self, name: String, work: F) -> Result<Worker<E>, io::Error>
		where F: FnOnce(WorkerSelf) -> Result<(),E>, F: Send + 'static, E: Send + 'static;

	fn spawn_anon<F,E>(&self, work: F) -> Result<Worker<E>, io::Error>
		where F: FnOnce(WorkerSelf) -> Result<(),E>, F: Send + 'static, E: Send + 'static;
}

#[must_use = "worker will be immediately joined if `Worker` is not used"]
pub struct Worker<E:Send+'static> {
	thread: Option<JoinHandle<Result<(),E>>>,
	signal: mpsc::Sender<()>,
	work_ended: mpsc::Receiver<()>,
	state: WorkerState,
}

pub struct WorkerSelf {
	receiver: mpsc::Receiver<()>,
	signal: mpsc::Sender<()>,
	name: Option<String>,
}

pub enum WorkerError<E> {
	Cancelled,
	Failed(E),
	Aborted(String),
}

pub enum TickError {
	Cancelled,
}

impl<E:fmt::Debug> convert::From<WorkerError<E>> for io::Error {
	fn from(err: WorkerError<E>) -> io::Error {
		io::Error::new(io::ErrorKind::Other, format!("{:?}", err))
	}
}

impl convert::From<TickError> for io::Error {
	fn from(err: TickError) -> io::Error {
		match err {
			TickError::Cancelled => io::Error::new(io::ErrorKind::Other, "thread cancelled"),
		}
	}
}

impl<E> convert::From<TickError> for WorkerError<E> {
	fn from(err: TickError) -> WorkerError<E> {
		match err {
			TickError::Cancelled => WorkerError::Cancelled,
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

impl WorkerSelf {
	pub fn spawn<F,E2>(&self, name: String, work: F) -> Result<Worker<E2>, io::Error>
		where F: FnOnce(WorkerSelf) -> Result<(),E2>, F: Send + 'static, E2: Send + 'static
	{
		_spawn(Some(self.signal.clone()), Some(name), work)
	}

	pub fn spawn_anon<F,E2>(&self, work: F) -> Result<Worker<E2>, io::Error>
		where F: FnOnce(WorkerSelf) -> Result<(),E2>, F: Send + 'static, E2: Send + 'static
	{
		_spawn(Some(self.signal.clone()), None, work)
	}

	pub fn tick(&self) -> Result<(),TickError> {
		// XXX should return `Cancelled`, not Disconnected
		println!("debug: {}: tick()", self.name());
		match self.receiver.try_recv() {
			Err(mpsc::TryRecvError::Empty) => Ok(()),
			Err(mpsc::TryRecvError::Disconnected) => Err(TickError::Cancelled),
			Ok(()) => Err(TickError::Cancelled),
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

fn _spawn<E, F>(signal_parent: Option<mpsc::Sender<()>>, name: Option<String>, work: F) -> Result<Worker<E>, io::Error>
	where F: FnOnce(WorkerSelf) -> Result<(),E>, F: Send + 'static, E: Send+'static
{
	// let signal_parent = parent.map(|p| p.signal.clone());
	let (sender, receiver) = mpsc::channel();
	let self_thread = WorkerSelf {
		signal: sender.clone(),
		name: name.clone(),
		receiver: receiver,
	};

	let (done_sender, done_receiver) = mpsc::channel();

	let builder = thread::Builder::new();
	let builder = match name {
		None => builder,
		Some(name) => builder.name(name),
	};
	let thread = try!(builder.spawn(move || {
		let result = work(self_thread);
		// XXX check race between end.send() and sendError?
		let _:Result<(), mpsc::SendError<()>> = done_sender.send(());
		match result {
			Ok(()) => Ok(()),
			Err(e) => {
				match signal_parent {
					// ignore send failure - it just means the parent has already ended
					Some(s) => {
						let _:Result<(),mpsc::SendError<()>> = s.send(());
						()
					},
					None => (),
				}
				Err(e)
			},
		}
	}));
	Ok(Worker {
		thread: Some(thread),
		signal: sender,
		work_ended: done_receiver,
		state: WorkerState::Running,
	})
}

pub fn spawn<F,E>(name: String, work: F) -> Result<Worker<E>, io::Error>
	where F: FnOnce(WorkerSelf) -> Result<(),E>, F: Send + 'static, E: Send+'static
{
	_spawn(None, Some(name), work)
}

pub fn spawn_anon<F,E>(work: F) -> Result<Worker<E>, io::Error>
	where F: FnOnce(WorkerSelf) -> Result<(),E>, F: Send + 'static, E: Send+'static
{
	_spawn(None, None, work)
}

impl<E:Send + 'static> Worker<E> {
	pub fn spawn<F,E2>(&self, name: String, work: F) -> Result<Worker<E2>, io::Error>
		where F: FnOnce(WorkerSelf) -> Result<(),E2>, F: Send + 'static, E2: Send + 'static
	{
		_spawn(Some(self.signal.clone()), Some(name), work)
	}

	pub fn spawn_anon<F,E2>(&self, work: F) -> Result<Worker<E2>, io::Error>
		where F: FnOnce(WorkerSelf) -> Result<(),E2>, F: Send + 'static, E2: Send + 'static
	{
		_spawn(Some(self.signal.clone()), None, work)
	}

	pub fn wait(&mut self) -> Result<(),WorkerError<E>> {
		// XXX this is premature - if join() panics,
		// guess we end up with an orphaned thread. Can that happen?
		self.state = WorkerState::Ended;
		match self.thread.take() {
			None => Ok(()), // already waited
			Some(t) => {
				match t.join() {
					Ok(Ok(())) => Ok(()),
					Ok(Err(e)) => Err(WorkerError::Failed(e)),
					Err(ref e) => {
						let desc = format!("Thread panic: {:?}", e);
						Err(WorkerError::Aborted(desc))
					},
				}
			}
		}
	}

	fn name(&self) -> String {
		// XXX do this without copying :(
		match self.thread.bind(|t| t.thread().name().map(|s|s.to_string())) {
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

	pub fn detach(&mut self) {
		self.state = match self.state {
			WorkerState::Running => WorkerState::Detached,
			WorkerState::Ended => WorkerState::Ended,
			WorkerState::Detached => WorkerState::Detached,
		};
	}

	pub fn terminate(&mut self) -> Result<(),WorkerError<E>> {
		match self.signal.send(()) {
			Ok(()) => (),
			Err(_) => {
				return Err(WorkerError::Aborted("failed to send terminate()".to_string()));
			},
		};
		self.wait()
	}
}

impl<E:Send+'static> Drop for Worker<E> {
	fn drop(&mut self) {
		match self.state {
			WorkerState::Running => {
				warn!("thread dropped without `detach` or `wait()`");
				match self.terminate() {
					Err(_) => (), // already dead
					Ok(()) => match self.wait() {
						Ok(()) => (),
						Err(_) => error!("thread failed during drop()"),
					},
				}
			},
			WorkerState::Detached | WorkerState::Ended => (),
		}
	}
}

impl<T:Send> LinkedSpawn for Worker<T> {
	fn spawn<F,E>(&self, name: String, work: F) -> Result<Worker<E>, io::Error>
		where F: FnOnce(WorkerSelf) -> Result<(),E>, F: Send + 'static, E: Send + 'static
	{
		_spawn(Some(self.signal.clone()), Some(name), work)
	}

	fn spawn_anon<F,E>(&self, work: F) -> Result<Worker<E>, io::Error>
		where F: FnOnce(WorkerSelf) -> Result<(),E>, F: Send + 'static, E: Send + 'static
	{
		_spawn(Some(self.signal.clone()), None, work)
	}
}

impl LinkedSpawn for WorkerSelf {
	fn spawn<F,E>(&self, name: String, work: F) -> Result<Worker<E>, io::Error>
		where F: FnOnce(WorkerSelf) -> Result<(),E>, F: Send + 'static, E: Send + 'static
	{
		_spawn(Some(self.signal.clone()), Some(name), work)
	}

	fn spawn_anon<F,E>(&self, work: F) -> Result<Worker<E>, io::Error>
		where F: FnOnce(WorkerSelf) -> Result<(),E>, F: Send + 'static, E: Send + 'static
	{
		_spawn(Some(self.signal.clone()), None, work)
	}
}


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
