#!/usr/bin/env python3
"""Serve the Nexa Office add-in locally with a user-provided trusted TLS identity."""

from __future__ import annotations

import argparse
import functools
import ssl
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit


ALLOWED_ASSET_PATHS = frozenset({"/taskpane.html", "/taskpane.js", "/support.html", "/icon.png"})


class OfficeAddInHandler(SimpleHTTPRequestHandler):
    def _asset_is_allowed(self) -> bool:
        return urlsplit(self.path).path in ALLOWED_ASSET_PATHS

    def do_GET(self) -> None:
        if not self._asset_is_allowed():
            self.send_error(404)
            return
        super().do_GET()

    def do_HEAD(self) -> None:
        if not self._asset_is_allowed():
            self.send_error(404)
            return
        super().do_HEAD()

    def end_headers(self) -> None:
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header(
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self' https://appsforoffice.microsoft.com; "
            "connect-src https://127.0.0.1:* http://127.0.0.1:*; "
            "img-src 'self' data:; style-src 'self' 'unsafe-inline'; "
            "frame-ancestors 'self' https://*.officeapps.live.com https://*.office.com https://*.microsoft365.com",
        )
        super().end_headers()


def serve(directory: Path, certificate: Path, private_key: Path, host: str, port: int) -> None:
    if host not in {"127.0.0.1", "localhost"}:
        raise ValueError("local Office add-in server host must be 127.0.0.1 or localhost")
    if not 1 <= port <= 65535:
        raise ValueError("port must be in 1..65535")
    directory = directory.expanduser().resolve(strict=True)
    certificate = certificate.expanduser().resolve(strict=True)
    private_key = private_key.expanduser().resolve(strict=True)
    if not directory.is_dir():
        raise ValueError(f"add-in directory is not a directory: {directory}")
    for sensitive_path in (certificate, private_key):
        if sensitive_path.is_relative_to(directory):
            raise ValueError("TLS certificate and private key must be outside the served asset directory")
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(certfile=certificate, keyfile=private_key)
    handler = functools.partial(OfficeAddInHandler, directory=str(directory))
    server = ThreadingHTTPServer((host, port), handler)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    print(f"Serving trusted Office add-in assets at https://{host}:{port}")
    try:
        server.serve_forever()
    finally:
        server.server_close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", default=str(Path(__file__).resolve().parent))
    parser.add_argument("--cert", required=True, help="PEM certificate trusted by Office/WebView2")
    parser.add_argument("--key", required=True, help="PEM private key for --cert")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=3000)
    args = parser.parse_args()
    try:
        serve(Path(args.directory), Path(args.cert), Path(args.key), args.host, args.port)
    except (OSError, ValueError, ssl.SSLError) as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
