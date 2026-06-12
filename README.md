# lazytrees

Interactive Rust TUI for managing Git worktrees, with CLI commands for scripted use.

Run it from inside any Git repository:

```sh
cargo run --
```

The default command opens the TUI. Use the keyboard to select a worktree, create a
new one, remove an existing one, prune stale metadata, refresh the list, or
open a tmux session inside the selected worktree.

CLI commands:

```sh
cargo run -- tui
cargo run -- new feature/search --base main
cargo run -- list
cargo run -- open ../repo-worktrees/feature-search
cargo run -- remove ../repo-worktrees/feature-search
cargo run -- prune
```

Defaults:

- Worktrees are created next to the main checkout in `<repo>-worktrees/<branch>`.
- Branch path separators are converted to `-`, so `feature/search` becomes `feature-search`.
- Creating or opening a worktree starts a new tmux session in that directory.
- Inside an existing tmux client, lazytrees switches to the new session instead of nesting tmux.
- The TUI is the primary interface; CLI commands remain stable for automation.

Install locally:

```sh
cargo install --path .
```

Then use:

```sh
lazytrees
lazytrees new feature/search --base main
lazytrees open ../repo-worktrees/feature-search
lazytrees remove ../repo-worktrees/feature-search
```
