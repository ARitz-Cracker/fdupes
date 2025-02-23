use std::{
	io::Write,
	path::{Path, PathBuf},
	sync::{Arc, LazyLock},
};

use bpaf::Bpaf;
use const_format::concatcp;
use file_closer::stop_file_closer_thread;
use indexer::{FileIndex, FileIndexItem};
mod deep_readdir;
mod file_closer;
mod indexer;
mod multi_thread_iter;
const VERSION_INFO: &'static str = concatcp!(
	env!("CARGO_PKG_NAME"),
	" ",
	env!("BUILD_VERSION"),
	"; rustc ",
	env!("RUSTC_VERSION"),
	"; build-date-time ",
	env!("BUILD_DATETIME"),
	"; build-feature ",
	env!("BUILD_FEATURE"),
	"; build-profile ",
	env!("BUILD_PROFILE"),
	"; build-target ",
	env!("BUILD_TARGET"),
	"; build-target-feature ",
	env!("BUILD_TARGET_FEATURE"),
);

// Would have loved to use Cow, but bpaf doesn't like that
pub fn space_seperation<'a>(mut input: &'a str) -> Vec<String> {
	input = input.trim();
	if input == "help" {
		return vec!["--help".into()];
	}
	enum ThingState {
		Normal,
		Quote,
		StringEnd,
		Escape(bool),
	}
	let mut escape_str = String::new();
	let mut state = ThingState::StringEnd;
	let mut result = Vec::new();
	let mut start_index = 0;
	for (i, c) in input.char_indices() {
		match state {
			ThingState::Normal => match c {
				' ' => {
					if escape_str.is_empty() {
						result.push(input[start_index..i].into());
					} else {
						result.push(std::mem::take(&mut escape_str));
					}
					state = ThingState::StringEnd;
				},
				'\\' => {
					state = ThingState::Escape(false);
				},
				_ => {
					if !escape_str.is_empty() {
						escape_str.push(c);
					}
				},
			},
			ThingState::Quote => match c {
				'"' => {
					if escape_str.is_empty() {
						result.push(input[start_index..i].into());
					} else {
						result.push(std::mem::take(&mut escape_str));
					}
					state = ThingState::StringEnd;
				},
				'\\' => {
					state = ThingState::Escape(true);
				},
				_ => {
					if !escape_str.is_empty() {
						escape_str.push(c);
					}
				},
			},
			ThingState::StringEnd => match c {
				' ' => {},
				'"' => {
					start_index = i + 1;
					state = ThingState::Quote;
				},
				_ => {
					start_index = i;
					state = ThingState::Normal;
				},
			},
			ThingState::Escape(escaping_in_quote) => {
				if escape_str.is_empty() {
					escape_str.push_str(&input[start_index..(i - 1)]);
				}
				escape_str.push(c);
				if escaping_in_quote {
					state = ThingState::Quote;
				} else {
					state = ThingState::Normal;
				}
			},
		}
	}
	match state {
		ThingState::Normal => {
			if escape_str.is_empty() {
				result.push(input[start_index..].into());
			} else {
				result.push(std::mem::take(&mut escape_str));
			}
		},
		ThingState::Quote => {
			eprintln!("WARNING: Command had a quote which didn't end!");
			if escape_str.is_empty() {
				result.push(input[start_index..].into());
			} else {
				result.push(std::mem::take(&mut escape_str));
			}
		},
		_ => {},
	}
	result
}

#[derive(Debug, Clone, Bpaf)]
#[bpaf(options, version(VERSION_INFO))]
pub struct InvokeArgs {
	// Number of threads to use during indexing. Defaults to the number of CPU threads the system reports.
	#[bpaf(argument("COUNT"), short, long, fallback(num_cpus::get()))]
	jobs: usize,
	// Paths to traverse
	#[bpaf(positional("PATH"))]
	path: Vec<PathBuf>,
}

#[derive(Debug, Clone, Bpaf)]
#[bpaf(options)]
pub enum Commands {
	#[bpaf(command)]
	/// Hello world!!!
	HelloWorld,
	#[bpaf(command)]
	/// The test command
	Test {
		#[bpaf(short, long)]
		argument: String,
	},
	#[bpaf(command)]
	Ls {
		#[bpaf(short, long)]
		recursive: bool,
		#[bpaf(short, long)]
		duplicates: bool,
	},
	#[bpaf(command)]
	Info {
		#[bpaf(positional("FILE"))]
		file: PathBuf,
	},
	#[bpaf(command)]
	Cd {
		#[bpaf(positional("DIR"))]
		dir: PathBuf,
	},
	#[bpaf(command)]
	// Removes all empty directories within....
	Rmedir {
		#[bpaf(positional("DIR"))]
		dir: PathBuf,
	},
	#[bpaf(command)]
	// Removes files from all other folders which are duplicates of any files within this folder
	Rmodupes {
		#[bpaf(positional("DIR"))]
		dir: PathBuf,
	},
	#[bpaf(command)]
	// Removes all duplicates within the specified folder, keeping the one with the shortest path
	Rmdupes {
		#[bpaf(positional("DIR"))]
		dir: PathBuf,
	},
	#[bpaf(command)]
	/// Prints version info
	Version,
	#[bpaf(command)]
	/// Exit
	Quit,
}

static THREADS: LazyLock<usize> = LazyLock::new(|| invoke_args().run().jobs);
static PATHS: LazyLock<Vec<PathBuf>> = LazyLock::new(|| invoke_args().run().path);
fn main() -> anyhow::Result<()> {
	if PATHS.is_empty() {
		anyhow::bail!("Needs at least one path")
	}
	println!("Creating index with {} threads...", *THREADS);
	let mut index = FileIndex::default();
	let mut virtual_root_contents: Vec<Arc<Path>> = Vec::new();
	for path in PATHS.iter() {
		let path = path.canonicalize()?;
		index.extend(FileIndex::from_folder(path.as_path().into())?);
		virtual_root_contents.push(path.as_path().into());
	}
	stop_file_closer_thread();
	index.paths_to_items.insert(
		PathBuf::from(":root").as_path().into(),
		FileIndexItem::Folder {
			contents: virtual_root_contents,
		},
	);

	let mut cwd = PathBuf::from(":root");
	let mut input = String::new();
	loop {
		input.clear();
		let cwd_as_path: Arc<Path> = cwd.as_path().into();
		if !index.paths_to_items.contains_key(&cwd_as_path) {
			println!("Going back to :root cuz the requested folder hasn't been explored.");
			cwd = PathBuf::from(":root");
			continue;
		}
		print!("fdupe {} > ", cwd.to_string_lossy());
		std::io::stdout().flush()?;
		std::io::stdin().read_line(&mut input)?;
		match commands().run_inner(space_seperation(&input).as_slice()) {
			Ok(command) => match command {
				Commands::HelloWorld => {
					println!("The hellowrold command!")
				},
				Commands::Test { argument } => {
					println!("test command {argument}")
				},
				Commands::Version => {
					println!("{VERSION_INFO}");
				},
				Commands::Quit => {
					println!("quit!");
					break;
				},
				Commands::Ls { duplicates, recursive } => {
					if recursive {
						for (file_path, file_item) in index.paths_to_items.iter() {
							let file_path_str = file_path.to_string_lossy();
							match file_item {
								FileIndexItem::File { hash } => {
									let dupe_count = index.file_instance_count(hash);
									if !duplicates || dupe_count > 1 {
										println!("F({}) {file_path_str}", index.file_instance_count(hash));
									}
								},
								FileIndexItem::Folder { contents } => {
									if !duplicates {
										println!("D({}) {file_path_str}", contents.len());
									}
								},
							}
						}
					} else {
						match index.paths_to_items.get(&cwd_as_path) {
							Some(FileIndexItem::Folder { contents }) => {
								for file_path in contents.iter() {
									let Some(file_item) = index.paths_to_items.get(file_path) else {
										continue;
									};
									let file_path_str = if cwd.to_string_lossy() == ":root" {
										file_path.to_string_lossy()
									} else {
										file_path.file_name().unwrap_or_default().to_string_lossy()
									};
									match file_item {
										FileIndexItem::File { hash } => {
											let dupe_count = index.file_instance_count(hash);
											if !duplicates || dupe_count > 1 {
												println!("F({}) {file_path_str}", index.file_instance_count(hash));
											}
										},
										FileIndexItem::Folder { contents } => {
											if !duplicates {
												println!("D({}) {file_path_str}", contents.len());
											}
										},
									}
								}
							},
							_ => {
								println!("Current directory isn't a directory? Curious.");
							},
						}
					}
				},
				Commands::Info { file } => {
					let full_path: Arc<Path> = cwd.join(&file).as_path().into();
					match index.paths_to_items.get(&full_path) {
						Some(item) => {
							println!("# Information about {}:", full_path.to_string_lossy());
							match item {
								FileIndexItem::File { hash } => {
									let mut dupes = index.hash_to_paths.get(hash).cloned().unwrap_or_default();
									dupes.remove(&full_path);
									println!("File with {} duplicates", dupes.len());
									for dupe in dupes {
										println!(" -  {}", dupe.to_string_lossy());
									}
								},
								FileIndexItem::Folder { contents } => {
									println!(
										"Directory with {} items. enter \"cd {}\" to view",
										contents.len(),
										file.to_string_lossy()
									)
								},
							}
						},
						None => {
							println!("{}: No such file or directory", full_path.to_string_lossy())
						},
					}
				},
				Commands::Cd { dir } => {
					if dir.to_string_lossy() == ".." {
						cwd.pop();
					} else {
						let new_dir = cwd.join(dir);
						if index.paths_to_items.get(new_dir.as_path().into()).is_some() {
							cwd = new_dir;
						} else {
							println!("{}: No such file or directory", new_dir.to_string_lossy())
						}
					}
				},
				Commands::Rmedir { dir } => {
					let new_dir = cwd.join(dir);
					println!(
						"Confirm (y/N) removal of empty directories within {}",
						new_dir.to_string_lossy()
					);
					input.clear();
					std::io::stdin().read_line(&mut input)?;
					if input.chars().next().map(|c: char| char::to_ascii_lowercase(&c)) != Some('y') {
						continue;
					}
					index.remove_empty_directories(&new_dir)?;
				},
				Commands::Rmodupes { dir } => {
					let new_dir = cwd.join(dir);
					println!(
						"Confirm (y/N) removal of ALL duplicates of files within {} FROM ALL OTHER FOLDERS",
						new_dir.to_string_lossy()
					);
					input.clear();
					std::io::stdin().read_line(&mut input)?;
					if input.chars().next().map(|c: char| char::to_ascii_lowercase(&c)) != Some('y') {
						continue;
					}
					index.remove_dupes_in_other_folders(&new_dir)?;
				},
				Commands::Rmdupes { dir } => {
					let new_dir = cwd.join(dir);
					println!(
						"Confirm (y/N) removal of ALL duplicates of files within {} FROM WITHIN THIS FOLDER",
						new_dir.to_string_lossy()
					);
					input.clear();
					std::io::stdin().read_line(&mut input)?;
					if input.chars().next().map(|c: char| char::to_ascii_lowercase(&c)) != Some('y') {
						continue;
					}
					index.remove_dupes_from_folder(&new_dir)?;
				},
			},
			Err(err) => match err {
				bpaf::ParseFailure::Stdout(msg, _) => println!("{msg}"),
				bpaf::ParseFailure::Completion(_) => {},
				bpaf::ParseFailure::Stderr(msg) => println!("Invalid command: {msg}"),
			},
		}
	}

	Ok(())
}
