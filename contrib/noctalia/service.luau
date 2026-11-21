-- Follows `pwsw status --watch --json` (one JSON object per line, schema: STATUS STREAM
-- in `man pwsw`) and publishes a plain copy of each update as state "pwsw".
-- The command never exits while the daemon is down (it reports "stopped" and
-- reconnects), so one stream lives as long as the plugin; Noctalia ends it on reload.

local COMMAND = "pwsw status --watch --json"

-- `runStream` has no exit callback: if pwsw is missing, the shell prints one line in
-- the stopped line's shape instead (the widget then says so) rather than nothing
local NOT_FOUND = '{"text":"pwsw not found","tooltip":"pwsw is not on PATH",'
  .. '"class":"stopped","alt":"stopped","daemon":"stopped","paused":false,'
  .. '"sink":null,"source":null}'
local SHELL_COMMAND = "command -v pwsw >/dev/null 2>&1 || { printf '%s\\n' '"
  .. NOT_FOUND
  .. "'; exit 0; }; exec "
  .. COMMAND

-- Only plain values cross into the widget's VM (JSON null decodes to nothing useful)
local function plain(status: any)
  local function str(v: any): string
    return if type(v) == "string" then v else ""
  end
  return {
    text = str(status.text),
    tooltip = str(status.tooltip),
    class = if type(status.class) == "string" then status.class else "stopped",
    daemon = str(status.daemon),
    paused = status.paused == true,
  }
end

noctalia.state.set("pwsw", {
  text = "",
  tooltip = "Waiting for `" .. COMMAND .. "` (is pwsw on PATH?)",
  class = "stopped",
  daemon = "stopped",
  paused = false,
})

-- `runStream` runs its command through the shell
local started = noctalia.runStream(SHELL_COMMAND, function(line: string)
  local status = noctalia.json.decode(line)
  if type(status) == "table" then
    noctalia.state.set("pwsw", plain(status))
  end
end)
if not started then
  noctalia.log("pwsw: could not start `" .. COMMAND .. "`")
end
