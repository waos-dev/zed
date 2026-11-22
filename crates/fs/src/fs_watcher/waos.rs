//! WaOS port: the native watcher of a WASI component, over WaOS's `file-watching`, in place of
//! notify's, which has no backend on WASI (its poll watcher needs a thread).
//!
//! A watch is of a folder the program holds, opened from the preopened root at the path Zed
//! watches; WaOS tells changes by paths relative to that folder, which are joined back onto it so
//! the events read as notify's would. WaOS watches folders alone, so a file Zed watches, its
//! `settings.json` or `~/.gitconfig`, is watched through its folder, not recursively, with only
//! that file's changes told, as notify's inotify backend watches a file. WaOS watches recursively on every system, inotify's folder
//! by folder work included, so Zed registers one watch per root (`OsWatcherKind::is_recursive`).
//!
//! `file-watching.watch` is asynchronous and `WatchBackend::watch` is not: each watch is started
//! in a task of its own, which reports a failure to start through the event sink, as notify
//! reports its backend's errors, rather than from `watch`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::FutureExt as _;
use futures::channel::oneshot;
use notify::event::{CreateKind, Flag, ModifyKind, RemoveKind, RenameMode};
use notify::{Event, EventKind};
use wasdk::file_watching::{self, Change};
use wasdk::filesystem::{Descriptor, DescriptorFlags, OpenFlags, PathFlags};

use super::WatchBackend;

/// How long WaOS gathers a batch after its first change: the worktree's own latency.
const LATENCY_MS: u32 = 100;

type EventSink = Arc<dyn Fn(notify::Result<Event>) + Send + Sync>;

pub(crate) struct WaosWatcher {
    sink: EventSink,
    /// Each watched folder's task, which ends its watch when this end is dropped.
    watches: HashMap<PathBuf, oneshot::Sender<()>>,
}

impl WaosWatcher {
    pub(crate) fn new(sink: impl Fn(notify::Result<Event>) + Send + Sync + 'static) -> Self {
        Self {
            sink: Arc::new(sink),
            watches: HashMap::new(),
        }
    }
}

impl WatchBackend for WaosWatcher {
    fn watch(&mut self, path: &Path, mode: notify::RecursiveMode) -> notify::Result<()> {
        let (stop, stopped) = oneshot::channel();
        let recursive = matches!(mode, notify::RecursiveMode::Recursive);
        wasdk::spawn(report_changes(
            Watched::at(path),
            recursive,
            self.sink.clone(),
            stopped,
        ));
        self.watches.insert(path.to_owned(), stop);
        Ok(())
    }

    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        match self.watches.remove(path) {
            Some(_stop) => Ok(()),
            None => Err(notify::Error::watch_not_found().add_path(path.to_owned())),
        }
    }
}

/// What a watch of a path watches: the folder WaOS watches, and the one file in it whose changes
/// are told, where the path is a file's.
struct Watched {
    folder: PathBuf,
    file: Option<OsString>,
}

impl Watched {
    /// The watch of `path`: its folder, where it is a file's, or itself; a path that cannot be
    /// read is watched as a folder, so the watch says why it cannot be.
    fn at(path: &Path) -> Watched {
        let is_file = std::fs::metadata(path).is_ok_and(|metadata| !metadata.is_dir());
        match (is_file, path.parent(), path.file_name()) {
            (true, Some(folder), Some(file)) => Watched {
                folder: folder.to_owned(),
                file: Some(file.to_owned()),
            },
            _ => Watched {
                folder: path.to_owned(),
                file: None,
            },
        }
    }

    /// Whether `change` is told: any in a watched folder; in a file's folder, the file's own and
    /// a rescan of the folder.
    fn tells(&self, change: &Change) -> bool {
        let Some(file) = &self.file else {
            return true;
        };
        let is_file = |relative: &str| Path::new(relative).as_os_str() == file;
        match change {
            Change::Created(path)
            | Change::Modified(path)
            | Change::Removed(path)
            | Change::Moved(path)
            | Change::Other(path) => is_file(path),
            Change::Renamed((from, to)) => is_file(from) || is_file(to),
            Change::Rescan(_) => true,
        }
    }
}

/// Watches what `watched` names and reports its changes to `sink`, until `stopped`.
async fn report_changes(
    watched: Watched,
    recursive: bool,
    sink: EventSink,
    stopped: oneshot::Receiver<()>,
) {
    let folder = watched.folder.clone();
    let recursive = recursive && watched.file.is_none();
    let watching = async {
        let descriptor = open_folder(&folder).await?;
        let mut batches = file_watching::watch(&descriptor, recursive, LATENCY_MS)
            .await
            .map_err(|error| format!("WaOS would not watch it: {error:?}"))?;
        log::info!(
            "WaOS watches {folder:?}, recursive: {recursive}, only {:?}",
            watched.file
        );
        while let Some(batch) = batches.next().await {
            log::debug!("WaOS tells {} changes: {batch:?}", batch.len());
            for change in batch.into_iter().filter(|change| watched.tells(change)) {
                sink(Ok(event_of(&folder, change)));
            }
        }
        Ok::<(), String>(())
    };
    futures::select! {
        watched = watching.fuse() => if let Err(error) = watched {
            sink(Err(notify::Error::generic(&error).add_path(folder)));
        },
        _ = stopped.fuse() => {}
    }
}

/// The folder at `path`, opened from the root the program is given, as `std::fs` opens it.
async fn open_folder(path: &Path) -> Result<Descriptor, String> {
    let root = wasdk::preopens::get_directories()
        .into_iter()
        .find_map(|(folder, at)| (at == "/").then_some(folder))
        .ok_or("no root is preopened")?;
    let relative = path.strip_prefix("/").unwrap_or(path);
    root.open_at(
        PathFlags::empty(),
        relative.to_string_lossy().into_owned(),
        OpenFlags::DIRECTORY,
        DescriptorFlags::READ,
    )
    .await
    .map_err(|error| format!("it did not open: {error:?}"))
}

/// The notify event of a change WaOS tells in `folder`.
fn event_of(folder: &Path, change: Change) -> Event {
    let at = |relative: &str| match relative {
        "." => folder.to_owned(),
        relative => folder.join(relative),
    };
    match change {
        Change::Created(path) => Event::new(EventKind::Create(CreateKind::Any)).add_path(at(&path)),
        Change::Modified(path) => Event::new(EventKind::Modify(ModifyKind::Any)).add_path(at(&path)),
        Change::Removed(path) => Event::new(EventKind::Remove(RemoveKind::Any)).add_path(at(&path)),
        Change::Renamed((from, to)) => {
            Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
                .add_path(at(&from))
                .add_path(at(&to))
        }
        // An end of a rename the system did not pair, as notify's own FSEvents backend reports it
        // (`Modify(Name(Any))`); WaOS tells it as `moved` since 1.0.0.
        Change::Moved(path) => {
            Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Any))).add_path(at(&path))
        }
        // "Something changed here", the system having not said what: notify's own `EventKind::Any`.
        Change::Other(path) => Event::new(EventKind::Any).add_path(at(&path)),
        Change::Rescan(path) => Event::new(EventKind::Other)
            .add_path(at(path.as_deref().unwrap_or(".")))
            .set_flag(Flag::Rescan),
    }
}
