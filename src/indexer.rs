use std::{
	collections::{BTreeMap, BTreeSet},
	fs::{self, read_dir, DirEntry, File, FileType},
	io::{Error as IoError, ErrorKind as IoErrorKind, Read},
	mem,
	path::Path,
	sync::Arc,
};

use sha2::Digest;

use crate::{
	deep_readdir::DeepReadDir, file_closer::deferred_file_drop, multi_thread_iter::multi_thread_map_iter, THREADS,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileHash {
	pub file_len: u64,
	pub digest_256: [u8; 32],
	pub digest_512: [u8; 64],
}
impl FileHash {
	pub fn from_file(mut file: File) -> Result<Self, IoError> {
		let mut hash_256 = sha2::Sha256::new();
		let mut hash_512 = sha2::Sha512::new();
		let mut file_buf = [0u8; 524288]; // 512KiB
		let mut total_read = 0u64;
		loop {
			match file.read(&mut file_buf) {
				Ok(0) => break,
				Ok(read_amount) => {
					let read_buf = &file_buf[0..read_amount];
					hash_256.update(read_buf);
					hash_512.update(read_buf);
					total_read += read_amount as u64;
				},
				Err(e) if matches!(e.kind(), IoErrorKind::Interrupted) => {},
				Err(e) => return Err(e),
			}
		}
		deferred_file_drop(file);
		Ok(Self {
			file_len: total_read,
			digest_256: hash_256.finalize().into(),
			digest_512: hash_512.finalize().into(),
		})
		//file.read()
	}
}

#[derive(Debug, Clone)]
pub enum FileIndexItem {
	File { hash: FileHash },
	Folder { contents: Vec<Arc<Path>> },
}
impl FileIndexItem {
	pub fn as_file(&self) -> Option<&FileHash> {
		match self {
			Self::File { hash } => Some(hash),
			_ => None,
		}
	}
	pub fn as_folder(&self) -> Option<&[Arc<Path>]> {
		match self {
			Self::Folder { contents } => Some(&contents),
			_ => None,
		}
	}
}

#[derive(Debug, Clone, Default)]
pub struct FileIndex {
	pub hash_to_paths: BTreeMap<FileHash, BTreeSet<Arc<Path>>>,
	pub paths_to_items: BTreeMap<Arc<Path>, FileIndexItem>,
}

impl FileIndex {
	pub fn file_instance_count(&self, hash: &FileHash) -> usize {
		self.hash_to_paths.get(hash).map(|v| v.len()).unwrap_or_default()
	}
	pub fn remove_empty_directories(&mut self, starting_with: &Path) -> anyhow::Result<()> {
		self.paths_to_items = mem::take(&mut self.paths_to_items)
			.into_iter()
			.rev()
			.filter_map(|(path, file_index_item)| -> Option<anyhow::Result<_>> {
				if path.starts_with(starting_with) && file_index_item.as_folder().is_some_and(<[_]>::is_empty) {
					println!("deleting: {}", path.to_string_lossy());
					match fs::remove_dir(path) {
						Ok(_) => None,
						Err(err) => Some(Err(err.into())),
					}
				} else {
					Some(Ok((path, file_index_item)))
				}
			})
			.collect::<anyhow::Result<BTreeMap<_, _>>>()?;
		Ok(())
	}
	pub fn remove_dupes_in_other_folders(&mut self, except: &Path) -> anyhow::Result<()> {
		for (_, paths) in self
			.hash_to_paths
			.iter_mut()
			.filter(|(_, paths)| paths.len() > 1 && paths.iter().any(|path| path.starts_with(except)))
		{
			for path in paths.clone() {
				if !path.starts_with(except) {
					println!("deleting: {}", path.to_string_lossy());
					fs::remove_file(&path)?;
					paths.remove(&path);
					self.paths_to_items.remove(&path);
				}
			}
		}
		Ok(())
	}
	pub fn remove_dupes_from_folder(&mut self, folder: &Path) -> anyhow::Result<()> {
		for (_, paths) in self.hash_to_paths.iter_mut() {
			let mut paths_to_remove = paths
				.iter()
				.filter(|path| path.starts_with(folder) && paths.len() > 1)
				.cloned()
				.collect::<Vec<_>>();

			if paths_to_remove.is_empty() {
				continue;
			}
			paths_to_remove.sort_by(|path_a, path_b| path_a.components().count().cmp(&path_b.components().count()));
			paths_to_remove.remove(1); // Keep one with shortest path
			while let Some(path_to_remove) = paths_to_remove.pop() {
				println!("deleting: {}", path_to_remove.to_string_lossy());
				fs::remove_file(&path_to_remove)?;
				paths.remove(&path_to_remove);
				self.paths_to_items.remove(&path_to_remove);
			}
		}
		Ok(())
	}
	pub fn from_folder(folder_path: Arc<Path>) -> anyhow::Result<Self> {
		let mut hash_to_paths: BTreeMap<FileHash, BTreeSet<Arc<Path>>> = BTreeMap::new();
		let mut paths_to_items: BTreeMap<Arc<Path>, FileIndexItem> = BTreeMap::new();
		println!("exploring: {}", folder_path.to_string_lossy());

		let mut top_level_contents = Vec::new();
		for inner_dir_entry in read_dir(&folder_path)? {
			let inner_dir_entry = inner_dir_entry?;
			let inner_file_type = inner_dir_entry.file_type()?;
			if inner_file_type.is_dir() || inner_file_type.is_file() {
				top_level_contents.push(Arc::from(inner_dir_entry.path()));
			}
		}
		top_level_contents.sort();
		paths_to_items.insert(
			folder_path.clone(),
			FileIndexItem::Folder {
				contents: top_level_contents,
			},
		);

		for iter_result in multi_thread_map_iter(
			DeepReadDir::new(&folder_path)?.filter_map(|dir_entry| -> Option<anyhow::Result<(DirEntry, FileType)>> {
				match dir_entry {
					Ok(dir_entry) => match dir_entry.file_type() {
						Ok(file_type) if file_type.is_dir() || file_type.is_file() => Some(Ok((dir_entry, file_type))),
						Ok(_) => {
							eprintln!("{}: ignoring special/system file", dir_entry.path().to_string_lossy());
							None
						},
						Err(err) => Some(Err(err.into())),
					},
					Err(err) => Some(Err(err.into())),
				}
			}),
			|dir_entry| -> anyhow::Result<(Arc<Path>, FileIndexItem)> {
				let (dir_entry, file_type) = dir_entry?;
				let file_path: Arc<Path> = Arc::from(dir_entry.path().as_path());
				let file_path_str = file_path.to_string_lossy();
				if file_type.is_dir() {
					println!("indexing: {file_path_str}");
					let mut contents = Vec::new();
					// Yeah, some folders are going to get read twice, whatever.
					for inner_dir_entry in read_dir(&file_path)? {
						let inner_dir_entry = inner_dir_entry?;
						let inner_file_type = inner_dir_entry.file_type()?;
						if inner_file_type.is_dir() || inner_file_type.is_file() {
							contents.push(Arc::from(inner_dir_entry.path()));
						}
					}
					contents.sort();
					return Ok((file_path, FileIndexItem::Folder { contents }));
				} else if file_type.is_file() {
					println!("hashing: {file_path_str}");
					let hash = FileHash::from_file(File::open(&file_path)?)?;
					println!("hashed: {file_path_str}");
					return Ok((file_path, FileIndexItem::File { hash }));
				} else {
					unreachable!("dir entry should have already been filtered")
				}
			},
			*THREADS,
		) {
			let (path, index_item) = iter_result?;
			match &index_item {
				FileIndexItem::File { hash } => {
					hash_to_paths.entry(*hash).or_default().insert(path.clone());
				},
				_ => {},
			}
			paths_to_items.insert(path, index_item);
		}
		Ok(Self {
			hash_to_paths,
			paths_to_items,
		})
	}
	pub fn extend(&mut self, other: FileIndex) {
		for (other_hash, other_paths) in other.hash_to_paths {
			let self_paths = self.hash_to_paths.entry(other_hash).or_default();
			self_paths.extend(other_paths);
		}
		self.paths_to_items.extend(other.paths_to_items.into_iter());
	}
}
