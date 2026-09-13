#!/usr/bin/env python3
import sys, struct, json, threading, time
def send(o):
    b = json.dumps(o).encode(); sys.stdout.buffer.write(struct.pack("@I", len(b)) + b); sys.stdout.buffer.flush()
def pusher():   # unsolicited host->extension push, no polling
    for i in range(3):
        time.sleep(1); send({"pushed": i})
threading.Thread(target=pusher, daemon=True).start()
while True:
    h = sys.stdin.buffer.read(4)
    if not h: break
    m = json.loads(sys.stdin.buffer.read(struct.unpack("@I", h)[0]))
    send({"echo": m})
