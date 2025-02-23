use std::{
	sync::{
		mpsc::{self, channel},
		Arc, Mutex, PoisonError,
	},
	thread,
};

pub fn multi_thread_map_iter<
	T: 'static + Send + Sync,
	U: 'static + Send + Sync,
	F: 'static + Send + Sync + Clone + Fn(T) -> U,
>(
	iter: impl Iterator<Item = T> + Send + 'static,
	callback: F,
	job_num: usize,
) -> mpsc::IntoIter<U> {
	let callback_orig = callback;
	let iter_orig = Arc::new(Mutex::new(iter));
	let (result_send_orig, result_recv) = channel::<U>();

	for thread_num in 0..job_num {
		let callback = callback_orig.clone();
		let iter = iter_orig.clone();
		let result_send = result_send_orig.clone();
		thread::Builder::new()
			.name(format!("MT iter #{thread_num}"))
			.spawn(move || loop {
				// Inner scope is defined to ensure mutex is unlocked while callback is running
				let iter_item = {
					let mut iter_lock = iter.lock().unwrap_or_else(PoisonError::into_inner);
					iter_lock.next()
				};
				if let Some(iter_item) = iter_item {
					let _ = result_send.send(callback(iter_item));
				} else {
					break;
				}
			})
			.unwrap();
	}
	result_recv.into_iter()
}
