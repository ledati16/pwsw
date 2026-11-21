# pwsw plugin for Noctalia

A [Noctalia](https://docs.noctalia.dev/) v5 bar widget that shows the audio output pwsw
applied (and marks it while rule-driven switching is paused), live:

- a service runs `pwsw status --watch --json` and passes every update to the widget
  (nothing is polled; the command reconnects by itself when the daemon restarts)
- the widget shows a glyph and the output's name, with pwsw's tooltip (output,
  microphone, deciding rule)
- **left click**: `pwsw next-sink`; **right click**: `pwsw toggle-pause`;
  **scroll**: next / previous sink; middle click keeps Noctalia's default (widget
  settings)

**Untested:** this plugin is not exercised by pwsw's CI and was written from Noctalia's
plugin documentation, not tried against a running Noctalia. Reports and fixes welcome.

## Install

1. Make sure `pwsw` is on Noctalia's `PATH` and the daemon runs (`pwsw doctor`).
2. Copy this directory to Noctalia's local plugin directory:

   ```bash
   mkdir -p "${XDG_DATA_HOME:-$HOME/.local/share}/noctalia/plugins"
   cp -r contrib/noctalia "${XDG_DATA_HOME:-$HOME/.local/share}/noctalia/plugins/pwsw"
   ```

3. Enable the plugin once (Settings → Plugins), then add its **PWSW** widget to a bar.

Edits to the `.luau` files reload automatically; changes to `plugin.toml` are picked up
on the next config reload.

## Requirements and assumptions

`plugin_api = 24` (Noctalia 5.0.0-beta.9 or later) for the argument-list form of
`noctalia.runAsync`. Confirmed from the documentation: the manifest fields and the
`[[service]]`/`[[widget]]` entries, `noctalia.runStream` (one callback per stdout
line, the process is ended when the plugin reloads or stops), `noctalia.runAsync`,
`noctalia.json.decode`, `noctalia.state.set`/`get`/`watch`, `barWidget.setGlyph`/
`setText`/`setTooltip`, and the `onClick`/`onRightClick`/`onScroll` callbacks.

Not confirmed by the documentation:

- glyph names other than `volume-high` (edit `GLYPHS` in `widget.luau` if one is blank);
- whether `runStream` strips the trailing newline (the JSON decoder should not care);
- whether `state.watch` also fires for the value already set (the widget reads
  `state.get` once at load to be safe).

The widget does not colour itself (`barWidget.setColor`), so your bar's colours apply.
