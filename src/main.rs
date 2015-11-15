// XXX disable these when things get less prototypey
#![allow(unused_imports)]
#![allow(dead_code)]

extern crate chrono;
extern crate hyper;
extern crate env_logger;
extern crate schedule_recv;
extern crate rustc_serialize;
extern crate glob;
extern crate regex;
extern crate dbus;
extern crate worker;
extern crate thread_scoped;

#[macro_use]
extern crate log;

#[macro_use]
mod util;

#[macro_use]
mod errors;

mod monitor;
mod system_monitor;
mod systemd;
mod systemd_common;
mod systemd_dbus;
// mod systemd_subprocess;
mod service;
mod journal;
mod config;
mod filter;
mod dbus_common;
mod dbus_notify;

pub use monitor::*;
pub use system_monitor::*;
use errors::InternalError;
use systemd::*;
use journal::*;
use std::sync::{Arc,Mutex};
use std::env;
use std::process;
use std::io;
use std::io::Write;
use std::fs::File;
use config::{Config,ConfigError, SourceConfig};

fn load_config(filename: String) -> Result<Config, ConfigError> {
	errln!("Loading config from {}", filename);
	let mut file = try!(File::open(filename));
	Config::load(&mut file)
}

fn run(config: Config) -> Result<(), errors::InternalError> {
	let pull_sources : Vec<Box<PullDataSource>> = Vec::new();
	let mut push_sources : Vec<Box<PushDataSource>> = Vec::new();

	for module in config.sources {
		match module {
			SourceConfig::Systemd(conf) => {
				let systemd = SystemdMonitor::new(conf);
				// pull_sources.push(systemd.poller());
				push_sources.push(systemd.pusher());
			},
			SourceConfig::Journal(conf) => {
				let journal = try!(Journal::new(conf));
				push_sources.push(Box::new(journal));
			},
		}
	}

	// XXX with scoped threads, we could get away with a ref instead of Arc
	let monitor = Arc::new(Mutex::new(try!(SystemMonitor::new(
		20000,
		50,
		pull_sources,
		push_sources
	))));

	let mut reaper = try!(worker::spawn("reaper".into(), |t| {
		let mut services : Vec<worker::Worker<InternalError>> = Vec::new();
		services.push(try!(service::main(monitor, &t)));
		t.await_cancel();
		Err(InternalError::new("reaper cancelled".into()))
	}));
	reaper.wait().map_err(|e| e.into())
}


macro_rules! fail{
	($($arg:tt)*) => {
		{
			errln!($($arg)*);
			process::exit(1);
		}
	}
}

fn main () {
	env_logger::init().unwrap();
	let mut stderr = io::stderr();
	let mut args = env::args().skip(1);
	let config = args.next().ok_or(ConfigError::new("--config required".to_string()));
	match args.next() {
		Some(_) => fail!("Too many arguments"),
		None => (),
	};

	let config = match config.and_then(load_config) {
		Ok(config) => config,
		Err(e) => {
			fail!("Error loading config: {}", e);
		},
	};

	match run(config) {
		Ok(config) => config,
		// XXX stderr
		Err(e) => {
			writeln!(&mut stderr, "Error: {}", e).unwrap();
			process::exit(1);
		},
	};

}
