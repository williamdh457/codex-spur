#!/usr/bin/env python3
"""mitmproxy addon: block only Kimi Work model-list cloud config.

Blocks:
  https://www.kimi.com/apiv2/kimi.gateway.config.v1.ConfigService/DescribeKimiWorkConfig

Does NOT block:
  - agent-gw.kimi.com (Agent / coding traffic)
  - ConfigService/GetConfig (discount window etc.)
  - Membership / Plugin / Skill / other apiv2 services

Usage:
  # install once: pip install mitmproxy
  mitmproxy -s scripts/kimi_block_work_model_config.py --listen-port 8080
  # or headless:
  mitmdump -s scripts/kimi_block_work_model_config.py --listen-port 8080

Then point macOS HTTP/HTTPS proxy to 127.0.0.1:8080 (or use Proxyman with the same path rule).
Install mitmproxy CA if HTTPS inspect is required.

Verify in Kimi log:
  rg 'DescribeKimiWorkConfig|using cached models' ~/Library/Logs/kimi-desktop/main.log | tail
"""

from __future__ import annotations

from mitmproxy import ctx, http

# Path suffix is enough; host is www.kimi.com in production logs.
BLOCK_PATH_MARKERS = (
    "DescribeKimiWorkConfig",
    "kimi.gateway.config.v1.ConfigService/DescribeKimiWorkConfig",
)

# Never block agent gateway even if something rewrites host.
NEVER_BLOCK_HOST_SUFFIXES = (
    "agent-gw.kimi.com",
)


def _should_block(flow: http.HTTPFlow) -> bool:
    host = (flow.request.pretty_host or "").lower()
    path = flow.request.path or ""
    url = flow.request.pretty_url or ""

    for suffix in NEVER_BLOCK_HOST_SUFFIXES:
        if host == suffix or host.endswith("." + suffix):
            return False

    for marker in BLOCK_PATH_MARKERS:
        if marker in path or marker in url:
            return True
    return False


class KimiWorkModelConfigBlock:
    def request(self, flow: http.HTTPFlow) -> None:
        if not _should_block(flow):
            return
        ctx.log.warn(f"[codex-spur] blocked Work model config: {flow.request.pretty_url}")
        # Fail closed so Kimi falls back to local kimi-work-models-cache.json
        flow.response = http.Response.make(
            503,
            b"blocked by codex-spur selective rule (DescribeKimiWorkConfig)",
            {"Content-Type": "text/plain; charset=utf-8", "Cache-Control": "no-store"},
        )


addons = [KimiWorkModelConfigBlock()]
