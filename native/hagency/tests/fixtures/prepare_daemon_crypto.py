#!/usr/bin/env python3
"""Prepare fresh, private OAuth grants for owner_daemon_crypto.rs.

Input is the disposable Server smoke harness client-fixture.json. This test
helper refuses non-loopback origins and never reads an installed account.
OAuth protocol follows sibling Server scripts/test-cross-device-config.py.
The disposable Pasion configuration must enable password registration without
requiring contact details so the group cases can create a separate human.
"""
import argparse
import base64
import hashlib
import http.cookiejar
import json
import os
from pathlib import Path
import secrets
import time
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlencode, urlparse
from urllib.request import build_opener, HTTPRedirectHandler, HTTPCookieProcessor, ProxyHandler, Request

class OAuthRejected(RuntimeError):
    def __init__(self, path, status):
        self.status = status
        super().__init__("cross-device OAuth rejected at " + path.split("?")[0]
                         + " (HTTP " + str(status) + ")")


def grant(base, username, password, device):
    class NoRedirect(HTTPRedirectHandler):
        def redirect_request(self, req, fp, code, msg, headers, newurl):
            return None

    browser = build_opener(ProxyHandler({}), NoRedirect(),
                           HTTPCookieProcessor(http.cookiejar.CookieJar()))

    def call(path, body=None, form=False, extra=None):
        data = None if body is None else (
            urlencode(body).encode() if form else json.dumps(body).encode())
        headers = {"Content-Type": "application/x-www-form-urlencoded"
                   if form else "application/json"}
        headers.update(extra or {})
        try:
            with browser.open(Request(base + path, data=data, headers=headers),
                              timeout=20) as reply:
                return json.load(reply)
        except HTTPError as error:
            if error.code in (302, 303, 307):
                return {"redirect_url": error.headers["Location"]}
            raise OAuthRejected(path, error.code) from None

    call("/auth/api/v1/auth/login", {"username": username, "password": password})
    redirect = "http://127.0.0.1:13377/console/server-login/callback"
    registered = call("/auth/oauth2/registration", {
        "client_name": "Cross-device configuration fixture " + device,
        "client_uri": "https://github.com/chrislearn/hagency-client",
        "application_type": "native", "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "redirect_uris": ["http://127.0.0.1/console/server-login/callback"]})
    verifier = secrets.token_urlsafe(48)
    state = secrets.token_urlsafe(32)
    challenge = base64.urlsafe_b64encode(
        hashlib.sha256(verifier.encode()).digest()).decode().rstrip("=")
    authorization = call("/auth/authorize?" + urlencode({
        "client_id": registered["client_id"], "response_type": "code",
        "redirect_uri": redirect, "state": state,
        "code_challenge": challenge, "code_challenge_method": "S256",
        "scope": "urn:matrix:client:api:* urn:matrix:client:device:" + device}))
    destination = authorization["redirect_url"]
    if urlparse(destination).path != "/console/server-login/callback":
        consent = destination.rstrip("/").rsplit("/", 1)[-1]
        destination = call("/auth/api/v1/oauth2/consent/" + consent,
                           {"action": "consent"}, extra={"Origin": base})["redirect_url"]
    parsed = urlparse(destination)
    query = parse_qs(parsed.query)
    if parsed.netloc != "127.0.0.1:13377" or query.get("state") != [state]:
        raise RuntimeError("cross-device callback identity mismatch")
    tokens = call("/auth/oauth2/token", {
        "grant_type": "authorization_code", "client_id": registered["client_id"],
        "redirect_uri": redirect, "code": query["code"][0],
        "code_verifier": verifier}, form=True)
    token = tokens["access_token"]
    # Fixture bootstrapping queues asynchronous Pasion user/device provisioning.
    # Its first job can run before the embedded Palpo listener and retry after
    # five seconds. Require actual authenticated Matrix readiness, never invent
    # an identity or bypass this check. Permanent rejection still fails.
    deadline = time.monotonic() + 30
    waiting = False
    while True:
        try:
            identity = call("/_matrix/client/v3/account/whoami", extra={
                "Authorization": "Bearer " + token})
            break
        except OAuthRejected as error:
            if error.status not in (401, 404, 500, 503) or time.monotonic() >= deadline:
                raise
            if not waiting:
                print("Waiting for fixture Matrix user/device provisioning (30s limit).",
                      flush=True)
                waiting = True
            time.sleep(0.5)
    return {"accessToken": token, "refreshToken": tokens["refresh_token"], "clientId": registered["client_id"],
            "owner": identity["user_id"]}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("fixture", type=Path)
    args = parser.parse_args()
    path = args.fixture.resolve()
    if path.stat().st_mode & 0o077:
        raise RuntimeError("fixture must be private")
    value = json.loads(path.read_text())
    url = urlparse(value["base"])
    if url.scheme != "http" or url.hostname != "127.0.0.1" or url.port is None or url.path or url.query or url.fragment or url.username:
        raise RuntimeError("disposable loopback Server fixture required")
    tag = secrets.token_hex(5)
    grants = {key: grant(value["base"], value["username"], value["password"], key + tag) for key in ["daemon", "human"]}
    # A separate real human, not a spoofed sender or an appservice puppet, is
    # needed to exercise encrypted multi-user room policy.
    username = "crypto_member_" + tag
    password = secrets.token_urlsafe(32)
    registration = build_opener(ProxyHandler({}), HTTPCookieProcessor(http.cookiejar.CookieJar()))
    def register(path, body):
        with registration.open(Request(value["base"] + path,
                data=json.dumps(body).encode(),
                headers={"Content-Type": "application/json", "Origin": value["base"]}), timeout=20) as response:
            result = json.load(response)
        if result.get("status") != "success":
            raise RuntimeError("fixture second-human registration failed at " + path + ": " + str(result.get("error")))
        return result
    registered = register("/auth/api/v1/auth/register", {"username": username,
        "password": password, "password_confirm": password})
    registration_path = "/auth/api/v1/auth/register/" + registered["id"]
    if registered.get("next_step") == "display_name":
        register(registration_path + "/display-name", {"display_name": "Encrypted fixture member"})
    register(registration_path + "/finish", {})
    grants["member"] = grant(value["base"], username, password, "member" + tag)
    output = path.parent / "daemon-crypto-fixture.json"
    if output.exists() and output.stat().st_mode & 0o077:
        raise RuntimeError("existing output is not private")
    with os.fdopen(os.open(output, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), "w") as file:
        json.dump({"base": value["base"], **grants}, file)
    print(output)

if __name__ == "__main__":
    main()
