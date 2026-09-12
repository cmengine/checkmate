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
CME_CLI ?= $(CME_ROOT)/target/debug/cme

.PHONY: all clean test schema-gen

all: $(TARGET)

# The §9.6 schema header, generated from THIS directory's engine.cm — the
# same file the cme-ffi build script feeds its copy of main.c. With the
# header present, main.c consumes it (CME_HOST_HAS_SCHEMA_GEN): REGISTER
# macro, typed pack/unpack helpers, and exact-arity interface calls.
chost_schema_gen.h: engine.cm $(CME_CLI)
	$(CME_CLI) codegen-c engine.cm > $@
	@echo "schema header generated and verified: $@"

$(TARGET): main.c chost_schema_gen.h $(LIBS)
	$(CC) $(CFLAGS) $(INCLUDES) -I. -DCME_HOST_HAS_SCHEMA_GEN -o $@ main.c $(LIBS)

test: $(TARGET)
	./$(TARGET)

schema-gen: chost_schema_gen.h

clean:
	rm -f $(TARGET) chost_schema_gen.h
