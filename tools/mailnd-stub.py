#!/usr/bin/env python3
"""Development stand-in for mailnd.

Accepts the nm-shim connection on bridge.sock and exposes a line-JSON control
socket (ctl.sock) for tools/bridgectl.py. Keeps no mail state; it exists to
exercise tbd until the real mailnd lands.

ctl requests (one JSON object per line, one reply line each):
  {"op": "status"}
  {"op": "call", "method": "...", "params": {...}, "timeout": 30}
  {"op": "wait", "event": "...", "after": <seq>, "timeout": 60}
"""

import asyncio
import itertools
import json
import os

RUN_DIR = os.environ.get("NOCTMALIA_RUN", "/run/noctmalia")
BRIDGE_SOCK = os.path.join(RUN_DIR, "bridge.sock")
CTL_SOCK = os.path.join(RUN_DIR, "ctl.sock")
EVENT_BUFFER = 1000


def log(*parts):
    print("mailnd-stub:", *parts, flush=True)


class Bridge:
    def __init__(self):
        self.writer = None
        self.pending = {}
        self.ids = itertools.count(1)
        self.events = []
        self.seq = 0
        self.hello = None
        self.changed = asyncio.Condition()

    async def on_shim(self, reader, writer):
        if self.writer:
            self.writer.close()
        self.writer = writer
        log("shim connected")
        try:
            while line := await reader.readline():
                message = json.loads(line)
                if "event" in message:
                    await self.record(message["event"], message.get("data"))
                elif message.get("id") in self.pending:
                    self.pending.pop(message["id"]).set_result(message)
                else:
                    log("unmatched message", line[:300])
        finally:
            if self.writer is writer:
                self.writer = None
                for future in self.pending.values():
                    future.set_result({"error": {"name": "Disconnected", "message": "bridge disconnected"}})
                self.pending.clear()
            log("shim disconnected")

    async def record(self, event, data):
        async with self.changed:
            self.seq += 1
            self.events.append((self.seq, event, data))
            del self.events[:-EVENT_BUFFER]
            if event == "bridge.hello":
                self.hello = data
            self.changed.notify_all()
        log(f"event #{self.seq} {event}", json.dumps(data)[:240])

    async def call(self, method, params, timeout):
        if not self.writer:
            return {"error": {"name": "Disconnected", "message": "bridge not connected"}}
        request_id = next(self.ids)
        future = asyncio.get_running_loop().create_future()
        self.pending[request_id] = future
        self.writer.write((json.dumps({"id": request_id, "method": method, "params": params}) + "\n").encode())
        await self.writer.drain()
        try:
            return await asyncio.wait_for(future, timeout)
        except asyncio.TimeoutError:
            self.pending.pop(request_id, None)
            return {"error": {"name": "Timeout", "message": f"{method}: no reply within {timeout}s"}}

    async def wait_event(self, name, after, timeout):
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        async with self.changed:
            while True:
                for seq, event, data in self.events:
                    if seq > after and event == name:
                        return {"result": {"seq": seq, "event": event, "data": data}}
                remaining = deadline - loop.time()
                if remaining <= 0:
                    return {"error": {"name": "Timeout", "message": f"no {name} after #{after} within {timeout}s"}}
                try:
                    await asyncio.wait_for(self.changed.wait(), remaining)
                except asyncio.TimeoutError:
                    pass

    async def on_ctl(self, reader, writer):
        try:
            while line := await reader.readline():
                request = json.loads(line)
                op = request.get("op")
                if op == "status":
                    reply = {"result": {"connected": self.writer is not None, "seq": self.seq, "hello": self.hello}}
                elif op == "call":
                    reply = await self.call(request["method"], request.get("params") or {}, request.get("timeout", 30))
                elif op == "wait":
                    reply = await self.wait_event(request["event"], request.get("after", 0), request.get("timeout", 60))
                else:
                    reply = {"error": {"name": "BadRequest", "message": f"unknown op: {op}"}}
                writer.write((json.dumps(reply) + "\n").encode())
                await writer.drain()
        finally:
            writer.close()


async def serve_unix(handler, path):
    if os.path.exists(path):
        os.unlink(path)
    server = await asyncio.start_unix_server(handler, path=path, limit=64 * 1024 * 1024)
    os.chmod(path, 0o666)  # tbd connects as uid 1000; the volume is private to the compose project
    return server


async def main():
    os.makedirs(RUN_DIR, exist_ok=True)
    bridge = Bridge()
    await serve_unix(bridge.on_shim, BRIDGE_SOCK)
    await serve_unix(bridge.on_ctl, CTL_SOCK)
    log("listening on", BRIDGE_SOCK, "and", CTL_SOCK)
    await asyncio.Event().wait()


if __name__ == "__main__":
    asyncio.run(main())
