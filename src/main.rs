extern crate chrono;
mod monitor;
mod systemd;

use chrono::{DateTime,UTC};
pub use monitor::*;
use systemd::*;



fn main () {
	let service = Service {
		name: "server",
	};


	let monitor = SystemdMonitor;

	let sd = Unit {
		service: &service,
		unit: &"foo.service",
	};

	let rv = monitor.scan();
	//let status = Status {
	//	state: state,
	//	service: &service,
	//	time: time,
	//};
	println!("monitor: {:?}", rv);
}
