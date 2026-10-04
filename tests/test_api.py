"""Local acceptance checks against the real binary and a disposable Proxmox HTTP fixture.
Run cargo build first, then: python -m unittest discover -s tests -v
No infrastructure credentials or third-party Python packages are required.
"""
import json, os, pathlib, socket, subprocess, tempfile, threading, time, unittest, urllib.request, urllib.error, urllib.parse
from email.parser import BytesParser
from oci_fixture import Registry, read_template
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
ROOT = pathlib.Path(__file__).resolve().parents[1]
class Proxmox(BaseHTTPRequestHandler):
    calls = []
    task_exit = 'OK'
    guests = {}
    start_fail_id = None
    payloads = []
    templates = {}
    uploads = []
    def log_message(self, *args): pass
    def do_GET(self): self.respond()
    def do_POST(self): self.respond()
    def do_DELETE(self): self.respond()
    def do_PUT(self): self.respond()
    def respond(self):
        Proxmox.calls.append((self.command,self.path))
        path = self.path.split('?')[0]; data = None; status = 200
        raw_bytes=self.rfile.read(int(self.headers.get('Content-Length','0')))
        content_type=self.headers.get('Content-Type','')
        if content_type.startswith('multipart/form-data'):
            message=BytesParser().parsebytes(('Content-Type: '+content_type+'\r\nMIME-Version: 1.0\r\n\r\n').encode()+raw_bytes)
            fields={}
            for part in message.walk():
                filename=part.get_filename()
                if filename:
                    Proxmox.templates[filename]=part.get_payload(decode=True);Proxmox.uploads.append((filename,Proxmox.templates[filename]))
        else:
            raw=raw_bytes.decode()
            fields={k:[v if isinstance(v,str) else json.dumps(v)] for k,v in json.loads(raw or '{}').items()} if 'application/json' in content_type else urllib.parse.parse_qs(raw)
        Proxmox.payloads.append((self.command,path,fields))
        parts=path.split('/')
        dynamic=next((int(x) for i,x in enumerate(parts) if i>0 and parts[i-1]=='lxc' and x.isdigit() and int(x) in Proxmox.guests),None)
        if dynamic is not None:
            guest=Proxmox.guests[dynamic]
            if path.endswith('/config'):
                if self.command=='PUT':
                    for k,v in fields.items():
                        if k=='delete':
                            for key in v[0].split(','): guest['config'].pop(key,None)
                        else: guest['config'][k]=v[0]
                    guest['config']['digest']=str(int(guest['config'].get('digest','0'))+1)
                data=guest['config']
            elif path.endswith('/status/current'): data={'status':guest['status']}
            elif path.endswith('/interfaces'): data=[{'name':'eth0','inet':'192.168.50.103/24'}]
            elif path.endswith('/snapshot'):
                snapshots=guest.setdefault('snapshots',{})
                if self.command=='POST': snapshots[fields['snapname'][0]]=dict(guest['config']);data='UPID:beta:1:2:3:snapshot:1:user:'
                else: data=[{'name':'current'}]+[{'name':name} for name in snapshots]
            elif '/snapshot/' in path and path.endswith('/rollback'):
                snap=parts[-2];guest['config']=dict(guest.get('snapshots',{})[snap]);data='UPID:beta:1:2:3:rollback:1:user:'
            elif path.endswith('/move_volume'):
                key=fields['volume'][0];target=Proxmox.guests[int(fields['target-vmid'][0])]
                if key not in guest['config'] or key in target['config']: status=400; data={'error':'ambiguous volume'}
                else: target['config'][key]=guest['config'].pop(key); data='UPID:beta:1:2:3:move:1:user:'
            elif '/status/' in path and self.command=='POST':
                action=parts[-1]
                failed=action=='start' and dynamic==Proxmox.start_fail_id
                if not failed: guest['status']='running' if action=='start' else 'stopped'
                data='UPID:beta:1:2:3:'+('fixturefail' if failed else action)+':1:user:'
            else: status=400;data={'error':'unsupported dynamic request'}
        elif path.endswith('/vzdump') and self.command=='POST': data='UPID:beta:1:2:3:backup:1:user:'
        elif path.endswith('/nodes'): data = [{'node':'alpha'},{'node':'beta'}]
        elif path.endswith('/cluster/resources'): data = [{'type':'node','node':'alpha','cpu':0.1,'mem':1024,'maxmem':4096,'disk':1024,'maxdisk':8192}, {'vmid':101,'name':'web','node':'beta','type':'lxc','status':'running'}, {'vmid':102,'name':'vm','node':'alpha','type':'qemu','status':'running'}]+[{'vmid':vmid,'name':'managed','node':'beta','type':'lxc','status':guest['status']} for vmid,guest in Proxmox.guests.items()]
        elif path.endswith('/cluster/nextid'): data = '103'
        elif path.endswith('/storage'): data = [{'storage':'disk','active':1,'content':'rootdir'}, {'storage':'local','active':1,'content':'vztmpl'}, {'storage':'archive','active':1,'content':'backup'}]
        elif path.endswith('/upload') and self.command=='POST':data='UPID:beta:1:2:3:upload:1:user:'
        elif '/content' in path and self.command == 'GET': data = [{'volid':'local:vztmpl/debian-12-standard_test_amd64.tar.zst','content':'vztmpl'}]+[{'volid':'local:vztmpl/'+name,'content':'vztmpl'} for name in Proxmox.templates]
        elif '/content/' in path and self.command=='DELETE':
            filename=urllib.parse.unquote(parts[-1]).split(':vztmpl/')[-1];Proxmox.templates.pop(filename,None);data=None
        elif path.endswith('/rrddata'): data = []
        elif path.endswith('/network'): data = [{'iface':'vmbr0','type':'bridge'}]
        elif '/tasks/' in path: data = {'status':'running' if Proxmox.task_exit == 'BLOCK' else 'stopped','exitstatus':'fixture start failure' if 'fixturefail' in path else Proxmox.task_exit}
        elif path.endswith('/status/current'): data = {'status':'running'}
        elif path.endswith('/interfaces'): data = [{'name':'eth0','inet':'192.168.50.103/24'}]
        elif path.endswith('/lxc') and self.command == 'POST':
            vmid=int(fields['vmid'][0]);data='UPID:beta:1:2:3:vzcreate:'+str(vmid)+':user:'
            if 'hostable_' in fields.get('ostemplate',[''])[0]:
                config={key:value[0] for key,value in fields.items()};config['digest']='1'
                for key,spec in list(config.items()):
                    if key.startswith('mp') and key[2:].isdigit():
                        pool,_=spec.split(',',1)[0].split(':',1);config[key]=pool+':subvol-'+str(vmid)+'-disk-'+key[2:]+','+spec.split(',',1)[1]
                Proxmox.guests[vmid]={'config':config,'status':'stopped'}
        elif '/lxc/101/status/' in path: data = 'UPID:beta:1:2:3:vzstart:101:user:'
        elif '/lxc/103/status/' in path: data = 'UPID:beta:1:2:3:vzstart:103:user:'
        elif path.endswith('/lxc/101/config'): data = {'hostname':'web','description':'external guest','tags':'hostable;managed'}
        elif path.endswith('/snapshot'): data = []
        else: status = 400; data = {'errors':{'fixture':'rejected'}}
        body = json.dumps({'data':data}).encode(); self.send_response(status); self.send_header('Content-Type','application/json'); self.end_headers(); self.wfile.write(body)
class API(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        binary = pathlib.Path(os.environ.get('HOSTABLE_TEST_BINARY',ROOT / 'backend' / 'target' / 'debug' / ('backend.exe' if os.name == 'nt' else 'backend')))
        if not binary.is_file(): raise RuntimeError('Build the backend binary with cargo build before running API tests')
        cls.temp = tempfile.TemporaryDirectory(prefix='hostable-api-')
        cls.addClassCleanup(cls.temp.cleanup)
        cls.pve = ThreadingHTTPServer(('127.0.0.1',0),Proxmox); threading.Thread(target=cls.pve.serve_forever,daemon=True).start()
        cls.addClassCleanup(cls.pve.server_close); cls.addClassCleanup(cls.pve.shutdown)
        with socket.socket() as sock: sock.bind(('127.0.0.1',0)); port = sock.getsockname()[1]
        cls.port = port
        cls.base = 'http://127.0.0.1:' + str(port)
        env = {k:v for k,v in os.environ.items() if not k.startswith(('PROXMOX_','HOSTABLE_','DATABASE_URL','SECUREWEB_'))}
        env.update(PORT=str(port),HOSTABLE_BIND='127.0.0.1',PROXMOX_NODE='alpha',PROXMOX_API_URL='http://127.0.0.1:'+str(cls.pve.server_port)+'/api2/json',DATABASE_URL='sqlite:///'+pathlib.Path(cls.temp.name,'metadata.db').as_posix()+'?mode=rwc',HOSTABLE_DATA_DIR=str(pathlib.Path(cls.temp.name,'data')))
        binary = pathlib.Path(os.environ.get('HOSTABLE_TEST_BINARY',ROOT / 'backend' / 'target' / 'debug' / ('backend.exe' if os.name == 'nt' else 'backend')))
        cls.env = env; cls.binary = binary
        cls.log = open(pathlib.Path(cls.temp.name,'server.log'),'w')
        cls.addClassCleanup(cls.log.close)
        cls.process = subprocess.Popen([str(binary),'start'],cwd=cls.temp.name,env=env,stdout=cls.log,stderr=subprocess.STDOUT)
        cls.addClassCleanup(cls.stop_process)
        for _ in range(100):
            try:
                urllib.request.urlopen(cls.base+'/api/health',timeout=1); break
            except Exception:
                if cls.process.poll() is not None: cls.log.flush(); raise RuntimeError(pathlib.Path(cls.temp.name,'server.log').read_text())
                time.sleep(.1)
        else: raise RuntimeError('API startup timed out')
        tokens = list(pathlib.Path(cls.temp.name).rglob('admin-token'))
        if not tokens: raise RuntimeError('Admin token was not persisted')
        cls.token = tokens[0].read_text().strip()
    @classmethod
    def stop_process(cls):
        if cls.process.poll() is None:
            cls.process.terminate()
            try: cls.process.wait(timeout=10)
            except subprocess.TimeoutExpired: cls.process.kill(); cls.process.wait(timeout=10)
    def request(self,path,method='GET',body=None,auth=True):
        headers = {'Authorization':'Bearer '+self.token} if auth else {}
        if body is not None: headers['Content-Type']='application/json'
        req = urllib.request.Request(self.base+'/api'+path,data=json.dumps(body).encode() if body is not None else None,headers=headers,method=method)
        try:
            with urllib.request.urlopen(req,timeout=10) as res: return res.status,json.loads(res.read())
        except urllib.error.HTTPError as e:
            text=e.read().decode()
            try: return e.code,json.loads(text)
            except ValueError: return e.code,text
    def restart_server(self, environment):
        self.stop_process();self.__class__.process=subprocess.Popen([str(self.binary),'start'],cwd=self.temp.name,env=environment,stdout=self.log,stderr=subprocess.STDOUT)
        for _ in range(100):
            try:
                if self.request('/health')[0]==200:return
            except Exception:pass
            time.sleep(.03)
        self.fail('Fixture restart timed out')
    def test_authentication_required(self):
        for path in ['/jobs','/databases','/lxcs','/secureweb/routes','/ready']:
            self.assertEqual(self.request(path,auth=False)[0],401,path)
    def test_unknown_api_is_not_the_dashboard(self): self.assertEqual(self.request('/missing')[0],404)
    def test_only_lxc_resources_are_managed(self):
        status,rows=self.request('/lxcs'); self.assertEqual(status,200); self.assertEqual([r['id'] for r in rows],[101]); self.assertEqual(rows[0]['node'],'beta')
    def test_operations_use_actual_node(self):
        Proxmox.task_exit='OK'; self.assertEqual(self.request('/lxcs/101/start','POST',{})[0],200)
        self.assertIn(('POST','/api2/json/nodes/beta/lxc/101/status/start'),Proxmox.calls)
    def test_qemu_is_rejected(self): self.assertGreaterEqual(self.request('/lxcs/102/start','POST',{})[0],400)
    def test_task_failure_is_not_success(self):
        Proxmox.task_exit='simulated failure'
        self.assertEqual(self.request('/lxcs/101/start','POST',{})[0],502); Proxmox.task_exit='OK'
    def test_missing_task_exit_is_not_success(self):
        Proxmox.task_exit=None
        self.assertEqual(self.request('/lxcs/101/start','POST',{})[0],502); Proxmox.task_exit='OK'
    def test_invalid_snapshot_is_rejected_before_pve(self):
        before=len(Proxmox.calls); self.assertEqual(self.request('/lxcs/101/snapshots','POST',{'snapname':'../escape'})[0],400); self.assertEqual(len(Proxmox.calls),before)
    def test_storage_discovery_is_node_scoped(self):
        self.assertEqual(self.request('/node/storages?node=beta')[0],200); self.assertIn(('GET','/api2/json/nodes/beta/storage'),Proxmox.calls)
    def test_shared_metadata_database_hook_is_rejected(self):
        self.assertEqual(self.request('/deploy','POST',{'use_hostable_db':True})[0],400)
    def test_merged_services_are_rejected(self):
        self.assertEqual(self.request('/lxc/stack/deploy','POST',{'images':['postgres:15','nginx:alpine']})[0],400)
    def test_invalid_deploy_has_no_side_effects(self):
        before=len(Proxmox.calls); self.assertEqual(self.request('/ansible/deploy','POST',{'hostname':'web','image':'alpine:latest','mountpoints':[{'storage':'disk','size_gb':8,'container':'/x/../etc'}]})[0],400); self.assertEqual(len(Proxmox.calls),before)
    def test_database_validation_precedes_guest_setup(self):
        req={'name':'db','ostemplate':'local:vztmpl/debian-12-standard_test_amd64.tar.zst','storage':'disk','bridge':'vmbr0','ip_address':'dhcp','data_size_gb':16,'allowed_cidrs':['192.168.1.0/24']}
        self.assertEqual(self.request('/databases','POST',req)[0],400)
        req['ip_address']='192.168.1.30/24'; req['allowed_cidrs']=['192.168.1.0/99']; self.assertEqual(self.request('/databases','POST',req)[0],400)
    def test_gateway_failure_is_visible(self):
        status,data=self.request('/secureweb/status'); self.assertEqual(status,200); self.assertFalse(data['connected']); self.assertEqual(data['active_routes'],0)
        self.assertGreaterEqual(self.request('/secureweb/routes','POST',{'domain':'web.example.com','target_ip':'192.168.1.1','target_port':80,'service_name':'web','vmid':101,'mode':'https'})[0],400)
        self.assertEqual(self.request('/secureweb/routes')[1],[])
    def test_deployment_failure_is_a_durable_failed_job(self):
        Proxmox.task_exit='injected create failure'
        status,data=self.request('/ansible/deploy','POST',{'hostname':'new-web','node':'beta','ostemplate':'local:vztmpl/debian-12-standard_test_amd64.tar.zst','storage_pool':'disk','net_bridge':'vmbr0','ip_address':'192.168.50.103/24'})
        self.assertEqual(status,202); task=data['task_id']
        for _ in range(100):
            job=self.request('/jobs/'+task)[1]
            if job['status'] in ['failed','succeeded']: break
            time.sleep(.05)
        self.assertEqual(job['status'],'failed'); self.assertEqual(job['events'][-1]['step'],'FAILED'); self.assertIn('injected create failure',job['events'][-1]['message']); Proxmox.task_exit='OK'
    def test_existing_container_cannot_be_overwritten(self):
        status,data=self.request('/ansible/deploy','POST',{'hostname':'overwrite','vmid':101,'image':'nginx:1.28-alpine'})
        self.assertEqual(status,202)
        for _ in range(100):
            job=self.request('/jobs/'+data['task_id'])[1]
            if job['status']=='failed': break
            time.sleep(.05)
        self.assertEqual(job['status'],'failed'); self.assertIn('already allocated',job['events'][-1]['message'])
    def test_templates_are_discovered_on_selected_node(self):
        status,templates=self.request('/node/templates?node=beta')
        self.assertEqual(status,200); self.assertEqual(templates[0]['volid'],'local:vztmpl/debian-12-standard_test_amd64.tar.zst')
    def test_os_template_deployment_confirms_running_and_real_address(self):
        Proxmox.task_exit='OK'
        status,data=self.request('/ansible/deploy','POST',{'hostname':'success-test','node':'beta','ostemplate':'local:vztmpl/debian-12-standard_test_amd64.tar.zst','storage_pool':'disk','ip_address':'192.168.50.103/24'})
        self.assertEqual(status,202)
        for _ in range(100):
            job=self.request('/jobs/'+data['task_id'])[1]
            if job['status'] in ['failed','succeeded']: break
            time.sleep(.05)
        self.assertEqual(job['status'],'succeeded',job['events']); self.assertIn('192.168.50.103',job['events'][-1]['message'])
    def test_database_retirement_rejects_a_reused_vmid(self):
        import sqlite3
        record={'id':'db_stale','name':'web','node':'beta','vmid':101,'host':'192.168.50.101','port':5432,'status':'ready','created_at':1,'task_id':'previous-task','storage':'disk','data_size_gb':16,'data_volume':None,'allowed_cidrs':['192.168.50.0/24'],'manager_cidr':'192.168.50.10/32','backup_interval_hours':0,'retention_count':7,'postgres_version':None,'error':None}
        connection=sqlite3.connect(self.temp.name+'/metadata.db')
        connection.execute("INSERT INTO hostable_records(kind,id,body) VALUES('database','db_stale',?)",(json.dumps(record),)); connection.commit()
        before=len(Proxmox.calls)
        try:
            status,result=self.request('/databases/db_stale/retire','POST',{})
            self.assertEqual(status,400,result)
            self.assertFalse(any(method=='POST' and '/status/stop' in path for method,path in Proxmox.calls[before:]))
        finally:
            connection.execute("DELETE FROM hostable_records WHERE id='db_stale'"); connection.commit(); connection.close()
    def test_rotation_cannot_publish_a_failed_restore_as_ready(self):
        import sqlite3
        app={'id':'appdb_incomplete','status':'restoring','instance_id':'no_instance','name':'incomplete_app','username':'incomplete_user','created_at':1,'last_backup_at':None,'last_backup_error':None}
        connection=sqlite3.connect(self.temp.name+'/metadata.db')
        connection.execute("INSERT INTO hostable_records(kind,id,body) VALUES('app_database','appdb_incomplete',?)",(json.dumps(app),)); connection.commit()
        try:
            self.assertEqual(self.request('/databases/apps/appdb_incomplete/rotate','POST',{})[0],400)
            self.assertEqual(self.request('/databases/apps/appdb_incomplete/revoke','POST',{})[0],400)
            status=json.loads(connection.execute("SELECT body FROM hostable_records WHERE id='appdb_incomplete'").fetchone()[0])['status']; self.assertEqual(status,'restoring')
        finally:
            connection.execute("DELETE FROM hostable_records WHERE id='appdb_incomplete'"); connection.commit(); connection.close()
    def test_query_token_only_works_on_websockets(self):
        self.assertEqual(self.request('/verify?token='+self.token,auth=False)[0],401)
    def test_websocket_requires_authentication(self):
        req=urllib.request.Request(self.base+'/api/ws/tasks/missing',headers={'Connection':'Upgrade','Upgrade':'websocket','Sec-WebSocket-Version':'13','Sec-WebSocket-Key':'dGhlIHNhbXBsZSBub25jZQ=='})
        with self.assertRaises(urllib.error.HTTPError) as e: urllib.request.urlopen(req,timeout=5)
        self.assertEqual(e.exception.code,401)
    def test_image_conversion_and_replacement_use_reviewed_digests_and_preserve_volumes(self):
        import sqlite3
        registry=Registry(pathlib.Path(self.temp.name,'rollout-registry'));self.addCleanup(registry.close)
        versions={version:registry.image(version) for version in ['v1','v2','v3']}
        connection=sqlite3.connect(self.temp.name+'/metadata.db')
        def cleanup():
            connection.execute("DELETE FROM hostable_records WHERE kind='workload' AND id='30000'");connection.commit();connection.close()
            Proxmox.guests={};Proxmox.templates={};Proxmox.uploads=[]
        self.addCleanup(cleanup)
        env=self.env.copy();env['HOSTABLE_OCI_CA_CERT']=str(registry.ca)
        self.restart_server(env);self.addCleanup(lambda:self.restart_server(self.env))
        status,result=self.request('/ansible/deploy','POST',{'hostname':'image-fixture','node':'beta','vmid':30000,'image':registry.repository+':v1','template_storage':'local','storage_pool':'disk','ip_address':'192.168.50.103/24','env_vars':{'DEPLOYMENT_SECRET':'private-image-fixture-value'},'mountpoints':[{'storage':'disk','size_gb':8,'container':'/data'}]})
        self.assertEqual(status,202,result);job=self.wait_job(result['task_id']);self.assertEqual(job['status'],'succeeded',job)
        original=self.request('/workloads/30000')[1]
        self.assertEqual(original['active']['pinned_image'],registry.repository+'@'+versions['v1']['digest'])
        archive=read_template(Proxmox.uploads[-1][1])
        self.assertEqual(archive['app/version']['data'],b'v1')
        self.assertIsNotNone(archive['sbin/init']['data']);self.assertIn(b'hostable-entrypoint.sh',archive['sbin/init']['data'])
        self.assertEqual(archive['etc/profile.d/hostable-env.sh']['mode'],0o600)
        self.assertIn(b'private-image-fixture-value',archive['etc/profile.d/hostable-env.sh']['data'])
        self.assertIn(b"cd '/app'",archive['usr/local/bin/hostable-entrypoint.sh']['data'])
        self.assertIn(b"'/docker-entrypoint.sh' '/usr/local/bin/app'",archive['usr/local/bin/hostable-entrypoint.sh']['data'])
        volume=Proxmox.guests[30000]['config']['mp0'];self.assertFalse(Proxmox.templates)
        status,preview=self.request('/workloads/30000/plan','POST',{'mode':'image','image':registry.repository+':v2','backup_storage':'archive'})
        self.assertEqual(status,200,preview);self.assertEqual(preview['plan']['pinned_image'],registry.repository+'@'+versions['v2']['digest'])
        registry.tags['v2']=versions['v3']['digest']
        status,result=self.request('/workloads/30000/apply','POST',{'plan_id':preview['plan']['id']});self.assertEqual(status,202,result)
        job=self.wait_job(result['task_id']);self.assertEqual(job['status'],'succeeded',job)
        current=self.request('/workloads/30000')[1];replacement=current['active']['vmid']
        self.assertNotEqual(replacement,30000);self.assertEqual(current['id'],'30000')
        self.assertEqual(current['active']['pinned_image'],registry.repository+'@'+versions['v2']['digest'])
        self.assertEqual(read_template(Proxmox.uploads[-1][1])['app/version']['data'],b'v2')
        self.assertEqual(Proxmox.guests[replacement]['config']['mp0'],volume);self.assertNotIn('mp0',Proxmox.guests[30000]['config'])
        creates=[fields for method,path,fields in Proxmox.payloads if method=='POST' and path.endswith('/lxc') and fields.get('vmid')==[str(replacement)]]
        self.assertEqual(creates[-1]['onboot'],['0']);self.assertNotIn('mp0',creates[-1])
        self.assertFalse(Proxmox.templates)
        preview=self.request('/workloads/30000/plan','POST',{'mode':'rollback','backup_storage':'archive'})[1]
        result=self.request('/workloads/30000/apply','POST',{'plan_id':preview['plan']['id']})[1]
        job=self.wait_job(result['task_id']);self.assertEqual(job['status'],'succeeded',job)
        self.assertEqual(self.request('/workloads/30000')[1]['active']['vmid'],30000)
        self.assertEqual(Proxmox.guests[30000]['config']['mp0'],volume);self.assertEqual(Proxmox.guests[replacement]['status'],'stopped')
        self.assertNotIn('private-image-fixture-value',json.dumps(job))

    def test_oci_pull_requires_trusted_tls_and_rejects_corrupt_layer_content(self):
        registry=Registry(pathlib.Path(self.temp.name,'integrity-registry'));self.addCleanup(registry.close)
        version=registry.image('v1');output=pathlib.Path(self.temp.name,'integrity-image.tar.xz')
        command=[str(self.binary),'pull-docker','--image',registry.repository+':v1','--out',str(output)]
        result=subprocess.run(command,cwd=self.temp.name,env=self.env,capture_output=True,text=True,timeout=20)
        self.assertNotEqual(result.returncode,0);self.assertFalse(output.exists())
        env=self.env.copy();env['HOSTABLE_OCI_CA_CERT']=str(registry.ca)
        registry.corrupt[version['layer']]=b'wrong image layer bytes'
        result=subprocess.run(command,cwd=self.temp.name,env=env,capture_output=True,text=True,timeout=20)
        self.assertNotEqual(result.returncode,0);self.assertIn('digest mismatch',result.stderr.lower());self.assertFalse(output.exists())
        self.assertFalse(pathlib.Path(str(output)+'.cache').exists())
        del registry.corrupt[version['layer']]
        for _ in range(2):
            # Completed pulls retain metadata but remove the unpacked cache.
            # A subsequent pull must download the layers again and emit a complete archive.
            before=registry.requests.count('/v2/fixture/app/blobs/'+version['layer'])
            result=subprocess.run(command,cwd=self.temp.name,env=env,capture_output=True,text=True,timeout=20)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(registry.requests.count('/v2/fixture/app/blobs/'+version['layer']),before+1)
            self.assertEqual(read_template(output.read_bytes())['app/version']['data'],b'v1')
            self.assertFalse(pathlib.Path(str(output)+'.cache').exists())

    def seeded_workload(self, stateful=True):
        import sqlite3
        connection=sqlite3.connect(self.temp.name+'/metadata.db')
        secrets=pathlib.Path(self.temp.name,'data','secrets');secrets.mkdir(exist_ok=True)
        revisions=[]
        for vmid in [300,301]:
            task='task_fixture_'+str(vmid)
            spec={'hostname':'managed','node':'beta','image':'example/app:1','storage_pool':'disk','ip_address':'192.168.50.103/24','env_vars':{'PRIVATE_SETTING':'never-return-this'},'mountpoints':[{'storage':'disk','size_gb':8,'container':'/data'}] if stateful else []}
            (secrets/('deploy_'+task+'.json')).write_text(json.dumps(spec))
            revisions.append({'vmid':vmid,'owner':task,'image':'example/app:1','pinned_image':None,'spec':task,'created_at':1})
            config={'hostname':'managed','description':'hostable.task='+task,'unprivileged':1,'net0':'name=eth0,bridge=vmbr0,ip=192.168.50.103/24','onboot':1 if vmid==301 else 0,'digest':'1'}
            if vmid==301 and stateful: config['mp0']='disk:subvol-301-disk-1,mp=/data,backup=1'
            Proxmox.guests[vmid]={'config':config,'status':'running' if vmid==301 else 'stopped'}
        workload={'id':'fixture_workload','name':'managed','node':'beta','active':revisions[1],'previous':[revisions[0]],'status':'ready','ip':'192.168.50.103','health':{},'update':{'mode':'image','timeout_seconds':900,'interval_hours':0},'operation':None,'last_update_at':None,'last_update_error':None}
        connection.execute("INSERT OR REPLACE INTO hostable_records(kind,id,body) VALUES('workload','fixture_workload',?)",(json.dumps(workload),));connection.commit()
        def cleanup():
            connection.execute("DELETE FROM hostable_records WHERE kind='workload' AND id='fixture_workload'");connection.commit();connection.close();Proxmox.guests={};Proxmox.start_fail_id=None;Proxmox.task_exit='OK'
        self.addCleanup(cleanup)
        return connection
    def wait_job(self, task):
        for _ in range(200):
            job=self.request('/jobs/'+task)[1]
            if job['status'] in ['failed','succeeded','cancelled','interrupted']: return job
            time.sleep(.03)
        self.fail('Job did not finish: '+str(job))
    def test_update_plan_requires_backup_for_persistent_volumes(self):
        self.seeded_workload(); before=len(Proxmox.calls)
        status,result=self.request('/workloads/fixture_workload/plan','POST',{'mode':'rollback'})
        self.assertEqual(status,400,result);self.assertIn('backup storage',str(result));self.assertFalse(any(m=='POST' for m,p in Proxmox.calls[before:]))
        Proxmox.guests[301]['config']['mp0']='disk:subvol-301-disk-1,mp=/data,backup=0'
        self.assertEqual(self.request('/workloads/fixture_workload/plan','POST',{'mode':'rollback','backup_storage':'archive'})[0],400)
    def test_rollback_moves_persistent_data_and_retains_previous_root(self):
        self.seeded_workload();Proxmox.task_exit='OK'
        status,preview=self.request('/workloads/fixture_workload/plan','POST',{'mode':'rollback','backup_storage':'archive'})
        self.assertEqual(status,200,preview);self.assertNotIn('never-return-this',json.dumps(preview))
        status,result=self.request('/workloads/fixture_workload/apply','POST',{'plan_id':preview['plan']['id']})
        self.assertEqual(status,202,result);job=self.wait_job(result['task_id']);self.assertEqual(job['status'],'succeeded',job)
        current=self.request('/workloads/fixture_workload')[1];self.assertEqual(current['active']['vmid'],300)
        self.assertIn('mp0',Proxmox.guests[300]['config']);self.assertNotIn('mp0',Proxmox.guests[301]['config'])
        self.assertEqual(Proxmox.guests[301]['status'],'stopped');self.assertEqual(str(Proxmox.guests[301]['config']['onboot']),'0')
        self.assertFalse(any(m=='DELETE' and '/lxc/' in p for m,p in Proxmox.calls))
        self.assertEqual(self.request('/workloads/fixture_workload/apply','POST',{'plan_id':preview['plan']['id']})[0],409)
    def test_failed_replacement_start_recovers_original_volume_and_service(self):
        self.seeded_workload();Proxmox.task_exit='OK';Proxmox.start_fail_id=300
        preview=self.request('/workloads/fixture_workload/plan','POST',{'mode':'rollback','backup_storage':'archive'})[1]
        result=self.request('/workloads/fixture_workload/apply','POST',{'plan_id':preview['plan']['id']})[1]
        job=self.wait_job(result['task_id']);self.assertEqual(job['status'],'failed',job)
        current=self.request('/workloads/fixture_workload')[1];self.assertEqual(current['active']['vmid'],301);self.assertIsNone(current['operation'])
        self.assertIn('mp0',Proxmox.guests[301]['config']);self.assertNotIn('mp0',Proxmox.guests[300]['config']);self.assertEqual(Proxmox.guests[301]['status'],'running')
    def test_active_update_blocks_direct_mutations_and_cancels_at_safe_checkpoint(self):
        self.seeded_workload();Proxmox.task_exit='OK'
        preview=self.request('/workloads/fixture_workload/plan','POST',{'mode':'rollback','backup_storage':'archive'})[1];Proxmox.task_exit='BLOCK'
        result=self.request('/workloads/fixture_workload/apply','POST',{'plan_id':preview['plan']['id']})[1]
        for _ in range(100):
            if any(e['step']=='BACKUP' for e in self.request('/jobs/'+result['task_id'])[1]['events']): break
            time.sleep(.02)
        self.assertEqual(self.request('/lxcs/301/stop','POST',{})[0],409)
        self.assertEqual(self.request('/jobs/'+result['task_id']+'/cancel','POST',{})[0],200);Proxmox.task_exit='OK'
        job=self.wait_job(result['task_id']);self.assertEqual(job['status'],'cancelled',job)
        self.assertEqual(Proxmox.guests[301]['status'],'running');self.assertIn('mp0',Proxmox.guests[301]['config'])
    def test_stale_update_plan_and_reused_vmid_are_rejected(self):
        self.seeded_workload(False)
        preview=self.request('/workloads/fixture_workload/plan','POST',{'mode':'rollback'})[1]
        Proxmox.guests[301]['config']['digest']='changed'
        self.assertEqual(self.request('/workloads/fixture_workload/apply','POST',{'plan_id':preview['plan']['id']})[0],409)
        Proxmox.guests[301]['config']['description']='external guest'
        self.assertEqual(self.request('/workloads/fixture_workload/plan','POST',{'mode':'rollback'})[0],400)
    def test_update_review_rejects_policy_changes_and_unrecorded_mounts_or_mappings(self):
        self.seeded_workload(False)
        preview=self.request('/workloads/fixture_workload/plan','POST',{'mode':'rollback'})[1]
        self.assertEqual(self.request('/workloads/fixture_workload/policy','PUT',{'health':{'port':8080},'update':{'mode':'image'}})[0],200)
        self.assertEqual(self.request('/workloads/fixture_workload/apply','POST',{'plan_id':preview['plan']['id']})[0],409)
        Proxmox.guests[301]['config']['mp1']='disk:subvol-301-disk-2,mp=/unrecorded,backup=1'
        self.assertEqual(self.request('/workloads/fixture_workload/plan','POST',{'mode':'rollback'})[0],400)
        del Proxmox.guests[301]['config']['mp1'];Proxmox.guests[301]['config']['unprivileged']=0
        self.assertEqual(self.request('/workloads/fixture_workload/plan','POST',{'mode':'rollback'})[0],400)
        Proxmox.guests[301]['config']['unprivileged']=1;Proxmox.guests[301]['config']['lxc']=[['lxc.idmap','u 0 200000 65536']]
        self.assertEqual(self.request('/workloads/fixture_workload/plan','POST',{'mode':'rollback'})[0],400)
    def test_interrupted_volume_cutover_is_inspected_and_recovered_after_restart(self):
        connection=self.seeded_workload();job_id='task_interrupted_cutover_fixture'
        row=connection.execute("SELECT body FROM hostable_records WHERE kind='workload' AND id='fixture_workload'").fetchone();workload=json.loads(row[0])
        workload['operation']=job_id;workload['status']='updating'
        rollout={'id':job_id,'workload_id':'fixture_workload','mode':'rollback','source':workload['active'],'target':workload['previous'][0],'stage':'CUTOVER','network':Proxmox.guests[301]['config']['net0'],'source_onboot':'1','source_running':True,'volumes':['mp0'],'snapshot':None,'pending_upid':'UPID:beta:1:2:3:move:1:user:'}
        job={'id':job_id,'kind':'rollback','resource':'fixture_workload','status':'running','created_at':1,'updated_at':1,'events':[],'sequence':0,'cancel_requested':False,'cancellable':True}
        for kind,record_id,body in [('workload','fixture_workload',workload),('rollout',job_id,rollout),('job',job_id,job)]:
            connection.execute('INSERT OR REPLACE INTO hostable_records(kind,id,body) VALUES(?,?,?)',(kind,record_id,json.dumps(body)))
        connection.commit()
        Proxmox.guests[300]['config']['mp0']=Proxmox.guests[301]['config'].pop('mp0');Proxmox.guests[301]['config']['onboot']=0
        for vmid in [300,301]:Proxmox.guests[vmid]['status']='stopped'
        self.restart_server(self.env)
        self.assertEqual(self.request('/jobs/'+job_id)[1]['status'],'interrupted')
        status,details=self.request('/jobs/'+job_id+'/inspect','POST',{});self.assertEqual(status,200,details);self.assertIn('rollback',details['actions'])
        self.assertEqual(self.request('/lxcs/301/start','POST',{})[0],409)
        self.assertEqual(self.request('/jobs/'+job_id+'/recover','POST',{'action':'rollback'})[0],202)
        recovered=self.wait_job(job_id);self.assertEqual(recovered['status'],'failed',recovered)
        current=self.request('/workloads/fixture_workload')[1];self.assertEqual(current['status'],'ready');self.assertIsNone(current['operation'])
        self.assertIn('mp0',Proxmox.guests[301]['config']);self.assertNotIn('mp0',Proxmox.guests[300]['config']);self.assertEqual(Proxmox.guests[301]['status'],'running')
        self.assertEqual(self.request('/jobs/'+job_id+'/inspect','POST',{})[1]['actions'],[])
        preview=self.request('/workloads/fixture_workload/plan','POST',{'mode':'rollback','backup_storage':'archive'})[1]
        completed_id=self.request('/workloads/fixture_workload/apply','POST',{'plan_id':preview['plan']['id']})[1]['task_id'];self.assertEqual(self.wait_job(completed_id)['status'],'succeeded')
        self.stop_process()
        for kind in ['job','rollout']:
            value=json.loads(connection.execute('SELECT body FROM hostable_records WHERE kind=? AND id=?',(kind,completed_id)).fetchone()[0])
            if kind=='job':value['status']='running'
            else:value['stage']='HEALTH'
            connection.execute('UPDATE hostable_records SET body=? WHERE kind=? AND id=?',(json.dumps(value),kind,completed_id))
        connection.commit();self.restart_server(self.env)
        self.assertIn('confirm_complete',self.request('/jobs/'+completed_id+'/inspect','POST',{})[1]['actions'])
        self.assertEqual(self.request('/jobs/'+completed_id+'/recover','POST',{'action':'confirm_complete'})[0],200)
        self.assertEqual(self.request('/jobs/'+completed_id)[1]['status'],'succeeded');self.assertEqual(self.request('/workloads/fixture_workload')[1]['active']['vmid'],300)
    def test_recipe_versions_are_immutable_and_invalid_scripts_are_rejected(self):
        recipe={'schema_version':1,'id':'fixture_app','version':1,'title':'Fixture','description':'Test','image':'example/app:1','health':{'port':8080},'update':{'mode':'shell','script':'/opt/app/update --non-interactive'}}
        self.assertEqual(self.request('/recipes','POST',recipe)[0],201);self.assertEqual(self.request('/recipes','POST',recipe)[0],409)
        recipe['version']=2;recipe['update']['script']='';self.assertEqual(self.request('/recipes','POST',recipe)[0],400)
    def test_guest_updates_require_explicit_verified_ssh(self):
        self.seeded_workload(False)
        self.assertEqual(self.request('/workloads/fixture_workload/policy','PUT',{'health':{'port':8080},'update':{'mode':'apt','interval_hours':24}})[0],400)
    def test_new_management_routes_require_authentication(self):
        for path in ['/workloads','/recipes','/monitoring','/recovery']:
            self.assertEqual(self.request(path,auth=False)[0],401,path)
        for path,method,body in [('/manager/updates','GET',None),('/manager/updates/check','POST',{}),('/manager/updates/policy','PUT',{'automatic':False,'check_interval_hours':6}),('/manager/updates/install','POST',{'version':'v1.5.20'})]:
            self.assertEqual(self.request(path,method,body,auth=False)[0],401,path)
    def test_manager_update_policy_is_validated_persistent_and_cannot_replace_source_install(self):
        status,current=self.request('/manager/updates');self.assertEqual(status,200,current)
        self.assertEqual(self.request('/health')[1]['version'],current['current_version'])
        before=self.binary.stat().st_mtime_ns
        self.assertFalse(current['can_install']);self.assertTrue(current['unavailable_reason'])
        try:
            for value in [0,169]:
                self.assertEqual(self.request('/manager/updates/policy','PUT',{'automatic':False,'check_interval_hours':value})[0],400)
            self.assertEqual(self.request('/manager/updates/policy','PUT',{'automatic':True,'check_interval_hours':6})[0],400)
            self.assertEqual(self.request('/manager/updates/policy','PUT',{'automatic':False,'check_interval_hours':12})[0],200)
            self.restart_server(self.env)
            self.assertEqual(self.request('/manager/updates')[1]['policy']['check_interval_hours'],12)
            self.assertEqual(self.request('/manager/updates/install','POST',{'version':'v1.5.20'})[0],400)
            self.assertEqual(self.binary.stat().st_mtime_ns,before)
            self.assertFalse(pathlib.Path(self.temp.name,'data/updates').exists())
        finally:
            self.request('/manager/updates/policy','PUT',current['policy'])
    def test_cli_uses_the_authenticated_manager_and_real_deployment_jobs(self):
        env=self.env.copy();env['HOSTABLE_MANAGER_URL']=self.base+'/api'
        spec=pathlib.Path(self.temp.name,'deployment.json');spec.write_text(json.dumps({'hostname':'cli-existing','node':'beta','image':'nginx:1.28-alpine','storage_pool':'disk','vmid':101}),encoding='utf-8')
        result=subprocess.run([str(self.binary),'deploy','--file',str(spec)],cwd=self.temp.name,env=env,capture_output=True,text=True,timeout=10)
        self.assertEqual(result.returncode,0,result.stderr);payload=json.loads(result.stdout);self.assertIn('task_id',payload)
        job=self.wait_job(payload['task_id']);self.assertEqual(job['status'],'failed');self.assertIn('already allocated',str(job['events']))
        result=subprocess.run([str(self.binary),'job','--id',payload['task_id']],cwd=self.temp.name,env=env,capture_output=True,text=True,timeout=10)
        self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(result.stdout)['status'],'failed');self.assertNotIn(self.token,result.stdout)
        self.seeded_workload(False);preview_dir=pathlib.Path(self.temp.name,'previews');preview_dir.mkdir();preview_dir.chmod(0o755);preview=preview_dir/'update.json'
        result=subprocess.run([str(self.binary),'plan-update','--workload','fixture_workload','--method','rollback','--out',str(preview)],cwd=self.temp.name,env=env,capture_output=True,text=True,timeout=10)
        self.assertEqual(result.returncode,0,result.stderr);plan=json.loads(preview.read_text(encoding='utf-8'));self.assertEqual(plan['plan']['mode'],'rollback');self.assertNotIn('never-return-this',preview.read_text())
        if os.name!='nt':self.assertEqual(preview_dir.stat().st_mode & 0o777,0o755);self.assertEqual(preview.stat().st_mode & 0o777,0o600)
    def fake_guest_transport(self):
        helper=pathlib.Path(self.temp.name,'fake-node-tools');helper.mkdir(exist_ok=True)
        source=helper/'ssh.rs'
        source.write_text(r"""use std::io::Read;
fn main(){let mut script=String::new();std::io::stdin().read_to_string(&mut script).unwrap();
let root=std::env::var("HOSTABLE_FAKE_SSH_DIR").unwrap();std::fs::write(std::path::Path::new(&root).join("last-script"),&script).unwrap();
std::fs::write(std::path::Path::new(&root).join("last-command"),std::env::args().collect::<Vec<_>>().join("\n")).unwrap();
if script.contains("df -Pk"){println!("Filesystem 1024-blocks Used Available Capacity Mounted on\nfixture 100 10 90 10% /");return;}
if script.contains("journalctl"){println!("application log never-return-this");return;}
if std::path::Path::new(&root).join("fail-update").exists(){std::process::exit(1);}println!("fixture update completed");}
""",encoding='utf-8')
        executable=helper/('ssh.exe' if os.name=='nt' else 'ssh')
        built=subprocess.run(['rustc',str(source),'-o',str(executable)],capture_output=True,text=True,timeout=60);self.assertEqual(built.returncode,0,built.stderr)
        for name in ['node-key','known-hosts']: (helper/name).write_text('disposable fixture',encoding='utf-8')
        env=self.env.copy();env.update(PATH=str(helper)+os.pathsep+env['PATH'],HOSTABLE_NODE_SSH_ENDPOINTS=json.dumps({'beta':{'host':'127.0.0.1','user':'root','port':22}}),HOSTABLE_NODE_SSH_KEY=str(helper/'node-key'),HOSTABLE_NODE_KNOWN_HOSTS=str(helper/'known-hosts'),HOSTABLE_FAKE_SSH_DIR=str(helper))
        self.restart_server(env);self.addCleanup(lambda:self.restart_server(self.env));return helper
    def test_shell_update_runs_inside_existing_guest_and_recovers_failed_command(self):
        self.seeded_workload(False);helper=self.fake_guest_transport();Proxmox.task_exit='OK'
        status,_=self.request('/workloads/fixture_workload/policy','PUT',{'health':{},'update':{'mode':'apk','timeout_seconds':30,'interval_hours':0}});self.assertEqual(status,200)
        preview=self.request('/workloads/fixture_workload/plan','POST',{'mode':'recipe'})[1]
        result=self.request('/workloads/fixture_workload/apply','POST',{'plan_id':preview['plan']['id']})[1]
        job=self.wait_job(result['task_id']);self.assertEqual(job['status'],'succeeded',job)
        script=(helper/'last-script').read_text();self.assertIn('apk upgrade',script);self.assertIn('pct exec 301 -- /bin/sh -s',(helper/'last-command').read_text());self.assertEqual(self.request('/workloads/fixture_workload')[1]['active']['vmid'],301)
        self.assertTrue(Proxmox.guests[301]['snapshots'])
        (helper/'fail-update').write_text('fail')
        preview=self.request('/workloads/fixture_workload/plan','POST',{'mode':'recipe'})[1]
        result=self.request('/workloads/fixture_workload/apply','POST',{'plan_id':preview['plan']['id']})[1]
        job=self.wait_job(result['task_id']);self.assertEqual(job['status'],'failed',job)
        self.assertIsNone(self.request('/workloads/fixture_workload')[1]['operation']);self.assertEqual(Proxmox.guests[301]['status'],'running')
        self.assertTrue(any(m=='POST' and '/snapshot/' in p and p.endswith('/rollback') for m,p in Proxmox.calls))
        logs=self.request('/workloads/fixture_workload/logs')[1]['output'];self.assertNotIn('never-return-this',logs);self.assertIn('[redacted]',logs)
    def test_z_restart_marks_running_jobs_interrupted_and_preserves_token(self):
        Proxmox.task_exit='BLOCK'
        status,data=self.request('/ansible/deploy','POST',{'hostname':'restart-test','node':'beta','ostemplate':'local:vztmpl/debian-12-standard_test_amd64.tar.zst','storage_pool':'disk','ip_address':'192.168.50.103/24'})
        self.assertEqual(status,202)
        for _ in range(100):
            if self.request('/jobs/'+data['task_id'])[1]['status']=='running': break
            time.sleep(.02)
        self.stop_process(); Proxmox.task_exit='OK'
        self.__class__.process=subprocess.Popen([str(self.binary),'start'],cwd=self.temp.name,env=self.env,stdout=self.log,stderr=subprocess.STDOUT)
        for _ in range(100):
            try:
                if self.request('/health')[0]==200: break
            except Exception: pass
            time.sleep(.05)
        self.assertEqual(self.request('/verify')[0],200)
        job=self.request('/jobs/'+data['task_id'])[1]
        self.assertEqual(job['status'],'interrupted'); self.assertEqual(job['events'][-1]['step'],'INTERRUPTED')
if __name__ == '__main__': unittest.main()
