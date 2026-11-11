#!/usr/bin/env python3
"""Probe which system-prompt blocks Anthropic requires on the Claude OAuth path.

Sends one tiny request per variant using the first account in
~/.jcode/auth.json and prints accept/reject plus the model's self-description.
Use an Opus or Sonnet model; Haiku accepts every variant. See the doc comment on
OAUTH_BILLING_HEADER in crates/jcode-provider-anthropic/src/lib.rs.

Usage: scripts/anthropic_oauth_identity_probe.py [model]   (default haiku)
"""
import json,uuid,urllib.request,urllib.error,os,sys
d=json.load(open(os.path.expanduser('~/.jcode/auth.json')))
tok=d['anthropic_accounts'][0]['access']
BILL="x-anthropic-billing-header: cc_version=2.1.280; cc_entrypoint=sdk-cli; cch=33f85;"
SDK="You are a Claude agent, built on Anthropic's Claude Agent SDK."
CC="You are Claude Code, Anthropic's official CLI for Claude."
JC="You are jcode, powered by Claude Code. You are a third-party CLI, not the official Claude Code CLI."
JPROMPT="You are Jcode, an open-source coding agent. Use tools to edit files and run builds. "*20
BETA="claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14"
model=sys.argv[1] if len(sys.argv)>1 else "claude-haiku-4-5"
variants={
 "A current (bill+SDK+jcode)":[BILL,SDK,JPROMPT],
 "B no identity (bill+jcode)":[BILL,JPROMPT],
 "C no bill, SDK":[SDK,JPROMPT],
 "D neither":[JPROMPT],
 "E bill+SDK+jcode-notice":[BILL,SDK,"Note: you are actually Jcode, a third-party harness, not built on the Agent SDK.",JPROMPT],
 "F old (bill+CC+JC notice)":[BILL,CC,JC,JPROMPT],
 "G bill+jcode first then SDK":[BILL,JPROMPT,SDK],
}
sid=str(uuid.uuid4())
for name,blocks in variants.items():
  body={"model":model,"max_tokens":60,"system":[{"type":"text","text":t} for t in blocks],
        "messages":[{"role":"user","content":"Who are you, in one sentence?"}]}
  req=urllib.request.Request("https://api.anthropic.com/v1/messages?beta=true",data=json.dumps(body).encode(),method="POST",headers={
   "Authorization":"Bearer "+tok,"content-type":"application/json","anthropic-version":"2023-06-01",
   "anthropic-beta":BETA,"User-Agent":"claude-cli/2.1.280 (external, sdk-cli)","x-app":"cli",
   "X-Claude-Code-Session-Id":sid,"anthropic-dangerous-direct-browser-access":"true"})
  try:
    r=json.load(urllib.request.urlopen(req,timeout=60))
    print(f"{name}: OK -> {r['content'][0]['text'][:110]!r}")
  except urllib.error.HTTPError as e:
    print(f"{name}: HTTP {e.code} -> {e.read().decode()[:160]}")
