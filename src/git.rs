use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

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
    pub agent_command: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LaunchOptions {
    pub path: Option<PathBuf>,
    pub agent_command: Option<String>,
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

pub fn launch_agent(command: &str, path: &Path) -> AppResult<()> {
    let status = Command::new("sh")
        .arg("-lc")
        .arg(command)
        .current_dir(path)
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(AppError::new(format!(
            "agent command exited with status {status}"
        )))
    }
}

pub fn default_agent_command() -> String {
    env::var("WT_AGENT_CMD").unwrap_or_else(|_| "opencode".to_owned())
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

pub fn shell_quote_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if text.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '/' | '.' | '_' | '-' | '+')
    }) {
        return text.into_owned();
    }

    format!("'{}'", text.replace('\'', "'\\''"))
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
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(AppError::new(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim()
        )))
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
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::new(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
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
            if let Some(worktree) = current.take() {
                worktrees.push(worktree);
            }
            continue;
        }

        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(worktree) = current.take() {
                worktrees.push(worktree);
            }
            current = Some(Worktree {
                path: PathBuf::from(path),
                head: None,
                branch: None,
                bare: false,
                detached: false,
                prunable: false,
            });
            continue;
        }

        let Some(worktree) = current.as_mut() else {
            continue;
        };

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

    if let Some(worktree) = current {
        worktrees.push(worktree);
    }

    worktrees
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
