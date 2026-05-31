import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Wayland

ShellRoot {
    Scope {
        id: root

        readonly property int barCount: 20
        readonly property real pillWidth: 112
        readonly property real pillHeight: 34
        readonly property real waveformWidth: 80
        readonly property real waveformHeight: 16
        readonly property real barWidth: 2.8
        readonly property real barGap: 1.25
        readonly property real barMinHeight: 3
        readonly property real barMaxHeight: 12
        readonly property real bottomMargin: 32
        readonly property int maxReconnectAttempts: 30
        readonly property int reconnectInitialDelayMs: 1000
        readonly property int reconnectMaxDelayMs: 8000
        readonly property var runtimeDir: Quickshell.env("XDG_RUNTIME_DIR")
        readonly property string socketUser: socketUserID()
        readonly property string socketPath: runtimeDir ? runtimeDir + "/dictator/osd.sock" : "/tmp/dictator-osd-" + socketUser + "/osd.sock"
        readonly property bool demoMode: parseBool(Quickshell.env("DICTATOR_OSD_DEMO"))
        readonly property bool debugMode: parseBool(Quickshell.env("DICTATOR_OSD_DEBUG"))
        readonly property string demoState: normalizeDemoState(Quickshell.env("DICTATOR_OSD_DEMO_STATE"))

        property bool socketRequested: false
        property bool reconnectExhausted: false
        property int reconnectAttempts: 0
        property string lastSocketError: ""
        property bool demoRunning: false
        property bool panelActive: false
        property bool shouldShow: false
        property real smoothedLevel: 0
        property string visualizerState: "idle"
        property var levels: []
        property real startedAt: Date.now()
        property real transcribingStartedAt: 0

        signal repaintRequested()

        Component.onCompleted: {
            resetLevels()
            if (demoMode) {
                startDemo()
            } else {
                requestSocketConnection()
            }
        }

        function clamp(value, min, max) {
            return Math.min(Math.max(value, min), max)
        }

        function debugLog(message) {
            if (debugMode) console.log("[dictator-osd] " + message)
        }

        function parseBool(value) {
            const normalized = String(value || "").toLowerCase()
            return normalized === "1" || normalized === "true" || normalized === "yes" || normalized === "on"
        }

        function socketUserID() {
            const user = sanitizeSocketUser(Quickshell.env("USER"))
            if (user.length > 0) return user
            const uid = sanitizeSocketUser(Quickshell.env("UID"))
            return uid.length > 0 ? uid : "unknown"
        }

        function sanitizeSocketUser(value) {
            const sanitized = String(value || "").replace(/[^A-Za-z0-9_-]/g, "")
            return sanitized
        }

        function isKnownState(value) {
            return value === "idle" || value === "recording" || value === "transcribing" || value === "typing" || value === "error"
        }

        function isVisibleState(value) {
            return value === "recording" || value === "transcribing" || value === "typing" || value === "error"
        }

        function isFiniteNumber(value) {
            return typeof value === "number" && isFinite(value)
        }

        function normalizeDemoState(value) {
            const normalized = String(value || "recording").toLowerCase()
            if (normalized === "recording" || normalized === "transcribing" || normalized === "typing" || normalized === "error") {
                return normalized
            }
            return "recording"
        }

        function hasDuration(event) {
            return isFiniteNumber(event.recording_duration_ms) && event.recording_duration_ms >= 0
        }

        function resetLevels() {
            const next = []
            for (let i = 0; i < barCount; i++) next.push(0)
            levels = next
            smoothedLevel = 0
            repaintRequested()
        }

        function setVisualizerState(value) {
            const previousState = visualizerState
            visualizerState = value
            debugLog("state=" + value)

            if (isVisibleState(value)) {
                panelActive = true
                shouldShow = true
                deactivateTimer.stop()
            } else {
                shouldShow = false
                deactivateTimer.restart()
                resetLevels()
            }

            if (value === "transcribing" && previousState !== "transcribing") {
                transcribingStartedAt = Date.now()
            }

            repaintRequested()
        }

        function requestSocketConnection() {
            demoRunning = false
            reconnectExhausted = false
            socketRequested = true
        }

        function reconnectDelay() {
            const delay = reconnectInitialDelayMs * Math.pow(1.35, reconnectAttempts)
            return Math.min(Math.round(delay), reconnectMaxDelayMs)
        }

        function scheduleReconnect() {
            if (demoRunning || reconnectTimer.running || reconnectExhausted) return

            socketRequested = false
            if (reconnectAttempts >= maxReconnectAttempts) {
                reconnectExhausted = true
                setVisualizerState("idle")
                console.warn("dictator OSD socket reconnect limit reached: " + lastSocketError)
                return
            }

            reconnectTimer.interval = reconnectDelay()
            reconnectAttempts += 1
            debugLog("reconnect attempt " + reconnectAttempts + " in " + reconnectTimer.interval + "ms")
            reconnectTimer.restart()
        }

        function startDemo() {
            reconnectTimer.stop()
            socketRequested = false
            reconnectAttempts = 0
            reconnectExhausted = false
            demoRunning = demoState === "recording"
            startedAt = Date.now()
            setVisualizerState(demoState)
        }

        function applyProtocolEvent(event) {
            if (!event || typeof event.type !== "string") return "invalid"

            if (event.type === "state") return applyStateEvent(event)
            if (event.type === "meter") return applyMeterEvent(event)

            return "ignored"
        }

        function applyStateEvent(event) {
            const value = String(event.value || "")
            if (!isKnownState(value)) return "invalid"

            if ((value === "recording" || value === "transcribing") && !hasDuration(event)) {
                return "invalid"
            }

            if (value === "error" && event.message !== undefined && typeof event.message !== "string") {
                return "invalid"
            }

            demoRunning = false
            setVisualizerState(value)
            return value
        }

        function applyMeterEvent(event) {
            if (!isFiniteNumber(event.rms) || !isFiniteNumber(event.peak)) return "invalid"
            if (visualizerState !== "recording") return "stale"

            demoRunning = false
            return pushRaw(event.rms, event.peak) ? "ok" : "invalid"
        }

        function applyProtocolLine(line) {
            const trimmed = line.trim()
            if (trimmed.length === 0) return "empty"

            try {
                const result = applyProtocolEvent(JSON.parse(trimmed))
                debugLog("event result=" + result + " line=" + trimmed)
                return result
            } catch (error) {
                debugLog("event result=invalid line=" + trimmed)
                return "invalid"
            }
        }

        function normalizeLevel(rms, peak) {
            const db = 20 * (Math.log(Math.max(rms, 0.00000001)) / Math.LN10)
            const rmsLevel = Math.pow(clamp((db + 50) / 38, 0, 1), 0.65)
            const peakLevel = Math.sqrt(clamp((peak - 0.03) / 0.77, 0, 1))
            return clamp(Math.max(rmsLevel * 0.88, peakLevel * 0.55), 0, 0.92)
        }

        function pushRaw(rms, peak) {
            if (isNaN(rms) || isNaN(peak)) return false

            const target = normalizeLevel(clamp(rms, 0, 1), clamp(peak, 0, 1))
            const alpha = target > smoothedLevel ? 0.55 : 0.25
            smoothedLevel = (smoothedLevel * (1 - alpha)) + (target * alpha)

            const next = levels.slice()
            while (next.length < barCount) next.unshift(0)
            next.push(smoothedLevel)
            while (next.length > barCount) next.shift()
            levels = next
            repaintRequested()
            return true
        }

        function displayLevel(level) {
            const floor = 0.2
            return Math.pow(clamp((level - floor) / (1 - floor), 0, 1), 1.35)
        }

        function demoSample(elapsedSeconds) {
            const phrase = Math.pow((Math.sin(elapsedSeconds * 1.7) * 0.5) + 0.5, 1.7)
            const syllable = Math.pow((Math.sin(elapsedSeconds * 11) * 0.5) + 0.5, 2.2)
            const consonant = Math.pow((Math.sin(elapsedSeconds * 29) * 0.5) + 0.5, 8)
            const rms = 0.004 + (phrase * 0.045) + (syllable * 0.025)
            const peak = Math.min((rms * 2.2) + (consonant * 0.24), 1)
            return { "rms": rms, "peak": peak }
        }

        function accentColor() {
            return visualizerState === "error" ? "#ff4f5f" : "#8be9ff"
        }

        function roundedRect(ctx, x, y, width, height, radius) {
            const r = Math.min(radius, width / 2, height / 2)
            ctx.beginPath()
            ctx.moveTo(x + r, y)
            ctx.lineTo(x + width - r, y)
            ctx.quadraticCurveTo(x + width, y, x + width, y + r)
            ctx.lineTo(x + width, y + height - r)
            ctx.quadraticCurveTo(x + width, y + height, x + width - r, y + height)
            ctx.lineTo(x + r, y + height)
            ctx.quadraticCurveTo(x, y + height, x, y + height - r)
            ctx.lineTo(x, y + r)
            ctx.quadraticCurveTo(x, y, x + r, y)
            ctx.closePath()
        }

        function drawWaveBars(ctx, width, height, levelsOverride, alphaScale) {
            const totalWidth = (barCount * barWidth) + ((barCount - 1) * barGap)
            const startX = (width - totalWidth) / 2
            const values = levelsOverride || levels
            const opacityScale = alphaScale === undefined ? 1 : alphaScale

            ctx.fillStyle = accentColor()
            for (let i = 0; i < barCount; i++) {
                const raw = i < values.length ? values[i] : 0
                const level = displayLevel(raw)
                const barHeight = barMinHeight + (level * (barMaxHeight - barMinHeight))
                const x = startX + (i * (barWidth + barGap))
                const y = (height - barHeight) / 2
                ctx.globalAlpha = Math.min(0.88, (0.39 + (level * 0.49)) * opacityScale)
                roundedRect(ctx, x, y, barWidth, barHeight, barWidth / 2)
                ctx.fill()
            }
            ctx.globalAlpha = 1
        }

        function drawTranscribing(ctx, width, height) {
            const elapsedSeconds = (Date.now() - transcribingStartedAt) / 1000
            drawWaveBars(ctx, width, height, levels, 0.38)

            const totalWidth = (barCount * barWidth) + ((barCount - 1) * barGap)
            const startX = (width - totalWidth) / 2
            const scanPosition = (elapsedSeconds * 12) % (barCount + 5)

            ctx.fillStyle = accentColor()
            for (let i = 0; i < barCount; i++) {
                const distance = Math.abs(i - scanPosition)
                const level = clamp(1 - (distance / 4), 0, 1)
                if (level <= 0) continue

                const barHeight = barMinHeight + (Math.pow(level, 0.8) * (barMaxHeight - barMinHeight))
                const x = startX + (i * (barWidth + barGap))
                const y = (height - barHeight) / 2
                ctx.globalAlpha = 0.18 + (level * 0.72)
                roundedRect(ctx, x, y, barWidth, barHeight, barWidth / 2)
                ctx.fill()
            }
            ctx.globalAlpha = 1
        }

        Timer {
            id: reconnectTimer
            repeat: false
            onTriggered: root.socketRequested = true
        }

        Timer {
            id: demoTimer
            interval: 16
            repeat: true
            running: root.demoRunning && root.shouldShow && root.visualizerState === "recording"
            triggeredOnStart: true
            onTriggered: {
                const sample = root.demoSample((Date.now() - root.startedAt) / 1000)
                root.pushRaw(sample.rms, sample.peak)
            }
        }

        Timer {
            id: transcribingAnimationTimer
            interval: 16
            repeat: true
            running: root.shouldShow && root.visualizerState === "transcribing"
            triggeredOnStart: true
            onTriggered: root.repaintRequested()
        }

        Timer {
            id: deactivateTimer
            interval: 140
            onTriggered: {
                if (!root.shouldShow) root.panelActive = false
            }
        }

        Socket {
            id: dictatorSocket
            path: root.socketPath
            connected: root.socketRequested

            onConnectedChanged: {
                if (connected) {
                    root.lastSocketError = ""
                    root.reconnectAttempts = 0
                    root.reconnectExhausted = false
                    root.debugLog("socket connected " + root.socketPath)
                } else if (root.socketRequested) {
                    root.scheduleReconnect()
                }
            }

            onError: function(error) {
                root.lastSocketError = String(error)
                root.debugLog("socket error " + root.lastSocketError)
                root.scheduleReconnect()
            }

            parser: SplitParser {
                splitMarker: "\n"

                onRead: function(data) {
                    root.applyProtocolLine(data)
                }
            }
        }

        LazyLoader {
            active: root.panelActive

            PanelWindow {
                anchors.bottom: true
                margins.bottom: root.bottomMargin
                exclusiveZone: 0
                aboveWindows: true
                focusable: false
                implicitWidth: root.pillWidth
                implicitHeight: root.pillHeight
                color: "transparent"
                mask: Region {}

                WlrLayershell.layer: WlrLayer.Overlay
                WlrLayershell.namespace: "dictator-osd"
                WlrLayershell.keyboardFocus: WlrKeyboardFocus.None

                Item {
                    anchors.fill: parent
                    opacity: root.shouldShow ? 1 : 0
                    scale: root.shouldShow ? 1 : 0.96

                    Behavior on opacity {
                        NumberAnimation {
                            duration: 90
                            easing.type: Easing.OutCubic
                        }
                    }

                    Behavior on scale {
                        NumberAnimation {
                            duration: 110
                            easing.type: Easing.OutCubic
                        }
                    }

                    Rectangle {
                        width: root.pillWidth
                        height: root.pillHeight
                        anchors.centerIn: parent
                        radius: height / 2
                        color: "#db050505"
                        border.color: "#24ffffff"
                        border.width: 1

                        Canvas {
                            id: waveform
                            width: root.waveformWidth
                            height: root.waveformHeight
                            anchors.centerIn: parent
                            antialiasing: true

                            Connections {
                                target: root
                                function onRepaintRequested() {
                                    waveform.requestPaint()
                                }
                            }

                            onPaint: {
                                const ctx = getContext("2d")
                                ctx.clearRect(0, 0, width, height)

                                if (root.visualizerState === "transcribing") {
                                    root.drawTranscribing(ctx, width, height)
                                } else {
                                    root.drawWaveBars(ctx, width, height)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
