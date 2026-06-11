use std::io::{self, Write};
use std::path::PathBuf;

use crate::error::{AppError, AppResult};
use crate::git::{
    LaunchOptions, NewOptions, Repo, build_new_worktree_plan, create_worktree,
    default_agent_command, format_worktree, launch_agent, list_worktrees as git_list_worktrees,
    prune_worktrees as git_prune_worktrees, remove_worktree as git_remove_worktree,
    shell_quote_path,
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
            "  lazytrees new [branch] [options]  create a worktree\n",
            "  lazytrees list                    list worktrees\n",
            "  lazytrees launch [options]        launch an agent in a worktree\n",
            "  lazytrees remove [path]           remove a worktree\n",
            "  lazytrees prune                   prune stale worktree metadata\n\n",
            "Options for new:\n",
            "  --base <ref>        base ref for a new branch, defaults to HEAD\n",
            "  --path <path>       worktree path, defaults to ../<repo>-worktrees/<branch>\n",
            "  --agent <command>   command to run inside the new worktree\n\n",
            "Options for launch:\n",
            "  --path <path>       existing worktree path\n",
            "  --agent <command>   command to run, defaults to WT_AGENT_CMD or opencode\n\n",
            "Options for remove:\n",
            "  --path <path>       existing worktree path\n\n",
            "Examples:\n",
            "  lazytrees\n",
            "  lazytrees new feature/search --base main --agent opencode\n",
            "  lazytrees launch --agent \"opencode\"\n",
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

    if prompt_missing
        && options.agent_command.is_none()
        && confirm("Launch an agent after creation?", false)?
    {
        let default_command = default_agent_command();
        let command = prompt("Agent command", Some(&default_command))?;
        options.agent_command = Some(command);
    }

    create_worktree(repo, &plan)?;
    println!("created: {}", plan.path.display());

    if let Some(command) = options.agent_command.as_deref() {
        println!("launching `{command}` in {}", plan.path.display());
        launch_agent(command, &plan.path)?;
    } else {
        println!("next: cd {}", shell_quote_path(&plan.path));
    }

    Ok(())
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

pub fn launch_worktree(repo: &Repo, args: &[String]) -> AppResult<()> {
    let mut options = parse_launch_options(args)?;
    let prompt_missing = options.path.is_none();

    if options.path.is_none() {
        options.path = Some(select_worktree(repo)?);
    }

    if options.agent_command.is_none() && prompt_missing {
        let default_command = default_agent_command();
        let command = prompt("Agent command", Some(&default_command))?;
        options.agent_command = Some(command);
    }

    if options.agent_command.is_none() {
        options.agent_command = Some(default_agent_command());
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

    let command = options
        .agent_command
        .as_deref()
        .ok_or_else(|| AppError::new("missing agent command"))?;
    println!("launching `{command}` in {}", path.display());
    launch_agent(command, path)
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
                index += 1;
                options.base = Some(required_arg(args, index, "--base")?.to_owned());
            }
            "--path" => {
                index += 1;
                options.path = Some(PathBuf::from(required_arg(args, index, "--path")?));
            }
            "--agent" => {
                index += 1;
                if index >= args.len() {
                    return Err(AppError::new("--agent requires a command"));
                }
                options.agent_command = Some(args[index..].join(" "));
                return Ok(options);
            }
            argument if argument.starts_with("--base=") => {
                options.base = Some(argument["--base=".len()..].to_owned());
            }
            argument if argument.starts_with("--path=") => {
                options.path = Some(PathBuf::from(&argument["--path=".len()..]));
            }
            argument if argument.starts_with("--agent=") => {
                options.agent_command = Some(argument["--agent=".len()..].to_owned());
            }
            argument if argument.starts_with('-') => {
                return Err(AppError::new(format!("unknown option `{argument}`")));
            }
            branch => {
                if options.branch.is_some() {
                    return Err(AppError::new(format!(
                        "unexpected extra argument `{branch}`"
                    )));
                }
                options.branch = Some(branch.to_owned());
            }
        }

        index += 1;
    }

    Ok(options)
}

fn parse_launch_options(args: &[String]) -> AppResult<LaunchOptions> {
    let mut options = LaunchOptions::default();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--path" => {
                index += 1;
                options.path = Some(PathBuf::from(required_arg(args, index, "--path")?));
            }
            "--agent" => {
                index += 1;
                if index >= args.len() {
                    return Err(AppError::new("--agent requires a command"));
                }
                options.agent_command = Some(args[index..].join(" "));
                return Ok(options);
            }
            argument if argument.starts_with("--path=") => {
                options.path = Some(PathBuf::from(&argument["--path=".len()..]));
            }
            argument if argument.starts_with("--agent=") => {
                options.agent_command = Some(argument["--agent=".len()..].to_owned());
            }
            argument if argument.starts_with('-') => {
                return Err(AppError::new(format!("unknown option `{argument}`")));
            }
            path => {
                if options.path.is_some() {
                    return Err(AppError::new(format!("unexpected extra argument `{path}`")));
                }
                options.path = Some(PathBuf::from(path));
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
                index += 1;
                options.path = Some(PathBuf::from(required_arg(args, index, "--path")?));
            }
            argument if argument.starts_with("--path=") => {
                options.path = Some(PathBuf::from(&argument["--path=".len()..]));
            }
            argument if argument.starts_with('-') => {
                return Err(AppError::new(format!("unknown option `{argument}`")));
            }
            path => {
                if options.path.is_some() {
                    return Err(AppError::new(format!("unexpected extra argument `{path}`")));
                }
                options.path = Some(PathBuf::from(path));
            }
        }

        index += 1;
    }

    Ok(options)
}

fn required_arg<'a>(args: &'a [String], index: usize, option: &str) -> AppResult<&'a str> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| AppError::new(format!("{option} requires a value")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_new_options_treats_agent_as_remaining_command() {
        let args = vec![
            "feat/demo".to_owned(),
            "--base".to_owned(),
            "main".to_owned(),
            "--agent".to_owned(),
            "opencode".to_owned(),
            "--continue".to_owned(),
        ];

        let options = parse_new_options(&args).expect("options parse");

        assert_eq!(options.branch.as_deref(), Some("feat/demo"));
        assert_eq!(options.base.as_deref(), Some("main"));
        assert_eq!(
            options.agent_command.as_deref(),
            Some("opencode --continue")
        );
    }

    #[test]
    fn parse_launch_options_accepts_path_positionally() {
        let args = vec![
            "../repo-worktrees/feat".to_owned(),
            "--agent".to_owned(),
            "opencode".to_owned(),
        ];

        let options = parse_launch_options(&args).expect("options parse");

        assert_eq!(
            options.path.as_deref(),
            Some(std::path::Path::new("../repo-worktrees/feat"))
        );
        assert_eq!(options.agent_command.as_deref(), Some("opencode"));
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
