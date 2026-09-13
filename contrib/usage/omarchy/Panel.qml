import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui
import "Model.js" as Model

// Popup for the usage bar widget. Data and fetching live on the host
// BarWidget (hostWidget); this panel renders usage rows plus a SETTINGS
// section (provider toggles, API keys, refresh interval) that persists
// through bar.shell.updateEntryInline.
Panel {
  id: root
  moduleName: "just.usage"
  ipcTarget: "just.usage"
  manageIpc: false

  property var anchorItem: null
  property var hostWidget: null
  readonly property var barIdentity: hostWidget || root

  readonly property color foreground: bar ? bar.barForeground : Color.foreground
  readonly property color urgent: bar ? bar.urgent : Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family

  readonly property var host: hostWidget
  // Every host read is guarded: hostWidget is injected shortly after
  // creation, and a binding that throws (host.names on a not-yet-injected
  // host) is discarded by QML and stays broken for the component's life.
  readonly property var report: (host && host.report !== undefined) ? host.report : ({})
  readonly property bool refreshing: !!(host && host.refreshing)
  readonly property string lastError: host && host.lastError !== undefined ? host.lastError : ""
  readonly property double nowMs: host && host.nowMs ? host.nowMs : Date.now()
  readonly property var enabledIds: (host && host.enabledIds) ? host.enabledIds : []
  // Usage view shows only configured providers; Settings shows all four.
  readonly property var visibleIds: (host && host.visibleIds) ? host.visibleIds : []
  readonly property var names: (host && host.names) ? host.names : ({})
  readonly property var brandColors: (host && host.brandColors) ? host.brandColors : ({})

  // Draft state for the text fields: committed to shell.json on
  // editingFinished (Enter/focus loss), so keystrokes never write config and
  // the `settings` re-injection never fights the caret.
  property var keyDrafts: ({})
  property bool showSettings: false
  // Auto-save: pasting a key without pressing Enter (or losing focus) must
  // still reach shell.json, so drafts commit after a short idle debounce.
  property string lastDraftId: ""
  property string lastDraftText: ""
  Timer {
    id: keyCommitTimer
    interval: 600
    onTriggered: {
      if (root.lastDraftId !== "") root.commitDraft(root.lastDraftId, root.lastDraftText)
      root.lastDraftId = ""
    }
  }
  readonly property var intervalOptions: [
    { value: "30", label: "30s" },
    { value: "60", label: "1 min" },
    { value: "120", label: "2 min" },
    { value: "300", label: "5 min" }
  ]

  // Which provider the usage view shows: "" = all configured ones. A bar
  // badge click passes its id; the generic toggle shows everything.
  property string selectedId: ""
  readonly property var shownIds: {
    if (root.selectedId === "") return root.visibleIds
    for (var i = 0; i < root.visibleIds.length; i++)
      if (root.visibleIds[i] === root.selectedId) return [root.selectedId]
    return []
  }

  // Reopening the panel must always land on the usage view, never the
  // settings page left over from the previous session.
  function open() {
    showSettings = false
    selectedId = ""
    root.controller.show()
  }
  function openWith(id) {
    if (root.opened && root.selectedId === id) {
      root.close()
      return
    }
    showSettings = false
    selectedId = id
    root.controller.show()
  }
  function close() { root.controller.hide() }
  function toggle() { root.opened ? root.close() : root.open() }
  function switchPanel(direction) {
    if (root.bar && typeof root.bar.switchPanelFrom === "function")
      return root.bar.switchPanelFrom(root.barIdentity, direction)
    return false
  }

  function tierColor(t) {
    if (t === "critical") return root.urgent
    if (t === "warn") return host ? host.warnColor : root.urgent
    return root.foreground
  }
  function entry(id) {
    return report && report[id] ? report[id] : null
  }
  function entryWindows(id) {
    var e = entry(id)
    return e && Array.isArray(e.windows) ? e.windows : []
  }
  function doRefresh() { if (host && host.refresh) host.refresh() }
  function openPlanUrl(id) { if (host && host.openPlanUrl) host.openPlanUrl(id) }
  function boolSetting(name, fallback) { return host ? host.boolSetting(name, fallback) : fallback }
  function strSetting(name, fallback) { return host ? host.strSetting(name, fallback) : fallback }
  function setEnabled(id, on) { if (host) host.setEnabled(id, on) }
  function setKey(id, key) { if (host) host.setKey(id, key) }
  function setInterval_(sec) { if (host) host.setSetting("refreshIntervalSec", sec) }
  function credentialHint(id, settingsKey) { return host ? host.credentialHint(id, settingsKey) : "" }

  // Live value; returns the draft when the field is being edited.
  function keyValue(id) {
    if (keyDrafts[id] !== undefined) return String(keyDrafts[id])
    return strSetting(id + "Key", "")
  }
  function draftChanged(id, text) {
    var next = {}
    for (var k in keyDrafts) next[k] = keyDrafts[k]
    next[id] = text
    keyDrafts = next
    lastDraftId = id
    lastDraftText = text
    keyCommitTimer.restart()
  }
  function commitDraft(id, text) {
    var next = {}
    for (var k in keyDrafts) next[k] = keyDrafts[k]
    delete next[id]
    keyDrafts = next
    if (text !== strSetting(id + "Key", "")) setKey(id, text)
  }

  onOpenedChanged: if (opened) { if (host) host.nowMs = Date.now(); doRefresh() }
  // While the Settings view is up the bar holds its refreshes — only the
  // configured interval timer keeps running, and it is ignored too (see
  // BarWidget.refresh()), so editing keys is never interrupted by churn.
  onShowSettingsChanged: {
    flick.contentY = 0
    if (host) {
      host.settingsOpen = showSettings
      // Leaving Settings: re-sync immediately instead of waiting a cycle.
      if (!showSettings && typeof host.refresh === "function") host.refresh()
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem
    owner: root.barIdentity
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(400))
    contentHeight: panel.fittedContentHeight(column.implicitHeight, Style.space(560))


    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      // A focused TextField simply takes activeFocus from the catcher, so
      // keys flow to the field while editing — no explicit guard needed.
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onTextKey: function(t) {
        if (t === "r" || t === "R") root.doRefresh()
        else if (t === "s" || t === "S") root.showSettings = !root.showSettings
      }

      Flickable {
        id: flick
        anchors.fill: parent
        contentWidth: width
        contentHeight: column.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        interactive: contentHeight > height
        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

        Column {
          id: column
          width: parent.width
          spacing: Style.space(12)

          PanelHero {
            width: parent.width
            title: "Usage"
            meta: root.refreshing ? "Refreshing…" : ("Updated " + Qt.formatTime(new Date(root.nowMs), "hh:mm"))
            foreground: root.foreground
            fontFamily: root.fontFamily
            iconComponent: Component {
              Text {
                textFormat: Text.PlainText
                text: "󰆼"
                color: root.foreground
                font.family: root.fontFamily
                font.pixelSize: Style.font.display
              }
            }
            trailingControl: Component {
              Row {
                spacing: Style.space(6)
                PanelActionButton {
                  iconText: "󰑓"
                  foreground: root.foreground
                  fontFamily: root.fontFamily
                  enabled: !root.refreshing
                  onClicked: root.doRefresh()
                }
                PanelActionButton {
                  iconText: "󰢻"
                  foreground: root.foreground
                  fontFamily: root.fontFamily
                  onClicked: root.showSettings = !root.showSettings
                }
              }
            }
          }

          Column {
            id: usageColumn
            visible: !root.showSettings
            width: parent.width
            spacing: Style.space(12)

            readonly property var editingField: null

            Text {
              visible: root.lastError !== ""
              width: parent.width
              textFormat: Text.PlainText
              text: root.lastError
              color: root.urgent
              font.family: root.fontFamily
              font.pixelSize: Style.font.bodySmall
              wrapMode: Text.WordWrap
            }

            Text {
              visible: root.shownIds.length === 0
              width: parent.width
              textFormat: Text.PlainText
              text: "NA — no provider configured yet. Open Settings (gear) to add an API key."
              color: root.dim
              font.family: root.fontFamily
              font.pixelSize: Style.font.body
              wrapMode: Text.WordWrap
              horizontalAlignment: Text.AlignHCenter
            }

            Repeater {
              model: root.shownIds
              Column {
                required property string modelData
                readonly property string pid: modelData
                readonly property var snap: root.entry(pid)
                readonly property var wins: root.entryWindows(pid)
                width: column.width
                spacing: Style.space(8)

                PanelSeparator { foreground: root.foreground }

                RowLayout {
                  width: parent.width
                  spacing: Style.space(8)

                  Rectangle {
                    width: 18
                    height: 18
                    radius: 4
                    // Kimi badge inverts: white tile, black letter.
                    color: pid === "kimi" ? "#ffffff" : (root.brandColors[pid] || root.foreground)
                    Text {
                      anchors.centerIn: parent
                      textFormat: Text.PlainText
                      text: pid === "kimi" ? "K" : pid === "zhipu" ? "Z" : pid === "opencode" ? "O" : "C"
                      color: pid === "kimi" ? "#000000" : "#ffffff"
                      font.family: root.fontFamily
                      font.pixelSize: 10
                      font.bold: true
                    }
                  }

                  Text {
                    textFormat: Text.PlainText
                    text: root.names[pid] || pid
                    color: root.foreground
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.body
                    font.bold: true
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                  }

                  Text {
                    visible: !!(snap && snap.plan)
                    textFormat: Text.PlainText
                    text: snap ? String(snap.plan || "") : ""
                    color: root.dim
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.caption
                    elide: Text.ElideRight
                    MouseArea {
                      anchors.fill: parent
                      hoverEnabled: true
                      cursorShape: Qt.PointingHandCursor
                      onClicked: root.openPlanUrl(pid)
                    }
                  }
                }

                Text {
                  visible: !!(snap && snap.error)
                  width: parent.width
                  textFormat: Text.PlainText
                  text: snap ? String(snap.error || "") : ""
                  color: root.urgent
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.caption
                  wrapMode: Text.WordWrap
                }

                Text {
                  visible: !(snap && snap.error) && wins.length === 0
                  width: parent.width
                  textFormat: Text.PlainText
                  text: root.refreshing ? "Loading…" : "No data yet."
                  color: root.dim
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.caption
                }

                Repeater {
                  model: wins
                  Column {
                    required property var modelData
                    readonly property var w: modelData
                    readonly property real pct: Math.max(0, Math.min(100, Number(w.pct || 0)))
                    readonly property string t: Model.tier(pct)
                    width: parent.width
                    spacing: Style.space(4)

                    RowLayout {
                      width: parent.width
                      spacing: Style.space(8)
                      Text {
                        textFormat: Text.PlainText
                        text: String(w.label || "")
                        color: root.dim
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.caption
                        Layout.fillWidth: true
                        elide: Text.ElideRight
                      }
                      Text {
                        textFormat: Text.PlainText
                        text: Math.round(pct) + "%"
                        color: root.tierColor(t)
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.caption
                      }
                    }

                    Rectangle {
                      width: parent.width
                      height: 6
                      radius: 3
                      color: Qt.darker(root.foreground, 2.2)
                      Rectangle {
                        width: parent.width * pct / 100
                        height: parent.height
                        radius: parent.radius
                        color: root.tierColor(t)
                      }
                    }

                    Text {
                      visible: Model.countdown(w.resetTs, root.nowMs) !== ""
                      width: parent.width
                      textFormat: Text.PlainText
                      text: Model.countdown(w.resetTs, root.nowMs)
                      color: root.dim
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.caption
                    }
                  }
                }
              }
            }
          }

          // ---------------- Settings ----------------
          Column {
            visible: root.showSettings
            width: parent.width
            spacing: Style.space(10)

            PanelSectionHeader {
              text: "PROVIDERS"
              foreground: root.foreground
              fontFamily: root.fontFamily
            }

            Repeater {
              model: ["kimi", "zhipu", "opencode", "commandcode"]
              Column {
                required property string modelData
                readonly property string pid: modelData
                readonly property bool on: root.boolSetting(pid + "Enabled", true)
                width: parent.width
                spacing: Style.space(6)

                RowLayout {
                  width: parent.width
                  spacing: Style.space(8)

                  Rectangle {
                    width: 16
                    height: 16
                    radius: 4
                    color: root.brandColors[pid] || root.foreground
                    Text {
                      anchors.centerIn: parent
                      textFormat: Text.PlainText
                      text: pid === "kimi" ? "K" : pid === "zhipu" ? "Z" : pid === "opencode" ? "O" : "C"
                      color: "#ffffff"
                      font.family: root.fontFamily
                      font.pixelSize: 9
                      font.bold: true
                    }
                  }

                  Text {
                    textFormat: Text.PlainText
                    text: root.names[pid] || pid
                    color: root.foreground
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.body
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                  }

                  ToggleSwitch {
                    checked: root.boolSetting(pid + "Enabled", true)
                    foreground: root.foreground
                    accent: Color.accent
                    interactive: !root.refreshing
                    onToggled: root.setEnabled(pid, !checked)
                  }
                }

                TextField {
                  width: parent.width
                  password: true
                  placeholderText: root.credentialHint(pid, root.strSetting(pid + "Key", "")) || "API key"
                  text: root.keyValue(pid)
                  foreground: root.foreground
                  font.family: root.fontFamily
                  // Refresh runs in a background process and never blocks
                  // the UI — the field stays editable throughout.
                  enabled: true
                  // textEdited (user input only), NOT textChanged: text is
                  // bound to keyValue() which depends on the drafts written
                  // here — textChanged would form a binding loop that churns
                  // commits and can overwrite a just-pasted key.
                  onTextEdited: root.draftChanged(pid, text)
                  onEditingFinished: root.commitDraft(pid, text)
                }
              }
            }

            PanelSeparator { foreground: root.foreground }
            PanelSectionHeader {
              text: "REFRESH INTERVAL"
              foreground: root.foreground
              fontFamily: root.fontFamily
            }

            ButtonGroup {
              width: parent.width
              options: root.intervalOptions
              value: String(root.host ? Math.round(root.host.refreshIntervalMs() / 1000) : "120")
              foreground: root.foreground
              background: Color.popups.background
              fontFamily: root.fontFamily
              onChanged: function(value) { root.setInterval_(parseInt(value, 10)) }
            }

            Text {
              width: parent.width
              textFormat: Text.PlainText
              text: "Settings live in ~/.config/omarchy/shell.json (bar layout entry \"just.usage\"). Keys are stored in plain text — same as the macOS app."
              color: root.dim
              font.family: root.fontFamily
              font.pixelSize: Style.font.caption
              wrapMode: Text.WordWrap
            }
          }
        }
      }
    }
  }
}
