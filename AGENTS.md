# Rules for agents

## Code Style

- Limit lines to under 100 chars
- Always run `cargo +nightly fmt` before commits

Before commits, run the script scripts/check_line_length.py and fix any errors.

## Commits

Keep commit messages to a single, short line.
Do not commit unless asked to by the user.

## Basic tests

```
cargo test
```

## Testing

Do not attempt to compile directly. Instead use the Makefile targets
to test as we need to generate a genesis file beofre running.

```
make testnet_load_test
```

Note that the build takes a long time, so do not interrupt the build
so do not use sleep or timeout when running this test.

## Logs

The default log location is in

`~/.rbft/testnet/logs/`

## Pushing

Before pushing.

Run the code style checks.
Run cargo test
Run cargo clippy
Run `make testnet_load_test`
