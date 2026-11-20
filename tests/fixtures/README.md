# Test fixtures

## `pwdump-alsa-profiles.json`

A trimmed, scrubbed `pw-dump` from a real desktop (onboard HD Audio
`alsa_card.pci-0000_0c_00.4` plus a GPU HDMI card `alsa_card.pci-0000_0a_00.1`,
PipeWire with WirePlumber), captured while the onboard card sat on the output-only
`output:iec958-stereo` profile with the microphone gone and `default.configured.audio.sink`
pointing at a node that no longer existed: the state the old profile-switching code left
behind.

Kept: only the two `alsa_card.*` `PipeWire:Interface:Device` objects (`EnumProfile` and
`Profile` params, minus routes), the `Audio/*` `PipeWire:Interface:Node` objects, and the
audio entries of the `default` `PipeWire:Interface:Metadata` object.

Scrubbing was done with an allow-list: every prop not needed by the parser was dropped,
including `object.serial`, `client.id`, `factory.id`, `module.id`, `device.bus-path`,
`device.sysfs.path`, `device.plugged.usec`, `device.vendor.*`/`device.product.*` ids,
all `alsa.*` props (long card names with IRQ/MMIO addresses, codec subsystem ids),
`api.alsa.card.longname`, `api.alsa.path`, `api.dbus.*`, and node format/latency params.
Clients, modules, factories, ports, links and application streams were removed entirely.

## `pwdump-synthetic.json`

Hand-written. Covers what the real dump lacks: a USB headset whose duplex profile is
`output:analog-stereo+input:mono-fallback` (currently on its input-only profile), a PCI
card where several profiles provide the same output with different inputs and
availabilities, a UCM (`HiFi`) card, and a Bluetooth card.
