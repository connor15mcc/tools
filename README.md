## tools

tools: a grab bag of useful tools + utilities, sharpened with age

the main tools binary employs a busybox-style dispatch based on argv0. To
install symlinks for each of the commands:

```
cargo install --path=. && tools install
```

## Usage

<!-- HELP START -->
```
personal tools binary manager

Usage: tools [COMMAND]

Commands:
  ilimit      Interactively tail a limited number of lines
  mdflow      Reflow markdown text
  gh-review   Review GitHub PRs from a search query or team config
  sample      Sample lines from input using various strategies
  install     Install symlinks for all commands
  petname     Generate a random petname
  nogolint    Add nolint directives to suppress golangci-lint findings
  usage-sync  Sync --help output of a command to README.md between marker comments
  gomodmerge  Tidy merged go.mod and go.sum files
  decay       Calculate decay score from timestamps
  hist        Generate a text-based histogram from numerical data
  yank        Copy command and its output to clipboard
  jj-iso      Create a random JJ workspace and launch a specified binary
  notes       note-taking utility
  help        Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```
<!-- HELP END -->
