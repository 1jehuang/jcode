import boto3
import hmac
import json
import os
import re
import secrets
import shlex
import time
from urllib.parse import quote

INSTANCE_ID = os.environ.get("INSTANCE_ID", "i-08214cf66cd3f80c7")
TOKEN = os.environ.get("WAKE_TOKEN", "REPLACE_WITH_WAKE_TOKEN")
HOST = os.environ.get("KCODE_GATEWAY_HOST", "100.109.78.41")
PORT = int(os.environ.get("KCODE_GATEWAY_PORT", "7643"))
REGION = os.environ.get("AWS_REGION", "us-east-1")
ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
PAIRING_CODE_RE = re.compile(r"Pairing code:\s+(\d{3})\s+(\d{3})")


def response(status, body="", content_type="application/json", headers=None):
    result_headers = {
        "Cache-Control": "no-store",
        "Content-Type": content_type,
        "Referrer-Policy": "no-referrer",
        "X-Content-Type-Options": "nosniff",
        "X-Frame-Options": "DENY",
    }
    if headers:
        result_headers.update(headers)
    return {"statusCode": status, "headers": result_headers, "body": body}


def json_response(status, obj):
    return response(status, json.dumps(obj))


def authorized(event):
    headers = {k.lower(): v for k, v in (event.get("headers") or {}).items()}
    supplied = headers.get("authorization", "")
    expected = f"Bearer {TOKEN}"
    return hmac.compare_digest(supplied, expected)


def instance_state(ec2):
    instance = ec2.describe_instances(InstanceIds=[INSTANCE_ID])["Reservations"][0]["Instances"][0]
    return instance["State"]["Name"]


def ssm_online(ssm):
    result = ssm.describe_instance_information(
        Filters=[{"Key": "InstanceIds", "Values": [INSTANCE_ID]}],
        MaxResults=5,
    )
    return any(item.get("PingStatus") == "Online" for item in result.get("InstanceInformationList", []))


def fetch_pair_code(ssm):
    host = shlex.quote(HOST)
    command = (
        "sudo -iu ec2-user env "
        f"KCODE_GATEWAY_HOST={host} "
        "/home/ec2-user/.local/bin/jcode pair"
    )
    command_id = ssm.send_command(
        InstanceIds=[INSTANCE_ID],
        DocumentName="AWS-RunShellScript",
        Parameters={"commands": [command]},
        TimeoutSeconds=35,
    )["Command"]["CommandId"]

    deadline = time.monotonic() + 35
    invocation = None
    while time.monotonic() < deadline:
        try:
            invocation = ssm.get_command_invocation(CommandId=command_id, InstanceId=INSTANCE_ID)
        except ssm.exceptions.InvocationDoesNotExist:
            time.sleep(1)
            continue
        if invocation["Status"] in {"Success", "Failed", "Cancelled", "TimedOut"}:
            break
        time.sleep(1)

    if not invocation or invocation.get("Status") != "Success":
        status = invocation.get("Status", "TimedOut") if invocation else "TimedOut"
        return {"error": f"pair command {status.lower()}"}

    text = ANSI_RE.sub(
        "",
        invocation.get("StandardOutputContent", "") + invocation.get("StandardErrorContent", ""),
    )
    match = PAIRING_CODE_RE.search(text)
    if not match:
        return {"error": "no pairing code in command output"}

    code = match.group(1) + match.group(2)
    return {
        "code": code,
        "host": HOST,
        "port": PORT,
        "uri": f"jcode://pair?host={HOST}&port={PORT}&code={code}",
        "expires_in": 300,
    }


def landing_page():
    nonce = secrets.token_urlsafe(18)
    html = """<!doctype html><html><head>
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>jcode server</title>
<style nonce="__NONCE__">
body{font-family:-apple-system,system-ui;background:#101314;color:#eee;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0}
.card{text-align:center;padding:32px;max-width:360px}h1{color:#4DD9A6;font-size:1.6em;margin-bottom:8px}
#s{font-size:1.05em;line-height:1.5}.dot{display:inline-block;width:10px;height:10px;border-radius:50%;margin-right:8px;background:#e6b450}
.ok .dot{background:#4DD9A6}.ok #s b{color:#4DD9A6}.spin{opacity:.75;font-size:.9em;margin-top:14px}
#pairbtn{display:none;margin-top:22px;background:#4DD9A6;color:#0c0f10;border:0;border-radius:12px;padding:14px 22px;font-size:1.05em;font-weight:600}
#pairout{margin-top:16px;font-size:1em;line-height:1.6}#pairout .code{font-size:1.9em;letter-spacing:.18em;color:#4DD9A6;font-weight:700}#pairout a{color:#4DD9A6}
</style></head><body><div class="card" id="c">
<h1>jcode server</h1><p id="s"><span class="dot"></span>Authenticating…</p>
<p class="spin" id="hint">checking every 5s…</p><button id="pairbtn">Pair this phone</button><div id="pairout"></div>
<script nonce="__NONCE__">
const fragment = new URLSearchParams(location.hash.slice(1));
if (fragment.get('t')) sessionStorage.setItem('jcode-wake-token', fragment.get('t'));
history.replaceState(null, '', location.pathname);
const token = sessionStorage.getItem('jcode-wake-token');
const api = async action => {
  if (!token) throw new Error('missing token; open the saved wake link');
  const r = await fetch(location.pathname, {method:'POST', cache:'no-store',
    headers:{'Authorization':'Bearer '+token,'Content-Type':'application/json'},
    body:JSON.stringify({action})});
  if (!r.ok) throw new Error(r.status === 403 ? 'invalid token' : 'request failed: '+r.status);
  return r.json();
};
async function poll(){
  try{
    const j=await api('status'),s=document.getElementById('s'),c=document.getElementById('c');
    if(j.healthy){c.classList.add('ok');s.innerHTML='<span class="dot"></span><b>Ready.</b> Open the jcode app now.';document.getElementById('hint').textContent='server is up';document.getElementById('pairbtn').style.display='inline-block';return;}
    s.innerHTML='<span class="dot"></span>Instance: '+j.state+' · services warming up…';
  }catch(e){document.getElementById('s').textContent=e.message;document.getElementById('hint').textContent='';return;}
  setTimeout(poll,5000);
}
async function pair(){
  const o=document.getElementById('pairout');o.textContent='generating code…';
  try{const j=await api('pair');if(j.code){o.innerHTML='<div class="code">'+j.code.slice(0,3)+' '+j.code.slice(3)+'</div><div>host '+j.host+':'+j.port+' · expires in 5 min</div><div style="margin-top:10px"><a href="'+j.uri+'">Open in jcode app</a></div>';}else{o.textContent='error: '+(j.error||'unknown');}}catch(e){o.textContent='error: '+e.message;}
}
document.getElementById('pairbtn').addEventListener('click',pair);
(async()=>{try{await api('wake');poll();}catch(e){document.getElementById('s').textContent=e.message;document.getElementById('hint').textContent='';}})();
</script></div></body></html>""".replace("__NONCE__", nonce)
    csp = (
        "default-src 'none'; "
        f"style-src 'nonce-{nonce}'; script-src 'nonce-{nonce}'; "
        "connect-src 'self'; base-uri 'none'; frame-ancestors 'none'"
    )
    return response(200, html, "text/html; charset=utf-8", {"Content-Security-Policy": csp})


def handler(event, context):
    method = (
        event.get("requestContext", {}).get("http", {}).get("method")
        or event.get("httpMethod")
        or "GET"
    ).upper()

    if method == "GET":
        legacy_token = (event.get("queryStringParameters") or {}).get("t")
        if legacy_token and hmac.compare_digest(legacy_token, TOKEN):
            return response(302, headers={"Location": f"/#t={quote(TOKEN, safe='')}"})
        return landing_page()

    if method != "POST":
        return json_response(405, {"error": "method not allowed"})
    if not authorized(event):
        return json_response(403, {"error": "forbidden"})

    try:
        payload = json.loads(event.get("body") or "{}")
    except json.JSONDecodeError:
        return json_response(400, {"error": "invalid JSON"})

    action = payload.get("action")
    ec2 = boto3.client("ec2", region_name=REGION)
    ssm = boto3.client("ssm", region_name=REGION)
    state = instance_state(ec2)

    if action == "wake":
        started = state == "stopped"
        if started:
            ec2.start_instances(InstanceIds=[INSTANCE_ID])
        return json_response(200, {"state": state, "started": started})
    if action == "status":
        healthy = state == "running" and ssm_online(ssm)
        return json_response(200, {"state": state, "healthy": healthy})
    if action == "pair":
        if state != "running":
            return json_response(409, {"error": f"instance {state}, wake it first"})
        if not ssm_online(ssm):
            return json_response(503, {"error": "instance management agent is not ready"})
        result = fetch_pair_code(ssm)
        return json_response(200 if "code" in result else 500, result)
    return json_response(400, {"error": "unknown action"})
