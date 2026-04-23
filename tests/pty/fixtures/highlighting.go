// Line comment
/* block comment */

package main

import (
    "fmt"
    "strings"
)

const MAX = 42

type Point struct {
    Name  string
    Value float64
}

func main() {
    s := "hello"
    raw := `raw string`
    r := 'x'
    n := 0xff
    xs := []int{1, 2, 3}

    for i, v := range xs {
        if v == 0 {
            continue
        }
        fmt.Println(i, v, strings.ToUpper(s), raw, r, n)
    }

    switch n {
    case 0:
        return
    default:
        fmt.Println("default")
    }
}
