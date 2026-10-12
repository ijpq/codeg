"""Exercise real CLI/ACP recovery in disposable homes using a loopback Responses fixture."""
import argparse
import base64
import struct
import zlib
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import queue
import re
import shutil
import sqlite3
import signal
import subprocess
import threading
import time
import uuid

def png_chunk(kind, body):
    return struct.pack('>I',len(body))+kind+body+struct.pack('>I',zlib.crc32(kind+body))
PNG = b'\x89PNG\r\n\x1a\n'+png_chunk(b'IHDR',struct.pack('>IIBBBBB',32,32,8,6,0,0,0))+png_chunk(b'IDAT',zlib.compress((b'\x00'+b'\x20\x80\xc0\xff'*32)*32))+png_chunk(b'IEND',b'')
IMAGE = 'data:image/png;base64,'+base64.b64encode(PNG).decode()


class Provider:
    def __init__(self):
        self.requests = []
        owner = self
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                owner.requests.append(body)
                item = {'id':'msg_fixture','type':'message','role':'assistant','status':'completed','content':[{'type':'output_text','text':'SYNTHETIC RESPONSE','annotations':[]}]}
                response = {'id':'resp_fixture','object':'response','created_at':1791763200,'status':'completed','model':body.get('model'),'output':[item],'usage':{'input_tokens':42,'output_tokens':3,'total_tokens':45,'input_tokens_details':{'cached_tokens':0},'output_tokens_details':{'reasoning_tokens':0}}}
                events = [
                    {'type':'response.created','response':dict(response,status='in_progress',output=[])},
                    {'type':'response.output_item.added','output_index':0,'item':dict(item,status='in_progress',content=[])},
                    {'type':'response.content_part.added','item_id':item['id'],'output_index':0,'content_index':0,'part':{'type':'output_text','text':'','annotations':[]}},
                    {'type':'response.output_text.delta','item_id':item['id'],'output_index':0,'content_index':0,'delta':'SYNTHETIC RESPONSE'},
                    {'type':'response.output_text.done','item_id':item['id'],'output_index':0,'content_index':0,'text':'SYNTHETIC RESPONSE'},
                    {'type':'response.output_item.done','output_index':0,'item':item},
                    {'type':'response.completed','response':response},
                ]
                data = ''.join('data: '+json.dumps(e)+'\n\n' for e in events).encode()
                self.send_response(200); self.send_header('Content-Type','text/event-stream'); self.send_header('Content-Length',str(len(data))); self.end_headers(); self.wfile.write(data)
        self.server = ThreadingHTTPServer(('127.0.0.1',0),Handler)
        threading.Thread(target=self.server.serve_forever,daemon=True).start()
        self.url = f'http://127.0.0.1:{self.server.server_port}/v1'

class RPC:
    def __init__(self, command, home, cwd, log, extra=None, trace=None):
        env = dict(os.environ); env['CODEX_HOME'] = str(home)
        env.update(extra or {})
        self.trace = trace
        if trace and os.name != 'nt':
            command = ['strace','-f','-ttt','-yy','-s','8','-P',str(trace[0]),'-e','trace=openat,read,pread64,lseek','-o',str(trace[1])] + command
        self.errors = log.open('wb')
        self.process = subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=self.errors,cwd=cwd,env=env,start_new_session=os.name != "nt")
        self.lines = queue.Queue(); self.notifications=[]; self.timeline=[]; self.sequence=0; self.peak_kib=0; self.monitor_done=threading.Event()
        def read():
            for line in self.process.stdout: self.lines.put(json.loads(line))
            self.lines.put({'process_ended':True})
        threading.Thread(target=read,daemon=True).start()
        def monitor():
            if os.name == 'nt': return
            while not self.monitor_done.wait(.02):
                pending=[self.process.pid]; seen=set(); rss=0
                while pending:
                    pid=pending.pop()
                    if pid in seen: continue
                    seen.add(pid)
                    try:
                        text=Path(f'/proc/{pid}/status').read_text()
                        m=re.search(r'^VmRSS:\s+(\d+)',text,re.M)
                        if m: rss+=int(m[1])
                        for tid in Path(f'/proc/{pid}/task').iterdir():
                            pending.extend(map(int,(tid/'children').read_text().split()))
                    except (OSError,ValueError): pass
                self.peak_kib=max(self.peak_kib,rss)
        threading.Thread(target=monitor,daemon=True).start()
    def send(self, data):
        self.process.stdin.write((json.dumps(data)+'\n').encode()); self.process.stdin.flush()
    def rpc(self, method, params, timeout=120):
        self.sequence+=1; rid=self.sequence; start=time.time()
        self.send({'jsonrpc':'2.0','id':rid,'method':method,'params':params})
        while True:
            message=self.lines.get(timeout=max(.01,start+timeout-time.time()))
            if message.get('process_ended'): raise RuntimeError('RPC process ended before response')
            if message.get('id') == rid and 'method' not in message:
                self.timeline.append({'method':method,'start_epoch':start,'end_epoch':time.time(),'elapsed_ms':round((time.time()-start)*1000,3)})
                if 'error' in message: raise RuntimeError(message['error'])
                return message.get('result'), self.timeline[-1]['elapsed_ms']
            if 'method' in message:
                self.notifications.append(message)
                if 'id' in message:
                    raise RuntimeError('Unexpected client request: '+message['method'])
    def wait_turn(self, timeout=60):
        limit=time.time()+timeout
        while time.time()<limit:
            message=self.lines.get(timeout=max(.01,limit-time.time()))
            self.notifications.append(message)
            if message.get('method')=='turn/completed':
                turn=message['params']['turn']; assert turn['status']=='completed',turn
                return
        raise TimeoutError('Turn did not complete')
    def close(self):
        self.monitor_done.set(); self.process.stdin.close()
        try: self.process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            if os.name != "nt": os.killpg(self.process.pid, signal.SIGTERM)
            else: self.process.terminate()
            self.process.wait(timeout=10)
        self.errors.close()


def line(kind,payload):
    return (json.dumps({'timestamp':'2026-10-12T00:00:00Z','type':kind,'payload':payload},separators=(',',':'))+'\n').encode()

def fixture(root, workspace, provider, size_mib, mode='valid'):
    home=root/'home'; rollout_dir=home/'sessions/2026/10/12'; rollout_dir.mkdir(parents=True)
    tid=str(uuid.uuid4()); turn=str(uuid.uuid4()); window=str(uuid.uuid4())
    path=rollout_dir/f'rollout-2026-10-12T00-00-00-{tid}.jsonl'
    (home/'config.toml').write_text(f'''model = "gpt-6-astra"
model_provider = "fixture"
approval_policy = "never"
sandbox_mode = "read-only"
[model_providers.fixture]
name = "Loopback fixture"
base_url = "{provider.url}"
wire_api = "responses"
requires_openai_auth = false
''')
    context={'turn_id':turn,'cwd':str(workspace),'approval_policy':'never','sandbox_policy':{'type':'read-only'},'model':'gpt-6-astra','effort':'high','summary':'auto','multi_agent_version':'v1'}
    meta={'id':tid,'timestamp':'2026-10-12T00:00:00Z','cwd':str(workspace),'originator':'codeg-resume-test','cli_version':'0.162.1','source':'cli','model_provider':'fixture'}
    old={'type':'message','role':'user','content':[{'type':'input_text','text':'PRE-COMPACTION-CONTEXT'},{'type':'input_image','image_url':IMAGE}]}
    cp={'message':'Native fixture checkpoint','replacement_history':[{'type':'message','role':'user','content':[{'type':'input_text','text':'ESSENTIAL-COMPACTED-CONTEXT'}]}],
        'window_number':1,'first_window_id':window,'window_id':window,
        'resume_metadata':{'multi_agent_version':'v1','last_started_turn_id':turn,'previous_turn_settings':{'model':'gpt-6-astra','comp_hash':None,'realtime_active':None}}}
    settings={'type':'thread_settings_applied','thread_id':tid,'thread_settings':{'model':'gpt-6-astra','model_provider_id':'fixture','approval_policy':'never','approvals_reviewer':'user','permission_profile':{'type':'disabled'},'cwd':str(workspace),'collaboration_mode':{'mode':'default','settings':{'model':'gpt-6-astra','reasoning_effort':'high','developer_instructions':None}},'disabled_plugin_ids':[]}}
    if mode=='legacy': cp.pop('resume_metadata')
    if mode=='invalid': cp['replacement_history']=None
    if mode=='ambiguous-user': cp['replacement_history']=[]
    if mode=='ambiguous-runtime':
        cp['resume_metadata']['multi_agent_version']=None
        context.pop('multi_agent_version',None)
    if mode=='checkpoint-turn-not-closed': cp['resume_metadata']['last_started_turn_id']='unclosed-checkpoint-turn'
    missing_checkpoint = mode=='missing'
    emit_settings = mode!='missing-settings'
    def write(size):
        with path.open('wb') as out:
            out.write(line('session_meta',meta));out.write(line('turn_context',dict(context,multi_agent_version='v1')));out.write(line('event_msg',settings));out.write(line('event_msg',{'type':'user_message','message':'FIRST USER','images':[],'local_images':[],'text_elements':[]}))
            # Dense bytes in a discarded extension field keep multi-GiB baseline
            # tests within runner RAM. This is not a copy of a user's history.
            padding=dict(old,codeg_fixture_padding='x'*(1024*1024))
            old_line=line('response_item',padding)
            for _ in range(size): out.write(old_line)
            if not missing_checkpoint: out.write(line('compacted',cp))
            if emit_settings: out.write(line('event_msg',settings))
            out.write(line('world_state',{'full':True,'state':{}}));out.write(line('turn_context',context))
            out.write(line('event_msg',{'type':'task_started','turn_id':turn,'model_context_window':262144}))
            if mode!='ambiguous-user':
                out.write(line('response_item',{'type':'message','role':'user','content':[{'type':'input_text','text':'INCREMENTAL-CONTEXT'},{'type':'input_image','image_url':IMAGE}]}))
            out.write(line('response_item',{'type':'function_call','call_id':'fixture-paired','name':'exec_command','arguments':'{}'}))
            out.write(line('response_item',{'type':'function_call_output','call_id':'fixture-paired','output':'PERSISTED TOOL RESULT'}))
            out.write(line('response_item',{'type':'function_call','call_id':'fixture-pending','name':'exec_command','arguments':'{}'}))
            out.write(line('response_item',{'type':'message','role':'assistant','phase':'final_answer','content':[{'type':'output_text','text':'PREVIOUS FINAL'}]}))
            out.write(line('event_msg',{'type':'task_complete','turn_id':turn,'last_agent_message':'PREVIOUS FINAL'}))
    write(0)
    return home,path,tid,write


def initialize(cli,home,workspace,log,fast=False,trace=None):
    rpc=RPC([str(cli),'app-server'],home,workspace,log,{'CODEG_LEGACY_CHECKPOINT_RESUME':'1' if fast else '0'},trace)
    _,ms=rpc.rpc('initialize',{'clientInfo':{'name':'codeg-resume-test','version':'1'},'capabilities':{'experimentalApi':True}})
    rpc.send({'jsonrpc':'2.0','method':'initialized','params':{}})
    return rpc,ms

def measure(cli,home,path,tid,workspace,logs,name,fast=False,trace=False,prompt=False):
    log=logs/(name+'.stderr.log');tr=(path,logs/(name+'.strace')) if trace else None
    rpc,handshake=initialize(cli,home,workspace,log,fast,tr)
    try:
        resumed,cold=rpc.rpc('thread/resume',{'threadId':tid,'excludeTurns':True,'cwd':str(workspace)})
        warm,warm_ms=rpc.rpc('thread/resume',{'threadId':tid,'excludeTurns':True,'cwd':str(workspace)})
        for response in [resumed,warm]:
            assert response['thread']['id']==tid,response
            assert response['thread']['historyMode']=='legacy',response
            assert response['thread']['turns']==[],response
        if prompt:
            rpc.rpc('turn/start',{'threadId':tid,'input':[{'type':'text','text':'VERIFY CONTEXT AND DO NOT RUN TOOLS'}]})
            rpc.wait_turn()
        result={'handshake_ms':handshake,'cold_resume_ms':cold,'warm_resume_ms':warm_ms,'peak_tree_rss_kib':rpc.peak_kib or None,'rollout_bytes':path.stat().st_size,'thread_id':tid,'history_mode':'legacy','returned_turns':0,'timeline':rpc.timeline,'trace':bool(trace and os.name!='nt')}
    finally: rpc.close()
    if trace and os.name!='nt':
        read_bytes=0;opens=0;phase={}
        for entry in tr[1].open():
            m=re.search(r'\s(\d+\.\d+) ',entry)
            label='outside'
            if m:
                t=float(m[1]);label=next((str(i)+':'+p['method'] for i,p in enumerate(result['timeline']) if p['start_epoch']<=t<=p['end_epoch']),'outside')
            n=re.search(r'(?:read\(|pread64\(|<\.\.\. (?:read|pread64) resumed>)[^\n]*\s+=\s+(\d+)',entry)
            if n:
                read_bytes+=int(n[1]);phase[label]=phase.get(label,0)+int(n[1])
            if 'openat(' in entry: opens+=1
        result.update(read_bytes=read_bytes,phase_read_bytes=phase,open_count=opens)
    (logs/(name+'.json')).write_text(json.dumps(result,indent=2)+'\n')
    return result

def normalize_fixture_home(value, home):
    # Normalize actual string values, before JSON encoding. In serialized JSON
    # Windows backslashes are escaped, so replacing the native path misses it.
    if isinstance(value, str):
        return value.replace(str(home), '<ISOLATED_CODEX_HOME>')
    if isinstance(value, list):
        return [normalize_fixture_home(item, home) for item in value]
    if isinstance(value, dict):
        return {key: normalize_fixture_home(item, home) for key, item in value.items()}
    return value


def prefix_hash(path, length):
    digest=hashlib.sha256()
    with path.open('rb') as file:
        remaining=length
        while remaining:
            chunk=file.read(min(remaining,1024*1024))
            if not chunk: raise AssertionError('Original rollout was truncated')
            digest.update(chunk); remaining-=len(chunk)
    return digest.hexdigest()


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--cli',type=Path,required=True);p.add_argument('--baseline-cli',type=Path);p.add_argument('--output',type=Path,required=True);p.add_argument('--size-mib',type=int,default=64);p.add_argument('--trace',action='store_true');p.add_argument('--checkpoint-mode',choices=['valid','legacy'],default='valid')
    args=p.parse_args();root=args.output.resolve();root.mkdir(parents=True,exist_ok=False)
    workspace=root/'workspace';workspace.mkdir();provider=Provider();summary={'checkpoint_mode':args.checkpoint_mode}
    try:
        baseline=args.baseline_cli or args.cli
        base_home,path,tid,write=fixture(root/'base',workspace,provider,args.size_mib,args.checkpoint_mode)
        # A normal persisted SQLite row avoids timing first-install metadata backfill
        # as if it were session/resume. Its initialization is recorded separately.
        summary['metadata_prime']=measure(baseline,base_home,path,tid,workspace,root,'metadata-prime')
        write(args.size_mib)
        snapshots={}
        for label,cli,fast in [('baseline',baseline,False),('optimized',args.cli,True)]:
            case=root/label;shutil.copytree(base_home,case/'home');home=case/'home'
            case_path=next((home/'sessions').rglob('*.jsonl'))
            original_size=case_path.stat().st_size
            original_hash=prefix_hash(case_path,original_size)
            for database in home.glob('state*.sqlite'):
                with sqlite3.connect(database) as db:
                    if db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='threads'").fetchone():
                        db.execute("UPDATE threads SET rollout_path=? WHERE id=?", (str(case_path),tid))
            count=len(provider.requests)
            result=measure(cli,home,case_path,tid,workspace,root,label,fast,args.trace,prompt=True)
            assert len(provider.requests)==count+1,'Unexpected repeated model request'
            snapshots[label]=normalize_fixture_home(provider.requests[-1]['input'], home)
            # New turn messages receive random protocol IDs; they do not change
            # model context. Historic tool call IDs and image data stay exact.
            for message in snapshots[label]:
                if message.get('type')=='message': message.pop('id',None)
            (root/(label+'.model-input.json')).write_text(json.dumps(provider.requests[-1],indent=2)+'\n')
            encoded=json.dumps(snapshots[label]);assert 'ESSENTIAL-COMPACTED-CONTEXT' in encoded
            assert 'INCREMENTAL-CONTEXT' in encoded
            images=[part for message in snapshots[label] if message.get('type')=='message' for part in message.get('content',[]) if part.get('type')=='input_image']
            assert len(images)==1 and images[0]['image_url'].startswith('data:image/'), 'Image was lost during recovery'
            assert base64.b64decode(images[0]['image_url'].split(',',1)[1]), 'Image payload is empty'
            assert 'PERSISTED TOOL RESULT' in encoded and 'fixture-paired' in encoded
            assert 'PRE-COMPACTION-CONTEXT' not in encoded
            assert prefix_hash(case_path,original_size)==original_hash,'Original history was rewritten'
            result['original_prefix_sha256']=original_hash
            result['original_prefix_bytes']=original_size
            result['original_prefix_preserved']=True
            result['restart']=measure(cli,home,case_path,tid,workspace,root,label+'-restart',fast,trace=args.trace,prompt=False)
            summary[label]=result
        assert snapshots['baseline']==snapshots['optimized'],'Model-visible context changed'
        summary['model_context_equal']=snapshots['baseline']==snapshots['optimized']
        summary['comparison_normalizes']=['transient message IDs','isolated CODEX_HOME prefix']
        if args.trace and os.name!='nt':
            optimized_reads=summary['optimized']['phase_read_bytes']['1:thread/resume']
            baseline_reads=summary['baseline']['phase_read_bytes']['1:thread/resume']
            if args.checkpoint_mode=='valid':
                assert optimized_reads < 64*1024*1024+1024*1024, 'Valid checkpoint exceeded its bounded reads'
                if args.size_mib>=8:
                    assert optimized_reads < baseline_reads/2, 'Valid checkpoint still replayed the full file'
                summary['bounded_checkpoint_reads_verified']=True
            else:
                original=summary['optimized']['original_prefix_bytes']
                assert original <= optimized_reads < original+1024*1024, 'Legacy fallback did not read full history exactly once'
                if args.size_mib>=8:
                    assert optimized_reads < baseline_reads*.6, 'Legacy fallback repeated its full read'
                summary['single_full_fallback_read_verified']=True
        for mode in ['legacy','invalid','missing','ambiguous-user','ambiguous-runtime','missing-settings','oversized-tail','checkpoint-turn-not-closed']:
            home,path,tid,write=fixture(root/mode,workspace,provider,2,mode)
            measure(args.cli,home,path,tid,workspace,root,mode+'-prime',False)
            write(2)
            if mode=='oversized-tail':
                with path.open('ab') as large:
                    large.write(line('response_item',{'type':'message','role':'assistant','content':[{'type':'output_text','text':'HUGE TAIL'}],'codeg_fixture_padding':'x'*(65*1024*1024)}))
            contexts={}
            for label,cli,fast in [('reference',baseline,False),('candidate',args.cli,True)]:
                case=root/(mode+'-'+label);shutil.copytree(home,case/'home')
                case_home=case/'home';case_path=next((case_home/'sessions').rglob('*.jsonl'))
                for database in case_home.glob('state*.sqlite'):
                    with sqlite3.connect(database) as db:
                        db.execute('UPDATE threads SET rollout_path=? WHERE id=?',(str(case_path),tid))
                result=measure(cli,case_home,case_path,tid,workspace,root,mode+'-'+label,fast,args.trace,prompt=True)
                context=normalize_fixture_home(provider.requests[-1]['input'], case_home)
                for message in context:
                    if message.get('type')=='message':message.pop('id',None)
                if mode=='checkpoint-turn-not-closed' and fast and args.trace and os.name!='nt':
                    assert result['phase_read_bytes']['1:thread/resume']>=2*1024*1024, 'Unmatched checkpoint turn skipped native full recovery'
                if fast and args.trace and os.name!='nt' and mode not in ['missing','oversized-tail']:
                    assert result['phase_read_bytes']['1:thread/resume'] < case_path.stat().st_size+1024*1024, 'Fallback re-read full history during host configuration'
                    result['single_full_fallback_read_verified']=True
                contexts[label]=context
                summary[mode+'_'+label]=result
            assert contexts['reference']==contexts['candidate'], 'Fallback changed context: '+mode
            summary[mode+'_context_equal']=True
        for change in ['append','replace','corrupt-tail']:
            home,path,tid,write=fixture(root/change,workspace,provider,2)
            measure(args.cli,home,path,tid,workspace,root,change+'-prime',False)
            write(2)
            summary[change+'_before']=measure(args.cli,home,path,tid,workspace,root,change+'-before',True)
            if change=='append':
                with path.open('ab') as file:
                    file.write(line('response_item',{'type':'message','role':'user','content':[{'type':'input_text','text':'APPENDED-DELTA'}]}))
            elif change=='replace':
                data=path.read_bytes().replace(b'ESSENTIAL-COMPACTED-CONTEXT',b'REPLACED-CHECKPOINT-CONTEXT')
                replacement=path.with_suffix('.replacement')
                replacement.write_bytes(data);os.replace(replacement,path)
            else:
                with path.open('ab') as file:file.write(b'{broken-tail}\n')
            summary[change+'_restart']=measure(args.cli,home,path,tid,workspace,root,change+'-restart',True,args.trace,prompt=True)
            captured=json.dumps(provider.requests[-1]['input'])
            if change=='append':assert 'APPENDED-DELTA' in captured
            if change=='replace':assert 'REPLACED-CHECKPOINT-CONTEXT' in captured and 'ESSENTIAL-COMPACTED-CONTEXT' not in captured
        (root/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
        print(json.dumps(summary,indent=2),flush=True)
    finally: provider.server.shutdown();provider.server.server_close()

if __name__=='__main__':main()
