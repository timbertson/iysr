#![feature(scoped)]
#![feature(convert)]
#![feature(collections)]
#![feature(collections_drain)]

extern crate chrono;
extern crate hyper;

#[macro_use]
extern crate log;


mod monitor;
mod systemd;
mod service;

pub use monitor::*;
use systemd::*;



fn main () {
	let monitor = SystemdMonitor::new();
	let rv = monitor.scan();
	match rv {
		Ok(rv) => println!("OK: {:?}", rv),
		Err(e) => println!("Error: {}", e),
	}
	service::main();
}
