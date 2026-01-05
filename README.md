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
  hist        Generate a text-based histogram from numerical data
  decay       Calculate decay score from timestamps
  gh-review   Review GitHub PRs from a search query or team config
  install     Install symlinks for all commands
  gomodmerge  Tidy merged go.mod and go.sum files
  usage-sync  Sync --help output of a command to README.md between marker comments or infer from
              placeholders
  mdflow      Reflow markdown text
  sample      Sample lines from input using various strategies
  ilimit      Interactively tail a limited number of lines
  petname     Generate a random petname
  notes       note-taking utility
  pmr         Poor Man's Refactorator - batch apply changes across repos
  help        Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```
<!-- HELP END -->
