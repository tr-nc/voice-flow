"""Activate only the disposable window used by check-linux-insertion.cjs."""
import ctypes as C
import subprocess
import sys

window = sys.argv[1]
label = subprocess.check_output(["xprop", "-id", window, "_NET_WM_NAME"], text=True)
if "Voice Flow insertion verification" not in label:
    raise SystemExit("Refusing to activate a window outside the insertion test")

x11 = C.CDLL("libX11.so.6")
x11.XOpenDisplay.restype = C.c_void_p
x11.XOpenDisplay.argtypes = [C.c_char_p]
display = x11.XOpenDisplay(None)
if not display:
    raise SystemExit("Cannot connect to X11")
x11.XDefaultRootWindow.argtypes = [C.c_void_p]
x11.XDefaultRootWindow.restype = C.c_ulong
x11.XInternAtom.argtypes = [C.c_void_p, C.c_char_p, C.c_int]
x11.XInternAtom.restype = C.c_ulong


class Data(C.Union):
    _fields_ = [("b", C.c_char * 20), ("s", C.c_short * 10), ("l", C.c_long * 5)]


class Client(C.Structure):
    _fields_ = [
        ("type", C.c_int), ("serial", C.c_ulong), ("send_event", C.c_int),
        ("display", C.c_void_p), ("window", C.c_ulong), ("message_type", C.c_ulong),
        ("format", C.c_int), ("data", Data),
    ]


class Event(C.Union):
    _fields_ = [("client", Client), ("pad", C.c_long * 24)]


event = Event()
message = event.client
message.type = 33  # ClientMessage
message.send_event = 1
message.display = display
message.window = int(window, 16)
message.message_type = x11.XInternAtom(display, b"_NET_ACTIVE_WINDOW", 0)
message.format = 32
message.data.l[0] = 2  # EWMH pager activation
x11.XSendEvent.argtypes = [C.c_void_p, C.c_ulong, C.c_int, C.c_long, C.POINTER(Event)]
x11.XFlush.argtypes = [C.c_void_p]
x11.XCloseDisplay.argtypes = [C.c_void_p]
x11.XSendEvent(display, x11.XDefaultRootWindow(display), 0, (1 << 20) | (1 << 19), C.byref(event))
x11.XFlush(display)
x11.XCloseDisplay(display)
