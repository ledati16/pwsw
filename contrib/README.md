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

View logs:

```bash
journalctl --user -u pwsw.service -f
```

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
