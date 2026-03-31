Print a shell completion script to stdout. Supports bash, zsh, fish, PowerShell, and elvish.

---

## Usage

```
sct completions <SHELL>
```

## Arguments

| Argument | Description |
|---|---|
| `<SHELL>` | One of: `bash`, `zsh`, `fish`, `powershell`, `elvish` |

---

## Installation

### bash

```bash
mkdir -p ~/.local/share/bash-completion/completions
sct completions bash > ~/.local/share/bash-completion/completions/sct
```

Or system-wide:

```bash
sct completions bash > /etc/bash_completion.d/sct
```

Reload with `source ~/.bashrc` or open a new shell.

### zsh

```zsh
mkdir -p ~/.zfunc
sct completions zsh > ~/.zfunc/_sct
```

Ensure `~/.zfunc` is on `$fpath` — add this to `~/.zshrc` **before** `compinit`:

```zsh
fpath=(~/.zfunc $fpath)
autoload -Uz compinit && compinit
```

Then open a new shell or run `exec zsh`.

### fish

```fish
sct completions fish > ~/.config/fish/completions/sct.fish
```

Takes effect immediately in new fish sessions.

### PowerShell

```powershell
sct completions powershell >> $PROFILE
```

Reload with `. $PROFILE` or open a new PowerShell session.

### elvish

```elvish
sct completions elvish >> ~/.elvish/lib/completions.elv
```

---

## Example

```bash
$ sct completions zsh > ~/.zfunc/_sct
$ exec zsh
$ sct <TAB>
codelist     completions  diff         embed        gui          info
lexical      markdown     mcp          ndjson       parquet      semantic
sqlite       tui
```
