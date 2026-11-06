import json,os,pathlib,sys,time
sys.stdin.reconfigure(encoding='utf-8')
sys.stdout.reconfigure(encoding='utf-8')
record=pathlib.Path(sys.argv[1]);mode=sys.argv[2]
def write(stage,**extra):
    with record.open('a') as stream:
        stream.write(json.dumps({'stage':stage,'pid':os.getpid(),'parent':os.getppid(),
                                 'time':time.time(),**extra})+'\n');stream.flush()
write('started',synthetic=os.environ.get('JCODE_SYNTHETIC_API_KEY'))
if mode=='broad':
    import ctypes
    kernel=ctypes.WinDLL('kernel32',use_last_error=True)
    kernel.OpenEventW.argtypes=[ctypes.c_uint32,ctypes.c_int,ctypes.c_wchar_p];kernel.OpenEventW.restype=ctypes.c_void_p
    kernel.WaitForSingleObject.argtypes=[ctypes.c_void_p,ctypes.c_uint32];kernel.WaitForSingleObject.restype=ctypes.c_uint32
    kernel.CloseHandle.argtypes=[ctypes.c_void_p]
    event=kernel.OpenEventW(0x100000,False,sys.argv[3])
    write('broad_ready',eventOpened=bool(event))
    result=kernel.WaitForSingleObject(event,30000) if event else 0xffffffff
    write('broad_natural_exit',waitResult=result)
    if event:kernel.CloseHandle(event)
    sys.exit(0)
try:
    for line in sys.stdin:
        message=json.loads(line);method=message.get('method');identity=message.get('id')
        if method=='shutdown':
            write('shutdown_received');time.sleep(3 if mode=='slow_shutdown' else 0.2);break
        if identity is None:continue
        if method=='fixture/pending':
            write('pending_request_received',requestId=identity)
            if mode=='pending_until_shutdown':continue
            break
        if method=='tools/call' and mode=='pending_until_shutdown':
            write('pending_tool_request_received',requestId=identity)
            continue
        if method=='initialize':
            if mode=='hold_init':
                write('initialization_held');continue
            if mode=='initialize_error':
                result={'jsonrpc':'2.0','id':identity,'error':{'code':-32000,'message':'synthetic initialization failure'}}
            else:
                result={'jsonrpc':'2.0','id':identity,'result':{'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'natural-fixture','version':'1'}}}
        elif method=='fixture/environment':result={'jsonrpc':'2.0','id':identity,'result':{key:os.environ.get(key) for key in message.get('params',{}).get('keys',[])}}
        elif method=='tools/list':result={'jsonrpc':'2.0','id':identity,'result':{'tools':[]}}
        else:result={'jsonrpc':'2.0','id':identity,'result':message.get('params',{})}
        print(json.dumps(result),flush=True)
        if method=='fixture/exit':break
except BrokenPipeError:
    write('broken_pipe_after_parent_close')
finally:
    write('natural_exit')
