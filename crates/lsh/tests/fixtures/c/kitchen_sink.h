// Line comment
/* Single-line block comment */
/* Multi-line
   block comment
   continues here */

#ifndef KITCHEN_SINK_H_
#define KITCHEN_SINK_H_

#include <stdio.h>
#include <stdint.h>
#include <stdbool.h>
#include "local_header.h"

#ifdef _WIN32
#  include <windows.h>
#endif

#if defined(__GNUC__) || defined(__clang__)
#  define NOB_PRINTF_FORMAT(a, b) __attribute__((format(printf, a, b)))
#else
#  define NOB_PRINTF_FORMAT(a, b)
#endif

#define NOB_ARRAY_LEN(arr) (sizeof(arr) / sizeof((arr)[0]))
#define NOB_UNUSED(x)      ((void)(x))
#define NOB_ASSERT         assert
#define NOB_VA(...)        __VA_ARGS__

// Numeric literals
int dec       = 42;
int hex       = 0xFF;
int oct       = 0755;
int bin       = 0b1010;
unsigned long big = 1000000000UL;
float pi      = 3.14f;
double avo    = 6.022e23;
double hexf   = 0x1.8p+1;

// Char literals
char a    = 'a';
char nl   = '\n';
char zero = '\0';
char hex_c = '\x1b';
char esc  = '\\';

// String literals (with prefixes)
const char     *s     = "hello, world";
const char     *multi = "line\nbreak \"escaped\"";
const wchar_t  *w     = L"wide";
const char     *u8s   = u8"utf-8";
const char16_t *u16s  = u"utf-16";
const char32_t *u32s  = U"utf-32";

// Char literals (with prefixes and longer escapes)
char esc_x   = '\x1b';
char esc_oct = '\012';
wchar_t wc   = L'\xFF';

// Types and qualifiers
typedef struct Nob_Cmd {
    const char **items;
    size_t       count;
    size_t       capacity;
} Nob_Cmd;

typedef enum {
    NOB_INFO = 0,
    NOB_WARNING,
    NOB_ERROR,
} Nob_Log_Level;

// Function declaration with attribute
__attribute__((deprecated))
static inline int nob_cmd_run(Nob_Cmd *cmd, bool async) {
    if (cmd == NULL) return -1;
    for (size_t i = 0; i < cmd->count; ++i) {
        printf("%s ", cmd->items[i]);
    }
    return 0;
}

// Compound literal (C99)
Nob_Cmd empty_cmd = (Nob_Cmd){0};

// Control flow
void demo(int x) {
    switch (x) {
        case 1:  break;
        case 2:  goto end;
        default: x = -1;
    }
    do { x++; } while (x < 10);
    for (int i = 0; i < 10; ++i) {
        if (i == 5) continue;
        if (i == 8) return;
    }
end:
    return;
}

#endif // KITCHEN_SINK_H_

/* A backslash-newline continues a string literal; a bare unterminated one
   stops at the line end. */
static const char *continued = "one \
two";
static const char *broken = "unterminated
int after_broken;
