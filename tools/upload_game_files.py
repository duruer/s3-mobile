#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Push your own Settlers III install to the phone app's upload server (launcher > Game files > Start upload
server; Android and iOS). Parallel connections, no process per file; files of the same size already on the phone
are skipped (resumable).

Usage: python3 -P tools/upload_game_files.py <game folder> "http://<phone ip>:8080/?k=<key>" [--jobs 8]
"""
import argparse
import concurrent.futures as cf
import http.client
import json
import os
import sys
import threading
import time
import urllib.parse


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("folder")
    ap.add_argument("url")
    ap.add_argument("--jobs", type=int, default=8)
    a = ap.parse_args()
    u = urllib.parse.urlsplit(a.url)
    key = urllib.parse.parse_qs(u.query)["k"][0]
    host, port = u.hostname, u.port or 80

    files = []
    for root, _, names in os.walk(a.folder):
        for n in names:
            full = os.path.join(root, n)
            rel = os.path.relpath(full, a.folder).replace(os.sep, "/")
            files.append((rel, full, os.path.getsize(full)))
    files.sort(key=lambda f: -f[2])  # big files first keeps the pipes full
    total = sum(f[2] for f in files)
    lock = threading.Lock()
    st = {"sent": 0, "skipped": 0, "failed": 0, "bytes": 0, "done_bytes": 0}
    t0 = time.time()
    local = threading.local()

    def conn():
        if getattr(local, "c", None) is None:
            local.c = http.client.HTTPConnection(host, port, timeout=600)
        return local.c

    def request(method, path, body=None, headers=None):
        for attempt in range(3):
            try:
                c = conn()
                c.request(method, path, body=body, headers=headers or {})
                r = c.getresponse()
                data = r.read()
                if r.getheader("Connection", "").lower() == "close" or r.version == 10:
                    c.close()
                    local.c = None
                return r.status, data
            except (OSError, http.client.HTTPException):
                if getattr(local, "c", None) is not None:
                    local.c.close()
                local.c = None
                if attempt == 2:
                    raise
                time.sleep(1)

    def one(f):
        rel, full, size = f
        q = urllib.parse.quote(rel, safe="")
        try:
            s, body = request("GET", f"/stat?k={key}&path={q}")
            if s == 200 and json.loads(body).get("size") == size:
                with lock:
                    st["skipped"] += 1
                    st["done_bytes"] += size
                return
            with open(full, "rb") as fh:
                s, body = request("POST", f"/upload?k={key}&path={q}", body=fh,
                                  headers={"Content-Length": str(size), "Content-Type": "application/octet-stream"})
            ok = s == 200
        except Exception as e:  # noqa: BLE001 - report and go on
            ok, body = False, str(e).encode()
        with lock:
            if ok:
                st["sent"] += 1
                st["bytes"] += size
            else:
                st["failed"] += 1
                print(f"FAILED {rel}: {body[:200]!r}", flush=True)
            st["done_bytes"] += size

    last = 0.0
    with cf.ThreadPoolExecutor(a.jobs) as ex:
        futs = [ex.submit(one, f) for f in files]
        for i, _ in enumerate(cf.as_completed(futs), 1):
            now = time.time()
            if now - last > 5 or i == len(files):
                last = now
                el = max(0.001, now - t0)
                print(f"[{i}/{len(files)}] {st['done_bytes'] / 1e6:.0f}/{total / 1e6:.0f} MB, "
                      f"{st['bytes'] / el / 1e6:.1f} MB/s, sent {st['sent']}, skipped {st['skipped']}, "
                      f"failed {st['failed']}", flush=True)
    print(f"done: {st['sent']} sent, {st['skipped']} skipped, {st['failed']} failed, {time.time() - t0:.0f} s")
    return 1 if st["failed"] else 0


if __name__ == "__main__":
    sys.exit(main())
