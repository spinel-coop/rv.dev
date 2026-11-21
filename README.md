# rv.dev

This is the [rv.dev](https://rv.dev/) source repo, based on the Fastly Compute [Rust Starter Kit](https://github.com/fastly/compute-starter-kit-rust-default) repo. It is a simple Rust app that serves HTTP responses for a few hard-coded paths.

## Ruby downloads

Paths under `/ruby/` serve the precompiled rubies that `rv` installs. They come from the `rv-ruby` Fastly Object Storage bucket, which [rv-ruby](https://github.com/spinel-coop/rv-ruby) fills.

- `/ruby/v1/index.json` lists the newest build of every ruby.
- `/ruby/3.4.11/20261014/ruby-3.4.11.arm64_sonoma.tar.gz` is one build, and never changes.
- `/ruby/3.4.11/arm64_sonoma` is the newest build for that platform. The `x-rv-build`, `x-rv-sha256`, `x-rv-size` and `x-rv-filename` headers say which one it is.
- `/healthz` checks that the bucket can be reached and has an index.

## Local dev

1. `bin/setup` - Install the dependencies (Fastly CLI, Rust, etc)
2. `bin/test` - Run the tests
3. `bin/lint` - The linting safety net
