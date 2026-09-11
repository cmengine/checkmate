# Makefile for the C host application (WHITEPAPER §13.2 consumer).
#
# Builds apps/c_host/main.c against the cme-ffi static library and runs it.
# The Rust test suite compiles this exact source file too (see
# crates/cme-ffi/build.rs), so `cargo test --workspace` exercises the same
# consumer end to end.

CME_ROOT ?= $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/../..)
CC ?= cc
CFLAGS ?= -std=c11 -Wall -Wextra
INCLUDES = -I$(CME_ROOT)/crates/cme-ffi/include
LIBDIR = $(CME_ROOT)/target/release
LIBS = $(LIBDIR)/libcme_ffi.a -lm -lpthread -ldl
TARGET = c_host_app

.PHONY: all clean test

all: $(TARGET)

$(TARGET): main.c $(LIBS)
	$(CC) $(CFLAGS) $(INCLUDES) -o $@ main.c $(LIBS)

test: $(TARGET)
	./$(TARGET)

clean:
	rm -f $(TARGET)
