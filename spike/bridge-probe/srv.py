import http.server, json, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n=int(self.headers.get('content-length',0)); b=self.rfile.read(n)
        print("==", self.path, b.decode()[:700], flush=True)
        self.send_response(200); self.send_header('Access-Control-Allow-Origin','*'); self.end_headers()
    def log_message(self,*a): pass
http.server.HTTPServer(('127.0.0.1',8765),H).serve_forever()
