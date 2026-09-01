#!/usr/bin/env bash
# Quoted strings that span more than one line.

single='first
second'

ansi_c=$'first\ttab
second'

double="first $USER
second ${HOME}"

backtick=`uname -s
uname -r`

# A jq program passed as a multi-line single-quoted argument.
jq "$FILTER"'
    delpaths([ $prev[] | [.] ])
    | . + ($base | slice($prev))
    | . + $overlay' --argjson prev "$prev" "$cfg"

# A backslash-escaped quote outside any string does not open one.
echo 'it'\''s a trap'

# Quotes inside a heredoc body are inert.
cat <<'EOF'
don't panic
EOF

echo done

# An unterminated quote runs to the end of the file.
echo "no closing quote
still inside the string
