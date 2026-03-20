.PHONY: build release run test lint fmt check clean install completions ci

build:
	cargo build

release:
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

install:
	cargo install --path .

completions:
	mkdir -p completions
	cargo run -- completions bash > completions/camctl.bash
	cargo run -- completions zsh > completions/_camctl
	cargo run -- completions fish > completions/camctl.fish

ci: lint test release
