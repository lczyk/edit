#!/usr/bin/env bash
# Print a terminal-friendly index of *.prop.md files in this directory,
# grouped by status. Within each group, sorted by date (newest first).
# Each prop file has a YAML frontmatter with `status:`, `date:`, and
# `description:` fields.

set -u
cd "$(dirname "$0")"

if [[ -t 1 ]]; then
	BOLD=$'\e[1m'; DIM=$'\e[2m'; GRN=$'\e[32m'; YLW=$'\e[33m'
	BLU=$'\e[34m'; RED=$'\e[31m'; RST=$'\e[0m'
else
	BOLD=''; DIM=''; GRN=''; YLW=''; BLU=''; RED=''; RST=''
fi

field() {
	awk -v key="$2" '
		NR==1 && /^---$/ { in_fm=1; next }
		in_fm && /^---$/ { exit }
		in_fm && $1 == key":" {
			sub("^[^:]*:[ \t]*", "")
			print
			exit
		}
	' "$1"
}

# File-name column width.
width=0
for f in *.prop.md; do
	[[ -f $f ]] || continue
	(( ${#f} > width )) && width=${#f}
done

group() {
	local status=$1 title=$2 color=$3
	local rows=()
	for f in *.prop.md; do
		[[ -f $f ]] || continue
		[[ $(field "$f" status) == "$status" ]] || continue
		rows+=("$(field "$f" date)|$f|$(field "$f" description)")
	done
	[[ ${#rows[@]} -eq 0 ]] && return
	printf '\n%s%s%s%s\n' "$BOLD" "$color" "$title" "$RST"
	# Sort by date descending (newest first).
	printf '%s\n' "${rows[@]}" | sort -t'|' -k1,1r | while IFS='|' read -r date f desc; do
		printf '  %s%s%s  %s%-*s%s  %s\n' \
			"$DIM" "$date" "$RST" \
			"$color" "$width" "$f" "$RST" \
			"$desc"
	done
}

group implemented IMPLEMENTED "$GRN"
group open        OPEN        "$BLU"
group shelved     SHELVED     "$YLW"
group rejected    REJECTED    "$RED"
echo
