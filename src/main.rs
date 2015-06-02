#![feature(scoped)]
#![feature(convert)]
#![feature(collections)]
#![feature(collections_drain)]
#![feature(std_misc)]
#![feature(custom_derive, plugin)]

#![plugin(tojson_macros)]

// XXX disable these when things get less prototypey
#![allow(unused_imports)]
#![allow(dead_code)]

extern crate chrono;
extern crate hyper;
extern crate env_logger;
extern crate schedule_recv;
extern crate rustc_serialize;

#[macro_use]
extern crate log;

#[macro_use]
mod util;
mod monitor;
mod system_monitor;
mod systemd;
mod service;
mod journal;
mod config;

pub use monitor::*;
pub use system_monitor::*;
use systemd::*;
use journal::*;
use std::sync::{Arc,Mutex};
use std::env;
use std::io;
use std::fs::File;
use config::{Config,ConfigError};

fn load_config(filename: String) -> Result<Config, ConfigError> {
	info!("loading config file {}", filename);
	let mut file = try!(File::open(filename));
	Config::load(&mut file)
}

fn main () {
	env_logger::init().unwrap();
	let mut args = env::args().skip(1);
	let config = args.next().ok_or(ConfigError::new("--config required".to_string()))
		.and_then(load_config);
	let _config = match config {
		Ok(config) => config,
		// XXX stderr
		Err(e) => {
			println!("Error loading config: {}", e);
			return;
		},
	};


	let systemd_system = SystemdMonitor::system("systemd.system".to_string());
	let systemd_user = SystemdMonitor::user("systemd.user".to_string());
	let journal = Journal::new().unwrap();
	let monitor = SystemMonitor::new(
		20000,
		50,
		vec!(systemd_system.poller(), systemd_user.poller()),
		vec!(systemd_system.pusher(), systemd_user.pusher(), Box::new(journal))
	).unwrap();
	service::main(monitor).unwrap();
}
