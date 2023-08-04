use std::{fs, path::Path, process::Command, sync::Arc};

use anyhow::*;
use futures_util::future::{BoxFuture, FutureExt};
use indicatif::{ProgressBar, ProgressStyle};
use tokio::sync::Mutex;

pub mod command_helper;
pub mod db_conn;
pub mod media_resize;
pub mod telemetry;

pub fn progress_bar(len: usize) -> ProgressBar {
	let pb = ProgressBar::new(len as u64);
	pb.set_style(
		ProgressStyle::default_bar()
			.template("{spinner} [{elapsed_precise}] {bar:40} ({pos}/{len}) {wide_msg}"),
	);
	pb.enable_steady_tick(250);
	pb
}

pub async fn join_set_progress(mut join_set: tokio::task::JoinSet<Result<()>>) -> Result<()> {
	// Run progress bar
	let pb = progress_bar(join_set.len());
	let mut errors = Vec::new();
	while let Some(res) = join_set.join_next().await {
		let res = res?;
		match res {
			Result::Ok(_) => {}
			Result::Err(err) => {
				errors.push(err);
			}
		}
		pb.inc(1);
	}
	pb.finish();

	// Log all errors
	for err in &errors {
		rivet_term::status::error("Error", &err);
	}

	// Return error
	if let Some(err) = errors.into_iter().next() {
		Err(err)
	} else {
		Ok(())
	}
}

#[derive(Clone)]
pub struct MultiProgress {
	progress_bar: ProgressBar,
	running: Arc<Mutex<Vec<String>>>,
}

impl MultiProgress {
	pub fn new(len: usize) -> MultiProgress {
		MultiProgress {
			progress_bar: progress_bar(len),
			running: Arc::new(Mutex::new(Vec::new())),
		}
	}

	pub async fn insert(&self, name: &str) {
		let mut running = self.running.lock().await;
		running.push(name.to_owned());
		self.update(&*running);
	}

	pub async fn remove(&self, name: &str) {
		let mut running = self.running.lock().await;
		running.retain(|n| n != name);
		self.progress_bar.inc(1);
		self.update(&*running);
	}

	pub fn finish(&self) {
		self.progress_bar.finish_with_message("");
	}

	fn update(&self, running: &Vec<String>) {
		self.progress_bar.set_message(running.join(", "));
	}
}

/// Returns the modified timestamp of all files recursively.
pub fn deep_modified_ts(path: &Path) -> Result<u128> {
	let mut max_modified_ts = 0;
	deep_modified_ts_inner(path, &mut max_modified_ts)?;
	Ok(max_modified_ts)
}

fn deep_modified_ts_inner(path: &Path, max_modified_ts: &mut u128) -> Result<()> {
	for entry in fs::read_dir(path)? {
		let entry = entry?;
		let file_name = entry.file_name();
		let file_name = file_name.to_str().unwrap();
		let file_type = entry.file_type()?;

		// Skip non-source files
		if file_name.starts_with(".")
			|| file_name == "node_modules"
			|| file_name == "target"
			|| file_name == "dist"
		{
			continue;
		}

		// Recurse
		if file_type.is_dir() {
			deep_modified_ts_inner(&path.join(entry.path()), max_modified_ts)?;
		}

		// Check if file is newer
		if file_type.is_file() {
			let meta = entry.metadata()?;
			let modified_ts = meta
				.modified()?
				.duration_since(std::time::UNIX_EPOCH)?
				.as_millis();
			if modified_ts > *max_modified_ts {
				*max_modified_ts = modified_ts;
			}
		}
	}

	Ok(())
}

pub fn ringadingding() {
	#[cfg(unix)]
	{
		print!("\x07");
	}
}

const GET_GIT_BRANCH: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();

pub async fn get_git_branch() -> String {
	GET_GIT_BRANCH
		.get_or_init(|| async {
			let git_cmd = Command::new("git")
				.arg("rev-parse")
				.arg("--abbrev-ref")
				.arg("HEAD")
				.output()
				.unwrap();
			assert!(git_cmd.status.success());
			String::from_utf8(git_cmd.stdout)
				.unwrap()
				.trim()
				.to_string()
		})
		.await
		.clone()
}

const GET_GIT_COMMIT: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();

pub async fn get_git_commit() -> String {
	GET_GIT_COMMIT
		.get_or_init(|| async {
			let git_cmd = Command::new("git")
				.arg("rev-parse")
				.arg("HEAD")
				.output()
				.unwrap();
			assert!(git_cmd.status.success());
			String::from_utf8(git_cmd.stdout)
				.unwrap()
				.trim()
				.to_string()
		})
		.await
		.clone()
}

pub fn copy_dir_all<'a>(src: &'a Path, dst: &'a Path) -> BoxFuture<'a, tokio::io::Result<()>> {
	async move {
		tokio::fs::create_dir_all(&dst).await?;

		let mut dir = tokio::fs::read_dir(src).await?;
		while let Some(entry) = dir.next_entry().await? {
			if tokio::fs::read_dir(entry.path()).await.is_ok() {
				copy_dir_all(&entry.path(), &dst.join(entry.file_name())).await?;
			} else {
				tokio::fs::copy(entry.path(), dst.join(entry.file_name())).await?;
			}
		}

		tokio::io::Result::Ok(())
	}
	.boxed()
}

pub fn pick_port() -> u16 {
	portpicker::pick_unused_port().expect("no free ports")
}
