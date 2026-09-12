#!/usr/bin/env bash
# The body of $(...) is shell code, on one line or across several.

now=$(date +%s)
quoted="$(uname -s) and $((1 + 2))"
test -z "$(chroot "$rootfs" /usr/bin/true)"

rootfs="$(install-slices \
    openssh-server_bins \
    bash_bins \
)"

bare=$(
    grep -c 'x' "$file" # trailing ) in a comment is inert
)

nested="$(dirname "$(readlink -f "$0")")"
echo after
