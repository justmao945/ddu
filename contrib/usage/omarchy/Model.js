function countdown(resetTs, nowMs) {
  if (!resetTs) return ""
  var diff = Math.max(0, Math.floor(resetTs * 1000 - nowMs) / 1000)
  if (diff <= 0) return "Reset"
  var totalMinutes = Math.floor(diff / 60)
  var days = Math.floor(totalMinutes / (60 * 24))
  var hours = Math.floor((totalMinutes % (60 * 24)) / 60)
  var minutes = totalMinutes % 60
  var parts = []
  if (days > 0) parts.push(days + "d")
  if (hours > 0) parts.push(hours + "h")
  if (minutes > 0 || parts.length === 0) parts.push(minutes + "m")
  var cd = parts.join(" ")
  return cd === "Reset" ? cd : "Resets in " + cd
}

// Warning color: theme orange preferred, yellow fallback ("orange = ...",
// "yellow = ..." in the active theme's colors.toml). Returns "" when missing.
function themeWarn(raw) {
  var text = String(raw || "")
  var m = text.match(/^\s*orange\s*=\s*["']?(#[0-9A-Fa-f]{6})/m)
  if (m) return m[1]
  m = text.match(/^\s*yellow\s*=\s*["']?(#[0-9A-Fa-f]{6})/m)
  if (m) return m[1]
  return ""
}

function tier(pct) {
  if (pct >= 90) return "critical"
  if (pct >= 70) return "warn"
  return "normal"
}

function worstTier(windows) {
  var worst = "normal"
  for (var i = 0; i < windows.length; i++) {
    var t = tier(windows[i].pct || 0)
    if (t === "critical") return "critical"
    if (t === "warn") worst = "warn"
  }
  return worst
}

function shortWindow(windows) {
  for (var i = 0; i < windows.length; i++)
    if (String(windows[i].label).indexOf("5h") >= 0) return windows[i]
  return windows.length > 0 ? windows[0] : null
}

function weeklyWindow(windows) {
  for (var i = 0; i < windows.length; i++)
    if (windows[i].label === "Weekly") return windows[i]
  return windows.length > 0 ? windows[0] : null
}

function pctText(w) {
  return w ? Math.round(w.pct || 0) + "%" : "—"
}

function barText(shortW) {
  // Bar pill shows just the 5-hour window; weekly stays in the panel.
  return pctText(shortW)
}

function parseOutput(raw) {
  try {
    var parsed = JSON.parse(String(raw || ""))
    if (parsed && typeof parsed === "object") return parsed
  } catch (e) {}
  return null
}

if (typeof module !== "undefined") {
  module.exports = {
    countdown: countdown, themeWarn: themeWarn, tier: tier, worstTier: worstTier,
    shortWindow: shortWindow, weeklyWindow: weeklyWindow,
    pctText: pctText, barText: barText, parseOutput: parseOutput
  }
}
