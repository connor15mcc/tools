#!/usr/bin/env bash

set -euxo pipefail

number_repos=80
log='git log --pretty=format:"%ad %an" --date=iso'
filter_bots='rg -v "lyft-refactorator-*|lyft-control-*|buildnotify-production-*|lyft-idl-*|lyft-metaservice|dependabot*|renovate*|GitHub CI|zimrideops"'
get_dates='cut -d" " -f2'
decay='tools decay'
format='cut -d" " -f3 | sed "s/^/$(basename $PWD) /"'

cat $PWD/golangci-repos.txt | head -n "$number_repos" |
	# this is obviously the "wrong" id, but it lets us use the checkouts already
	# on disk... we should just use a persistent checkout dir for PMR
	tools pmr --id golangci --dry-run "$log | $filter_bots | $get_dates | $decay | $format >> $PWD/decay.txt"
