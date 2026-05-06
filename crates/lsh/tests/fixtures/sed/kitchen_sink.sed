#!/usr/bin/sed -nf
# kitchen-sink sed fixture for lsh highlighting

# strip leading whitespace
s/^[ \t]*//

# normalise tabs to two spaces, globally
s/\t/  /g

# print only lines matching FOO, then keep going
/FOO/p

# delete blank lines
/^$/d

# range: from a marker to end-of-file
/BEGIN/,$ {
    s/old/new/2gI
    s|alt-delim|won't be highlighted|g
}

# transliterate vowels to uppercase via y command
y/aeiou/AEIOU/

# numeric address + last-line
1,$ {
    =
    p
}

# label + branch
:loop
N
s/\n/, /g
t loop

# backreferences in replacement
s/\(foo\)\(bar\)/\2-\1/
s/whole/[&]/g

# write the result to a file, then quit
w out.txt
q
