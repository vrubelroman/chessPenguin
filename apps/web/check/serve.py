"""The site's layout for the web version, locally: the built program at /app/ and the
site's static files at /static/ (screenshots of the device page). Only for checks:
the site itself serves /app/ when the web version is published (docs/dev/web.md).

    python3 -I apps/web/check/serve.py <app-dir> <website-static-dir> [port]
"""

import functools
import http.server
import pathlib
import sys

APP, STATIC = pathlib.Path(sys.argv[1]).resolve(), pathlib.Path(sys.argv[2]).resolve()
PORT = int(sys.argv[3]) if len(sys.argv) > 3 else 18765


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {
        **http.server.SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "text/javascript",
    }

    def translate_path(self, path):
        path = path.split("?", 1)[0].split("#", 1)[0]
        for prefix, base in (("/app/", APP), ("/static/", STATIC), ("/screenshots/", STATIC / "screenshots")):
            if path.startswith(prefix):
                target = (base / path[len(prefix):]).resolve()
                if target == base or base in target.parents:
                    return str(target / "index.html" if target.is_dir() else target)
        return str(APP / "__missing__")

    def log_message(self, *args):
        pass


http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
