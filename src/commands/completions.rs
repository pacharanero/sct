//! `sct completions` — Print shell completion scripts to stdout.
//!
//! Supports bash, zsh, fish, powershell, and elvish.
//!
//! Installation:
//!
//!   # bash (add to ~/.bashrc or drop in /etc/bash_completion.d/)
//!   sct completions bash > ~/.local/share/bash-completion/completions/sct
//!
//!   # zsh (add to a directory on $fpath, then `compinit`)
//!   mkdir -p ~/.zfunc
//!   sct completions zsh > ~/.zfunc/_sct
//!   # ensure ~/.zfunc is on fpath — add to ~/.zshrc before compinit:
//!   #   fpath=(~/.zfunc $fpath)
//!
//!   # fish
//!   sct completions fish > ~/.config/fish/completions/sct.fish
//!
//!   # PowerShell
//!   sct completions powershell >> $PROFILE
//!
//!   # elvish
//!   sct completions elvish >> ~/.elvish/lib/completions.elv

use anyhow::Result;
use clap::{Command, Parser};
use clap_complete::{generate, Shell};

#[derive(Parser, Debug)]
pub struct Args {
    /// Shell to generate completions for.
    pub shell: Shell,
}

pub fn run(args: Args, mut cmd: Command) -> Result<()> {
    generate(args.shell, &mut cmd, "sct", &mut std::io::stdout());
    Ok(())
}
