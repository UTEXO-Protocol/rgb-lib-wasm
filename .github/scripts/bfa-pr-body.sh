#!/usr/bin/env bash
# Print the description of the bfa into dev pull request: what bfa has that dev has not.
# Usage: bfa-pr-body.sh <dev ref> <bfa ref>
set -euo pipefail
dev=${1:-origin/dev}
bfa=${2:-HEAD}
base=$(git merge-base "$dev" "$bfa")

echo "Everything on \`bfa\` that is not on \`dev\`, kept open as a draft so the BFA line can be reviewed in one diff. This description is rewritten on every push to \`bfa\`."
echo
echo "## Pull requests"
echo
# These three were merged into dev first, then moved to bfa without merge commits
echo "- #34 Update RGB crates to 0.11.2-rc.1 with an in-memory RGB store"
echo "- #35 Add BFA asset support for hold, send and receive"
echo "- #36 Add burn, burn_begin and burn_end for IFA and BFA"
git log --reverse --first-parent --merges --format='%H' "$base..$bfa" | while read -r c; do
  n=$(git log -1 --format=%s "$c" | sed -n 's/^Merge pull request #\([0-9]*\).*/\1/p')
  title=$(git log -1 --format=%b "$c" | sed -n '1p')
  if [ -n "$n" ]; then echo "- #$n $title"; fi
done
echo
echo "## Size"
echo
git diff --shortstat "$base" "$bfa" | sed 's/^ *//'
