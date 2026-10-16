import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import St from 'gi://St';
import Cairo from 'cairo';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const APP = 'io.github.justwhisper.App';
const PATH = '/io/github/justwhisper/App';
const IFACE = 'io.github.justwhisper.Overlay';

export default class JustWhisperOverlay extends Extension {
    enable() {
        this._state = 'hidden';
        this._levels = Array(21).fill(0);
        this._targets = Array(21).fill(0);
        this._width = 172;
        this._lastUpdate = 0;
        this._area = new St.DrawingArea({
            width: 208, height: 64, reactive: false, can_focus: false,
            visible: false, opacity: 0,
        });
        this._paintId = this._area.connect('repaint', () => this._paint());
        // No keyboard grab, window activation, or pointer input region.
        // GNOME 50 removed affectsInputRegion from addChrome. This non-reactive
        // actor can sit directly in uiGroup without reserving any input region.
        Main.uiGroup.add_child(this._area);
        this._subscription = Gio.DBus.session.signal_subscribe(
            APP, IFACE, 'Update', PATH, null, Gio.DBusSignalFlags.NONE,
            (_connection, _sender, _path, _interface, _signal, parameters) => {
                const [state, levels] = parameters.deep_unpack();
                if (!['hidden', 'recording', 'processing'].includes(state))
                    return;
                this._lastUpdate = GLib.get_monotonic_time();
                this._targets = Array.from({length: 21}, (_, i) =>
                    Math.max(0, Math.min(1, Number(levels[i]) || 0)));
                this._setState(state);
            });
        this._watch = Gio.bus_watch_name(Gio.BusType.SESSION, APP,
            Gio.BusNameWatcherFlags.NONE, () => {}, () => this._setState('hidden'));
        this._timer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 16, () => {
            if (this._state !== 'hidden') {
                if (GLib.get_monotonic_time() - this._lastUpdate > 3_000_000) {
                    this._setState('hidden');
                    return GLib.SOURCE_CONTINUE;
                }
                const targetWidth = this._state === 'processing' ? 48 : 172;
                this._width += (targetWidth - this._width) * 0.18;
                this._levels = this._levels.map((value, i) =>
                    value + (this._targets[i] - value) * 0.3);
                this._area.queue_repaint();
            }
            return GLib.SOURCE_CONTINUE;
        });
        this._monitorId = Main.layoutManager.connect('monitors-changed', () => this._position());
    }

    _position() {
        const index = global.display.focus_window?.get_monitor()
            ?? Main.layoutManager.primaryIndex;
        const monitor = Main.layoutManager.monitors[index] ?? Main.layoutManager.primaryMonitor;
        if (monitor)
            this._area.set_position(monitor.x + (monitor.width - 208) / 2,
                monitor.y + monitor.height - 120);
    }

    _setState(state) {
        if (!this._area || this._state === state)
            return;
        this._state = state;
        this._area.remove_all_transitions();
        if (state === 'hidden') {
            this._area.ease({opacity: 0, duration: 140,
                onComplete: () => {
                    if (this._state === 'hidden')
                        this._area?.hide();
                }});
        } else {
            this._position();
            this._area.show();
            this._area.ease({opacity: 255, duration: 140,
                mode: Clutter.AnimationMode.EASE_OUT_QUAD});
        }
    }

    _paint() {
        const cr = this._area.get_context();
        const [w, h] = this._area.get_surface_size();
        // Surface size can include GNOME's display scale.
        cr.scale(w / 208, h / 64);
        const width = this._width;
        const x = (208 - width) / 2;
        const y = 8;
        const r = 24;
        cr.setSourceRGBA(0.015, 0.015, 0.02, 0.97);
        cr.arc(x + width - r, y + r, r, -Math.PI / 2, Math.PI / 2);
        cr.arc(x + r, y + r, r, Math.PI / 2, 3 * Math.PI / 2);
        cr.closePath();
        cr.fillPreserve();
        cr.setSourceRGBA(1, 1, 1, 0.16);
        cr.setLineWidth(1);
        cr.stroke();
        cr.setSourceRGBA(1, 1, 1, 1);
        cr.setLineWidth(2.5);
        cr.setLineCap(Cairo.LineCap.ROUND);
        if (this._state === 'processing') {
            const angle = GLib.get_monotonic_time() / 1_000_000 * 4;
            cr.arc(104, 32, 9, angle, angle + 4.3);
            cr.stroke();
        } else {
            for (let i = 0; i < 21; i++) {
                const height = 3 + this._levels[i] * 25;
                const bx = 104 + (i - 10) * 6;
                cr.moveTo(bx, 32 - height / 2);
                cr.lineTo(bx, 32 + height / 2);
                cr.stroke();
            }
        }
        cr.$dispose();
    }

    disable() {
        if (this._subscription)
            Gio.DBus.session.signal_unsubscribe(this._subscription);
        if (this._watch)
            Gio.bus_unwatch_name(this._watch);
        if (this._timer)
            GLib.source_remove(this._timer);
        if (this._monitorId)
            Main.layoutManager.disconnect(this._monitorId);
        if (this._area) {
            this._area.remove_all_transitions();
            this._area.disconnect(this._paintId);
            this._area.destroy();
        }
        this._area = null;
        this._subscription = this._watch = this._timer = this._monitorId = 0;
    }
}
