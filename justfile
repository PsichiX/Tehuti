list:
  just --list

format:
  cargo fmt --all

build:
  cargo build --all --all-features
  cargo build --examples --all --all-features

test:
  cargo test --all --all-features -- --nocapture

miri:
  cargo +nightly miri test --manifest-path ./crates/_/Cargo.toml -- --nocapture
  
clippy:
  cargo clippy --all --all-features
  cargo clippy --tests --all --all-features

checks:
  just format
  just build
  just clippy
  just test
  just miri

clean:
  find . -name target -type d -exec rm -r {} +
  just remove-lockfiles

remove-lockfiles:
  find . -name Cargo.lock -type f -exec rm {} +

list-outdated:
  cargo outdated -R -w

update:
  cargo update --manifest-path ./crates/_/Cargo.toml --aggressive
  cargo update --manifest-path ./crates/mock/Cargo.toml --aggressive
  cargo update --manifest-path ./crates/socket/Cargo.toml --aggressive
  
# book:
#   mdbook build book

# book-dev:
#   mdbook watch book --open

build-samples:
  cargo build --manifest-path ./crates/samples/Cargo.toml --examples

run-sample NAME:
  #!/usr/bin/env bash
  SAMPLE=$(find ./target/debug/examples -type f -executable -name "{{NAME}}*" | head -n 1)
  if [ -z "$SAMPLE" ]; then
    echo "No sample found matching: {{NAME}}*"
    exit 1
  fi
  echo "Running sample: $SAMPLE"
  "$SAMPLE"

publish:
  cargo publish --no-verify --manifest-path ./crates/_/Cargo.toml
  sleep 1
  cargo publish --no-verify --manifest-path ./crates/mock/Cargo.toml
  sleep 1
  cargo publish --no-verify --manifest-path ./crates/socket/Cargo.toml
