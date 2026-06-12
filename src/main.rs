mod cli;
mod error;
mod git;
mod tui;

use std::env;

use error::AppResult;
use git::discover_repo;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> AppResult<()> {
    let args: Vec<String> = env::args().skip(1).collect();

    if cli::is_help_request(&args) {
        cli::print_help();
        return Ok(());
    }

    let repo = discover_repo()?;

    match args.first().map(String::as_str) {
        None | Some("tui") => tui::run(&repo),
        Some("new") => cli::new_worktree(&repo, &args[1..]),
        Some("list") => cli::list_worktrees(&repo),
        Some("open") => cli::open_worktree(&repo, &args[1..]),
        Some("remove") | Some("rm") => cli::remove_worktree(&repo, &args[1..]),
        Some("prune") => cli::prune_worktrees(&repo),
        Some("help") => {
            cli::print_help();
            Ok(())
        }
        Some(command) => Err(error::AppError::new(format!(
            "unknown command `{command}`. Run `lazytrees --help`."
        ))),
    }
}
