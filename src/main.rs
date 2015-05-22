#![feature(scoped)]
#![feature(convert)]
#![feature(collections)]
#![feature(collections_drain)]
#![feature(std_misc)]

extern crate chrono;
extern crate hyper;
extern crate env_logger;
extern crate schedule_recv;

#[macro_use]
extern crate log;


mod monitor;
mod system_monitor;
mod systemd;
mod service;

pub use monitor::*;
pub use system_monitor::*;
use systemd::*;



fn main () {
	env_logger::init().unwrap();
	let mut monitor = SystemMonitor::new(5000);
	monitor.add(String::from_str("systemd"), Box::new(SystemdMonitor::new())).unwrap();
	//let monitor = SystemdMonitor::new();
	//let rv = monitor.scan();
	//match rv {
	//	Ok(rv) => println!("OK: {:?}", rv),
	//	Err(e) => println!("Error: {}", e),
	//}
	service::main(monitor);
}
