# burntty

`burntty` is a persistent PTY session manager with an Expect-style automation API.

The daemon owns each PTY-backed process. CLI clients connect over a local Unix
socket to spawn sessions, send input, wait for output, capture buffered output,
resize terminals, and kill or remove sessions.

## Build

```sh
cargo build
```

## Release

GitHub Actions builds and publishes Linux binaries when a `burntty-v*` tag is
pushed:

```sh
git tag burntty-v0.1.0
git push origin burntty-v0.1.0
```

The container image downloads `burntty` and `burntty-daemon` from the latest
GitHub release by default. Pin a release during Docker builds with:

```sh
docker build --build-arg BURNTTY_VERSION=burntty-v0.1.0 -f containers/claude-code/Dockerfile .
```

## Example

```sh
cargo run -p burntty -- spawn shell -- /bin/sh
cargo run -p burntty -- send-line shell 'echo ready'
cargo run -p burntty -- expect shell ready --include-history
cargo run -p burntty -- capture shell --last-lines 20
cargo run -p burntty -- attach shell
cargo run -p burntty -- kill shell
```

The CLI starts `burntty-daemon` automatically on first use. Override the socket
path with `BURNTTY_SOCKET=/path/to/socket`.
