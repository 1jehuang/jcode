import json,uuid,urllib.request,urllib.error,os,sys
tok=json.load(open(os.path.expanduser('~/.jcode/auth.json')))['anthropic_accounts'][0]['access']
BILL="x-anthropic-billing-header: cc_version=2.1.280; cc_entrypoint=sdk-cli; cch=33f85;"
model=sys.argv[1] if len(sys.argv)>1 else "claude-opus-4-5"
full={"User-Agent":"claude-cli/2.1.280 (external, sdk-cli)","x-app":"cli","anthropic-dangerous-direct-browser-access":"true",
 "X-Stainless-Lang":"js","X-Stainless-Runtime":"node","X-Stainless-Package-Version":"0.81.0"}
BETA_FULL="claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14"
def run(name,headers,beta,system,tools=None):
  body={"model":model,"max_tokens":30,"system":[{"type":"text","text":t} for t in system],"messages":[{"role":"user","content":"Say hi."}]}
  if tools: body["tools"]=tools
  h={"Authorization":"Bearer "+tok,"content-type":"application/json","anthropic-version":"2023-06-01","anthropic-beta":beta,"X-Claude-Code-Session-Id":str(uuid.uuid4()),**headers}
  req=urllib.request.Request("https://api.anthropic.com/v1/messages?beta=true",data=json.dumps(body).encode(),method="POST",headers=h)
  try: urllib.request.urlopen(req,timeout=60); print(f"{name}: OK")
  except urllib.error.HTTPError as e: print(f"{name}: HTTP {e.code} {e.read().decode()[:120]}")
J=["You are Jcode, a coding agent."]
run("1 baseline (bill + full headers)",full,BETA_FULL,[BILL]+J)
run("2 bill, plain UA, no x-app/stainless",{"User-Agent":"jcode/0.99"},BETA_FULL,[BILL]+J)
run("3 bill, full headers, beta oauth only",full,"oauth-2025-04-20",[BILL]+J)
run("4 bill, plain UA, beta oauth only",{"User-Agent":"jcode/0.99"},"oauth-2025-04-20",[BILL]+J)
run("5 no bill, plain UA, beta oauth only",{"User-Agent":"jcode/0.99"},"oauth-2025-04-20",J)
t=[{"name":n,"description":"x","input_schema":{"type":"object","properties":{}}} for n in ["bash","read","edit","agentgrep","swarm","todo"]]
run("6 bill + full headers + jcode tool names",full,BETA_FULL,[BILL]+J,t)
run("7 bill, plain UA, oauth beta, jcode tools",{"User-Agent":"jcode/0.99"},"oauth-2025-04-20",[BILL]+J,t)
