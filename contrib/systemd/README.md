# systemd User Service for PWSW

This directory contains a systemd user service unit for automatically starting the PWSW daemon.

## Installation

### 1. Install the service file

Copy the service file to your systemd user directory:

```bash
mkdir -p ~/.config/systemd/user
cp contrib/systemd/pwsw.service ~/.config/systemd/user/
```

### 2. Update the ExecStart path (if needed)

If you installed `pwsw` somewhere other than `~/.cargo/bin/pwsw`, edit the service file:

```bash
# Edit the ExecStart line to point to your pwsw binary
nano ~/.config/systemd/user/pwsw.service
```

Common installation paths:
- `~/.cargo/bin/pwsw` (default for `cargo install --path .`)
- `/usr/local/bin/pwsw` (system-wide installation)
- Custom path specified during installation

### 3. Reload systemd and enable the service

```bash
# Reload systemd to recognize the new service
systemctl --user daemon-reload

# Enable the service to start on login
systemctl --user enable pwsw.service

# Start the service now
systemctl --user start pwsw.service
```

## Managing the Service

### Check status

```bash
systemctl --user status pwsw.service
```

### View logs

```bash
# View recent logs
journalctl --user -u pwsw.service

# Follow logs in real-time
journalctl --user -u pwsw.service -f

# View logs since boot
journalctl --user -u pwsw.service -b
```

### Start/Stop/Restart

```bash
# Start the service
systemctl --user start pwsw.service

# Stop the service
systemctl --user stop pwsw.service

# Restart the service
systemctl --user restart pwsw.service
```

### Enable/Disable auto-start

```bash
# Enable (start on login)
systemctl --user enable pwsw.service

# Disable (don't start on login)
systemctl --user disable pwsw.service
```

## Uninstallation

```bash
# Stop and disable the service
systemctl --user stop pwsw.service
systemctl --user disable pwsw.service

# Remove the service file
rm ~/.config/systemd/user/pwsw.service

# Reload systemd
systemctl --user daemon-reload
```

## Troubleshooting

### Service fails to start

1. Check the binary path:
   ```bash
   which pwsw
   ```

2. Verify the binary works manually:
   ```bash
   pwsw daemon --foreground
   ```

3. Check the service logs:
   ```bash
   journalctl --user -u pwsw.service -n 50
   ```

### Service not starting on login

Make sure lingering is enabled (allows user services to run even when not logged in):

```bash
loginctl enable-linger $USER
```

### PipeWire not available

The service requires PipeWire to be running. Check PipeWire status:

```bash
systemctl --user status pipewire.service
```

If PipeWire is not running as a systemd service, you may need to adjust the `After=` and `Requires=` directives in the service file.

## Notes

- The service uses `--foreground` mode because systemd manages the daemon lifecycle
- Logs are sent to the systemd journal (use `journalctl` to view them)
- The service automatically restarts on failure with a 5-second delay
- The `%h` in `ExecStart` expands to your home directory

## Security Hardening (Optional)

The service file includes commented-out security options. To enable them, uncomment the lines in `pwsw.service`:

```ini
ProtectSystem=strict
ProtectHome=read-only
PrivateTmp=yes
NoNewPrivileges=yes
```

**Warning:** These options provide additional security isolation but may cause issues if PWSW needs access to specific system resources. Test thoroughly before enabling in production.
