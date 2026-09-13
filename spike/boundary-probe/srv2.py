import http.server, os, sys, threading, time
T0 = time.time(); seen = threading.Event()
def watchdog():
    if not seen.wait(240):
        print("no contact from spike within 240s", flush=True); os._exit(3)
threading.Thread(target=watchdog, daemon=True).start()
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        seen.set(); self.send_response(200); self.end_headers()
    def do_POST(self):
        seen.set()
        body = self.rfile.read(int(self.headers.get("content-length", 0))).decode()
        print(f"[{time.time() - T0:5.0f}s] {self.path[1:]}  {body[:450]}", flush=True)
        self.send_response(200); self.end_headers()
        if self.path == "/done": os._exit(0)
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", 8765), H).serve_forever()
