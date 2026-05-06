#!/usr/bin/awk -f
# kitchen-sink awk fixture for lsh highlighting

BEGIN {
    FS = ","
    OFS = "\t"
    count = 0
    total = 0.0
    pi = 3.14159
    hex = 0xFF
    sci = 1.5e-3
    print "starting", FILENAME
}

function fmt(n,    s) {
    s = sprintf("%.2f", n)
    return s
}

# main loop -- one record per line
/^#/ { next }                       # skip comments
NF >= 3 && $2 ~ /^[0-9]+$/ {
    name = $1
    qty  = $2 + 0
    cost = $3 + 0.0
    items[name] += qty
    total += qty * cost
    count++
    if (qty > 100) {
        printf "bulk: %s x%d @%s\n", name, qty, fmt(cost)
    } else {
        printf "%s\t%d\t%s\n", name, qty, fmt(cost)
    }
}

END {
    for (k in items) {
        print k, items[k]
    }
    printf "total %d records, sum=%s\n", count, fmt(total)
    if (NR == 0) exit 1
}
