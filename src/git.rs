use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use crate::error::{AppError, AppResult};

#[derive(Debug)]
pub struct Repo {
    pub root: PathBuf,
    pub default_worktree_parent: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub prunable: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NewOptions {
    pub branch: Option<String>,
    pub base: Option<String>,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OpenOptions {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorktreePlan {
    pub branch: String,
    pub base: Option<String>,
    pub path: PathBuf,
    pub branch_exists: bool,
}

pub fn discover_repo() -> AppResult<Repo> {
    let root = PathBuf::from(capture_git(
        None::<&Path>,
        ["rev-parse", "--show-toplevel"],
    )?);
    let worktrees = list_worktrees_from_cwd().unwrap_or_default();
    let primary_root = worktrees
        .first()
        .map(|worktree| worktree.path.clone())
        .unwrap_or_else(|| root.clone());

    let repo_name = primary_root
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| AppError::new("could not determine repository name"))?;
    let parent = primary_root
        .parent()
        .ok_or_else(|| AppError::new("could not determine repository parent directory"))?;

    Ok(Repo {
        root,
        default_worktree_parent: parent.join(format!("{repo_name}-worktrees")),
    })
}

pub fn build_new_worktree_plan(repo: &Repo, options: NewOptions) -> AppResult<NewWorktreePlan> {
    let branch = options
        .branch
        .ok_or_else(|| AppError::new("missing branch name"))?;

    validate_branch_name(repo, &branch)?;

    let branch_exists = local_branch_exists(repo, &branch)?;
    if branch_exists && branch_is_checked_out(repo, &branch)? {
        return Err(AppError::new(format!(
            "branch `{branch}` is already checked out in another worktree"
        )));
    }

    let raw_path = options
        .path
        .unwrap_or_else(|| default_worktree_path(repo, &branch));
    let path = absolute_path(&raw_path)?;

    if path.exists() {
        return Err(AppError::new(format!(
            "path already exists: {}",
            path.display()
        )));
    }

    let base = if branch_exists {
        None
    } else {
        Some(options.base.unwrap_or_else(|| "HEAD".to_owned()))
    };

    Ok(NewWorktreePlan {
        branch,
        base,
        path,
        branch_exists,
    })
}

pub fn create_worktree(repo: &Repo, plan: &NewWorktreePlan) -> AppResult<()> {
    if let Some(parent) = plan.path.parent() {
        fs::create_dir_all(parent)?;
    }

    let path_text = plan.path.to_string_lossy().into_owned();
    if plan.branch_exists {
        run_git(repo, ["worktree", "add", path_text.as_str(), &plan.branch])?;
        return Ok(());
    }

    run_git(
        repo,
        [
            "worktree",
            "add",
            "-b",
            &plan.branch,
            path_text.as_str(),
            plan.base.as_deref().unwrap_or("HEAD"),
        ],
    )?;
    Ok(())
}

pub fn list_worktrees(repo: &Repo) -> AppResult<Vec<Worktree>> {
    let output = capture_git(Some(&repo.root), ["worktree", "list", "--porcelain"])?;
    Ok(parse_worktree_list(&output))
}

pub fn prune_worktrees(repo: &Repo) -> AppResult<()> {
    run_git(repo, ["worktree", "prune"])
}

pub fn remove_worktree(repo: &Repo, path: &Path) -> AppResult<()> {
    let path_text = path.to_string_lossy().into_owned();
    run_git(repo, ["worktree", "remove", path_text.as_str()])
}

pub fn open_tmux_session(path: &Path) -> AppResult<()> {
    if !path.is_dir() {
        return Err(AppError::new(format!(
            "not a directory: {}",
            path.display()
        )));
    }

    let session = available_tmux_session_name(path)?;
    if env::var_os("TMUX").is_some() {
        let mut create = Command::new("tmux");
        create
            .args(["new-session", "-d", "-s"])
            .arg(&session)
            .arg("-c")
            .arg(path);
        run_tmux(&mut create, "tmux new-session")?;

        let target = exact_tmux_target(&session);
        let mut switch = Command::new("tmux");
        switch.args(["switch-client", "-t"]).arg(target);
        run_tmux(&mut switch, "tmux switch-client")?;
    } else {
        let mut create = Command::new("tmux");
        create
            .args(["new-session", "-s"])
            .arg(&session)
            .arg("-c")
            .arg(path);
        run_tmux(&mut create, "tmux new-session")?;
    }

    Ok(())
}

pub fn default_worktree_path(repo: &Repo, branch: &str) -> PathBuf {
    repo.default_worktree_parent
        .join(branch_to_dir_name(branch))
}

pub fn format_worktree(worktree: &Worktree) -> String {
    let branch = worktree.branch_label();
    let prunable = if worktree.prunable { " prunable" } else { "" };
    format!("{}  [{}{}]", worktree.path.display(), branch, prunable)
}

impl Worktree {
    pub fn branch_label(&self) -> &str {
        if let Some(branch) = self.branch.as_deref() {
            branch
        } else if self.detached {
            "detached"
        } else if self.bare {
            "bare"
        } else {
            "unknown"
        }
    }

    pub fn short_head(&self) -> &str {
        self.head
            .as_deref()
            .map(|head| head.get(..7).unwrap_or(head))
            .unwrap_or("-")
    }
}

fn list_worktrees_from_cwd() -> AppResult<Vec<Worktree>> {
    let output = capture_git(None::<&Path>, ["worktree", "list", "--porcelain"])?;
    Ok(parse_worktree_list(&output))
}

fn validate_branch_name(repo: &Repo, branch: &str) -> AppResult<()> {
    let output = Command::new("git")
        .args(["check-ref-format", "--branch", branch])
        .current_dir(&repo.root)
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        Err(AppError::new(format!("invalid branch name `{branch}`")))
    }
}

fn local_branch_exists(repo: &Repo, branch: &str) -> AppResult<bool> {
    let ref_name = format!("refs/heads/{branch}");
    let status = Command::new("git")
        .args(["show-ref", "--verify", "--quiet", ref_name.as_str()])
        .current_dir(&repo.root)
        .status()?;

    status_to_bool(status, "git show-ref")
}

fn branch_is_checked_out(repo: &Repo, branch: &str) -> AppResult<bool> {
    Ok(list_worktrees(repo)?.iter().any(|worktree| {
        worktree
            .branch
            .as_deref()
            .is_some_and(|checked_out| checked_out == branch)
    }))
}

fn run_git<'a>(repo: &Repo, args: impl IntoIterator<Item = &'a str>) -> AppResult<()> {
    let args: Vec<&str> = args.into_iter().collect();
    let output = Command::new("git")
        .args(&args)
        .current_dir(&repo.root)
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        Err(git_failure(&args, &output.stderr))
    }
}

fn capture_git<'a>(
    cwd: Option<&Path>,
    args: impl IntoIterator<Item = &'a str>,
) -> AppResult<String> {
    let args: Vec<&str> = args.into_iter().collect();
    let mut command = Command::new("git");
    command.args(&args);
    if let Some(directory) = cwd {
        command.current_dir(directory);
    }

    let output = command.output()?;
    if !output.status.success() {
        return Err(git_failure(&args, &output.stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_failure(args: &[&str], stderr: &[u8]) -> AppError {
    let stderr = String::from_utf8_lossy(stderr);
    AppError::new(format!("git {} failed: {}", args.join(" "), stderr.trim()))
}

fn status_to_bool(status: ExitStatus, command: &str) -> AppResult<bool> {
    if status.success() {
        return Ok(true);
    }

    match status.code() {
        Some(1) => Ok(false),
        _ => Err(AppError::new(format!(
            "{command} failed with status {status}"
        ))),
    }
}

fn absolute_path(path: &Path) -> AppResult<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn available_tmux_session_name(path: &Path) -> AppResult<String> {
    let base = tmux_session_name(path);
    if !tmux_session_exists(&base)? {
        return Ok(base);
    }

    let mut suffix = 2;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !tmux_session_exists(&candidate)? {
            return Ok(candidate);
        }
        suffix += 1;
    }
}

fn tmux_session_name(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("worktree");
    format!("lazytrees-{}", sanitize_tmux_session_name(name))
}

fn sanitize_tmux_session_name(name: &str) -> String {
    let mut sanitized = String::new();
    let mut last_was_dash = false;

    for character in name.chars() {
        let replacement = if character.is_ascii_alphanumeric() {
            character.to_ascii_lowercase()
        } else if matches!(character, '-' | '_') {
            character
        } else {
            '-'
        };

        if replacement == '-' && last_was_dash {
            continue;
        }

        last_was_dash = replacement == '-';
        sanitized.push(replacement);
    }

    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "worktree".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn tmux_session_exists(name: &str) -> AppResult<bool> {
    let target = exact_tmux_target(name);
    let status = Command::new("tmux")
        .args(["has-session", "-t"])
        .arg(target)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(tmux_io_error)?;

    Ok(status.success())
}

fn exact_tmux_target(name: &str) -> String {
    format!("={name}")
}

fn run_tmux(command: &mut Command, description: &str) -> AppResult<()> {
    let status = command.status().map_err(tmux_io_error)?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::new(format!(
            "{description} failed with status {status}"
        )))
    }
}

fn tmux_io_error(error: io::Error) -> AppError {
    if error.kind() == io::ErrorKind::NotFound {
        AppError::new("tmux is required but was not found in PATH")
    } else {
        AppError::from(error)
    }
}

fn branch_to_dir_name(branch: &str) -> String {
    branch
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | ' ' | '\t' => '-',
            character => character,
        })
        .collect()
}

fn parse_worktree_list(output: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current: Option<Worktree> = None;

    for line in output.lines() {
        if line.is_empty() {
            finish_current_worktree(&mut current, &mut worktrees);
            continue;
        }

        if let Some(path) = line.strip_prefix("worktree ") {
            finish_current_worktree(&mut current, &mut worktrees);
            current = Some(new_worktree(path));
            continue;
        }

        if let Some(worktree) = current.as_mut() {
            apply_worktree_field(worktree, line);
        }
    }

    finish_current_worktree(&mut current, &mut worktrees);
    worktrees
}

fn finish_current_worktree(current: &mut Option<Worktree>, worktrees: &mut Vec<Worktree>) {
    if let Some(worktree) = current.take() {
        worktrees.push(worktree);
    }
}

fn new_worktree(path: &str) -> Worktree {
    Worktree {
        path: PathBuf::from(path),
        head: None,
        branch: None,
        bare: false,
        detached: false,
        prunable: false,
    }
}

fn apply_worktree_field(worktree: &mut Worktree, line: &str) {
    if let Some(head) = line.strip_prefix("HEAD ") {
        worktree.head = Some(head.to_owned());
    } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
        worktree.branch = Some(branch.to_owned());
    } else if line == "bare" {
        worktree.bare = true;
    } else if line == "detached" {
        worktree.detached = true;
    } else if line.starts_with("prunable") {
        worktree.prunable = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_to_dir_name_replaces_path_unfriendly_characters() {
        assert_eq!(branch_to_dir_name("feat/search ui"), "feat-search-ui");
        assert_eq!(
            branch_to_dir_name("bugfix\\windows:path"),
            "bugfix-windows-path"
        );
    }

    #[test]
    fn tmux_session_name_uses_sanitized_path_basename() {
        assert_eq!(
            tmux_session_name(Path::new("/repo-worktrees/Feat Search!")),
            "lazytrees-feat-search"
        );
        assert_eq!(tmux_session_name(Path::new("/")), "lazytrees-worktree");
    }

    #[test]
    fn parse_worktree_list_reads_porcelain_output() {
        let output = "worktree /repo\nHEAD abc123\nbranch refs/heads/main\n\nworktree /repo-worktrees/feat\nHEAD def456\nbranch refs/heads/feat/demo\n\nworktree /repo-worktrees/detached\nHEAD 999999\ndetached\nprunable gitdir file points to non-existent location\n";

        let worktrees = parse_worktree_list(output);

        assert_eq!(worktrees.len(), 3);
        assert_eq!(worktrees[0].path, PathBuf::from("/repo"));
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
        assert_eq!(worktrees[1].branch.as_deref(), Some("feat/demo"));
        assert!(worktrees[2].detached);
        assert!(worktrees[2].prunable);
    }
}
