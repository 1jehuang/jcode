import json,re,collections
inp='/home/jeremy/jcode-transcript-export/handpick/digests/shard_01.json'; out='/home/jeremy/jcode-transcript-export/handpick/scores/shard_01.scores.json'
data=json.load(open(inp))
def score(x):
 ts=x.get('todos') or []; n=len(ts); calls=x.get('todo_calls',0) or 0; st=[str(t.get('status','')).lower() for t in ts]; cs=[str(t.get('content','')).strip() for t in ts]; ne=[c for c in cs if c]; comp=sum(s=='completed' for s in st); pending=sum(s in ('pending','in_progress') for s in st)
 if not ne or all(len(c)<=12 for c in ne): base=0
 else:
  avg=sum(map(len,ne))/len(ne); spec=sum(bool(re.search(r'\b(map|trace|audit|inspect|verify|enumerate|identify|classify|test|run|search|document|review|inventory|analy[sz]|implement|fix|write|report|check|validate)\b',c,re.I)) for c in ne)/len(ne)
  if n>=3 and avg>=35 and spec>=.5:
   base=8
   if comp>=max(2,n*.6): base=9
   if comp==n and calls>=3: base=10
  elif n>=2 and avg>=20:
   base=5 if pending else 6
   if comp>=n*.5 and calls>=2: base=7
  else: base=2 if calls<=1 or pending else 4
 if x.get('is_debug'): base-=2
 base=max(0,min(10,base))
 if not ne: reason='No meaningful todo items.'
 elif base<=1: reason='Boilerplate or junk todos with little planning value.'
 elif base<=4: reason='Sparse or vague plan, with limited completion evidence.'
 elif base<=7: reason='Reasonable plan, but generic or incompletely updated.'
 elif comp==n and calls>=3: reason='Specific decomposed plan, repeatedly updated and fully completed.'
 elif comp>=2: reason='Specific multi-step plan with substantial completion evidence.'
 else: reason='Specific multi-step plan, but completion remains partial.'
 if x.get('is_debug'): reason='Debug session penalty applied; '+reason[0].lower()+reason[1:]
 return {'file':x.get('file'),'score':base,'reason':reason[:99]}
r=[score(x) for x in data]
with open(out,'w') as f: json.dump(r,f,ensure_ascii=False,indent=2); f.write('\n')
print(len(r),dict(sorted(collections.Counter(x['score'] for x in r).items())))
