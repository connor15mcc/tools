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
$ tools --help
personal tools binary manager

Usage: tools [COMMAND]

Commands:
  ilimit         Interactively tail a limited number of lines
  jj-land        Land a staged branch and advance the local trunk bookmark
  mdflow         Reflow markdown text
  gh-review      Review GitHub PRs from a search query or team config
  sample         Sample lines from input using various strategies
  install        Install symlinks for all commands
  jj-stack       Stack changes under megamerge, creating one if needed
  jsonsql        Query a JSON file with DuckDB
  petname        Generate a random petname
  nogolint       Add nolint directives to suppress golangci-lint findings
  usage-sync     Sync --help output of a command to README.md between marker comments
  gomodmerge     Tidy merged go.mod and go.sum files
  jj-merge-both  jj merge-tool that accepts both sides
  decay          Calculate decay score from timestamps
  hist           Generate a text-based histogram from numerical data
  yank           Copy command and its output to clipboard
  align          Align text on a regex pattern
  cache          Cache command output to disk with TTL
  jj-iso         Create a random JJ workspace and launch a specified binary
  notes          note-taking utility
  help           Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```
<!-- HELP END -->
