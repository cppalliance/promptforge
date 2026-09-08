#!/usr/bin/env python3
"""Live-provider parity probe for the gateway speech surface. Dev-only; never in CI.

Drives the official OpenAI SDK against a real gateway instance configured with
a throwaway profile backed by Together AI's Orpheus speech model, then re-runs
the same calls against Together directly so the provider dialect can be
compared with what the gateway forwards.

Gating: the probe needs TOGETHER_API_KEY (process environment, or the repo
``.env``). When the key is absent the script prints a skip message and exits 0
without touching the network. The key is never printed and never written to
any file: the throwaway config references it as ``${TOGETHER_API_KEY}`` and
the gateway subprocess receives it through its environment.

Usage: ``python tools/gateway-tts-parity.py`` from anywhere. Exit code is 0 on
skip or when every assertion passes, 1 otherwise.

Dependencies: the third-party ``openai`` and ``httpx`` packages; the repo
carries no requirements file, so install them into the running interpreter.
"""

import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

import httpx
import openai

TOGETHER_BASE_URL = "https://api.together.xyz/v1"
TOGETHER_MODEL = "canopylabs/orpheus-3b-0.1-ft"
GATEWAY_MODEL = "orpheus"
VOICES = ["tara", "leah", "jess", "leo", "dan", "mia", "zac", "zoe"]
# Local-only shared secret for the throwaway config's [server] api_key; not a
# real credential, never leaves the loopback listener.
GATEWAY_KEY = "parity-throwaway-local-key"
PROBE_TEXT = "PromptForge gateway parity probe: the quick brown fox jumps over the lazy dog."
EMOTION_TEXT = "Angle-bracket emotion tags must reach the provider untouched. <laugh>"
READY_TIMEOUT_SECONDS = 90.0
CALL_TIMEOUT_SECONDS = 180.0

FAILURES = []


def check(label, condition, detail):
    """Record a hard assertion; failures fail the run."""
    print(f"{'PASS' if condition else 'FAIL'} {label}: {detail}")
    if not condition:
        FAILURES.append(label)


def observe(label, detail):
    """Record dialect data; never fails the run."""
    print(f"NOTE {label}: {detail}")


def read_dotenv_key(path):
    """Parse TOGETHER_API_KEY out of a dotenv file; None when absent."""
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return None
    for line in lines:
        line = line.strip()
        if line.startswith("#") or "=" not in line:
            continue
        name, _, value = line.partition("=")
        if name.strip() == "TOGETHER_API_KEY":
            return value.strip().strip("'\"") or None
    return None


def ensure_gateway_binary(repo):
    """Locate the built gateway, building the debug profile when missing."""
    for profile in ("debug", "release"):
        binary = repo / "target" / profile / ("promptforge-gateway.exe" if os.name == "nt" else "promptforge-gateway")
        if binary.is_file():
            return binary
    print("no built gateway found; running `cargo build -p gateway` (one time)")
    subprocess.run(["cargo", "build", "-p", "gateway"], cwd=repo, check=True)
    binary = repo / "target" / "debug" / ("promptforge-gateway.exe" if os.name == "nt" else "promptforge-gateway")
    if not binary.is_file():
        raise SystemExit("cargo build finished but the gateway binary is missing")
    return binary


def free_port():
    """An ephemeral loopback port, released before the gateway takes it."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def render_config(port):
    """The throwaway gateway config: one Together endpoint, one speech model."""
    voices = ", ".join(json.dumps(voice) for voice in VOICES)
    return f"""config-version = 2

[server]
bind = "127.0.0.1:{port}"
api_key = "{GATEWAY_KEY}"

[[endpoint]]
id = "together"
protocol = "openai"
base_url = "{TOGETHER_BASE_URL}"
api_key = "${{TOGETHER_API_KEY}}"

[[model]]
name = "{GATEWAY_MODEL}"
kind = "speech"
description = "Orpheus 3B conversational speech synthesis (parity probe)"
upstream = "{TOGETHER_MODEL}"
endpoints = ["together"]
context = 8192
voices = [{voices}]

[[profile]]
name = "parity"
models = ["{GATEWAY_MODEL}"]
"""


def wait_ready(port, proc, log_path):
    """Poll /v1/models until the gateway serves or the deadline passes."""
    deadline = time.monotonic() + READY_TIMEOUT_SECONDS
    url = f"http://127.0.0.1:{port}/v1/models"
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            tail = log_path.read_text(encoding="utf-8", errors="replace")[-2000:]
            raise SystemExit(f"gateway exited during boot (code {proc.returncode}); log tail:\n{tail}")
        request = urllib.request.Request(url, headers={"Authorization": f"Bearer {GATEWAY_KEY}"})
        try:
            with urllib.request.urlopen(request, timeout=2) as response:
                if response.status == 200:
                    return
        except OSError:
            time.sleep(0.25)
    raise SystemExit(f"gateway did not serve within {READY_TIMEOUT_SECONDS:.0f}s; log: {log_path}")


def speech_create(client, **kwargs):
    """One speech call as an observation dict; HTTP errors are data, not exceptions."""
    try:
        raw = client.audio.speech.with_raw_response.create(**kwargs)
        http = raw.http_response
        body = http.read()
        return {
            "ok": True,
            "status": http.status_code,
            "content_type": http.headers.get("content-type", ""),
            "transfer_encoding": http.headers.get("transfer-encoding", ""),
            "content_length": http.headers.get("content-length", ""),
            "bytes": len(body),
            "body": body,
        }
    except openai.APIStatusError as error:
        return {"ok": False, "status": error.status_code, "error": str(error)[:300]}
    except openai.APIError as error:
        return {"ok": False, "status": None, "error": str(error)[:300]}


def describe(result):
    """One-line shape summary of a speech observation, credential-free."""
    if not result["ok"]:
        return f"status={result['status']} error={result['error']!r}"
    return (
        f"status={result['status']} content-type={result['content_type']!r} "
        f"transfer-encoding={result['transfer_encoding']!r} content-length={result['content_length']!r} "
        f"bytes={result['bytes']} magic={result['body'][:4].hex()}"
    )


def is_mp3(body):
    return body[:3] == b"ID3" or (len(body) > 1 and body[0] == 0xFF and body[1] & 0xE0 == 0xE0)


def is_wav(body):
    return body[:4] == b"RIFF" and body[8:12] == b"WAVE"


def voices_call(base_url, api_key):
    """GET /v1/audio/voices as an observation dict; the SDK has no such method."""
    try:
        response = httpx.get(
            f"{base_url}/audio/voices",
            headers={"Authorization": f"Bearer {api_key}"},
            timeout=30.0,
        )
        snippet = response.text[:300]
        try:
            payload = response.json()
        except ValueError:
            payload = None
        return {"ok": response.is_success, "status": response.status_code, "json": payload, "snippet": snippet}
    except httpx.HTTPError as error:
        return {"ok": False, "status": None, "json": None, "snippet": str(error)[:300]}


def run_speech_surface(label, base_url, api_key, model, gateway):
    """The three parity calls plus dialect probes against one base URL.

    Both sides assert the provider-stable subset the first live run observed
    (wav mapping, emotion-tag 200, unknown-field tolerance); the gateway run
    adds the phase-1 contract checks. ``observe`` remains for genuinely
    volatile provider behavior: default format, framing, and byte counts.
    """
    client = openai.OpenAI(
        base_url=base_url,
        api_key=api_key,
        timeout=CALL_TIMEOUT_SECONDS,
        max_retries=0,
    )

    default = speech_create(client, model=model, voice="tara", input=PROBE_TEXT)
    observe(f"{label} speech default format", describe(default))
    if gateway:
        check(
            f"{label} default format is mp3",
            default["ok"] and default["content_type"].split(";")[0].strip() == "audio/mpeg" and is_mp3(default["body"]),
            describe(default),
        )
        check(
            f"{label} default response is streamed",
            default["ok"] and default["content_length"] == "",
            f"content-length={default.get('content_length')!r} transfer-encoding={default.get('transfer_encoding')!r}",
        )

    wav = speech_create(client, model=model, voice="tara", input=PROBE_TEXT, response_format="wav")
    observe(f"{label} speech wav", describe(wav))
    wav_ok = wav["ok"] and wav["content_type"].split(";")[0].strip() == "audio/wav" and is_wav(wav["body"])
    check(f"{label} wav format maps to audio/wav with a RIFF body", wav_ok, describe(wav))

    emotion = speech_create(client, model=model, voice="tara", input=EMOTION_TEXT)
    observe(f"{label} emotion-tag input", describe(emotion))
    check(
        f"{label} emotion-tag input returns 200 with audio",
        emotion["ok"] and emotion["bytes"] > 0,
        describe(emotion),
    )

    voices = voices_call(base_url, api_key)
    if gateway:
        entries = (voices["json"] or {}).get("voices") if voices["json"] else None
        ids = [entry.get("id") for entry in entries] if isinstance(entries, list) else []
        check(
            f"{label} voices union shape",
            voices["ok"] and ids == sorted(VOICES) and all(
                isinstance(entry, dict) and entry.get("name") == entry.get("id") for entry in entries
            ),
            f"status={voices['status']} ids={ids}",
        )
    else:
        observe(f"{label} voices route", f"status={voices['status']} body={voices['snippet']!r}")

    # Fields outside the OpenAI speech contract ride the gateway's verbatim
    # passthrough to the provider; both sides tolerated every probe with a
    # 2xx on the first live run, so tolerance is asserted, not observed.
    for field, value in (("instructions", "Speak with a calm tone."), ("sample_rate", 44100), ("promptforge_probe", 1)):
        probe = speech_create(
            client,
            model=model,
            voice="tara",
            input=PROBE_TEXT,
            extra_body={field: value},
        )
        check(f"{label} passthrough field {field!r} tolerated", probe["ok"], describe(probe))


def main():
    repo = Path(__file__).resolve().parent.parent
    key = os.environ.get("TOGETHER_API_KEY") or read_dotenv_key(repo / ".env")
    if not key:
        print(
            "SKIP: TOGETHER_API_KEY is neither set in the environment nor present in "
            f"{repo / '.env'}; the live parity probe did not run."
        )
        return 0

    binary = ensure_gateway_binary(repo)
    port = free_port()
    with tempfile.TemporaryDirectory(prefix="promptforge-tts-parity-") as scratch:
        scratch = Path(scratch)
        config = scratch / "gateway.toml"
        config.write_text(render_config(port), encoding="utf-8")
        log_path = scratch / "gateway.log"
        with log_path.open("wb") as log:
            env = dict(os.environ, TOGETHER_API_KEY=key)
            proc = subprocess.Popen(
                [str(binary), "--config", str(config), "--profile", "parity", "--no-tray"],
                stdout=log,
                stderr=subprocess.STDOUT,
                env=env,
            )
            try:
                wait_ready(port, proc, log_path)
                print(f"gateway up on 127.0.0.1:{port} (throwaway profile 'parity')")
                run_speech_surface("gateway", f"http://127.0.0.1:{port}/v1", GATEWAY_KEY, GATEWAY_MODEL, gateway=True)
            finally:
                proc.terminate()
                try:
                    proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=10)

    print("--- live provider (Together AI direct) ---")
    run_speech_surface("together", TOGETHER_BASE_URL, key, TOGETHER_MODEL, gateway=False)

    if FAILURES:
        print(f"PARITY FAIL: {len(FAILURES)} assertion(s) failed: {', '.join(FAILURES)}")
        return 1
    print("PARITY OK: every assertion passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
