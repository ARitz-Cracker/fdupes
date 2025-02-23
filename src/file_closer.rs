//use std::sync::Arc;

use std::{
	convert::identity,
	fs::File,
	sync::{
		mpsc::{self, Sender},
		PoisonError, RwLock,
	},
	thread,
};

//use std::sy
static FILE_CLOSER_SENDER: RwLock<Option<Sender<File>>> = RwLock::new(None);

fn new_file_closer_thread() {
	let mut sender_lock = FILE_CLOSER_SENDER.write().unwrap_or_else(PoisonError::into_inner);
	let (closer_sender, closer_receiver) = mpsc::channel();
	*sender_lock = Some(closer_sender);
	drop(sender_lock);
	thread::Builder::new()
		.name("File Closer".into())
		.spawn(move || {
			// I read somewhere that closing a file takes time, so we're going to do it on a different thread I guess.
			for file in closer_receiver.iter() {
				drop(file);
			}
		})
		.unwrap();
}
pub fn stop_file_closer_thread() {
	let mut sender_lock = FILE_CLOSER_SENDER.write().unwrap_or_else(PoisonError::into_inner);
	*sender_lock = None;
}

pub fn deferred_file_drop(file: File) {
	let sender_lock = FILE_CLOSER_SENDER.read().unwrap_or_else(PoisonError::into_inner);
	let Some(sender) = &*sender_lock else {
		drop(sender_lock);
		new_file_closer_thread();
		deferred_file_drop(file);
		return;
	};
	match sender.send(file) {
		Ok(_) => {},
		Err(file) => {
			drop(sender_lock);
			new_file_closer_thread();
			deferred_file_drop(file.0);
		},
	}
}
