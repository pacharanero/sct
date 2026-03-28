# `sct completions` — Print shell completion scripts

Prints a shell completion script to stdout for the specified shell. Redirect to the appropriate
location for your shell to enable tab-completion of `sct` subcommands and flags.

---

## Synopsis

```bash
sct completions <shell>
```

## Arguments

| Argument | Description |
|---|---|
| `<shell>` | Target shell: `bash`, `zsh`, `fish`, `powershell`, or `elvish`. |

---

## Installation

```bash
# bash — drop in completion directory
sct completions bash > ~/.local/share/bash-completion/completions/sct

# zsh — add to $fpath, then compinit
mkdir -p ~/.zfunc
sct completions zsh > ~/.zfunc/_sct
# Add to ~/.zshrc before compinit:
#   fpath=(~/.zfunc $fpath)

# fish
sct completions fish > ~/.config/fish/completions/sct.fish

# PowerShell
sct completions powershell >> $PROFILE

# elvish
sct completions elvish >> ~/.elvish/lib/completions.elv
```
