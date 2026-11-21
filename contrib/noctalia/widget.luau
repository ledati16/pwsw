-- Bar widget: glyph + output name (plus a paused marker), tooltip from pwsw.
-- Left click: next sink. Right click: pause/resume the rules. Scroll: next/previous
-- sink. Middle click keeps Noctalia's default (widget settings).

-- Glyph per `class`. Only "volume-high" appears in Noctalia's docs; the others are
-- Tabler names. If one shows up blank, put another glyph name here.
local GLYPHS = {
  active = "volume-high",
  fallback = "volume",
  paused = "player-pause",
  stopped = "volume-off",
}

local function show(status: any)
  if type(status) ~= "table" then
    return
  end
  barWidget.setGlyph(GLYPHS[status.class] or GLYPHS.active)
  local text = status.text or ""
  if status.paused then
    text ..= " (paused)"
  end
  barWidget.setText(text)
  barWidget.setTooltip(status.tooltip or "")
end

local function pwsw(command: string)
  -- argv form: no shell involved; output is not needed
  noctalia.runAsync({ "pwsw", command })
end

noctalia.state.watch("pwsw", show)
show(noctalia.state.get("pwsw"))

-- Updates arrive through `state.watch`; `update()` only repaints now and then in
-- case the widget was (re)created after the last update
noctalia.setUpdateInterval(5000)

function update()
  show(noctalia.state.get("pwsw"))
end

function onClick()
  pwsw("next-sink")
end

function onRightClick()
  pwsw("toggle-pause")
end

function onScroll(axis: string, steps: number, startsGesture: boolean?)
  -- One switch per gesture: a touchpad swipe would otherwise switch many times
  if axis ~= "vertical" or steps == 0 or startsGesture == false then
    return
  end
  pwsw(if steps < 0 then "prev-sink" else "next-sink")
end
