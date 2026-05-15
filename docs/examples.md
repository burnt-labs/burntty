# Examples

Build the binaries:

```sh
cargo build
```

Start a shell session and automate it:

```sh
target/debug/burntty spawn shell -- /bin/sh
target/debug/burntty send-line shell 'echo READY'
target/debug/burntty expect shell READY --include-history --timeout 2s
target/debug/burntty capture shell --last-lines 20
target/debug/burntty attach shell
target/debug/burntty kill shell
target/debug/burntty rm shell
```

Use a regex expect:

```sh
target/debug/burntty spawn py -- python3 -i
target/debug/burntty expect py '>>> ' --regex --include-history
target/debug/burntty send-line py '1 + 1'
target/debug/burntty expect py '^2$' --regex --include-history
```

Use an explicit socket:

```sh
BURNTTY_SOCKET=/tmp/my-burntty.sock target/debug/burntty list
```

Run the fake legacy fixture:

```sh
target/debug/burntty spawn legacy -- ./examples/legacy-login.sh
target/debug/burntty expect legacy 'login:' --include-history
target/debug/burntty send-line legacy admin
target/debug/burntty expect legacy 'password:' --include-history
target/debug/burntty send-secret legacy secret
target/debug/burntty expect legacy 'legacy>' --include-history
target/debug/burntty send-line legacy status
target/debug/burntty expect legacy OK --include-history
```
