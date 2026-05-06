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

func builtins() {
    s := make([]string, 0)
    s = append(s, "x")
    n := len(s)
    c := cap(s)
    _ = copy(s, s)
    delete(m, "key")
    if ok := true; ok {
        panic("boom")
    }
    _ = new(int)
    _ = complex(1, 2)
    _ = real(c)
    _ = imag(c)
    _ = close(ch)
    v := recover()
    _ = v
    println("print")
    print("no newline")
}

var (
    b bool
    by byte
    e error
    a any
    c comparable
)
