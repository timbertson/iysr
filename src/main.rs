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


mod monitor;
mod system_monitor;
mod systemd;
mod service;

pub use monitor::*;
pub use system_monitor::*;
use systemd::*;

fn main () {
	env_logger::init().unwrap();
	let mut monitor = SystemMonitor::new(20000);
	monitor.add(String::from_str("systemd.system"), Box::new(SystemdMonitor::system())).unwrap();
	monitor.add(String::from_str("systemd.user"), Box::new(SystemdMonitor::user())).unwrap();
	service::main(monitor).unwrap();
}
