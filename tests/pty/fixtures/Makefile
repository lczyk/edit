# Build configuration
CC := gcc
CFLAGS ?= -O2 -Wall
SRC := main.c util.c
OBJ := $(SRC:.c=.o)
TARGET = app

.PHONY: all clean

all: $(TARGET)

$(TARGET): $(OBJ)
	$(CC) $(CFLAGS) -o $@ $^

%.o: %.c
	$(CC) $(CFLAGS) -c -o $@ $<

clean:
	rm -f $(OBJ) $(TARGET)

ifeq ($(DEBUG),1)
CFLAGS += -g
endif

include config.mk
