# kitchen-sink jq fixture for lsh highlighting

# numbers, strings, constants
def pi: 3.14159;
def hex: 0xFF;
def truthy: true;
def maybe: null;

# string interpolation, format strings
def greet($who): "hello, \($who)!" | @text;
def csv_row: [.a, .b, .c] | @csv;

# control flow
def classify(n):
  if n < 0 then "negative"
  elif n == 0 then "zero"
  else "positive"
  end;

# comprehension via reduce + map
def sum_evens: reduce .[] as $x (0; if $x % 2 == 0 then . + $x else . end);

# pipeline + builtins
.users
| map(select(.active))
| sort_by(.name)
| group_by(.team)
| map({team: .[0].team, count: length, names: map(.name)})

# field access, optional, slice, iterator
.foo.bar?
| .[2:5]
| .[]

# update assignments + alternative
.score //= 0
| .score += 1
| .name |= ascii_upcase

# variables
.items as $all
| $all | length

# try/catch
try (.bad | tonumber) catch "n/a"

# regex builtins
split(",") | map(test("^[0-9]+$"))

# Strings carry raw newlines, interpolation included.
| "line one
line two \(.field
+ 1) tail"
