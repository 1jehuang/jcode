import json,uuid,urllib.request,urllib.error,os,sys
a=json.load(open(os.path.expanduser('~/.jcode/openai-auth.json')))['openai_accounts'][int(os.environ.get('ACCT','0'))]
model=sys.argv[1] if len(sys.argv)>1 else "gpt-5.5"
J="You are Jcode, an open-source coding agent."
for name,extra in [("A originator=codex_cli_rs",{"originator":"codex_cli_rs"}),("B no originator",{}),("C originator=jcode",{"originator":"jcode"})]:
  body={"model":model,"instructions":J,"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"Who are you, in one sentence?"}]}],"stream":True,"store":False}
  h={"Authorization":"Bearer "+a['access_token'],"chatgpt-account-id":a['account_id'],"content-type":"application/json","session-id":str(uuid.uuid4()),**extra}
  req=urllib.request.Request("https://chatgpt.com/backend-api/codex/responses",data=json.dumps(body).encode(),headers=h,method="POST")
  try:
    txt="";
    for line in urllib.request.urlopen(req,timeout=90):
      line=line.decode().strip()
      if line.startswith("data:"):
        try: e=json.loads(line[5:])
        except: continue
        if e.get("type")=="response.output_text.delta": txt+=e["delta"]
    print(f"{name}: OK -> {txt[:100]!r}")
  except urllib.error.HTTPError as e: print(f"{name}: HTTP {e.code} -> {e.read().decode()[:200]}")
