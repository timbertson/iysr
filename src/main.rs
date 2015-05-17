#![feature(scoped)]
#![feature(convert)]
#![feature(collections)]
#![feature(collections_drain)]

extern crate chrono;
mod monitor;
mod systemd;

pub use monitor::*;
use systemd::*;



fn main () {
	let monitor = SystemdMonitor::new();
	let rv = monitor.scan();
	match rv {
		Ok(rv) => println!("OK: {:?}", rv),
		Err(e) => println!("Error: {}", e),
	}
}
