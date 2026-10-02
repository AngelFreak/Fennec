"""Screenshots every mockup state with headless Chrome over CDP.

usage: python3 mockshots.py <project-dir> <out-dir>
"""
import base64
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request

import websocket

PROJECT, OUT = sys.argv[1], sys.argv[2]
os.makedirs(OUT, exist_ok=True)

SETTINGS = ["Speech model", "Dictation", "AI providers", "AI defaults", "Privacy", "Storage"]
STATES = [
    ("Main", "dictate", []),
    ("Main", "dictate-ai-menu", ["AI actions"]),
    ("Main", "summary", ["Summary"]),
    ("Main", "actions", ["Actions · 3"]),
    ("Ingest", "files", []),
    ("Export", "export-docx", []),
    ("Export", "export-txt", ["TXT"]),
    ("Export", "export-pdf", ["PDF"]),
    ("Project", "project", []),
    ("Project", "project-tag", ["#meeting · 3"]),
    ("Project", "project-actions", ["Actions · 3"]),
    ("Project", "project-ask", ["Ask"]),
    ("Templates", "templates", []),
    ("Cleanup", "cleanup", []),
] + [("Settings", "settings-" + s.lower().replace(" ", "-"), [s]) for s in SETTINGS]

CLICK = """
(text) => {
  const all = [...document.querySelectorAll('body *')];
  const hits = all.filter(e => e.textContent.trim() === text);
  // the innermost match, then let the click bubble to the handler
  const el = hits.find(e => ![...e.children].some(c => c.textContent.trim() === text));
  if (!el) return false;
  el.click();
  return true;
}
"""

port = 9333
profile = tempfile.mkdtemp()
chrome = subprocess.Popen([
    "google-chrome", "--headless=new", "--disable-gpu", "--no-sandbox", "--hide-scrollbars",
    f"--remote-debugging-port={port}", f"--user-data-dir={profile}",
    "--window-size=1280,800", "about:blank",
], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    for _ in range(50):
        try:
            targets = json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/json"))
            break
        except OSError:
            time.sleep(0.2)
    page = next(t for t in targets if t["type"] == "page")
    ws = websocket.create_connection(page["webSocketDebuggerUrl"], timeout=30, suppress_origin=True)
    seq = [0]

    def call(method, **params):
        seq[0] += 1
        ws.send(json.dumps({"id": seq[0], "method": method, "params": params}))
        while True:
            msg = json.loads(ws.recv())
            if msg.get("id") == seq[0]:
                if "error" in msg:
                    raise RuntimeError(f"{method}: {msg['error']}")
                return msg.get("result", {})

    call("Emulation.setDeviceMetricsOverride", width=1280, height=800, deviceScaleFactor=1, mobile=False)
    for page_name, shot, clicks in STATES:
        call("Page.navigate", url=f"file://{PROJECT}/{page_name}.dc.html")
        time.sleep(2.5)
        ok = True
        for text in clicks:
            r = call("Runtime.evaluate", expression=f"({CLICK})({json.dumps(text)})", returnByValue=True)
            ok = ok and r["result"].get("value") is True
            time.sleep(0.6)
        data = call("Page.captureScreenshot", format="png")["data"]
        with open(f"{OUT}/{shot}.png", "wb") as f:
            f.write(base64.b64decode(data))
        print(shot, "ok" if ok else "CLICK FAILED")
finally:
    chrome.terminate()
