use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

/// A file-system change event, normalized across OS-native backends
/// (inotify / fsevents / ReadDirectoryChangesW via the `notify` crate).
#[derive(Debug, Clone, serde::Serialize)]
pub struct FsEvent {
    pub path: PathBuf,
    pub kind: String, // created | modified | removed | renamed | other
}

/// A live recursive watch on one directory.
///
/// The notify handle and its event channel travel together so a watch can be
/// torn down cleanly. There is deliberately no background "keep the watcher
/// alive" thread: the *caller* owns the handle for exactly as long as it wants
/// events, and dropping it stops the OS watch — which also drops the channel's
/// last sender, so a reader blocked on [`FsWatcher::into_parts`]'s receiver
/// wakes on a disconnect and exits.
pub struct FsWatcher {
    watcher: RecommendedWatcher,
    rx: Receiver<FsEvent>,
}

impl FsWatcher {
    /// Split into the notify handle (keep it alive to keep watching; drop it to
    /// stop) and the event receiver (read until it disconnects). Splitting lets
    /// the handle live in one place — e.g. app state — while a thread drains
    /// events, and lets a new watch *replace* an old one cleanly.
    pub fn into_parts(self) -> (RecommendedWatcher, Receiver<FsEvent>) {
        (self.watcher, self.rx)
    }
}

/// Start a recursive watcher on `path`. Returns a handle that stays live as
/// long as it (or its notify half) is held.
pub fn watch(path: &Path) -> notify::Result<FsWatcher> {
    let (tx, rx) = channel::<FsEvent>();

    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                for p in event.paths {
                    let kind = match event.kind {
                        notify::EventKind::Create(_) => "created".to_string(),
                        notify::EventKind::Modify(_) => "modified".to_string(),
                        notify::EventKind::Remove(_) => "removed".to_string(),
                        notify::EventKind::Any | notify::EventKind::Other => "other".to_string(),
                        notify::EventKind::Access(_) => continue,
                    };
                    // Receiver gone (the watch was replaced) — nothing to do.
                    let _ = tx.send(FsEvent { path: p, kind });
                }
            }
        },
        Config::default().with_poll_interval(Duration::from_secs(2)),
    )?;

    watcher.watch(path, RecursiveMode::Recursive)?;

    Ok(FsWatcher { watcher, rx })
}
