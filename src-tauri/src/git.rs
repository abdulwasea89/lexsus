use std::path::Path;

/// A single changed file with its diff (used by the git panel).
#[derive(Debug, Clone, serde::Serialize)]
pub struct GitFileStatus {
    pub path: String,
    pub status: String,
    // untracked | modified | staged | deleted | renamed
    pub additions: usize,
    pub deletions: usize,
}

/// Open a git repository rooted at `path`.
pub fn open_repo(path: &Path) -> Result<git2::Repository, git2::Error> {
    git2::Repository::open(path)
}

/// Return the current branch name, if any.
pub fn current_branch(repo: &git2::Repository) -> Option<String> {
    repo.head()
        .ok()
        .and_then(|h| h.shorthand().map(|s| s.to_string()))
}

/// Collect per-file status of the working tree (status, diff line counts).
pub fn status(repo: &git2::Repository) -> Result<Vec<GitFileStatus>, git2::Error> {
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    let statuses = repo.statuses(Some(&mut options))?;
    let mut out = Vec::new();
    for entry in statuses.iter() {
        let path = match entry.path() {
            Some(p) => p.to_string(),
            None => continue,
        };
        let status = entry.status();
        let kind = if status.contains(git2::Status::WT_NEW) {
            "untracked".to_string()
        } else if status.contains(git2::Status::INDEX_NEW) {
            "staged".to_string()
        } else if status.contains(git2::Status::WT_DELETED)
            || status.contains(git2::Status::INDEX_DELETED)
        {
            "deleted".to_string()
        } else if status.contains(git2::Status::INDEX_RENAMED) {
            "renamed".to_string()
        } else {
            "modified".to_string()
        };
        let (additions, deletions) = diff_counts(repo, &path);
        out.push(GitFileStatus {
            path,
            status: kind,
            additions,
            deletions,
        });
    }
    Ok(out)
}

fn diff_counts(repo: &git2::Repository, path: &str) -> (usize, usize) {
    let mut adds = 0usize;
    let mut dels = 0usize;
    if let Ok(head) = repo.head() {
        let head_commit = match head.peel_to_commit() {
            Ok(c) => c,
            Err(_) => return (0, 0),
        };
        let head_tree = match head_commit.tree() {
            Ok(t) => t,
            Err(_) => return (0, 0),
        };
        let mut opts = git2::DiffOptions::new();
        opts.pathspec(path);
        let diff = repo.diff_tree_to_workdir_with_index(Some(&head_tree), Some(&mut opts));
        if let Ok(diff) = diff {
            for delta in diff.deltas() {
                let f = delta.flags();
                if f.contains(git2::DiffFlags::BINARY) {
                    continue;
                }
            }
            if let Ok(stats) = diff.stats() {
                adds = stats.insertions();
                dels = stats.deletions();
            }
        }
    }
    (adds, dels)
}

/// Create a commit from the current staged state with the given message.
pub fn commit(repo: &git2::Repository, message: &str) -> Result<git2::Oid, git2::Error> {
    let mut index = repo.index()?;
    index.write_tree()?;
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;
    let sig = repo.signature()?;
    let parent = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.id());
    let oid = match parent {
        Some(pid) => {
            let parent_commit = repo.find_commit(pid)?;
            repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent_commit])?
        }
        None => repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[])?,
    };
    Ok(oid)
}

// --- M1.6: full git panel backend -------------------------------------------/// Per-file working-tree diff (patch text + line counts).

#[derive(Debug, Clone, serde::Serialize)]
pub struct FileDiff {
    pub path: String,
    pub status: String,
    pub added: usize,
    pub deleted: usize,
    pub patch: String,
}

/// Diff the working tree (plus index) against HEAD. Untracked files are/// included as pure-additions. Returns one entry per changed file.
pub fn diff_workdir(repo: &git2::Repository) -> Result<Vec<FileDiff>, git2::Error> {
    let head_tree = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .and_then(|c| c.tree().ok());
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let diff = match head_tree {
        Some(tree) => repo.diff_tree_to_workdir_with_index(Some(&tree), Some(&mut opts))?,
        None => repo.diff_index_to_workdir(None, Some(&mut opts))?,
    };
    let mut patches: std::collections::BTreeMap<String, String> = Default::default();
    let mut counts: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    let mut statuses: std::collections::BTreeMap<String, String> = Default::default();
    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        let path = delta
            .old_file()
            .path()
            .or_else(|| delta.new_file().path())
            .map(|p| p.to_string_lossy().into_owned());
        if let Some(path) = path {
            let entry = patches.entry(path.clone()).or_default();
            entry.push(line.origin());
            entry.push_str(&String::from_utf8_lossy(line.content()));

            match line.origin() {
                '+' => counts.entry(path.clone()).or_insert((0, 0)).0 += 1,
                '-' => counts.entry(path.clone()).or_insert((0, 0)).1 += 1,
                _ => {}
            }
            statuses
                .entry(path)
                .or_insert_with(|| match delta.status() {
                    git2::Delta::Added => "untracked".to_string(),
                    git2::Delta::Deleted => "deleted".to_string(),
                    git2::Delta::Renamed => "renamed".to_string(),
                    _ => "modified".to_string(),
                });
        }
        true
    })?;
    Ok(patches
        .into_iter()
        .map(|(path, patch)| {
            let (added, deleted) = counts.remove(&path).unwrap_or((0, 0));
            FileDiff {
                status: statuses
                    .remove(&path)
                    .unwrap_or_else(|| "modified".to_string()),
                path,
                added,
                deleted,
                patch,
            }
        })
        .collect())
}

/// Stage a single file.
pub fn stage(repo: &git2::Repository, path: &str) -> Result<(), git2::Error> {
    let mut index = repo.index()?;
    let p = std::path::Path::new(path);
    // `add_path` refuses a directory — "could not find 'src' to stat" — but
    // "stage this directory" is an ordinary request, and it is what a person
    // means when they name one. `add_all` with the directory as a pathspec is
    // the call git itself makes, and it recurses.
    let is_dir = repo.workdir().map(|w| w.join(p).is_dir()).unwrap_or(false);
    if is_dir {
        index.add_all([p], git2::IndexAddOption::DEFAULT, None)?;
    } else {
        index.add_path(p)?;
    }
    index.write()?;
    Ok(())
}

/// Reset one path's index entry to HEAD's version — what "unstage" means.
///
/// The obvious implementation, `Index::remove_path`, is wrong for a file that
/// is already committed: dropping the entry does not unstage the file, it
/// stages its *deletion* — so "unstage this unchanged file" would quietly
/// queue up its removal. `reset_default` is `git reset HEAD -- <path>`, which
/// restores the committed version for a tracked file and drops the entry for
/// one that exists only in the index. The working tree is untouched either way.
pub fn unstage(repo: &git2::Repository, path: &str) -> Result<(), git2::Error> {
    let p = std::path::Path::new(path);
    match repo.head().and_then(|h| h.peel(git2::ObjectType::Commit)) {
        Ok(head) => repo.reset_default(Some(&head), std::iter::once(p)),
        // Unborn HEAD: there is no committed version to restore, so dropping
        // the entry is the only meaning left.
        Err(_) => {
            let mut index = repo.index()?;
            index.remove_path(p)?;
            index.write()?;
            Ok(())
        }
    }
}

/// Whether `path` has a staged change — i.e. whether unstaging it would do
/// anything at all.
///
/// The comparison is index-versus-HEAD, not "is it in the index". A committed
/// file is in the index, so the weaker question would call it staged and let
/// `git_unstage` report success for a call that changes nothing; and an
/// untracked file is in neither, so the weaker question would call it
/// unstaged for the wrong reason.
pub fn has_staged_change(repo: &git2::Repository, path: &str) -> Result<bool, git2::Error> {
    let p = std::path::Path::new(path);
    // Stage 0 is the ordinary, non-conflicted slot. A path present only at
    // stages 1–3 is mid-merge and has no single version to compare.
    let index_entry = repo.index()?.get_path(p, 0).map(|e| e.id);
    let head_entry = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_tree().ok())
        .and_then(|t| t.get_path(p).ok())
        .map(|e| e.id());
    Ok(index_entry != head_entry)
}

/// Stage everything in the working tree.
pub fn stage_all(repo: &git2::Repository) -> Result<(), git2::Error> {
    let mut index = repo.index()?;
    index.add_all(["*"], git2::IndexAddOption::DEFAULT, None)?;
    index.write()?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub is_current: bool,
}

pub fn branches(repo: &git2::Repository) -> Result<Vec<BranchInfo>, git2::Error> {
    let current = current_branch(repo);
    let mut out = Vec::new();
    let iter = repo.branches(None)?;
    for branch in iter.flatten() {
        let (branch, _kind) = branch;
        if let Some(name) = branch.name().ok().flatten() {
            out.push(BranchInfo {
                name: name.to_string(),
                is_current: current.as_deref() == Some(name),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Paths with uncommitted changes that a checkout would overwrite.
///
/// Split out of [`checkout`] so the *tool* layer can report this as a coded
/// error. It used to arrive as a `git2::Error` whose message happened to start
/// with "cannot switch", which meant a caller deciding whether to retry had to
/// pattern-match English to tell "you have unsaved work" from "no such
/// branch". The check itself is unchanged — only where the answer is decided.
///
/// Untracked files are not dirty: the safe checkout in [`checkout`] still
/// refuses if switching would clobber one.
pub fn dirty_paths(repo: &git2::Repository) -> Result<Vec<String>, git2::Error> {
    let mut status_opts = git2::StatusOptions::new();
    status_opts.include_untracked(false).include_ignored(false);
    Ok(repo
        .statuses(Some(&mut status_opts))?
        .iter()
        .filter_map(|e| e.path().map(|p| p.to_string()))
        .collect())
}

pub fn checkout(repo: &git2::Repository, name: &str) -> Result<(), git2::Error> {
    let refname = format!("refs/heads/{name}");
    let commit = repo.find_commit(repo.refname_to_id(&refname)?)?;
    let tree = commit.tree()?;

    // Roadmap invariant: git_checkout must refuse on a dirty tree. A force
    // checkout would silently overwrite staged/unstaged tracked changes, so
    // refuse while any exist.
    let dirty = dirty_paths(repo)?;
    if !dirty.is_empty() {
        let mut msg = format!(
            "cannot switch to '{name}': {} uncommitted change(s) — commit or stash before switching ({})",
            dirty.len(),
            dirty[0]
        );
        for path in dirty.iter().skip(1).take(2) {
            msg.push_str(", ");
            msg.push_str(path);
        }
        if dirty.len() > 3 {
            msg.push_str(&format!(", and {} more", dirty.len() - 3));
        }
        msg.push(')');
        return Err(git2::Error::from_str(&msg));
    }

    // Safe checkout: the tree is clean so nothing tracked can be lost, and
    // without force() libgit2 refuses to overwrite an untracked file that a
    // target branch happens to contain.
    let mut co = git2::build::CheckoutBuilder::new();
    repo.checkout_tree(tree.as_object(), Some(&mut co))?;
    repo.set_head(&refname)?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CommitInfo {
    pub oid: String,
    pub message: String,
    pub author: String,
    pub timestamp: i64,
}

pub fn log(repo: &git2::Repository, limit: usize) -> Result<Vec<CommitInfo>, git2::Error> {
    let mut walk = repo.revwalk()?;
    walk.push_head()?;
    walk.set_sorting(git2::Sort::TIME)?;
    let mut out = Vec::new();
    for oid in walk.take(limit) {
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
        out.push(CommitInfo {
            oid: oid.to_string(),
            message: commit.summary().unwrap_or("").to_string(),
            author: commit.author().name().unwrap_or("").to_string(),
            timestamp: commit.time().seconds(),
        });
    }
    Ok(out)
}

/// Full patch text of a single commit (against its parent; root commits/// diff against the empty tree).
/// Resolve a revision to a commit.
///
/// A caller names a commit the way a person does — a full id, an abbreviated
/// one, `HEAD`, `HEAD~2`, a tag, a branch — and `Oid::from_str` accepts only
/// the first. Revision parsing is what the caller means by "commit id", so it
/// happens here rather than being pushed onto every call site.
fn resolve_commit<'r>(
    repo: &'r git2::Repository,
    rev: &str,
) -> Result<git2::Commit<'r>, git2::Error> {
    repo.revparse_single(rev)
        .and_then(|o| o.peel_to_commit())
        .map_err(|e| git2::Error::from_str(&format!("no such commit {rev:?}: {e}")))
}

/// Whether the index differs from HEAD — i.e. whether a commit would record
/// anything at all.
///
/// Committing with nothing staged writes an empty commit: legal in git,
/// essentially never what was meant, and tedious to undo. The tool layer uses
/// this to refuse instead.
pub fn has_staged_changes(repo: &git2::Repository) -> Result<bool, git2::Error> {
    let index_tree = repo.index()?.write_tree()?;
    let head_tree = repo.head().and_then(|h| h.peel_to_tree()).map(|t| t.id());
    Ok(head_tree.ok() != Some(index_tree))
}

pub fn commit_diff(repo: &git2::Repository, oid: &str) -> Result<String, git2::Error> {
    let commit = resolve_commit(repo, oid)?;
    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
    let tree = commit.tree()?;
    let diff = match parent_tree {
        Some(pt) => repo.diff_tree_to_tree(Some(&pt), Some(&tree), None)?,
        None => repo.diff_tree_to_tree(None, Some(&tree), None)?,
    };
    let mut out = String::new();
    diff.print(git2::DiffFormat::Patch, |_d, _h, line| {
        out.push(line.origin());
        out.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;
    Ok(out)
}

/// Create a branch, optionally from a given revision, optionally checking it
/// out.
///
/// `checkout` only runs once the branch exists, and uses the same safe
/// checkout as [`checkout`] — no force, so an untracked file the branch would
/// clobber stops the switch rather than being overwritten.
pub fn create_branch(
    repo: &git2::Repository,
    name: &str,
    from: Option<&str>,
    checkout: bool,
) -> Result<(), git2::Error> {
    let start = match from {
        Some(rev) => repo.revparse_single(rev)?.peel_to_commit()?,
        None => repo
            .head()
            .and_then(|h| h.peel_to_commit())
            // An unborn HEAD means no commit exists to branch from. Report it
            // rather than inventing an empty tree, which would produce a
            // branch that cannot be committed to.
            .map_err(|e| git2::Error::from_str(&format!("no commit to branch from: {e}")))?,
    };
    let branch = repo.branch(name, &start, false)?;
    if checkout {
        let refname = branch
            .get()
            .name()
            .ok_or_else(|| git2::Error::from_str("branch has a non-UTF-8 name"))?
            .to_string();
        repo.checkout_tree(start.as_object(), None)?;
        repo.set_head(&refname)?;
    }
    Ok(())
}

/// Everything about one commit: who, when, what, and its patch.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CommitDetail {
    pub oid: String,
    pub summary: String,
    pub message: String,
    pub author: String,
    pub email: Option<String>,
    pub timestamp: i64,
    pub patch: String,
}

/// One commit in full. `patch` is the same text [`commit_diff`] returns.
pub fn show(repo: &git2::Repository, oid: &str) -> Result<CommitDetail, git2::Error> {
    let commit = resolve_commit(repo, oid)?;
    let author = commit.author();
    Ok(CommitDetail {
        oid: commit.id().to_string(),
        summary: commit.summary().unwrap_or("").to_string(),
        message: commit.message().unwrap_or("").to_string(),
        author: author.name().unwrap_or("").to_string(),
        email: author.email().map(str::to_string),
        timestamp: commit.time().seconds(),
        patch: commit_diff(repo, oid)?,
    })
}

// --- worktrees ---------------------------------------------------------------

/// A git worktree this session created.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WorktreeInfo {
    /// Absolute path of the worktree's working directory.
    pub path: String,
    /// The branch checked out in it.
    pub branch: String,
}

/// Where worktrees live, relative to the repository root.
///
/// Under `.lexsus/` rather than a sibling directory so the whole thing is one
/// directory to ignore and one directory to delete. It is also inside the
/// repo, which means a workspace-wide `grep` would otherwise walk it — the
/// search tools already skip `.git`, and the caller is expected to add
/// `.lexsus/` to `.gitignore` (the worktree itself is a checkout, not
/// something to commit).
pub const WORKTREE_DIR: &str = ".lexsus/worktrees";

/// Create a worktree at `<root>/.lexsus/worktrees/<name>` on a new branch
/// `<name>` based on HEAD.
///
/// The branch is created here rather than by `git worktree add -b` because
/// `git2` exposes exactly those two steps. If the worktree directory already
/// exists, or the branch already exists, this fails rather than reusing
/// either — a name collision is a caller that lost track of an earlier
/// worktree, and silently adopting it would make `exit_worktree` ambiguous.
pub fn add_worktree(
    repo: &git2::Repository,
    root: &Path,
    name: &str,
) -> Result<WorktreeInfo, git2::Error> {
    let head = repo.head()?.peel_to_commit()?;
    let branch = repo.branch(name, &head, false)?;
    let reference = branch.into_reference();
    let path = root.join(WORKTREE_DIR).join(name);
    // libgit2 will not create missing parents, and the first worktree in a
    // fresh repo is exactly that case.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| git2::Error::from_str(&e.to_string()))?;
    }
    let mut opts = git2::WorktreeAddOptions::new();
    opts.reference(Some(&reference));
    match repo.worktree(name, &path, Some(&mut opts)) {
        Ok(_) => Ok(WorktreeInfo {
            path: path.to_string_lossy().into_owned(),
            branch: name.to_string(),
        }),
        Err(e) => {
            // Roll the branch back so a failed add does not leave a dangling
            // branch that makes the next attempt fail for the wrong reason.
            if let Ok(mut b) = repo.find_branch(name, git2::BranchType::Local) {
                if b.delete().is_ok() {
                    // best effort
                }
            }
            Err(e)
        }
    }
}

/// Remove a worktree and its branch, and delete its directory.
///
/// `discard` must be true to remove a worktree with uncommitted changes; the
/// caller checks that, and this function only performs the removal. The
/// directory is deleted first, then the administrative entry is pruned, so a
/// partially-removed worktree cannot be left half-registered.
pub fn remove_worktree(
    repo: &git2::Repository,
    name: &str,
    discard: bool,
) -> Result<(), git2::Error> {
    let wt = repo.find_worktree(name)?;
    let path = wt.path().to_path_buf();
    if path.exists() {
        if !discard {
            return Err(git2::Error::from_str(
                "worktree has uncommitted changes; pass action=discard to remove it",
            ));
        }
        std::fs::remove_dir_all(&path).map_err(|e| git2::Error::from_str(&e.to_string()))?;
    }
    let mut opts = git2::WorktreePruneOptions::new();
    opts.valid(true).working_tree(true);
    wt.prune(Some(&mut opts))?;
    // The branch created alongside the worktree goes with it; a worktree
    // whose branch outlives it is a name collision waiting to happen.
    if let Ok(mut b) = repo.find_branch(name, git2::BranchType::Local) {
        let _ = b.delete();
    }
    Ok(())
}

/// Whether the worktree named `name` has any uncommitted change.
///
/// Opens the worktree as its own repository, so the answer is about that
/// tree rather than about the one the process happens to be standing in.
pub fn worktree_dirty(repo: &git2::Repository, name: &str) -> Result<bool, git2::Error> {
    let wt = repo.find_worktree(name)?;
    let path = wt.path().to_path_buf();
    if !path.exists() {
        return Ok(false);
    }
    let wt_repo = git2::Repository::open(&path)?;
    let mut status_opts = git2::StatusOptions::new();
    // Unlike `dirty_paths`, untracked files count: a worktree whose only
    // content is a new file is not "clean" in the sense that matters here.
    status_opts.include_untracked(true).include_ignored(false);
    Ok(!wt_repo.statuses(Some(&mut status_opts))?.is_empty())
}
