use std::io::{self, Write};
use std::path::PathBuf;

use crate::error::{AppError, AppResult};
use crate::git::{
    NewOptions, OpenOptions, Repo, build_new_worktree_plan, create_worktree, format_worktree,
    list_worktrees as git_list_worktrees, open_tmux_session,
    prune_worktrees as git_prune_worktrees, remove_worktree as git_remove_worktree,
};

pub fn is_help_request(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "-h" || arg == "--help")
}

pub fn print_help() {
    println!(
        "{}",
        concat!(
            "lazytrees\n\n",
            "Interactive TUI for Git worktrees, with scriptable CLI commands.\n\n",
            "Usage:\n",
            "  lazytrees                         open the TUI\n",
            "  lazytrees tui                     open the TUI\n",
            "  lazytrees new [branch] [options]  create a worktree and open tmux\n",
            "  lazytrees list                    list worktrees\n",
            "  lazytrees open [path]             open tmux in a worktree\n",
            "  lazytrees remove [path]           remove a worktree\n",
            "  lazytrees prune                   prune stale worktree metadata\n\n",
            "Options for new:\n",
            "  --base <ref>        base ref for a new branch, defaults to HEAD\n",
            "  --path <path>       worktree path, defaults to ../<repo>-worktrees/<branch>\n\n",
            "Options for open:\n",
            "  --path <path>       existing worktree path\n\n",
            "Options for remove:\n",
            "  --path <path>       existing worktree path\n\n",
            "Examples:\n",
            "  lazytrees\n",
            "  lazytrees new feature/search --base main\n",
            "  lazytrees open ../repo-worktrees/feature-search\n",
            "  lazytrees remove ../repo-worktrees/feature-search\n"
        )
    );
}

pub fn new_worktree(repo: &Repo, args: &[String]) -> AppResult<()> {
    let mut options = parse_new_options(args)?;
    let prompt_missing = options.branch.is_none();

    if options.branch.is_none() {
        options.branch = Some(prompt_required("Branch name")?);
    }

    if options.base.is_none() && prompt_missing {
        let base = prompt("Base ref", Some("HEAD"))?;
        options.base = Some(base);
    }

    let plan = build_new_worktree_plan(repo, options.clone())?;

    println!();
    println!("Create worktree");
    println!("  branch: {}", plan.branch);
    if plan.branch_exists {
        println!("  mode: existing branch");
    } else {
        println!(
            "  mode: new branch from {}",
            plan.base.as_deref().unwrap_or("HEAD")
        );
    }
    println!("  path: {}", plan.path.display());

    create_worktree(repo, &plan)?;
    println!("created: {}", plan.path.display());
    println!("opening tmux session in {}", plan.path.display());
    open_tmux_session(&plan.path)
}

pub fn list_worktrees(repo: &Repo) -> AppResult<()> {
    let worktrees = git_list_worktrees(repo)?;

    if worktrees.is_empty() {
        println!("No worktrees found.");
        return Ok(());
    }

    for worktree in worktrees {
        println!("{}", format_worktree(&worktree));
    }

    Ok(())
}

pub fn open_worktree(repo: &Repo, args: &[String]) -> AppResult<()> {
    let mut options = parse_open_options(args)?;

    if options.path.is_none() {
        options.path = Some(select_worktree(repo)?);
    }

    let path = options
        .path
        .as_ref()
        .ok_or_else(|| AppError::new("missing worktree path"))?;
    if !path.is_dir() {
        return Err(AppError::new(format!(
            "not a directory: {}",
            path.display()
        )));
    }

    println!("opening tmux session in {}", path.display());
    open_tmux_session(path)
}

pub fn prune_worktrees(repo: &Repo) -> AppResult<()> {
    git_prune_worktrees(repo)?;
    println!("pruned stale worktree metadata");
    Ok(())
}

pub fn remove_worktree(repo: &Repo, args: &[String]) -> AppResult<()> {
    let options = parse_remove_options(args)?;
    let prompt_missing = options.path.is_none();
    let path = match options.path {
        Some(path) => path,
        None => select_worktree(repo)?,
    };

    if prompt_missing {
        let label = format!("Remove worktree {}?", path.display());
        if !confirm(&label, false)? {
            println!("remove cancelled");
            return Ok(());
        }
    }

    git_remove_worktree(repo, &path)?;
    println!("removed: {}", path.display());
    Ok(())
}

fn select_worktree(repo: &Repo) -> AppResult<PathBuf> {
    let worktrees = git_list_worktrees(repo)?;
    if worktrees.is_empty() {
        return Err(AppError::new("no worktrees found"));
    }

    println!("Worktrees:");
    for (index, worktree) in worktrees.iter().enumerate() {
        println!("  {}) {}", index + 1, format_worktree(worktree));
    }

    loop {
        let input = prompt("Choose worktree", Some("1"))?;
        match input.parse::<usize>() {
            Ok(selection) if (1..=worktrees.len()).contains(&selection) => {
                return Ok(worktrees[selection - 1].path.clone());
            }
            _ => println!("Enter a number between 1 and {}.", worktrees.len()),
        }
    }
}

fn prompt(label: &str, default: Option<&str>) -> AppResult<String> {
    match default {
        Some(default_value) => print!("{label} [{default_value}]: "),
        None => print!("{label}: "),
    }
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();

    if trimmed.is_empty() {
        Ok(default.unwrap_or_default().to_owned())
    } else {
        Ok(trimmed.to_owned())
    }
}

fn prompt_required(label: &str) -> AppResult<String> {
    loop {
        let value = prompt(label, None)?;
        if !value.is_empty() {
            return Ok(value);
        }
        println!("{label} is required.");
    }
}

fn confirm(label: &str, default: bool) -> AppResult<bool> {
    let hint = if default { "Y/n" } else { "y/N" };
    loop {
        let input = prompt(label, Some(hint))?;
        if input == hint {
            return Ok(default);
        }

        match input.to_ascii_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("Please answer yes or no."),
        }
    }
}

fn parse_new_options(args: &[String]) -> AppResult<NewOptions> {
    let mut options = NewOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--base" => {
                options.base = Some(required_option_value(args, &mut index, "--base")?.to_owned());
            }
            "--path" => {
                options.path = Some(required_path_value(args, &mut index)?);
            }
            argument => {
                if let Some(base) = argument.strip_prefix("--base=") {
                    options.base = Some(base.to_owned());
                } else if let Some(path) = argument.strip_prefix("--path=") {
                    options.path = Some(PathBuf::from(path));
                } else if argument.starts_with('-') {
                    return Err(AppError::new(format!("unknown option `{argument}`")));
                } else {
                    if options.branch.is_some() {
                        return Err(AppError::new(format!(
                            "unexpected extra argument `{argument}`"
                        )));
                    }
                    options.branch = Some(argument.to_owned());
                }
            }
        }

        index += 1;
    }

    Ok(options)
}

fn parse_open_options(args: &[String]) -> AppResult<OpenOptions> {
    let mut options = OpenOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--path" => {
                options.path = Some(required_path_value(args, &mut index)?);
            }
            argument => {
                if let Some(path) = argument.strip_prefix("--path=") {
                    options.path = Some(PathBuf::from(path));
                } else if argument.starts_with('-') {
                    return Err(AppError::new(format!("unknown option `{argument}`")));
                } else {
                    set_positional_path(&mut options.path, argument)?;
                }
            }
        }

        index += 1;
    }

    Ok(options)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct RemoveOptions {
    path: Option<PathBuf>,
}

fn parse_remove_options(args: &[String]) -> AppResult<RemoveOptions> {
    let mut options = RemoveOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--path" => {
                options.path = Some(required_path_value(args, &mut index)?);
            }
            argument => {
                if let Some(path) = argument.strip_prefix("--path=") {
                    options.path = Some(PathBuf::from(path));
                } else if argument.starts_with('-') {
                    return Err(AppError::new(format!("unknown option `{argument}`")));
                } else {
                    set_positional_path(&mut options.path, argument)?;
                }
            }
        }

        index += 1;
    }

    Ok(options)
}

fn required_option_value<'a>(
    args: &'a [String],
    index: &mut usize,
    option: &str,
) -> AppResult<&'a str> {
    *index += 1;
    args.get(*index)
        .map(String::as_str)
        .ok_or_else(|| AppError::new(format!("{option} requires a value")))
}

fn required_path_value(args: &[String], index: &mut usize) -> AppResult<PathBuf> {
    Ok(PathBuf::from(required_option_value(args, index, "--path")?))
}

fn set_positional_path(path: &mut Option<PathBuf>, argument: &str) -> AppResult<()> {
    if path.is_some() {
        return Err(AppError::new(format!(
            "unexpected extra argument `{argument}`"
        )));
    }

    *path = Some(PathBuf::from(argument));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_new_options_accepts_branch_base_and_path() {
        let args = vec![
            "feat/demo".to_owned(),
            "--base".to_owned(),
            "main".to_owned(),
            "--path".to_owned(),
            "../repo-worktrees/feat-demo".to_owned(),
        ];

        let options = parse_new_options(&args).expect("options parse");

        assert_eq!(options.branch.as_deref(), Some("feat/demo"));
        assert_eq!(options.base.as_deref(), Some("main"));
        assert_eq!(
            options.path.as_deref(),
            Some(std::path::Path::new("../repo-worktrees/feat-demo"))
        );
    }

    #[test]
    fn parse_open_options_accepts_path_positionally() {
        let args = vec!["../repo-worktrees/feat".to_owned()];

        let options = parse_open_options(&args).expect("options parse");

        assert_eq!(
            options.path.as_deref(),
            Some(std::path::Path::new("../repo-worktrees/feat"))
        );
    }

    #[test]
    fn parse_remove_options_accepts_path_positionally() {
        let args = vec!["../repo-worktrees/feat".to_owned()];

        let options = parse_remove_options(&args).expect("options parse");

        assert_eq!(
            options.path.as_deref(),
            Some(std::path::Path::new("../repo-worktrees/feat"))
        );
    }
}
