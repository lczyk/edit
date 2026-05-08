#!/usr/bin/env fish

# Comments
# This is a comment

# Numbers
echo 42
echo 3.14
echo -7
echo 0xFF
echo 0b1010
echo 0o777
echo 1.5e10

# Constants
true
false

# Single-quoted strings (no escapes, no interpolation)
echo 'hello world'
echo 'no $expansion here'
echo 'no \n escapes either'

# Double-quoted strings (variable expansion + escapes)
echo "hello $USER"
echo "escaped \" quote"
echo "escaped \\ backslash"
echo "tab\there"
echo "value: $argv"

# Variables
set name "world"
set -l local_var 42
set -g global_var "hello"
set -x EXPORTED "visible"
set -e old_var
set -S name
set list one two three
echo $name
echo $list[1]
echo $list[2..3]
echo $PATH

# Control flow
if test -f /etc/passwd
    echo "exists"
else if test -d /tmp
    echo "tmp exists"
else
    echo "neither"
end

for i in 1 2 3
    echo $i
    continue
end

while true
    break
end

switch $argv[1]
    case start
        echo "starting"
    case stop
        echo "stopping"
    case '*'
        echo "unknown"
end

# Begin/end blocks
begin
    set -l scoped "only here"
    echo $scoped
end

# Logical operators
test -f /etc/passwd; and echo "yes"
test -f /etc/passwd; or echo "no"
test -f /etc/passwd && echo "yes"
test -f /etc/passwd || echo "no"
not test -f /nonexistent

# Command substitution
set files (ls /tmp)
set today (date +%Y-%m-%d)
echo "today is "(date +%Y-%m-%d)

# Functions
function greet
    echo "Hello, $argv[1]"
    return 0
end

function cleanup --on-event fish_exit
    echo "bye"
end

function __fish_helper --description "private helper"
    echo "internal"
end

greet "fish"
cleanup

# Builtins
abbr -a gco 'git checkout'
alias ll 'ls -la'
cd /tmp
echo "hello"
printf "%s\n" "hello"
read -l -P "name? " user_name
source /dev/null
eval 'echo hi'
exec /bin/sh
emit custom_event
builtin echo "raw"
command ls
test -f /etc/passwd
time sleep 0

# Argument parsing
function my_cmd
    argparse 'h/help' 'n/name=' -- $argv
    or return
    if set -q _flag_help
        echo "usage: my_cmd [-h] [-n NAME]"
        return 0
    end
    echo "name: $_flag_name"
end

# String operations
string match -r '^\d+$' "123"
string replace -a old new "old string old"
string split , "a,b,c"
string join \n one two three
string length "hello"
string sub -s 2 -l 3 "hello"
string trim "  hello  "

# Math
math "2 + 3"
math "sqrt(144)"

# Status
status is-interactive
status filename

# Redirections
echo "out" > /dev/null
echo "append" >> /tmp/log
echo "stderr" 2>/dev/null
echo "both" &>/dev/null

# Pipelines
echo hello | cat
ls | string match '*.txt'

# Path manipulation
fish_add_path /usr/local/bin
set -a PATH /opt/bin

# Completions
complete -c my_cmd -s h -l help -d "Show help"
complete -c my_cmd -s n -l name -r -d "Set name"

# Event handlers
function on_var_change --on-variable MY_VAR
    echo "MY_VAR changed to $MY_VAR"
end

# Jobs
sleep 10 &
jobs
fg
bg
wait
disown

# Exit
exit 0
