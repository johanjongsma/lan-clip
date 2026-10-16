import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

// lan-clip: clipboard sync between lan-mouse machines. The bar icon shows
// whether peers are reachable; left click opens the popup, right click pauses.
BarWidget {
  id: root
  moduleName: "jongsma.lan-clip"

  property bool popupOpen: false
  property bool installed: true
  property bool running: false
  property bool paused: false
  property var peers: []
  property var last: null
  property int concealed: 0

  readonly property bool opened: popupOpen
  readonly property int upCount: peers.filter(function(p) { return p.reachable }).length
  readonly property color fg: root.bar ? root.bar.barForeground : Color.foreground
  readonly property color dim: Qt.darker(root.bar ? root.bar.foreground : Color.foreground, 1.4)
  readonly property color urgent: root.bar ? root.bar.urgent : Color.urgent
  readonly property string icon: paused ? "󰅛" : "󰅇"
  readonly property string statusText: {
    if (!installed) return "lan-clip is not installed"
    if (!running) return "daemon not running"
    if (paused) return "paused"
    if (peers.length === 0) return "no lan-mouse peers"
    return upCount + " of " + peers.length + " peer" + (peers.length === 1 ? "" : "s") + " connected"
  }
  readonly property bool degraded: !running || (!paused && peers.length > 0 && upCount === 0)

  // ~/.local/bin and ~/.cargo/bin are not always on the shell's PATH.
  function cli(args) {
    return ["sh", "-c", "PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"; command -v lan-clip >/dev/null || exit 127; exec lan-clip " + args]
  }

  function poll() {
    if (statusProc.running) return
    statusProc.command = cli("status --json")
    statusProc.running = true
  }

  function togglePause() {
    paused = !paused
    actionProc.command = cli("toggle")
    actionProc.running = true
  }

  function ago(t) {
    if (!t) return "never"
    var s = Math.max(0, Math.round(Date.now() / 1000 - t))
    if (s < 60) return s + "s ago"
    if (s < 3600) return Math.round(s / 60) + "m ago"
    return Math.round(s / 3600) + "h ago"
  }

  function open() { popupOpen = true }
  function close() { popupOpen = false }

  visible: true
  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  Timer {
    interval: root.popupOpen ? 1000 : 4000
    repeat: true
    running: true
    triggeredOnStart: true
    onTriggered: root.poll()
  }

  Process {
    id: statusProc
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        try {
          var s = JSON.parse(String(text || ""))
          root.running = true
          root.paused = !!s.paused
          root.peers = s.peers || []
          root.last = s.last || null
          root.concealed = s.concealed_skipped || 0
        } catch (e) {
          root.running = false
        }
      }
    }
    onExited: function(code) {
      root.installed = code !== 127
      if (code !== 0) root.running = false
    }
  }

  IpcHandler {
    target: "jongsma.lan-clip"

    function open() { root.open() }
    function close() { root.close() }
    function toggle() { root.popupOpen = !root.popupOpen }
    function togglePause() { root.togglePause() }
  }

  Process {
    id: actionProc
    onExited: root.poll()
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: root.icon
    dimmed: root.paused || !root.running
    active: root.degraded && root.installed
    tooltipText: "Clipboard sync: " + root.statusText
    onPressed: function(b) {
      if (b === Qt.RightButton) root.togglePause()
      else root.popupOpen = !root.popupOpen
    }
  }

  PopupCard {
    id: popup
    anchorItem: button
    bar: root.bar
    owner: root
    open: root.popupOpen
    contentWidth: popup.fittedContentWidth(Style.space(380))
    contentHeight: popup.fittedContentHeight(column.implicitHeight)

    Column {
      id: column
      anchors.fill: parent
      spacing: Style.space(12)

      Item {
        width: parent.width
        implicitHeight: Math.max(heroIcon.implicitHeight, heroLabels.implicitHeight, syncSwitch.implicitHeight)

        Text {
          id: heroIcon
          anchors.left: parent.left
          anchors.verticalCenter: parent.verticalCenter
          textFormat: Text.PlainText
          text: root.icon
          color: root.bar ? root.bar.foreground : Color.foreground
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.display
          opacity: root.paused || !root.running ? 0.5 : 1.0
        }

        ToggleSwitch {
          id: syncSwitch
          visible: root.running
          checked: !root.paused
          foreground: root.bar ? root.bar.foreground : Color.foreground
          anchors.right: parent.right
          anchors.verticalCenter: parent.verticalCenter
          onToggled: root.togglePause()
        }

        Column {
          id: heroLabels
          anchors.left: heroIcon.right
          anchors.leftMargin: Style.space(14)
          anchors.right: parent.right
          anchors.rightMargin: syncSwitch.visible ? syncSwitch.width + Style.space(12) : 0
          anchors.verticalCenter: parent.verticalCenter
          spacing: Style.space(2)

          Text {
            width: parent.width
            text: "Clipboard sync"
            color: root.bar ? root.bar.foreground : Color.foreground
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.title
            font.bold: true
            elide: Text.ElideRight
          }

          Text {
            width: parent.width
            textFormat: Text.PlainText
            text: root.statusText.toUpperCase()
            color: root.degraded ? root.urgent : root.dim
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.caption
            font.bold: true
            font.letterSpacing: 1.2
            elide: Text.ElideRight
          }
        }
      }

      PanelSeparator {
        foreground: root.bar ? root.bar.foreground : Color.foreground
      }

      Text {
        width: parent.width
        visible: !root.running
        textFormat: Text.PlainText
        wrapMode: Text.WordWrap
        text: root.installed
          ? "Start it with: systemctl --user start lan-clip"
          : "Install the lan-clip daemon on this machine and on each lan-mouse peer, then run: lan-clip install"
        color: root.dim
        font.family: root.bar ? root.bar.fontFamily : Style.font.family
        font.pixelSize: Style.font.bodySmall
      }

      Repeater {
        model: root.running ? root.peers : []

        Item {
          width: column.width
          implicitHeight: peerName.implicitHeight

          Text {
            id: peerName
            anchors.left: parent.left
            anchors.right: peerState.left
            anchors.rightMargin: Style.space(8)
            textFormat: Text.PlainText
            text: (modelData.reachable ? "● " : "○ ") + modelData.name
            color: root.bar ? root.bar.foreground : Color.foreground
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.body
            elide: Text.ElideRight
          }

          Text {
            id: peerState
            anchors.right: parent.right
            textFormat: Text.PlainText
            text: modelData.reachable ? (modelData.address || "") : "unreachable"
            color: modelData.reachable ? root.dim : root.urgent
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.bodySmall
          }
        }
      }

      Column {
        width: parent.width
        visible: root.running && root.last !== null
        spacing: Style.space(2)

        Text {
          width: parent.width
          textFormat: Text.PlainText
          text: root.last
            ? (root.last.direction === "sent" ? "Sent " : "Received ") + root.last.kind
              + (root.last.direction === "sent" ? " to " : " from ") + root.last.peer + " · " + root.ago(root.last.at)
            : ""
          color: root.dim
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.caption
          elide: Text.ElideRight
        }

        Text {
          width: parent.width
          textFormat: Text.PlainText
          text: root.last ? root.last.summary : ""
          color: root.bar ? root.bar.foreground : Color.foreground
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.body
          wrapMode: Text.WordWrap
          maximumLineCount: 3
          elide: Text.ElideRight
        }
      }

      Text {
        width: parent.width
        visible: root.running && root.concealed > 0
        textFormat: Text.PlainText
        text: root.concealed + " password-manager cop" + (root.concealed === 1 ? "y" : "ies") + " kept on this machine"
        color: root.dim
        font.family: root.bar ? root.bar.fontFamily : Style.font.family
        font.pixelSize: Style.font.caption
      }
    }
  }
}
