#!/usr/bin/env python3
"""GTK4 layer-shell overlay for Dictator streaming preview."""

from __future__ import annotations

import json
import os
import socket
import subprocess
from pathlib import Path
from typing import TYPE_CHECKING

import gi

gi.require_version("Gtk", "4.0")

try:
    gi.require_version("Gtk4LayerShell", "1.0")
    HAS_LAYER_SHELL = True
except ValueError:
    HAS_LAYER_SHELL = False

from gi.repository import GLib, Gtk

if HAS_LAYER_SHELL:
    from gi.repository import Gtk4LayerShell

if TYPE_CHECKING:
    pass

SOCKET_PATH = "/tmp/dictator-overlay.sock"


class DictatorOverlay(Gtk.Application):
    """GTK4 overlay application for Dictator streaming preview."""

    def __init__(self) -> None:
        super().__init__(application_id="com.dictator.overlay")
        self.window: Gtk.ApplicationWindow | None = None
        self.label: Gtk.Label | None = None
        self.socket_source: int | None = None
        self.server_socket: socket.socket | None = None
        self.client_socket: socket.socket | None = None
        self.text: str = ""
        self.stable_len: int = 0

    def do_activate(self) -> None:
        """Activate the application."""
        self.window = Gtk.ApplicationWindow(application=self)
        self.window.set_title("Dictator")
        self.window.set_default_size(600, 100)

        if HAS_LAYER_SHELL:
            Gtk4LayerShell.init_for_window(self.window)
            Gtk4LayerShell.set_layer(self.window, Gtk4LayerShell.Layer.OVERLAY)
            Gtk4LayerShell.set_keyboard_mode(
                self.window, Gtk4LayerShell.KeyboardMode.ON_DEMAND
            )
            Gtk4LayerShell.set_anchor(self.window, Gtk4LayerShell.Edge.BOTTOM, True)
            Gtk4LayerShell.set_margin(self.window, Gtk4LayerShell.Edge.BOTTOM, 50)

        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        box.set_margin_start(20)
        box.set_margin_end(20)
        box.set_margin_top(10)
        box.set_margin_bottom(10)

        self.label = Gtk.Label(label="Listening...")
        self.label.set_wrap(True)
        self.label.set_xalign(0)
        self.label.add_css_class("transcript")
        box.append(self.label)

        hint_label = Gtk.Label(label="Enter to confirm | Escape to cancel")
        hint_label.add_css_class("hint")
        box.append(hint_label)

        self.window.set_child(box)

        css_provider = Gtk.CssProvider()
        css_provider.load_from_string(
            """
            .transcript {
                font-size: 18px;
                font-weight: 500;
            }
            .hint {
                font-size: 12px;
                opacity: 0.7;
            }
            window {
                background-color: rgba(30, 30, 30, 0.95);
                border-radius: 10px;
            }
            """
        )
        Gtk.StyleContext.add_provider_for_display(
            self.window.get_display(),
            css_provider,
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION,
        )

        key_controller = Gtk.EventControllerKey()
        key_controller.connect("key-pressed", self.on_key_pressed)
        self.window.add_controller(key_controller)

        self.start_ipc_server()
        self.position_window()
        self.window.present()

    def position_window(self) -> None:
        """Position window near the focused application using Hyprland IPC."""
        if not HAS_LAYER_SHELL:
            return

        try:
            # Get active window info
            win_result = subprocess.run(
                ["hyprctl", "activewindow", "-j"],
                capture_output=True,
                text=True,
                timeout=1,
                check=False,
            )
            # Get monitor info for the active window
            mon_result = subprocess.run(
                ["hyprctl", "monitors", "-j"],
                capture_output=True,
                text=True,
                timeout=1,
                check=False,
            )

            if win_result.returncode != 0 or not win_result.stdout:
                return

            win_data = json.loads(win_result.stdout)
            win_x = win_data.get("at", [0, 0])[0]
            win_y = win_data.get("at", [0, 0])[1]
            win_h = win_data.get("size", [0, 0])[1]
            win_monitor = win_data.get("monitor", 0)

            # Find the monitor dimensions
            mon_height = 1080  # fallback
            mon_y_offset = 0
            if mon_result.returncode == 0 and mon_result.stdout:
                monitors = json.loads(mon_result.stdout)
                for mon in monitors:
                    if mon.get("id") == win_monitor or mon.get("name") == win_monitor:
                        mon_height = mon.get("height", 1080)
                        mon_y_offset = mon.get("y", 0)
                        break

            # Calculate desired position (below active window)
            desired_top = win_y + win_h + 10
            overlay_height = 100  # approximate

            # Check if overlay would go off-screen
            if desired_top + overlay_height > mon_y_offset + mon_height:
                # Fall back to bottom-anchored position (default from do_activate)
                return

            Gtk4LayerShell.set_anchor(
                self.window, Gtk4LayerShell.Edge.BOTTOM, False
            )
            Gtk4LayerShell.set_anchor(self.window, Gtk4LayerShell.Edge.TOP, True)
            Gtk4LayerShell.set_anchor(self.window, Gtk4LayerShell.Edge.LEFT, True)
            Gtk4LayerShell.set_margin(
                self.window, Gtk4LayerShell.Edge.TOP, desired_top
            )
            Gtk4LayerShell.set_margin(self.window, Gtk4LayerShell.Edge.LEFT, win_x)
        except (subprocess.SubprocessError, json.JSONDecodeError, KeyError):
            pass

    def start_ipc_server(self) -> None:
        """Start the Unix socket IPC server."""
        socket_path = Path(SOCKET_PATH)
        if socket_path.exists():
            socket_path.unlink()

        self.server_socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server_socket.bind(SOCKET_PATH)
        self.server_socket.listen(1)
        self.server_socket.setblocking(False)

        self.socket_source = GLib.io_add_watch(
            self.server_socket.fileno(),
            GLib.IO_IN,
            self.on_socket_connect,
        )

    def on_socket_connect(self, _fd: int, _condition: GLib.IOCondition) -> bool:
        """Handle new socket connection."""
        if self.server_socket is None:
            return False

        try:
            self.client_socket, _ = self.server_socket.accept()
            self.client_socket.setblocking(False)
            GLib.io_add_watch(
                self.client_socket.fileno(),
                GLib.IO_IN,
                self.on_socket_data,
            )
        except BlockingIOError:
            pass
        return True

    def on_socket_data(self, _fd: int, _condition: GLib.IOCondition) -> bool:
        """Handle incoming socket data."""
        if self.client_socket is None:
            return False

        try:
            data = self.client_socket.recv(4096)
            if not data:
                self.client_socket.close()
                self.client_socket = None
                return False

            msg = json.loads(data.decode("utf-8"))
            self.handle_message(msg)
        except (BlockingIOError, json.JSONDecodeError):
            pass
        except OSError:
            self.client_socket = None
            return False
        return True

    def handle_message(self, msg: dict) -> None:
        """Handle a message from the daemon."""
        msg_type = msg.get("type")

        if msg_type == "update":
            self.text = msg.get("text", "")
            self.stable_len = msg.get("stable_len", 0)
            self.update_label()

        elif msg_type == "show":
            if self.window:
                self.window.present()

        elif msg_type == "hide":
            if self.window:
                self.window.hide()

    def update_label(self) -> None:
        """Update the label with current text."""
        if self.label is None:
            return

        if not self.text:
            self.label.set_markup("<i>Listening...</i>")
            return

        stable = GLib.markup_escape_text(self.text[: self.stable_len])
        tentative = GLib.markup_escape_text(self.text[self.stable_len :])

        markup = f"{stable}<i>{tentative}</i>"
        self.label.set_markup(markup)

    def on_key_pressed(
        self,
        _controller: Gtk.EventControllerKey,
        keyval: int,
        _keycode: int,
        _state: int,
    ) -> bool:
        """Handle key press events."""
        if keyval == 65293:  # Return/Enter
            self.confirm()
            return True
        elif keyval == 65307:  # Escape
            self.cancel()
            return True
        return False

    def confirm(self) -> None:
        """Confirm the transcription."""
        if self.client_socket:
            try:
                self.client_socket.send(json.dumps({"type": "confirm"}).encode("utf-8"))
            except OSError:
                pass
        self.quit()

    def cancel(self) -> None:
        """Cancel the transcription."""
        if self.client_socket:
            try:
                self.client_socket.send(json.dumps({"type": "cancel"}).encode("utf-8"))
            except OSError:
                pass
        self.quit()

    def do_shutdown(self) -> None:
        """Clean up on shutdown."""
        if self.socket_source:
            GLib.source_remove(self.socket_source)

        if self.client_socket:
            self.client_socket.close()

        if self.server_socket:
            self.server_socket.close()

        socket_path = Path(SOCKET_PATH)
        if socket_path.exists():
            socket_path.unlink()

        Gtk.Application.do_shutdown(self)


def main() -> None:
    """Entry point."""
    app = DictatorOverlay()
    app.run()


if __name__ == "__main__":
    main()
