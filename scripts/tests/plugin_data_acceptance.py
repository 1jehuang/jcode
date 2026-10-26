#!/usr/bin/env python3
"""Exercise plugin DATA through real private daemon hook/MCP/LSP child processes."""
import argparse
import http.server
import json
import os
from pathlib import Path
import shlex
import sys
import time

from agent_profile_instruction_acceptance import Fixture, Peer, require
from plugin_history_acceptance import Acceptance, document

PEER = r'''import json, os, pathlib, sys
mode = sys.argv[1]
data = pathlib.Path(os.environ['CLAUDE_PLUGIN_DATA'])
assert data.is_dir(), 'DATA not prepared before child launch'
record = {'pid':os.getpid(), 'data':str(data), 'root':os.environ.get('CLAUDE_PLUGIN_ROOT'), 'mode':mode,
          'expanded':os.environ.get('DATA_EXPANDED')}
(data / (mode+'.json')).write_text(json.dumps(record))
if mode == 'hook':
    sys.stdin.read()
    sys.exit(0)
def send(value):
    raw=json.dumps(value).encode()
    if mode == 'lsp':
        sys.stdout.buffer.write(b'Content-Length: '+str(len(raw)).encode()+b'\r\n\r\n'+raw)
        sys.stdout.buffer.flush()
    else:
        print(raw.decode(),flush=True)
while True:
    if mode == 'lsp':
        length=None
        while True:
            line=sys.stdin.buffer.readline()
            if not line: sys.exit(0)
            if line in (b'\r\n',b'\n'): break
            if line.lower().startswith(b'content-length:'): length=int(line.split(b':',1)[1])
        if length is None: sys.exit(2)
        msg=json.loads(sys.stdin.buffer.read(length))
    else:
        line=sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        msg=json.loads(line)
    if 'id' not in msg: continue
    method=msg.get('method')
    if method=='initialize':
        result=({'capabilities':{'documentSymbolProvider':True}} if mode=='lsp' else
                {'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'data-fixture','version':'1'}})
    elif method=='tools/list': result={'tools':[]}
    elif method=='textDocument/documentSymbol': result=[]
    else: result=None
    send({'jsonrpc':'2.0','id':msg['id'],'result':result})
    if method=='shutdown': sys.exit(0)
'''


def provider(f, file_path):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            raw = self.rfile.read(int(self.headers['Content-Length']))
            data = json.loads(raw)
            require(self.path == '/v1/chat/completions' and data.get('stream') is True,
                    'unexpected private provider request')
            require(self.headers.get('Authorization') is None, 'private provider received credentials')
            f.captures.append(data)
            (f.root / f'data-provider-{len(f.captures):03}.json').write_bytes(raw)
            previous_tool = any(m.get('role') == 'tool' for m in data.get('messages', []))
            if previous_tool:
                chunks = [dict(choices=[dict(index=0, delta={'role':'assistant','content':'Data fixture done.'}, finish_reason=None)]),
                          dict(choices=[dict(index=0, delta={}, finish_reason='stop')])]
            else:
                arguments = json.dumps({'operation':'documentSymbols','file_path':str(file_path),
                                        'intent':'Verify installed plugin data in private fixture'})
                chunks = [dict(choices=[dict(index=0, delta={'role':'assistant','tool_calls':[
                    {'index':0,'id':'call_data_fixture','type':'function',
                     'function':{'name':'lsp','arguments':arguments}}]}, finish_reason=None)]),
                          dict(choices=[dict(index=0, delta={}, finish_reason='tool_calls')])]
            body = ''.join('data: '+json.dumps(c)+'\n\n' for c in chunks)+'data: [DONE]\n\n'
            self.send_response(200)
            self.send_header('Content-Type','text/event-stream')
            self.send_header('Content-Length',str(len(body.encode())))
            self.end_headers()
            self.wfile.write(body.encode())
    f.http.RequestHandlerClass = Handler


def execute(a):
    f = a.f
    source = f.root / 'data-market'
    root = source / 'plugin'
    root.mkdir(parents=True)
    (root / 'peer.py').write_text(PEER)
    # The installed root, not the source tree, is authoritative at execution.
    hook_command = shlex.quote(sys.executable)+' "${CLAUDE_PLUGIN_ROOT}/peer.py" hook'
    manifest = {'name':'data-fixture','version':'1.0.0',
                'hooks':{'PreToolUse':[{'matcher':'lsp','hooks':[{'type':'command','command':hook_command}]}]},
                'mcpServers':{'env':{'command':sys.executable,'args':['${CLAUDE_PLUGIN_ROOT}/peer.py','mcp'],
                                     'env':{'DATA_EXPANDED':'${CLAUDE_PLUGIN_DATA:-bad-fallback}'}}},
                'lspServers':{'fixture':{'command':sys.executable,'args':['${CLAUDE_PLUGIN_ROOT}/peer.py','lsp'],
                                        'extensionToLanguage':{'.datafixture':'fixture'},
                                        'env':{'DATA_EXPANDED':'${CLAUDE_PLUGIN_DATA}'}}}}
    document(root / '.claude-plugin/plugin.json',manifest)
    document(source / '.claude-plugin/marketplace.json',
             {'name':'data-market','plugins':[{'name':'data-fixture','source':'./plugin'}]})
    a.cli('marketplace','add',str(source))
    first = a.cli('install','data-fixture@data-market')
    a.cli('list')
    data_dir = f.jhome / 'plugins/data/data-fixture-data-market'
    require(not data_dir.exists(),'install/list created DATA before execution')
    a.check('plugin installation and listing leave persistent DATA uncreated')
    workspace_file = f.cwd / 'query.datafixture'
    workspace_file.write_text('private data fixture\n')
    provider(f,workspace_file)
    p = Peer(f)
    p.subscribe()
    p.request('message',content='Run private plugin DATA acceptance')
    deadline = time.monotonic()+10
    markers = [data_dir / (name+'.json') for name in ('hook','mcp','lsp')]
    records = None
    while time.monotonic()<deadline:
        try:
            records = [json.loads(path.read_text()) for path in markers]
            break
        except (OSError, json.JSONDecodeError):
            time.sleep(.05)
    require(records is not None, 'missing complete child DATA evidence: '+str(markers))
    for record in records:
        require(record['data']==str(data_dir),'wrong DATA identity: '+str(record))
        require(record['root']==first['root'],'wrong installed plugin root: '+str(record))
        if record['mode']!='hook':
            require(record['expanded']==str(data_dir),'DATA placeholder/default expansion mismatch')
    require(any(m.get('role')=='tool' and 'documentSymbols' in str(m.get('content',''))
                and 'error' not in str(m.get('content','')).lower()
                for capture in f.captures[1:] for m in capture.get('messages',[])),
            'actual provider continuation did not observe successful LSP tool output')
    a.report['runtime_v1'] = records
    require(f.env['CLAUDE_PLUGIN_DATA'] != str(data_dir), 'parent spoof fixture missing')
    a.check('actual hook, MCP and LSP children override inherited DATA with one prepared identity path')
    deadline = time.monotonic()+10
    connected = False
    while time.monotonic()<deadline:
        state = p.request('runtime_command', 'runtime_command_result', command='mcp', args=[])['data']
        if any(s.get('name')=='plugin:data-fixture@data-market:env' and s.get('status')=='connected'
               for s in state.get('servers',[])):
            connected = True
            break
        time.sleep(.05)
    require(connected, 'real plugin MCP handshake did not connect')
    a.check('public daemon MCP state confirms the DATA peer completed its handshake')
    sentinel = data_dir / 'durable.txt'
    sentinel.write_text('survives cache replacement and uninstall\n')
    manifest['version']='2.0.0'
    document(root / '.claude-plugin/plugin.json',manifest)
    a.cli('marketplace','update','data-market')
    changed = a.cli('update','data-fixture@data-market')
    require(changed['root']!=first['root'],'update did not replace cache root')
    require(sentinel.read_text()=='survives cache replacement and uninstall\n','update erased DATA')
    resumed = Peer(f)
    resumed.subscribe()
    resumed.request('message',content='Run updated private plugin DATA acceptance')
    deadline = time.monotonic()+10
    updated_records = None
    while time.monotonic()<deadline:
        try:
            seen = [json.loads(path.read_text()) for path in markers]
            if all(record['root']==changed['root'] for record in seen):
                updated_records = seen
                break
        except (OSError, json.JSONDecodeError):
            pass
        time.sleep(.05)
    require(updated_records is not None,'updated plugin did not execute all children from its new cache')
    require(all(record['data']==str(data_dir) for record in updated_records), 'update changed runtime DATA directory')
    a.report['runtime_v2'] = updated_records
    a.check('updated plugin actually reexecutes hooks, MCP and LSP against the same persistent DATA')
    a.cli('uninstall','data-fixture@data-market')
    require(sentinel.read_text()=='survives cache replacement and uninstall\n','uninstall erased DATA')
    a.check('persistent DATA survives immutable-cache update and scoped uninstall')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,required=True)
    parser.add_argument('--execute',action='store_true')
    args = parser.parse_args()
    if not args.execute:
        print('Prepared only; pass --execute with an approved built binary.')
        return
    require(args.binary.is_absolute() and args.binary.is_file() and not args.binary.is_symlink(),
            'explicit built binary required')
    a = Acceptance(Fixture(args.binary,seconds=150,plugin_data_spoof='JCODE_DATA_PARENT_SPOOF_5831'))
    try:
        execute(a)
        a.report['status']='passed'
    except BaseException as error:
        a.report.update(status='failed',error=repr(error))
        raise
    finally:
        a.report['cleanup']=a.close()
        clean=a.report['cleanup']
        child_pids = {record['pid'] for key in ('runtime_v1','runtime_v2') for record in a.report.get(key,[])}
        remaining = set(child_pids)
        deadline = time.monotonic()+3
        while remaining and time.monotonic()<deadline:
            for pid in tuple(remaining):
                try:
                    os.kill(pid,0)
                except ProcessLookupError:
                    remaining.remove(pid)
            if remaining:
                time.sleep(.05)
        clean['remaining_fixture_child_pids'] = sorted(remaining)
        if remaining or clean['errors'] or clean['tester_errors'] or not clean['reaped'] or not clean['http_stopped']:
            a.report['status']='failed'
        (a.f.root / 'plugin-data-report.json').write_text(json.dumps(a.report,indent=2)+'\n')
        print('Artifacts: '+str(a.f.root),flush=True)
        require(a.report['status']=='passed','plugin DATA acceptance failed; inspect retained report')


if __name__=='__main__':
    main()
