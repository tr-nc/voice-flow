import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Pango from 'gi://Pango';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const BUS_NAME = 'dev.voiceflow.Preview';
const OBJECT_PATH = '/dev/voiceflow/Preview';
const PANEL_WIDTH = 720;
const LABEL_WIDTH = 684;
const MIN_HEIGHT = 94;
const MAX_HEIGHT = 280;
const VERTICAL_CHROME = 62;
const MAX_VISIBLE_CHARACTERS = 700;
const READY_PROMPT = 'Your mic is ready start speaking';

const PREVIEW_INTERFACE = `
<node>
  <interface name="dev.voiceflow.Preview">
    <method name="Ping">
      <arg type="b" name="ready" direction="out"/>
    </method>
    <method name="Show">
      <arg type="s" name="phase" direction="in"/>
      <arg type="s" name="text" direction="in"/>
      <arg type="s" name="message" direction="in"/>
    </method>
    <method name="Hide"/>
  </interface>
</node>`;

function visibleTail(value) {
    const characters = Array.from(value.trim());
    if (characters.length <= MAX_VISIBLE_CHARACTERS)
        return characters.join('');
    return `…${characters.slice(-MAX_VISIBLE_CHARACTERS).join('')}`;
}

class PreviewOverlay {
    constructor() {
        this._laters = global.compositor.get_laters();
        this._layoutId = 0;
        this._revealed = false;
        this._panel = new St.BoxLayout({
            style_class: 'voice-flow-preview',
            vertical: true,
            reactive: false,
            can_focus: false,
            visible: false,
            opacity: 0,
            width: PANEL_WIDTH,
        });
        this._label = new St.Label({
            style_class: 'voice-flow-preview-label',
            reactive: false,
            can_focus: false,
            x_expand: false,
            y_expand: false,
            width: LABEL_WIDTH,
        });
        const textActor = this._label.clutter_text;
        textActor.set_line_wrap(true);
        textActor.set_line_wrap_mode(Pango.WrapMode.WORD_CHAR);
        textActor.set_ellipsize(Pango.EllipsizeMode.NONE);
        textActor.set_single_line_mode(false);
        this._panel.add_child(this._label);
        Main.layoutManager.addTopChrome(this._panel, {
            affectsStruts: false,
            trackFullscreen: false,
        });
    }

    show(phase, text, message) {
        const content = this._contentFor(phase, text, message);
        if (!content) {
            this.hide();
            return;
        }

        this._label.text = content.text;
        this._label.remove_style_class_name('voice-flow-preview-label-prompt');
        this._label.remove_style_class_name('voice-flow-preview-label-final');
        if (content.style === 'prompt')
            this._label.add_style_class_name('voice-flow-preview-label-prompt');
        else if (content.style === 'final')
            this._label.add_style_class_name('voice-flow-preview-label-final');

        const needsReveal = !this._revealed;
        if (needsReveal)
            this._panel.opacity = 0;
        this._panel.show();
        this._panel.queue_relayout();
        this._scheduleLayout(needsReveal);
    }

    hide() {
        if (this._layoutId) {
            this._laters.remove(this._layoutId);
            this._layoutId = 0;
        }
        this._revealed = false;
        this._panel.opacity = 0;
        this._panel.hide();
    }

    destroy() {
        this.hide();
        Main.layoutManager.removeChrome(this._panel);
        this._panel.destroy();
    }

    _scheduleLayout(needsReveal) {
        if (this._layoutId)
            this._laters.remove(this._layoutId);
        this._layoutId = this._laters.add(Meta.LaterType.BEFORE_REDRAW, () => {
            this._layoutId = 0;
            const [, naturalLabelHeight] = this._label.get_preferred_height(LABEL_WIDTH);
            const height = Math.max(
                MIN_HEIGHT,
                Math.min(MAX_HEIGHT, Math.ceil(naturalLabelHeight + VERTICAL_CHROME)),
            );
            const [pointerX, pointerY] = global.get_pointer();
            const monitor = Main.layoutManager.monitors.find(candidate =>
                pointerX >= candidate.x &&
                pointerX < candidate.x + candidate.width &&
                pointerY >= candidate.y &&
                pointerY < candidate.y + candidate.height
            ) ?? Main.layoutManager.primaryMonitor;
            const x = monitor.x + Math.floor((monitor.width - PANEL_WIDTH) / 2);
            const y = monitor.y + Math.floor((monitor.height - height) / 2);

            this._panel.set_size(PANEL_WIDTH, height);
            this._panel.set_position(x, y);
            this._panel.set_clip(0, 0, PANEL_WIDTH, height);
            if (needsReveal) {
                this._panel.opacity = 255;
                this._revealed = true;
            }
            return GLib.SOURCE_REMOVE;
        });
    }

    _contentFor(phase, text, message) {
        if (phase === 'idle' || (phase === 'complete' && text.trim()))
            return null;
        if (text.trim()) {
            return {
                text: visibleTail(text),
                style: phase === 'inserting' ? 'final' : 'streaming',
            };
        }
        if (phase === 'connecting' || phase === 'listening')
            return {text: READY_PROMPT, style: 'prompt'};
        if (message.trim())
            return {text: visibleTail(message), style: 'prompt'};
        return null;
    }
}

class PreviewService {
    constructor(overlay) {
        this._overlay = overlay;
    }

    Ping() {
        return true;
    }

    Show(phase, text, message) {
        this._overlay.show(phase, text, message);
    }

    Hide() {
        this._overlay.hide();
    }
}

export default class VoiceFlowPreviewExtension extends Extension {
    enable() {
        this._overlay = new PreviewOverlay();
        this._service = new PreviewService(this._overlay);
        this._dbus = Gio.DBusExportedObject.wrapJSObject(PREVIEW_INTERFACE, this._service);
        this._dbus.export(Gio.DBus.session, OBJECT_PATH);
        this._nameId = Gio.DBus.session.own_name(
            BUS_NAME,
            Gio.BusNameOwnerFlags.NONE,
            null,
            null,
        );
    }

    disable() {
        if (this._nameId) {
            Gio.DBus.session.unown_name(this._nameId);
            this._nameId = 0;
        }
        if (this._dbus) {
            this._dbus.unexport();
            this._dbus = null;
        }
        this._service = null;
        if (this._overlay) {
            this._overlay.destroy();
            this._overlay = null;
        }
    }
}
