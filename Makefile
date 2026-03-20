.PHONY: build build-release run test lint fmt check clean

build:
	cargo build

build-release:
	cargo build --release

run:
	cargo run -- $(ARGS)

test:
	cargo nextest run 2>/dev/null || cargo test

lint:
	cargo clippy -- -D warnings

fmt:
	cargo fmt

check: fmt lint test

clean:
	cargo clean
