# just commands

This project keeps operational scripts in `just/` and exposes them through the
root `justfile`.

## Common commands

Run from any directory inside the repository:

```sh
just build
just macos-deploy
just macos-restart
just macos-status
just macos-logs
just macos-follow
just macos-health
```

`just macos-deploy` builds the release binary, overwrites the installed binary
and static assets, writes a macOS LaunchDaemon plist, and starts the service.

The default deployment target is:

```text
Install dir: /usr/local/openaiplus
Plist:       /Library/LaunchDaemons/ai.actrue.openaiplus.plist
Logs:        /var/log/openaiplus/stdout.log
             /var/log/openaiplus/stderr.log
Config:      /usr/local/openaiplus/config.toml
```

If `config.toml` exists in the repository root, deploy copies it to the install
directory. Otherwise, the first deploy seeds the install config from
`config.example.toml` and keeps an existing installed config unchanged.

## Overrides

All paths are resolved by the script, so commands can run from different working
directories and machines. Override defaults with environment variables:

```sh
OPENAIPLUS_INSTALL_DIR=/opt/openaiplus just macos-deploy
OPENAIPLUS_LAUNCHD_LABEL=com.example.openaiplus just macos-status
OPENAIPLUS_LOG_DIR=/var/log/openaiplus-prod just macos-logs 200
OPENAIPLUS_CONFIG_SOURCE=/path/to/config.toml just macos-deploy
```

The service plist sets `OPENAIPLUS_CONFIG` to the installed config file and sets
`RUST_LOG` from the current `RUST_LOG` value, defaulting to `info`.

## Service management

```sh
just macos-start
just macos-stop
just macos-restart
just macos-uninstall
```

`just macos-uninstall` stops the LaunchDaemon and removes the plist only. It
keeps installed files and logs so config and diagnostics are not destroyed by
accident.
