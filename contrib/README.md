# Contrib Files

## Systemd Service

Start the daemon automatically on login:

```bash
systemctl --user enable --now pwsw.service
```

Check status:

```bash
systemctl --user status pwsw.service
```

View logs (also written to `~/.local/share/pwsw/daemon.log`, which the TUI shows):

```bash
journalctl --user -u pwsw.service -f
```

### What the unit does

- **Session lifecycle:** `WantedBy=`/`PartOf=`/`After=graphical-session.target`, so it
  starts once your graphical session (and `WAYLAND_DISPLAY`) is up and stops with it.
  Your compositor must start `graphical-session.target` (most do via a systemd/uwsm
  integration); otherwise start it manually.
- **PipeWire:** `Wants=` + `After=pipewire.service`. pwsw holds no PipeWire connection
  (it runs `pw-dump`/`pw-cli`/`pw-metadata` per action), so it is not bound to
  PipeWire's lifetime and keeps running across PipeWire restarts.
- **Restarts:** `Restart=on-failure` every 5 s, at most 5 starts per 60 s.
- **Sandbox:** `ProtectSystem=strict` (everything read-only, including your home)
  except `~/.local/share/pwsw` (log) and `$XDG_RUNTIME_DIR` (socket); the config is
  only read; private
  `/tmp`; `NoNewPrivileges`, `MemoryDenyWriteExecute`, `LockPersonality`,
  `RestrictRealtime`, `RestrictSUIDSGID`, `SystemCallArchitectures=native`,
  `UMask=0077`. `ExecStartPre=+mkdir -p …` creates the log directory outside
  the sandbox so a first start works.

If you set `XDG_CONFIG_HOME`/`XDG_DATA_HOME` to non-default locations, adjust
`ReadWritePaths=` and the `ExecStartPre=` line (the daemon only needs to *write* the
log directory; the config is read).

### Manual Installation (cargo users)

If you installed via `cargo install` instead of a package:

```bash
mkdir -p ~/.config/systemd/user
cp contrib/pwsw.service ~/.config/systemd/user/

# Edit ExecStart path from /usr/bin/pwsw to ~/.cargo/bin/pwsw
nano ~/.config/systemd/user/pwsw.service

systemctl --user daemon-reload
systemctl --user enable --now pwsw.service
```

Check the edited unit with `systemd-analyze --user verify ~/.config/systemd/user/pwsw.service`.

## Without systemd

`pwsw daemon` starts a detached background daemon (own process group, survives the
terminal closing) and waits until it answers on its socket. If it fails to start,
the error and the last lines of `~/.local/share/pwsw/daemon.log` are printed.
