"""Time CodeG -> real ACP -> real CLI on owned synthetic data, without model calls."""
import argparse
import concurrent.futures
import importlib.util
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import sqlite3
import subprocess
import time
import urllib.request

spec=importlib.util.spec_from_file_location('resume_fixture',Path(__file__).with_name('test-codex-resume.py'))
h=importlib.util.module_from_spec(spec);spec.loader.exec_module(h)

PROXY = '''#!/usr/bin/env python3
import json,os,subprocess,sys,threading,time
from pathlib import Path
command=[os.environ['CODEG_TEST_CLI']]+sys.argv[1:]
p=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=sys.stderr.buffer)
pending={}; lock=threading.Lock()
log=Path(os.environ['CODEG_TEST_RPC_LOG']).open('a',buffering=1)
def record(value):
 with lock:log.write(json.dumps(value)+'\\n')
def upstream():
 for line in sys.stdin.buffer:
  data=json.loads(line)
  if 'id' in data and 'method' in data:
   pending[str(data['id'])]=(time.time(),data['method'])
   record({'direction':'request','id':data['id'],'method':data['method'],'epoch':time.time(),'excludeTurns':data.get('params',{}).get('excludeTurns'),'bytes':len(line),'cli_pid':p.pid,'thread_id':data.get('params',{}).get('threadId')})
  p.stdin.write(line);p.stdin.flush()
 p.stdin.close()
threading.Thread(target=upstream,daemon=True).start()
for line in p.stdout:
 data=json.loads(line)
 if 'id' in data and 'method' not in data:
  entry=pending.pop(str(data['id']),None)
  if entry:record({'direction':'response','id':data['id'],'method':entry[1],'elapsed_ms':round((time.time()-entry[0])*1000,3),'epoch':time.time(),'bytes':len(line),'error':data.get('error'),'thread_id':data.get('result',{}).get('thread',{}).get('id')})
 sys.stdout.buffer.write(line);sys.stdout.buffer.flush()
p.wait();log.close();sys.exit(p.returncode)
'''

def post(base,token,method,params):
    request=urllib.request.Request(base+'/api/'+method,data=json.dumps(params).encode(),headers={'Content-Type':'application/json','Authorization':'Bearer '+token})
    start=time.time()
    with urllib.request.urlopen(request,timeout=300) as response:result=json.load(response)
    return result,{'method':method,'start_epoch':start,'end_epoch':time.time(),'elapsed_ms':round((time.time()-start)*1000,3)}

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli',type=Path,required=True);parser.add_argument('--acp',type=Path,required=True);parser.add_argument('--server',type=Path);parser.add_argument('--output',type=Path,required=True);parser.add_argument('--size-mib',type=int,default=64);parser.add_argument('--fast',action='store_true');parser.add_argument('--verify-prompt',action='store_true')
    args=parser.parse_args();root=args.output.resolve();root.mkdir(parents=True,exist_ok=False)
    workspace=root/'workspace';workspace.mkdir();provider=h.Provider()
    home,path,tid,write=h.fixture(root/'fixture',workspace,provider,args.size_mib)
    h.measure(args.cli,home,path,tid,workspace,root,'metadata-prime',False);write(args.size_mib)
    proxy=root/'codex-proxy';proxy.write_text(PROXY);proxy.chmod(0o700)
    rpc_log=root/'cli-rpc.jsonl'
    extra={'CODEX_PATH':str(proxy),'CODEG_TEST_CLI':str(args.cli.resolve()),'CODEG_TEST_RPC_LOG':str(rpc_log),'CODEG_LEGACY_CHECKPOINT_RESUME':'1' if args.fast else '0'}
    summary={'thread_id':tid,'rollout_bytes':path.stat().st_size,'fast':args.fast,'acp_version':subprocess.check_output([str(args.acp),'--version'],text=True).strip(),'cli_version':subprocess.check_output([str(args.cli),'--version'],text=True).strip()}
    rpc=h.RPC([str(args.acp)],home,workspace,root/'acp.stderr.log',extra)
    try:
        _,handshake=rpc.rpc('initialize',{'protocolVersion':1,'clientInfo':{'name':'codeg-resume-test','version':'1'},'clientCapabilities':{}})
        response,cold=rpc.rpc('session/resume',{'sessionId':tid,'cwd':str(workspace),'mcpServers':[]})
        _,warm=rpc.rpc('session/resume',{'sessionId':tid,'cwd':str(workspace),'mcpServers':[]})
        responses=[json.loads(line) for line in rpc_log.read_text().splitlines()]
        resumed=[entry for entry in responses if entry['direction']=='response' and entry['method']=='thread/resume']
        assert len(resumed)==2 and all(entry['thread_id']==tid for entry in resumed),resumed
        summary['acp']={'handshake_ms':handshake,'cold_resume_ms':cold,'warm_resume_ms':warm,'timeline':rpc.timeline,'peak_tree_rss_kib':rpc.peak_kib}
        assert not provider.requests, 'ACP restore unexpectedly called the model'
        if args.verify_prompt:
            reply,prompt_ms=rpc.rpc('session/prompt',{'sessionId':tid,'prompt':[{'type':'text','text':'VERIFY RESTORED CONTEXT AND DO NOT RUN TOOLS'}]})
            assert reply['stopReason']=='end_turn',reply
            assert len(provider.requests)==1,'ACP prompt did not run exactly once'
            model_input=json.dumps(provider.requests[-1]['input'])
            assert 'ESSENTIAL-COMPACTED-CONTEXT' in model_input and 'PERSISTED TOOL RESULT' in model_input
            assert any(part.get('type')=='input_image' for message in provider.requests[-1]['input'] if message.get('type')=='message' for part in message.get('content',[]))
            summary['acp']['first_prompt']={'elapsed_ms':prompt_ms,'stop_reason':reply['stopReason'],'model_calls':1,'context_tools_images_preserved':True}

    finally:rpc.close()
    prompt_count=len(provider.requests)
    if args.server:
        data=root/'server-data';data.mkdir();token=secrets.token_hex(32)
        with socket.socket() as reservation:reservation.bind(('127.0.0.1',0));port=reservation.getsockname()[1]
        base=f'http://127.0.0.1:{port}'
        env=dict(os.environ,**extra);env.update({'CODEX_HOME':str(home),'CODEG_HOST':'127.0.0.1','CODEG_PORT':str(port),'CODEG_TOKEN':token,'CODEG_DATA_DIR':str(data),'CODEG_HOME':str(root/'server-home'),'CODEG_ACP_IDLE_TIMEOUT_SECS':'180','PATH':str(args.acp.parent)+os.pathsep+os.environ['PATH'],'RUST_LOG':'info'})
        log=(root/'server.log').open('wb');proc=subprocess.Popen([str(args.server)],env=env,cwd=workspace,stdout=log,stderr=subprocess.STDOUT,start_new_session=os.name!='nt')
        try:
            deadline=time.time()+30
            while time.time()<deadline:
                if proc.poll() is not None:raise RuntimeError('Isolated server exited')
                try:
                    post(base,token,'health',{});break
                except OSError:time.sleep(.05)
            else:raise TimeoutError('Isolated server startup timed out')
            folder,_=post(base,token,'add_folder_to_history',{'path':str(workspace)})
            cid,_=post(base,token,'create_conversation',{'folderId':folder['id'],'agentType':'codex','title':'SYNTHETIC long-session recovery'})
            with sqlite3.connect(data/'codeg.db') as db:
                db.execute('UPDATE conversation SET external_id=?, status=? WHERE id=?',(tid,'completed',cid))
                db.execute("UPDATE agent_setting SET enabled=1 WHERE agent_type='codex'")
            restored,cold=post(base,token,'acp_restore_conversation',{'conversationId':cid})
            reused,warm=post(base,token,'acp_restore_conversation',{'conversationId':cid})
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                requests=list(pool.map(lambda _:post(base,token,'acp_restore_conversation',{'conversationId':cid}),range(2)))
            assert restored['externalSessionId']==tid and reused['externalSessionId']==tid,(restored,reused)
            assert restored['connectionId']==reused['connectionId']
            assert reused['reusedExisting'],reused
            assert all(r['connectionId']==restored['connectionId'] for r,_ in requests)
            summary['codeg']={'conversation_id':cid,'cold':cold,'warm':warm,'parallel_reopen':[timing for _,timing in requests],'connection_id':restored['connectionId'],'external_id_preserved':True,'warm_reused':True}
        finally:
            if os.name!='nt':os.killpg(proc.pid,signal.SIGTERM)
            else:proc.terminate()
            proc.wait(timeout=15);log.close()
            log_path=root/'server.log'
            log_path.write_text(log_path.read_text(errors='replace').replace(token,'<ISOLATED_TEST_TOKEN>'))
    summary['cli_rpc']=[json.loads(line) for line in rpc_log.read_text().splitlines()]
    if args.server:
        server_resumes=[r for r in summary['cli_rpc'] if r.get('direction')=='response' and r.get('method')=='thread/resume' and r['epoch']>=summary['codeg']['cold']['start_epoch']]
        assert len(server_resumes)==1, 'Reopening spawned an additional CLI resume'
        summary['codeg']['cli_resume_count_for_cold_warm_parallel']=len(server_resumes)
    assert len(provider.requests)==prompt_count,'Restore unexpectedly called the model provider'
    summary['model_calls_during_restore']=0
    (root/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');print(json.dumps(summary,indent=2))
    provider.server.shutdown();provider.server.server_close()

if __name__=='__main__':main()
