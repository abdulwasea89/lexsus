use std::path::Path;

/// Render a git timestamp as `YYYY-MM-DD HH:MM:SS` in the author's local
/// offset (`seconds` since epoch, `offset_minutes` east of UTC). Uses the
/// standard civil-from-days algorithm so no date crate is needed.
fn format_utc(seconds: i64, offset_minutes: i32) -> String {
    let local = seconds + offset_minutes as i64 * 60;
    let days = local.div_euclid(86_400);
    let rem = local.rem_euclid(86_400);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let yr = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dd = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let yr = if mo <= 2 { yr + 1 } else { yr };
    format!("{yr:04}-{mo:02}-{dd:02} {hh:02}:{mm:02}:{ss:02}")
}

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
    index.add_path(std::path::Path::new(path))?;
    index.write()?;
    Ok(())
}

/// Unstage a single file (index only; working tree untouched).
pub fn unstage(repo: &git2::Repository, path: &str) -> Result<(), git2::Error> {
    let mut index = repo.index()?;
    index.remove_path(std::path::Path::new(path))?;
    index.write()?;
    Ok(())
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

pub fn checkout(repo: &git2::Repository, name: &str) -> Result<(), git2::Error> {
    let refname = format!("refs/heads/{name}");
    let commit = repo.find_commit(repo.refname_to_id(&refname)?)?;
    let tree = commit.tree()?;

    // Roadmap invariant: git_checkout must refuse on a dirty tree. A force
    // checkout would silently overwrite staged/unstaged tracked changes, so
    // refuse while any exist. (Untracked files alone don't block — the safe
    // checkout below still refuses if switching would clobber one.)
    let mut status_opts = git2::StatusOptions::new();
    status_opts.include_untracked(false).include_ignored(false);
    let dirty: Vec<String> = repo
        .statuses(Some(&mut status_opts))?
        .iter()
        .filter_map(|e| e.path().map(|p| p.to_string()))
        .collect();
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

/// Create a new branch at the current HEAD, without switching to it.
pub fn create_branch(repo: &git2::Repository, name: &str) -> Result<(), git2::Error> {
    if repo.find_branch(name, git2::BranchType::Local).is_ok() {
        return Err(git2::Error::from_str(&format!(
            "branch already exists: {name}"
        )));
    }
    let head = repo.head()?;
    let commit = head.peel_to_commit()?;
    repo.branch(name, &commit, false)?;
    Ok(())
}

/// Whether the index differs from HEAD — i.e. a commit would not be empty.
/// On a repo with no commits yet, any staged entry counts as a change.
pub fn has_staged_changes(repo: &git2::Repository) -> Result<bool, git2::Error> {
    let mut index = repo.index()?;
    let tree_oid = index.write_tree()?;
    // The well-known empty-tree object id.
    const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    match repo.head().ok().and_then(|h| h.peel_to_commit().ok()) {
        Some(head) => {
            let head_tree = head.tree()?;
            Ok(head_tree.id() != tree_oid)
        }
        None => Ok(tree_oid.to_string() != EMPTY_TREE && !index.is_empty()),
    }
}

/// Render a commit: oid, author, timestamp, message and the full patch
/// against its parent (root commits diff against the empty tree).
pub fn show(repo: &git2::Repository, oid: &str) -> Result<String, git2::Error> {
    let oid = git2::Oid::from_str(oid).map_err(|e| git2::Error::from_str(&e.to_string()))?;
    let commit = repo.find_commit(oid)?;
    let author = commit.author();
    let when = author.when();
    let mut out = format!(
        "commit {}\nAuthor: {}\nDate:   {}\n\n    {}\n",
        commit.id(),
        author
            .name()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
        format_utc(when.seconds(), when.offset_minutes()),
        commit.summary().unwrap_or(""),
    );
    if let Some(body) = commit.message() {
        // summary() strips the first line; append the remainder indented.
        if let Some(rest) = body.strip_prefix(commit.summary().unwrap_or_default()) {
            for line in rest.trim_start_matches('\n').lines() {
                out.push_str(&format!("    {}\n", line.trim_end()));
            }
        }
    }
    out.push('\n');
    // Diff against the parent (or empty tree for a root commit).
    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
    let tree = commit.tree()?;
    let diff = match parent_tree {
        Some(pt) => repo.diff_tree_to_tree(Some(&pt), Some(&tree), None)?,
        None => repo.diff_tree_to_tree(None, Some(&tree), None)?,
    };
    diff.print(git2::DiffFormat::Patch, |_d, _h, line| {
        out.push(line.origin());
        out.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;
    Ok(out)
}

/// Full patch text of a single commit (against its parent; root commits/// diff against the empty tree).
pub fn commit_diff(repo: &git2::Repository, oid: &str) -> Result<String, git2::Error> {
    let commit = repo.find_commit(
        git2::Oid::from_str(oid).map_err(|e| git2::Error::from_str(&e.to_string()))?,
    )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "lexsus-git-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repo = git2::Repository::init(&p).unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "t").unwrap();
        cfg.set_str("user.email", "t@example.com").unwrap();
        drop(repo);
        p
    }

    fn repo(p: &Path) -> git2::Repository {
        git2::Repository::open(p).unwrap()
    }

    fn stage_all(r: &git2::Repository) {
        let mut index = r.index().unwrap();
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
    }

    fn commit(r: &git2::Repository, msg: &str) {
        let sig = r.signature().unwrap();
        stage_all(r);
        let tree = r
            .find_tree(r.index().unwrap().write_tree().unwrap())
            .unwrap();
        let oid = match r.head().ok().and_then(|h| h.peel_to_commit().ok()) {
            Some(p) => r
                .commit(Some("HEAD"), &sig, &sig, msg, &tree, &[&p])
                .unwrap(),
            None => r.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[]).unwrap(),
        };
        assert!(r.find_commit(oid).is_ok());
    }

    #[test]
    fn has_staged_changes_tracks_the_index() {
        let dir = scratch("staged");
        let r = repo(&dir);
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        // Nothing staged yet (file untracked, not added).
        assert!(!has_staged_changes(&r).unwrap());
        stage_all(&r);
        assert!(has_staged_changes(&r).unwrap());
        commit(&r, "initial");
        assert!(!has_staged_changes(&r).unwrap(), "clean after commit");
    }

    #[test]
    fn create_branch_and_refuses_duplicate() {
        let dir = scratch("branch");
        let r = repo(&dir);
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        commit(&r, "initial");
        create_branch(&r, "feat/x").unwrap();
        // A duplicate of an existing local branch is refused.
        assert!(create_branch(&r, "feat/x").is_err());
        // Creating does not switch HEAD.
        assert!(current_branch(&r).is_some());
    }
}
