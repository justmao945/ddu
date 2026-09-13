import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui
import "Model.js" as Model

// Usage: live plan usage for Kimi / Zhipu GLM / OpenCode Go / Command Code.
// Bar pill shows a brand avatar plus "<5h>/<weekly>" per enabled provider;
// click pops the panel (usage rows + settings). Right-click refreshes.
BarWidget {
  id: root
  moduleName: "just.usage"

  // bar.barForeground, not bar.foreground: the plugin facade receives the raw
  // theme bar text color on both, but barForeground is the transparency-aware
  // one the bar itself paints with.
  readonly property color foreground: bar ? bar.barForeground : Color.foreground
  readonly property color urgent: bar ? bar.urgent : Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family
  // Warn color: the theme's orange (yellow fallback) read from colors.toml;
  // Color has no orange role, and Color.accent is a selection blue here.
  property string warnColor: "#eb9b4a"

  FileView {
    id: colorsFile
    path: (Quickshell.env("HOME") || "") + "/.local/state/omarchy/current/theme/colors.toml"
    watchChanges: true
    printErrors: false
    onLoaded: {
      var w = Model.themeWarn(text())
      if (w !== "") root.warnColor = w
    }
  }

  Component.onCompleted: colorsFile.reload()

  // Kimi has no brand color in the Swift tray either (it uses the neutral
  // label color), so it is simply absent here and falls back to foreground.
  readonly property var brandColors: ({
    zhipu: "#296bfa",
    opencode: "#1eb373",
    commandcode: "#9961f5"
  })
  readonly property var urls: ({
    kimi: "https://www.kimi.com/membership/subscription?tab=quota",
    zhipu: "https://bigmodel.cn/coding-plan/personal/usage",
    opencode: "https://opencode.ai/go",
    commandcode: "https://commandcode.ai/usage"
  })

  property var report: ({})
  property string lastError: ""
  property bool refreshing: false
  property double nowMs: Date.now()

  function boolSetting(name, fallback) {
    var v = setting(name, fallback)
    if (typeof v === "boolean") return v
    var s = String(v).toLowerCase()
    return s !== "off" && s !== "false" && s !== "0" && s !== "no"
  }
  function strSetting(name, fallback) {
    var v = setting(name, fallback)
    return v === undefined || v === null ? fallback : String(v)
  }

  function fetchEnv() {
    var e = {}
    e.USAGE_KIMI_ENABLED = boolSetting("kimiEnabled", true) ? "On" : "Off"
    e.USAGE_ZHIPU_ENABLED = boolSetting("zhipuEnabled", true) ? "On" : "Off"
    e.USAGE_OPENCODE_ENABLED = boolSetting("opencodeEnabled", true) ? "On" : "Off"
    e.USAGE_COMMANDCODE_ENABLED = boolSetting("commandcodeEnabled", true) ? "On" : "Off"
    e.USAGE_KIMI_KEY = strSetting("kimiKey", "")
    e.USAGE_ZHIPU_KEY = strSetting("zhipuKey", "")
    e.USAGE_OPENCODE_KEY = strSetting("opencodeKey", "")
    e.USAGE_COMMANDCODE_KEY = strSetting("commandcodeKey", "")
    return e
  }

  function refreshIntervalMs() {
    var s = Number(setting("refreshIntervalSec", 120))
    if (!isFinite(s) || s < 30) s = 30
    return Math.min(600, Math.round(s)) * 1000
  }

  readonly property var enabledIds: {
    var list = []
    if (boolSetting("kimiEnabled", true)) list.push("kimi")
    if (boolSetting("zhipuEnabled", true)) list.push("zhipu")
    if (boolSetting("opencodeEnabled", true)) list.push("opencode")
    if (boolSetting("commandcodeEnabled", true)) list.push("commandcode")
    return list
  }

  function entry(id) {
    return report && report[id] ? report[id] : null
  }
  function entryWindows(id) {
    var e = entry(id)
    return e && Array.isArray(e.windows) ? e.windows : []
  }
  function tierColor(t) {
    if (t === "critical") return root.urgent
    if (t === "warn") return root.warnColor
    return root.foreground
  }
  function entryTier(id) {
    var e = entry(id)
    if (!e || e.error || entryWindows(id).length === 0) return "dim"
    return Model.worstTier(entryWindows(id))
  }
  function entryColor(id) {
    var t = entryTier(id)
    return t === "dim" ? root.dim : tierColor(t)
  }
  function entryText(id) {
    var ws = entryWindows(id)
    return Model.barText(Model.shortWindow(ws), Model.weeklyWindow(ws))
  }
  // Providers shown in the bar and the panel's usage view: enabled AND
  // holding a credential (settings key or CLI login) — unconfigured ones
  // live only in Settings. Empty → the pill degrades to a single "NA".
  readonly property var visibleIds: {
    var list = []
    for (var i = 0; i < enabledIds.length; i++) {
      var e = entry(enabledIds[i])
      if (e && e.authed === true) list.push(enabledIds[i])
    }
    return list
  }

  function openPlanUrl(id) {
    Qt.openUrlExternally(root.urls[id])
  }
  // Settings view open: no fetches at all (the panel needs a stable view
  // and must not fight its own text fields). Closing settings re-syncs.
  property bool settingsOpen: false

  // A request made while a fetch is still in flight (e.g. a toggle flipped
  // mid-refresh) would otherwise be silently dropped until the next cycle.
  property bool _refetchQueued: false

  function refresh() {
    if (root.settingsOpen) return
    if (fetchProc.running) {
      _refetchQueued = true
      return
    }
    _output = ""
    refreshing = true
    fetchProc.environment = root.fetchEnv()
    fetchProc.running = true
  }

  function applyOutput(raw) {
    var parsed = Model.parseOutput(raw)
    if (!parsed) {
      lastError = "Failed to read usage status"
      return
    }
    report = parsed
    lastError = ""
  }

  // Settings persistence: same contract the clock uses (bar.shell facade →
  // shell.updateEntryInline). The panel reads values back through `settings`
  // (re-injected by the bar when shell.json changes) and calls these setters;
  // field draft state lives in the panel so keystrokes never round-trip.
  function setSetting(name, value) {
    if (!bar || !bar.shell || typeof bar.shell.updateEntryInline !== "function") return
    var entry = { id: root.moduleName }
    for (var k in settings) if (k !== "id" && k !== name) entry[k] = settings[k]
    entry[name] = value
    bar.shell.updateEntryInline(root.moduleName, entry)
  }
  function setEnabled(id, on) {
    setSetting(id + "Enabled", on ? "On" : "Off")
    refresh()
  }
  function setKey(id, key) {
    setSetting(id + "Key", key)
    refresh()
  }
  function credentialHint(id, settingsKey) {
    if (String(settingsKey || "") !== "") return ""
    if (id === "opencode") return "Empty = reuse opencode CLI login"
    if (id === "commandcode") return "Empty = reuse cmd CLI login"
    return ""
  }

  // Shape contract for shell.summon/hide/toggle routing (Bar.findPanelWidget
  // requires open/close/opened on the bar-widget root).
  readonly property bool opened: panelLoader.item ? panelLoader.item.opened === true : false
  function open() { if (panelLoader.item) panelLoader.item.open() }
  function close() { if (panelLoader.item) panelLoader.item.close() }
  function togglePanel() { if (panelLoader.item) panelLoader.item.toggle() }
  readonly property bool popoutSwitchClosing: panelLoader.item ? panelLoader.item.popoutSwitchClosing === true : false
  function closeForPopoutSwitch() { if (panelLoader.item) panelLoader.item.closeForPopoutSwitch() }

  function injectPanel() {
    var target = panelLoader.item
    if (!target) return
    if ("bar" in target) target.bar = root.bar
    if ("settings" in target) target.settings = root.settings
    if ("anchorItem" in target) target.anchorItem = button
    if ("hostWidget" in target) target.hostWidget = root
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  property bool _bootstrapped: false
  // The timer's first tick fires before the bar injects settings, so the
  // startup fetch would run with empty keys and show NA until the next
  // cycle. Bootstrap on the first real settings injection, and re-fetch
  // (debounced — settings re-inject in bursts) whenever they change again,
  // so a freshly pasted key shows up without waiting a full cycle.
  onSettingsChanged: {
    injectPanel()
    if (!_bootstrapped) {
      _bootstrapped = true
      refresh()
      return
    }
    settingsRefresh.restart()
  }
  Timer {
    id: settingsRefresh
    interval: 1000
    onTriggered: root.refresh()
  }

  Loader {
    id: panelLoader
    active: true
    source: Qt.resolvedUrl("Panel.qml")
    visible: false
    onLoaded: {
      root.injectPanel()
      Qt.callLater(root.injectPanel)
    }
  }

  IpcHandler {
    target: "just.usage"
    function open(): void { root.open() }
    function close(): void { root.close() }
    function show(): void { root.open() }
    function hide(): void { root.close() }
    function toggle(): void { root.togglePanel() }
    function openProvider(id: string): string { if (panelLoader.item && panelLoader.item.openWith) panelLoader.item.openWith(id); return "ok" }
    function refresh(): string { root.refresh(); return "ok" }
  }

  Timer {
    id: tick
    interval: 30000
    repeat: true
    running: true
    onTriggered: root.nowMs = Date.now()
  }

  Timer {
    id: refreshTimer
    interval: root.refreshIntervalMs()
    repeat: true
    running: true
    onTriggered: root.refresh()
  }

  readonly property string scriptPath: String(Qt.resolvedUrl("usage_fetch.py")).replace(/^file:\/\//, "")

  property string _output: ""
  Process {
    id: fetchProc
    command: ["python3", root.scriptPath]
    stdout: StdioCollector { onStreamFinished: root._output = text }
    stderr: StdioCollector {}
    onExited: function(exitCode) {
      root.refreshing = false
      if (exitCode === 0) root.applyOutput(root._output)
      else root.lastError = "Usage fetch failed"
      if (root._refetchQueued) {
        root._refetchQueued = false
        Qt.callLater(root.refresh)
      }
    }
  }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: "usage"
    labelVisible: false
    hasVisualContent: true
    // The custom icon+text rows are wider than the hidden label, so the
    // slot must be sized from the row itself — otherwise the pill overflows
    // and paints over the neighbouring widgets.
    fixedWidth: pillRow.implicitWidth + Math.round(Style.space(17))

    onPressed: function(b) {
      if (b === Qt.RightButton) {
        root.refresh()
        return
      }
      if (b !== Qt.LeftButton) return
      // Segments open their own provider. The gaps between them are dead
      // space — a stray click must never expand to all four providers.
      // Only the unconfigured NA state falls back to the generic toggle.
      if (root.visibleIds.length === 0) root.togglePanel()
    }

    Row {
      id: pillRow
      anchors.centerIn: parent
      spacing: 10

      // Uniform bar icon: neutral badge tinted from the bar foreground —
      // same shape and tone for every provider, matching the bar's own
      // monochrome icons. Sized from the bar font so it scales with the
      // theme like every other widget glyph (a hard 12px looks tiny on a
      // scaled-up bar).
      component PillBadge: Rectangle {
        id: badge
        property string letter: ""
        property bool dimmed: false
        readonly property real side: Math.max(10, Math.round(button.fontSize * 1.05))
        width: side
        height: side
        radius: Math.max(2, Math.round(side / 4))
        anchors.verticalCenter: parent ? parent.verticalCenter : undefined
        color: Qt.alpha(root.foreground, dimmed ? 0.10 : 0.18)
        Text {
          anchors.centerIn: parent
          textFormat: Text.PlainText
          text: badge.letter
          color: badge.dimmed ? root.dim : root.foreground
          font.family: root.fontFamily
          font.pixelSize: Math.max(7, Math.round(badge.side * 0.66))
          font.bold: true
        }
      }

      Row {
        visible: root.visibleIds.length === 0
        spacing: 4

        PillBadge {
          letter: "?"
          dimmed: true
        }

        Text {
          textFormat: Text.PlainText
          anchors.verticalCenter: parent.verticalCenter
          text: "NA"
          color: root.dim
          font.family: button.fontFamily
          font.pixelSize: button.fontSize
          renderType: Text.NativeRendering
          verticalAlignment: Text.AlignVCenter
        }
      }

      Repeater {
        model: root.visibleIds
        Item {
          id: providerRow
          required property string modelData
          readonly property string pid: modelData
          width: providerRowLayout.implicitWidth
          height: providerRowLayout.implicitHeight

          // The slot-level modulePointer MouseArea swallows every press and
          // dispatches clicks through the bar's click-target registry — child
          // MouseAreas inside a plugin bar widget never see them. Registering
          // this row as a target makes the bar route its segment of the pill
          // here (registered after the button, and the registry matches
          // last-first, so segments win over the whole-button fallback).
          function triggerPress(button) {
            if (panelLoader.item && panelLoader.item.openWith)
              panelLoader.item.openWith(providerRow.pid)
          }
          function registerSelf() {
            if (!root.bar || !root.bar.registerClickTarget) return
            if (root.bar.unregisterClickTarget) root.bar.unregisterClickTarget(providerRow)
            root.bar.registerClickTarget(providerRow)
          }
          Component.onCompleted: registerSelf()
          Connections {
            target: root
            function onBarChanged() { providerRow.registerSelf() }
          }
          Component.onDestruction: if (root.bar && root.bar.unregisterClickTarget) root.bar.unregisterClickTarget(providerRow)

          Row {
            id: providerRowLayout
            spacing: 4

            PillBadge {
              letter: providerRow.pid === "kimi" ? "K" : providerRow.pid === "zhipu" ? "Z" : providerRow.pid === "opencode" ? "O" : "C"
            }

            Text {
              textFormat: Text.PlainText
              anchors.verticalCenter: parent.verticalCenter
              text: root.entryText(providerRow.pid)
              color: root.entryColor(providerRow.pid)
              font.family: button.fontFamily
              font.pixelSize: button.fontSize
              renderType: Text.NativeRendering
              verticalAlignment: Text.AlignVCenter
            }
          }
        }
      }
    }
  }
}
