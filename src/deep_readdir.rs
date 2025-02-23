use std::{
	fs::{read_dir, DirEntry, FileType, ReadDir},
	io::Error as IoError,
	path::Path,
};

#[derive(Debug)]
pub struct DeepReadDir {
	inner: Vec<ReadDir>,
}
impl DeepReadDir {
	pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, IoError> {
		Ok(Self {
			inner: vec![read_dir(path)?],
		})
	}
}
impl Iterator for DeepReadDir {
	type Item = Result<DirEntry, IoError>;

	fn next(&mut self) -> Option<Self::Item> {
		let Some(inner_iter) = self.inner.last_mut() else {
			return None;
		};
		let inner_iter_result = inner_iter.next();
		if let Some(inner_iter_result) = inner_iter_result.as_ref() {
			if let Ok(dir_entry) = inner_iter_result.as_ref() {
				if dir_entry
					.file_type()
					.is_ok_and(|file_type: FileType| file_type.is_dir())
				{
					// We're ignoring errors on reading sub-dirs.
					if let Ok(new_readdir) = read_dir(dir_entry.path()) {
						self.inner.push(new_readdir);
					}
				}
			}
		} else {
			self.inner.pop();
			if !self.inner.is_empty() {
				return self.next();
			}
		}
		inner_iter_result
	}
}
