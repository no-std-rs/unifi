import importlib.util
import sys
import fcntl
import pty
import select
import signal
import termios
import time
import base64
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import ssl
import subprocess
import tempfile
import threading
import unittest
import yaml
ROOT=Path(__file__).resolve().parents[1]
loader=importlib.util.spec_from_file_location('mock_server',ROOT/'tools/mock_server.py')
mock=importlib.util.module_from_spec(loader);loader.loader.exec_module(mock)
class CliTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.environment_dir=tempfile.TemporaryDirectory(prefix='unifi-test-config-')
        cls.spec=json.loads((ROOT/'schemas/network.json').read_text())
        cls.server=mock.MockServer(('127.0.0.1',0),cls.spec)
        cls.thread=threading.Thread(target=cls.server.serve_forever,daemon=True);cls.thread.start()
        cls.url=f'http://127.0.0.1:{cls.server.server_port}'
    @classmethod
    def tearDownClass(cls):cls.server.shutdown();cls.server.server_close();cls.thread.join();cls.environment_dir.cleanup()
    def cli(self,*args,environment=None,binary=False,cwd=None):
        env={key:value for key,value in os.environ.items() if not key.startswith('UNIFI_')}
        env.update({'UNIFI_API_KEY':'mock-only-test-key','UNIFI_CONFIG':str(Path(self.environment_dir.name)/'config.json'),**(environment or {})})
        return subprocess.run([str(ROOT/'target/debug/unifi'),*args],cwd=cwd or ROOT,text=not binary,capture_output=True,timeout=30,env={key:value for key,value in env.items() if value is not None})
    def test_session_errors_binary_stdout_quiet_and_atomic_credentials(self):
        mode={'error':False,'headers':[]}
        class Handler(BaseHTTPRequestHandler):
            protocol_version='HTTP/1.1'
            def log_message(self,*args):pass
            def reply(self,data):
                self.send_response(200);self.send_header('Content-Length',str(len(data)));self.send_header('Connection','close');self.end_headers();self.wfile.write(data)
            def do_GET(self):
                mode['headers'].append((self.headers.get('Cookie'),self.headers.get('X-CSRF-Token')))
                if self.path.startswith('/proxy/network/dl/autobackup/') or '/vpn/openvpn/' in self.path:self.reply(b'\x1f\x8b\x00\xffmock-binary');return
                if mode['error']:self.reply(b'{"meta":{"rc":"error","msg":"mock failure"}}')
                else:self.reply(b'{"meta":{"rc":"ok"},"data":[]}')
            def do_POST(self):
                self.rfile.read(int(self.headers.get('Content-Length','0')))
                self.reply(b'{"meta":{"rc":"error"}}' if mode['error'] else b'{"meta":{"rc":"ok"}}')
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                cache=Path(tmp)/'session.json';url=f'http://127.0.0.1:{server.server_port}'
                cache.write_text(json.dumps({'origin':url,'cookie':'TOKEN=cached','csrf':'cached-csrf'}))
                args=['--url',url,'--cookie-file',str(cache)]
                result=self.cli(*args,'wifi','neighbors',environment={'UNIFI_COOKIE':'TOKEN=explicit'})
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual(mode['headers'][-1],('TOKEN=explicit',None))
                mode['error']=True
                result=self.cli(*args,'wifi','neighbors','--quiet')
                self.assertEqual(result.returncode,1,result.stderr)
                self.assertEqual(result.stdout,'')
                result=self.cli(*args,'session','logout','--allow-write','--quiet')
                self.assertEqual(result.returncode,1,result.stderr)
                self.assertTrue(cache.exists())
                mode['error']=False
                for command in [('system','backup','download','mock.unf'),('vpn','remote-access','download-config','mock-server')]:
                    result=self.cli(*args,*command,binary=True)
                    self.assertEqual(result.returncode,0,result.stderr)
                    self.assertEqual(result.stdout,b'\x1f\x8b\x00\xffmock-binary')
                    result=self.cli(*args,*command,'--quiet',binary=True)
                    self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(result.stdout,b'')
        finally:server.shutdown();server.server_close();thread.join()
    def test_policy_after_system_preserves_rules_and_reports_partial_failure(self):
        site='00000000-0000-4000-8000-000000000001';source='00000000-0000-4000-8000-000000000002';destination='00000000-0000-4000-8000-000000000003';created='00000000-0000-4000-8000-000000000004';before='00000000-0000-4000-8000-000000000005';after='00000000-0000-4000-8000-000000000006'
        state={'writes':[],'fail':False}
        class Handler(BaseHTTPRequestHandler):
            protocol_version='HTTP/1.1'
            def log_message(self,*args):pass
            def reply(self,status,value):
                body=b'' if status==204 else json.dumps(value).encode();self.send_response(status);self.send_header('Content-Length',str(len(body)));self.send_header('Connection','close');self.end_headers();self.wfile.write(body)
            def do_GET(self):self.reply(500,{'error':'fixture'}) if state['fail'] else self.reply(200,{'orderedFirewallPolicyIds':{'beforeSystemDefined':[created,before],'afterSystemDefined':[after]}})
            def do_POST(self):
                body=json.loads(self.rfile.read(int(self.headers['Content-Length'])));state['writes'].append((self.command,body));self.reply(200,{**body,'id':created})
            def do_PUT(self):
                body=json.loads(self.rfile.read(int(self.headers['Content-Length'])));state['writes'].append((self.command,body));self.reply(204,None)
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            args=['firewall','policies','create','--name','fixture after-system','--action','block','--source-zone',source,'--dest-zone',destination,'--after-system','--site',site,'--url',f'http://127.0.0.1:{server.server_port}','--allow-write','--output','json']
            result=self.cli(*args)
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(result.stdout)['id'],created)
            self.assertEqual(state['writes'][-1],('PUT',{'orderedFirewallPolicyIds':{'beforeSystemDefined':[before],'afterSystemDefined':[after,created]}}))
            count=len(state['writes']);state['fail']=True;result=self.cli(*args)
            self.assertEqual(result.returncode,2,result.stderr);self.assertIn('policy was created',result.stderr);self.assertEqual(len(state['writes']),count+1)
        finally:server.shutdown();server.server_close();thread.join()
    def test_explicit_connection_without_profile_directory_and_cloud_queries(self):
        with tempfile.TemporaryDirectory() as tmp:
            env={'HOME':None,'XDG_CONFIG_HOME':None,'UNIFI_CONFIG':None}
            result=self.cli('system','info','--url',self.url,'--api-key','mock-only-test-key',environment=env,cwd=tmp)
            self.assertEqual(result.returncode,0,result.stderr)
            result=self.cli('config','init',environment=env,cwd=tmp)
            self.assertEqual(result.returncode,2);self.assertIn('saving configuration requires',result.stderr);self.assertEqual(list(Path(tmp).iterdir()),[])
        spec=json.loads((ROOT/'schemas/site-manager.json').read_text())
        fixtures={('GET','/v1/hosts?pageSize=1'):{'data':[{'id':'host-one'}],'nextToken':'second'},('GET','/v1/hosts?pageSize=1&nextToken=second'):{'data':[{'id':'host-two'}]},('GET','/v1/sites'):{'data':[{'hostId':'host-one','siteId':'site-one','meta':{'name':'fixture site'}}]}}
        server=mock.MockServer(('127.0.0.1',0),spec,fixtures);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            url=f'http://127.0.0.1:{server.server_port}'
            result=self.cli('cloud','hosts','--url',url,'--all','--limit','1','--output','json')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual([v['id'] for v in json.loads(result.stdout)],['host-one','host-two']);self.assertIn(('GET','/v1/hosts?pageSize=1&nextToken=second'),server.raw_requests)
            result=self.cli('cloud','isp','query','--url',url,'--sites','site-one,host-two/site-two','--allow-write')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(server.request_bodies[-1][2]),{'sites':[{'hostId':'host-one','siteId':'site-one'},{'hostId':'host-two','siteId':'site-two'}]})
            result=self.cli('cloud','isp','query','--url',url,'--sites','[{"hostId":"host-one","siteId":"site-one","beginTimestamp":"2026-10-01T00:00:00Z"}]','--allow-write')
            self.assertEqual(result.returncode,0,result.stderr);self.assertIn('beginTimestamp',json.loads(server.request_bodies[-1][2])['sites'][0])
            before=len(server.request_bodies);result=self.cli('cloud','isp','query','--url',url,'--sites','["site-one"]','--allow-write')
            self.assertEqual(result.returncode,2);self.assertEqual(len(server.request_bodies),before)
        finally:server.shutdown();server.server_close();thread.join()
    def test_firewall_acl_zone_helpers_and_preserved_policy_order(self):
        site='00000000-0000-4000-8000-000000000001';source='00000000-0000-4000-8000-000000000002';destination='00000000-0000-4000-8000-000000000003';network='00000000-0000-4000-8000-000000000004';policy='00000000-0000-4000-8000-000000000005';other='00000000-0000-4000-8000-000000000006'
        args=['--url',self.url,'--site',site,'--allow-write','--output','json'];base=f'/v1/sites/{site}'
        def path_for(operation):return next(path for path,methods in self.spec['paths'].items() if any(o.get('operationId')==operation for o in methods.values())).replace('{siteId}',site)
        ordering=path_for('getFirewallPolicyOrdering');query=f'?sourceFirewallZoneId={source}&destinationFirewallZoneId={destination}'
        fixtures={('GET',base+'/firewall/zones/'+source):{'id':source,'name':'source','networkIds':[network]},('GET',base+'/firewall/zones/'+destination):{'id':destination,'name':'destination','networkIds':[network]},('GET',ordering+query):{'orderedFirewallPolicyIds':{'beforeSystemDefined':[other],'afterSystemDefined':[policy]}},('GET',base+'/firewall/policies/'+policy):{'id':policy,'name':'old','enabled':True,'loggingEnabled':False,'action':{'type':'ALLOW','allowReturnTraffic':True},'source':{'zoneId':source},'destination':{'zoneId':destination},'ipProtocolScope':{'ipVersion':'IPV4_AND_IPV6'},'vendor_field':7}}
        self.server.fixtures.update(fixtures)
        try:
            result=self.cli(*args,'firewall','policies','create','--name','fixture policy','--action','allow','--source-zone',source,'--dest-zone',destination,'--src-ip','192.0.2.0/24','--dst-port','443,8000-8010','--logging','--allow-return-traffic','false','--ip-version','IPV4_ONLY','--states','NEW,ESTABLISHED')
            self.assertEqual(result.returncode,0,result.stderr);body=json.loads(self.server.request_bodies[-1][2]);self.assertEqual(body['action'],{'type':'ALLOW','allowReturnTraffic':False});self.assertEqual(body['source']['trafficFilter']['ipAddressFilter']['items'],[{'type':'SUBNET','value':'192.0.2.0/24'}]);self.assertEqual(body['ipProtocolScope'],{'ipVersion':'IPV4'});self.assertTrue(body['loggingEnabled'])
            result=self.cli(*args,'firewall','policies','update',policy,'--dst-port','8443')
            self.assertEqual(result.returncode,0,result.stderr);body=json.loads(self.server.request_bodies[-1][2]);self.assertEqual(body['vendor_field'],7);self.assertEqual(body['destination']['trafficFilter']['portFilter']['items'],[{'type':'PORT_NUMBER','value':8443}]);self.assertEqual(body['source'],{'zoneId':source})
            result=self.cli(*args,'firewall','policies','patch',policy,'--enabled','false','--logging','false')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(self.server.request_bodies[-1][2]),{'enabled':False,'loggingEnabled':False})
            result=self.cli(*args,'firewall','zones','create','--name','fixture zone','--networks',network)
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(self.server.request_bodies[-1][2]),{'networkIds':[network],'name':'fixture zone'})
            result=self.cli(*args,'acl','create','--name','fixture acl','--rule-type','ipv4','--action','block','--source-zone',source,'--dest-zone',destination,'--protocol','tcp,udp','--source-port','1000-1002','--destination-port','443')
            self.assertEqual(result.returncode,0,result.stderr);body=json.loads(self.server.request_bodies[-1][2]);self.assertEqual(body['type'],'IPV4');self.assertEqual(body['sourceFilter'],{'type':'NETWORKS','networkIds':[network],'portFilter':[1000,1001,1002]});self.assertEqual(body['protocolFilter'],['TCP','UDP'])
            result=self.cli(*args,'firewall','policies','reorder','--set',policy,'--source-zone',source,'--dest-zone',destination)
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(self.server.request_bodies[-1][2]),{'orderedFirewallPolicyIds':{'beforeSystemDefined':[policy],'afterSystemDefined':[]}})
            before=len(self.server.requests);result=self.cli(*args,'firewall','policies','reorder','--set',policy+','+policy,'--source-zone',source,'--dest-zone',destination)
            self.assertEqual(result.returncode,2);self.assertEqual(len(self.server.requests),before)
            zones=('GET',base+'/firewall/zones');self.server.fixtures[zones]={'data':[{'id':source,'name':'source'},{'id':destination,'name':'destination'}],'count':2,'totalCount':2}
            try:
                result=self.cli(*args,'firewall','policies','reorder','--get','--source-zone','source','--dest-zone','destination')
                self.assertEqual(result.returncode,0,result.stderr);self.assertTrue(any(method=='GET' and path.endswith(ordering+query) for method,path in self.server.raw_requests))
            finally:del self.server.fixtures[zones]
        finally:
            for key in fixtures:del self.server.fixtures[key]
    def test_legacy_nat_and_firewall_group_convenience_payloads(self):
        interface_id='0123456789abcdef01234567';external='00000000-0000-4000-8000-000000000002'
        state={'writes':[],'rules':[{'id':'rule-id','rule_index':7,'type':'DNAT','description':'old','enabled':True,'in_interface':interface_id,'source_filter':{'filter_type':'ADDRESS_AND_PORT','address':'192.0.2.0/24','port':'80','invert_address':True},'vendor_field':7}], 'groups':[{'_id':'group-id','name':'old group','group_type':'port-group','group_members':['80'],'vendor_field':9}]}
        class Handler(BaseHTTPRequestHandler):
            protocol_version='HTTP/1.1'
            def log_message(self,*args):pass
            def reply(self,data):
                body=json.dumps(data).encode();self.send_response(200);self.send_header('Content-Length',str(len(body)));self.send_header('Connection','close');self.end_headers();self.wfile.write(body)
            def do_GET(self):
                if self.path.endswith('/nat'):data=state['rules']
                elif self.path.endswith('/rest/firewallgroup'):data={'meta':{'rc':'ok'},'data':state['groups']}
                else:data={'meta':{'rc':'ok'},'data':[{'_id':interface_id,'external_id':external,'name':'fixture network'}]}
                self.reply(data)
            def do_POST(self):
                body=json.loads(self.rfile.read(int(self.headers.get('Content-Length','0'))));state['writes'].append((self.command,self.path,body));self.reply(body)
            do_PUT=do_POST
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            args=['--url',f'http://127.0.0.1:{server.server_port}','--allow-write','--output','json']
            result=self.cli(*args,'nat','policies','create','--name','fixture nat','--type','dnat','--interface-id',interface_id,'--protocol','tcp','--src-address','192.0.2.0/24','--dst-port','80','--translated-address','192.0.2.4','--translated-port','8080')
            self.assertEqual(result.returncode,0,result.stderr);body=state['writes'][-1][2];self.assertEqual(body['rule_index'],8);self.assertEqual(body['description'],'fixture nat');self.assertEqual(body['in_interface'],interface_id);self.assertEqual(body['ip_address'],'192.0.2.4');self.assertEqual(body['destination_filter']['port'],'80')
            result=self.cli(*args,'nat','policies','update','rule-id','--name','updated','--type','snat','--interface-id',external,'--src-port','443')
            self.assertEqual(result.returncode,0,result.stderr);body=state['writes'][-1][2];self.assertEqual(body['out_interface'],interface_id);self.assertNotIn('in_interface',body);self.assertEqual(body['vendor_field'],7);self.assertEqual(body['source_filter']['address'],'192.0.2.0/24');self.assertEqual(body['source_filter']['port'],'443');self.assertTrue(body['source_filter']['invert_address'])
            result=self.cli(*args,'firewall','groups','create','--name','fixture group','--type','address-group','--members','192.0.2.1,192.0.2.2')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(state['writes'][-1][2],{'group_type':'address-group','group_members':['192.0.2.1','192.0.2.2'],'name':'fixture group'})
            result=self.cli(*args,'firewall','groups','update','group-id','--members','443')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(state['writes'][-1][2]['vendor_field'],9);self.assertEqual(state['writes'][-1][2]['group_members'],['443'])
            result=self.cli(*args,'sites','create','--name','fixture-site','--description','fixture description')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(state['writes'][-1][2],{'cmd':'add-site','name':'fixture-site','desc':'fixture description'})
        finally:server.shutdown();server.server_close();thread.join()
    def test_dotenv_profile_selection_precedes_controller_fallbacks(self):
        with tempfile.TemporaryDirectory() as directory:
            directory=Path(directory);config=directory/'profiles.json'
            config.write_text(json.dumps({'activeProfile':'default','profiles':{'default':{'controller':'http://127.0.0.1:1','apiKey':'mock-default'},'home':{'controller':self.url,'apiKey':'mock-home'}}}))
            (directory/'.env').write_text(f'UNIFI_CONFIG={config}\nUNIFI_PROFILE=home\n')
            env={'UNIFI_CONFIG':None,'UNIFI_PROFILE':None,'UNIFI_API_KEY':None}
            result=self.cli('system','info',environment=env,cwd=directory);self.assertEqual(result.returncode,0,result.stderr)
            result=self.cli('system','info',environment={**env,'UNIFI_PROFILE':'typo'},cwd=directory)
            self.assertEqual(result.returncode,2,result.stderr);self.assertIn('does not exist',result.stderr)
            result=self.cli('system','info','--profile','home',environment={**env,'UNIFI_PROFILE':'typo'},cwd=directory);self.assertEqual(result.returncode,0,result.stderr)

    def test_type_conversions_clear_stale_fields_and_delete_force_is_forwarded(self):
        site='00000000-0000-4000-8000-000000000001';resource='00000000-0000-4000-8000-000000000002';zone='00000000-0000-4000-8000-000000000003';network='00000000-0000-4000-8000-000000000004'
        args=['--url',self.url,'--site',site,'--allow-write','--output','json'];base=f'/v1/sites/{site}'
        result=self.cli(*args,'wifi','create','--name','fixture wifi','--security','wpa2-personal','--passphrase','fixture password','--fast-roaming','--band-steering')
        self.assertEqual(result.returncode,0,result.stderr);wifi=json.loads(self.server.request_bodies[-1][2]);self.assertTrue(wifi['securityConfiguration']['fastRoamingEnabled'])
        keys=[('GET',base+'/wifi/broadcasts/'+resource),('GET',base+'/acl-rules/'+resource),('GET',base+'/firewall/zones/'+zone)]
        self.server.fixtures.update({keys[0]:{'id':resource,**wifi},keys[1]:{'id':resource,'type':'IPV4','name':'fixture acl','action':'BLOCK','enabled':True,'protocolFilter':['TCP'],'sourceFilter':{'type':'NETWORKS','networkIds':[network]},'destinationFilter':{'type':'PORTS','portFilter':[443]}},keys[2]:{'id':zone,'networkIds':[network]}})
        try:
            for extra in [[],['--fast-roaming','false']]:
                result=self.cli(*args,'wifi','update',resource,'--broadcast-type','iot-optimized',*extra)
                self.assertEqual(result.returncode,0,result.stderr);body=json.loads(self.server.request_bodies[-1][2]);self.assertNotIn('fastRoamingEnabled',body['securityConfiguration']);self.assertNotIn('bandSteeringEnabled',body);self.assertEqual(body['securityConfiguration']['passphrase'],'fixture password')
            self.server.fixtures[keys[0]]={'id':resource,**body}
            result=self.cli(*args,'wifi','update',resource,'--broadcast-type','standard')
            self.assertEqual(result.returncode,0,result.stderr);body=json.loads(self.server.request_bodies[-1][2]);self.assertEqual(body['type'],'STANDARD');self.assertEqual(body['broadcastingFrequenciesGHz'],[2.4,5]);self.assertEqual(body['securityConfiguration']['passphrase'],'fixture password')
            result=self.cli(*args,'acl','update',resource,'--rule-type','mac','--source-zone',zone)
            self.assertEqual(result.returncode,0,result.stderr);body=json.loads(self.server.request_bodies[-1][2]);self.assertEqual(body['type'],'MAC');self.assertEqual(body['networkIdFilter'],network)
            for key in ['sourceFilter','destinationFilter','protocolFilter']:self.assertNotIn(key,body)
            for command,value in [('networks',True),('wifi',False)]:
                result=self.cli(*args,command,'delete',resource,'--force',str(value).lower())
                self.assertEqual(result.returncode,0,result.stderr);self.assertTrue(self.server.raw_requests[-1][1].endswith('?force='+str(value).lower()))
        finally:
            for key in keys:del self.server.fixtures[key]

    def test_schema_checked_convenience_builders_and_preserving_updates(self):
        site='00000000-0000-4000-8000-000000000001';network='00000000-0000-4000-8000-000000000002'
        args=['--url',self.url,'--site',site,'--allow-write','--output','json']
        result=self.cli(*args,'wifi','create','--name','fixture wifi','--security','wpa3-personal','--passphrase','fixture password','--network',network,'--frequencies','2.4,5,6','--hidden','--band-steering','--fast-roaming')
        self.assertEqual(result.returncode,0,result.stderr)
        body=json.loads(self.server.request_bodies[-1][2]);self.assertEqual(body['network'],{'type':'SPECIFIC','networkId':network});self.assertEqual(body['securityConfiguration']['passphrase'],'fixture password');self.assertEqual(body['broadcastingFrequenciesGHz'],[2.4,5,6]);self.assertTrue(body['hideName']);self.assertTrue(body['bandSteeringEnabled'])
        result=self.cli(*args,'wifi','create','--name','fixture iot','--security','open','--broadcast-type','iot-optimized')
        self.assertEqual(result.returncode,0,result.stderr);self.assertNotIn('broadcastingFrequenciesGHz',json.loads(self.server.request_bodies[-1][2]))
        before=len(self.server.request_bodies)
        result=self.cli(*args,'wifi','create','--name','invalid iot','--broadcast-type','iot-optimized','--band-steering')
        self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(len(self.server.request_bodies),before)
        result=self.cli(*args,'networks','create','--name','fixture vlan','--vlan','25','--ipv4-host','192.0.2.1/24','--dhcp','--dhcp-start','192.0.2.25','--dhcp-stop','192.0.2.100','--dns','203.0.113.53','--isolated','--internet','false')
        self.assertEqual(result.returncode,0,result.stderr);body=json.loads(self.server.request_bodies[-1][2]);self.assertEqual(body['management'],'GATEWAY');self.assertEqual(body['vlanId'],25);self.assertFalse(body['internetAccessEnabled']);self.assertEqual(body['ipv4Configuration']['dhcpConfiguration']['ipAddressRange'],{'start':'192.0.2.25','stop':'192.0.2.100'})
        result=self.cli(*args,'dns','create','--record-type','a','--domain','fixture.example','--value','192.0.2.4','--ttl','300')
        self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(self.server.request_bodies[-1][2]),{'enabled':True,'type':'A_RECORD','domain':'fixture.example','ipv4Address':'192.0.2.4','ttlSeconds':300})
        result=self.cli(*args,'traffic-lists','create','--name','fixture ports','--list-type','ports','--items','80,443,1000-1005')
        self.assertEqual(result.returncode,0,result.stderr);body=json.loads(self.server.request_bodies[-1][2]);self.assertEqual(body['type'],'PORTS');self.assertEqual(body['items'][-1],{'type':'PORT_NUMBER_RANGE','start':1000,'stop':1005})
        result=self.cli(*args,'traffic-lists','create','--name','fixture v6','--list-type','ipv6-addresses','--items','2001:db8::1,2001:db8::/64')
        self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(len(json.loads(self.server.request_bodies[-1][2])['items']),2)
        count=len(self.server.request_bodies)
        for value in ['2001:db8::1-2001:db8::2','2001:db8::2-2001:db8::1','192.0.2.1-192.0.2.2','192.0.2.1-2001:db8::1']:
            result=self.cli(*args,'traffic-lists','create','--name','invalid','--list-type','ipv6-addresses','--items',value)
            self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(len(self.server.request_bodies),count)
        self.server.fixtures[('GET',f'/v1/sites/{site}/networks/{network}') ]={'id':network,'name':'old','enabled':True,'management':'UNMANAGED','vlanId':25,'vendor_field':{'keep':True}}
        try:
            result=self.cli(*args,'networks','update',network,'--name','updated')
            self.assertEqual(result.returncode,0,result.stderr);body=json.loads(self.server.request_bodies[-1][2]);self.assertEqual(body['vendor_field'],{'keep':True});self.assertEqual(body['vlanId'],25);self.assertEqual(body['name'],'updated');self.assertNotIn('id',body)
        finally:del self.server.fixtures[('GET',f'/v1/sites/{site}/networks/{network}')]
    def test_legacy_local_filters_and_read_ordering(self):
        site='00000000-0000-4000-8000-000000000001'
        path='/proxy/network/api/s/default/rest/user'
        self.server.fixtures[('GET',path)]={'meta':{'rc':'ok'},'data':[{'_id':key,'use_fixedip':fixed,'tags':[{'name':'ALPHA'}]} for key,fixed in [('first',True),('second',True),('third',False)]]}
        try:
            result=self.cli('clients','reservations','--url',self.url,'--filter',"tags.name.eq('alpha')",'--offset','1','--limit','1','--output','json')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual([v['_id'] for v in json.loads(result.stdout)],['second'])
            result=self.cli('acl','reorder','--get','--site',site,'--url',self.url,'--output','json')
            self.assertEqual(result.returncode,0,result.stderr);self.assertIsInstance(json.loads(result.stdout),dict)
        finally:del self.server.fixtures[('GET',path)]
    def test_malformed_profile_containers_and_conflicting_body_sources(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'config.json';before=len(self.server.requests)
            for value in [[],False,42,'broken',None]:
                for config in [{'profiles':value},{'profiles':{'default':value}}]:
                    path.write_text(json.dumps(config))
                    result=self.cli('system','info','--url',self.url,environment={'UNIFI_CONFIG':str(path)})
                    self.assertEqual(result.returncode,2,result.stderr);self.assertIn('must be an object',result.stderr)
            self.assertEqual(len(self.server.requests),before)
            path.write_text('{"profiles":{"fixture":{}}}')
            result=self.cli('system','info','--profile','typo','--url',self.url,environment={'UNIFI_CONFIG':str(path)})
            self.assertEqual(result.returncode,2);self.assertIn('does not exist',result.stderr);self.assertEqual(len(self.server.requests),before)
            path.write_text('{"action":"RESTART"}')
            result=self.cli('call','network','executeAdoptedDeviceAction','--param','siteId=test','--param','deviceId=test','--url',self.url,'--body',str(path),'--data','{"action":"RESTART"}','--allow-write')
            self.assertEqual(result.returncode,2);self.assertIn('mutually exclusive',result.stderr);self.assertEqual(len(self.server.requests),before)
    def test_keyring_profile_references_and_exact_secret_transport(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp=Path(tmp);tool=tmp/'secret-tool';database=tmp/'secrets.json';config=tmp/'config.json'
            tool.write_text("""#!/usr/bin/python3
import base64,json,os,sys
from pathlib import Path
path=Path(os.environ['UNIFI_TEST_KEYRING_DB'])
values=json.loads(path.read_text()) if path.exists() else {}
name=sys.argv[sys.argv.index('username')+1]
assert sys.argv[sys.argv.index('service')+1]=='unifly'
if sys.argv[1]=='store':
    values[name]=base64.b64encode(sys.stdin.buffer.read()).decode()
    path.write_text(json.dumps(values));path.chmod(0o600)
elif name in values:sys.stdout.buffer.write(base64.b64decode(values[name]))
else:sys.exit(1)
""")
            tool.chmod(0o700)
            env={'UNIFI_SECRET_TOOL':str(tool),'UNIFI_TEST_KEYRING_DB':str(database),'UNIFI_CONFIG':str(config)}
            result=self.cli('config','init','--profile','fixture','--controller',self.url,environment=env,cwd=tmp)
            self.assertEqual(result.returncode,0,result.stderr)
            password='  fixture \"secret\"\nline 😀  \n'
            result=self.cli('config','set-password','--profile','fixture','--secret-kind','both',environment={**env,'UNIFI_PASSWORD':password},cwd=tmp)
            self.assertEqual(result.returncode,0,result.stderr)
            profile=json.loads(config.read_text())['profiles']['fixture']
            self.assertEqual(profile['apiKey'],'keyring:fixture/api-key');self.assertEqual(profile['password'],'keyring:fixture/password')
            self.assertNotIn(password,config.read_text());self.assertNotIn(password,result.stdout+result.stderr)
            self.assertEqual(base64.b64decode(json.loads(database.read_text())['fixture/password']).decode(),password)
            result=self.cli('system','info','--profile','fixture',environment={**env,'UNIFI_API_KEY':None},cwd=tmp)
            self.assertEqual(result.returncode,0,result.stderr)
            requests=[]
            class Login(BaseHTTPRequestHandler):
                def log_message(self,*args):pass
                def do_POST(self):
                    requests.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
                    self.send_response(200);self.send_header('Set-Cookie','TOKEN=fixture-session; Path=/');self.send_header('Content-Length','2');self.end_headers();self.wfile.write(b'{}')
            server=ThreadingHTTPServer(('127.0.0.1',0),Login);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
            try:
                for key,value in [('username','fixture-user'),('controller',f'http://127.0.0.1:{server.server_port}')]:
                    result=self.cli('config','set','profiles.fixture.'+key,value,environment=env,cwd=tmp);self.assertEqual(result.returncode,0,result.stderr)
                values=json.loads(database.read_text());del values['fixture/api-key'];database.write_text(json.dumps(values))
                result=self.cli('session','login','--profile','fixture','--cookie-file',str(tmp/'session.json'),'--allow-write',environment={**env,'UNIFI_API_KEY':None,'UNIFI_PASSWORD':None},cwd=tmp)
                self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(requests[0]['password'],password)
            finally:server.shutdown();server.server_close();thread.join()
            result=self.cli('config','show',environment=env,cwd=tmp)
            self.assertNotIn(password,result.stdout)
    def test_tui_command_palette_and_terminal_restoration(self):
        for interrupted in [False,True]:
            master,slave=pty.openpty();original=termios.tcgetattr(slave)
            config=Path(self.environment_dir.name)/'tui.json';config.write_text(json.dumps({'activeProfile':'fixture','profiles':{'fixture':{'controller':self.url,'apiKeyEnv':'FIXTURE_TUI_KEY'}}}))
            env={key:value for key,value in os.environ.items() if not key.startswith('UNIFI_')};env.update({'FIXTURE_TUI_KEY':'mock-only-test-key','UNIFI_CONFIG':str(config)})
            before=len(self.server.requests)
            launch='import fcntl,os,sys,termios;fcntl.ioctl(0,termios.TIOCSCTTY,0);os.execv(sys.argv[1],sys.argv[1:])'
            child=subprocess.Popen([sys.executable,'-c',launch,str(ROOT/'target/debug/unifi'),'tui','--site','00000000-0000-4000-8000-000000000001','--url',self.url,'--count','2','--color','never'],stdin=slave,stdout=slave,stderr=slave,start_new_session=True,cwd=ROOT,env=env)
            pid=child.pid
            os.close(slave);output=b'';sent=False;done=0;deadline=time.monotonic()+15
            try:
                while time.monotonic()<deadline:
                    if select.select([master],[],[],0.2)[0]:
                        try:chunk=os.read(master,65536)
                        except OSError:break
                        if not chunk:break
                        output+=chunk
                        if b'UniFi' in output and not sent:
                            if interrupted:os.kill(pid,signal.SIGTERM)
                            else:os.write(master,b':system info\r')
                            sent=True
                    done=child.poll() is not None
                    if done:break
                else:self.fail('TUI failed to finish')
                status=child.wait(timeout=2);done=True
                self.assertEqual(status,143 if interrupted else 0,output.decode(errors='replace'))
                self.assertIn(b'\x1b[?1049l',output)
                self.assertEqual(termios.tcgetattr(master),original)
                self.assertGreater(len(self.server.requests),before);self.assertNotIn(b'API_KEY is missing',output)
                if not interrupted:self.assertIn(b'Command',output)
            finally:
                if child.poll() is None:child.kill()
                child.wait(timeout=2)
                os.close(master)

    def test_guided_local_setup_credentials_and_terminal_restoration(self):
        launch='import fcntl,os,sys,termios;fcntl.ioctl(0,termios.TIOCSCTTY,0);os.execv(sys.argv[1],sys.argv[1:])'
        for mode in ['env','session','cancel','interrupt']:
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as tmp:
                directory=Path(tmp);config=directory/'profiles.json'
                initial={'activeProfile':'old','profiles':{'old':{'controller':self.url}}};config.write_text(json.dumps(initial))
                steps=[(b'Profile name [',b'guided\r'),(b'Controller URL [',self.url.encode()+b'\r')]
                if mode=='session':
                    steps += [(b'Authentication (',b'session\r'),(b'Username [',b'fixture-user\r'),(b'Password: ',b'fixture-hidden-password\r'),(b'Password storage (',b'file\r')]
                else:steps += [(b'Authentication (',b'\r'),(b'API key storage (',b'\r')]
                steps += [(b'Verify TLS certificates (',b'\r'),(b'Site [',b'fixture-site\r'),(b'Save profile (',b'no\r' if mode=='cancel' else b'\r')]
                if mode=='interrupt':steps=[(b'Profile name [',b'\x03')]
                master,slave=pty.openpty();original=termios.tcgetattr(slave)
                env={key:value for key,value in os.environ.items() if not key.startswith('UNIFI_')};env.update({'UNIFI_API_KEY':'mock-only-test-key','UNIFI_CONFIG':str(config)})
                child=subprocess.Popen([sys.executable,'-c',launch,str(ROOT/'target/debug/unifi'),'config','init'],stdin=slave,stdout=slave,stderr=slave,start_new_session=True,cwd=directory,env=env)
                os.close(slave);output=b'';cursor=0;next_step=0;deadline=time.monotonic()+15
                try:
                    while time.monotonic()<deadline:
                        if select.select([master],[],[],0.1)[0]:
                            try:chunk=os.read(master,65536)
                            except OSError:break
                            if not chunk:break
                            output+=chunk
                        if next_step<len(steps):
                            prompt,answer=steps[next_step];position=output.find(prompt,cursor)
                            if position>=0:
                                os.write(master,answer);cursor=position+len(prompt);next_step+=1
                        if child.poll() is not None:break
                    status=child.wait(timeout=2)
                    self.assertEqual(next_step,len(steps),output.decode(errors='replace'))
                    self.assertEqual(status,130 if mode=='interrupt' else 2 if mode=='cancel' else 0,output.decode(errors='replace'))
                    self.assertEqual(termios.tcgetattr(master),original)
                    self.assertNotIn(b'fixture-hidden-password',output);self.assertNotIn(b'mock-only-test-key',output)
                    saved=json.loads(config.read_text())
                    if mode in ['cancel','interrupt']:self.assertEqual(saved,initial)
                    else:
                        self.assertEqual(saved['profiles']['old'],initial['profiles']['old']);self.assertEqual(saved['activeProfile'],'guided')
                        profile=saved['profiles']['guided'];self.assertEqual(profile['site'],'fixture-site')
                        if mode=='session':self.assertEqual(profile['password'],'fixture-hidden-password');self.assertEqual(profile['authMode'],'session');self.assertNotIn('apiKey',profile)
                        else:self.assertEqual(profile['apiKeyEnv'],'UNIFI_API_KEY');self.assertNotIn('mock-only-test-key',config.read_text())
                        self.assertEqual(config.stat().st_mode & 0o777,0o600)
                finally:
                    if child.poll() is None:child.kill()
                    child.wait(timeout=2);os.close(master)

    def test_cloud_setup_validates_discovery_and_custom_environment_credentials(self):
        state={'requests':[],'sites':[{'id':'00000000-0000-4000-8000-000000000001','name':'Home','internalReference':'fixture-site'}]}
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_GET(self):
                state['requests'].append((self.command,self.path,self.headers.get('X-API-Key')))
                if self.path=='/v1/hosts':value={'data':[{'id':'fixture/console','type':'console','reportedState':{'name':'Console','state':'connected'}}]}
                elif '/integration/v1/sites?' in self.path:value={'data':state['sites'],'count':len(state['sites']),'totalCount':len(state['sites'])}
                else:value={'meta':{'rc':'ok'},'data':[{'subsystem':'wlan','status':'ok'}]}
                data=json.dumps(value).encode();self.send_response(200);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                directory=Path(tmp);config=directory/'config.json';env={'UNIFI_CONFIG':str(config)}
                args=['config','cloud-setup','--profile','cloud','--controller',f'http://127.0.0.1:{server.server_port}']
                result=self.cli(*args,environment=env,cwd=directory);self.assertEqual(result.returncode,0,result.stderr)
                saved=json.loads(config.read_text());self.assertEqual(saved['activeProfile'],'cloud');profile=saved['profiles']['cloud']
                self.assertEqual(profile['authMode'],'cloud');self.assertEqual(profile['console'],'fixture/console');self.assertEqual(profile['site'],'fixture-site');self.assertEqual(profile['apiKeyEnv'],'UNIFI_API_KEY');self.assertNotIn('mock-only-test-key',config.read_text())
                profile['apiKeyEnv']='FIXTURE_API_TOKEN';config.write_text(json.dumps(saved))
                result=self.cli('system','health','--output','json',environment={**env,'UNIFI_API_KEY':None,'FIXTURE_API_TOKEN':'mock-custom-key'},cwd=directory)
                self.assertEqual(result.returncode,0,result.stderr);self.assertTrue(state['requests'][-1][1].endswith('/fixture%2Fconsole/proxy/network/api/s/fixture-site/stat/health'));self.assertEqual(state['requests'][-1][2],'mock-custom-key')
                before=config.read_text();result=self.cli(*args,environment=env,cwd=directory);self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(config.read_text(),before)
                state['sites'].append({'id':'00000000-0000-4000-8000-000000000002','name':'HOME','internalReference':'upper'})
                for selector in ['Unknown','home']:
                    result=self.cli(*args,'--profile','other','--site',selector,environment=env,cwd=directory);self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(config.read_text(),before)
                result=self.cli(*args,'--profile','other','--site','HOME',environment=env,cwd=directory);self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(config.read_text())['profiles']['other']['site'],'upper')
                self.assertTrue(all(method=='GET' for method,_,_ in state['requests']))
        finally:server.shutdown();server.server_close();thread.join()
    def test_port_merge_export_jsonc_and_reservations(self):
        device={'_id':'device-id','mac':'aa:bb:cc:dd:ee:ff','name':'fixture switch','port_table':[{'port_idx':1,'name':'first'},{'port_idx':2,'name':'second'},{'port_idx':3}], 'port_overrides':[{'port_idx':1,'name':'old','speed':'1000','autoneg':False,'vendor_field':7},{'port_idx':2,'name':'untouched','poe_mode':'off'}]}
        networks=[{'_id':'network-id','name':'fixture net','ip_subnet':'192.0.2.0/24'}]
        state={'writes':[],'users':[{'_id':'user-id','mac':'aa:bb:cc:dd:ee:01','network_id':'network-id','name':'keep name'}]}
        class Handler(BaseHTTPRequestHandler):
            protocol_version='HTTP/1.1'
            def log_message(self,*args):pass
            def reply(self,data):
                body=json.dumps({'meta':{'rc':'ok'},'data':data}).encode();self.send_response(200);self.send_header('Content-Length',str(len(body)));self.send_header('Connection','close');self.end_headers();self.wfile.write(body)
            def do_GET(self):
                if self.path.endswith('/stat/device'):data=[device,{'_id':'child-id','mac':'aa:bb:cc:dd:ee:02','name':'fixture downstream','model':'AP','uplink':{'uplink_mac':'AA-BB-CC-DD-EE-FF','uplink_remote_port':1}}]
                elif self.path.endswith('/rest/networkconf'):data=networks
                elif self.path.endswith('/rest/user'):data=state['users']
                else:data=[{'name':'fixture client','sw_mac':'AA-BB-CC-DD-EE-FF','sw_port':1}]
                self.reply(data)
            def do_PUT(self):
                body=json.loads(self.rfile.read(int(self.headers.get('Content-Length','0'))));state['writes'].append((self.command,self.path,body))
                if 'port_overrides' in body:device['port_overrides']=body['port_overrides']
                self.reply([body])
            do_POST=do_PUT
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            args=['--url',f'http://127.0.0.1:{server.server_port}','--output','json']
            result=self.cli(*args,'devices','port-set','fixture switch','1','--name','new // label','--speed','auto')
            self.assertEqual(result.returncode,2);self.assertEqual(state['writes'],[])
            result=self.cli(*args,'devices','port-set','fixture switch','1','--name','new // label','--speed','auto','--mode','trunk','--tagged-vlans','','--native-vlan','fixture net','--allow-write')
            self.assertEqual(result.returncode,0,result.stderr)
            ports=state['writes'][-1][2]['port_overrides'];updated=next(p for p in ports if p['port_idx']==1)
            self.assertEqual(updated['vendor_field'],7);self.assertNotIn('speed',updated);self.assertIs(updated['autoneg'],True)
            self.assertEqual(updated['tagged_vlan_mgmt'],'custom');self.assertEqual(updated['tagged_networkconf_ids'],[]);self.assertEqual(updated['native_networkconf_id'],'network-id')
            updated['tagged_networkconf_ids']=['network-id'];device['port_overrides']=ports
            result=self.cli(*args,'devices','port-set','fixture switch','1','--mode','mirror','--allow-write')
            self.assertEqual(result.returncode,0,result.stderr);updated=next(p for p in state['writes'][-1][2]['port_overrides'] if p['port_idx']==1)
            self.assertEqual(updated['op_mode'],'mirror');self.assertEqual(updated['tagged_vlan_mgmt'],'block_all');self.assertEqual(updated['tagged_networkconf_ids'],[])
            self.assertIn({'port_idx':2,'name':'untouched','poe_mode':'off'},ports)
            result=self.cli(*args,'devices','ports','fixture switch','--with-clients')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(result.stdout)[0]['clients'][0]['name'],'fixture client')
            result=self.cli('devices','ports','AABB.CCDD.EEFF','--url',f'http://127.0.0.1:{server.server_port}','--with-clients')
            self.assertEqual(result.returncode,0,result.stderr);self.assertIn('fixture client',result.stdout)
            result=self.cli(*args,'devices','ports-export','fixture switch','--all','--with-clients')
            self.assertEqual(result.returncode,0,result.stderr);export=json.loads(result.stdout)
            self.assertEqual(len(export['ports']),3);self.assertEqual(export['ports'][0]['mode'],'mirror');self.assertNotIn('tagged_network_ids',export['ports'][0]);self.assertEqual(export['ports'][0]['last_seen']['devices'][0]['name'],'fixture downstream');self.assertEqual(export['ports'][0]['last_seen']['clients'][0]['name'],'fixture client')
            with tempfile.TemporaryDirectory() as tmp:
                file=Path(tmp)/'ports.jsonc'
                file.write_text('/* fixture */'+json.dumps(export)[:-1]+', // trailing comma\n}')
                result=self.cli(*args,'devices','port-set','fixture switch','--from-file',str(file),'--allow-write')
                self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(next(p for p in state['writes'][-1][2]['port_overrides'] if p['port_idx']==1)['name'],'new // label');self.assertTrue(all('last_seen' not in p for p in state['writes'][-1][2]['port_overrides']))
                file.write_text('{"ports":[{"index":1,"reset":true},{"index":99,"name":"bad"}]}')
                count=len(state['writes']);result=self.cli(*args,'devices','port-set','fixture switch','--from-file',str(file),'--allow-write')
                self.assertEqual(result.returncode,2);self.assertEqual(len(state['writes']),count)
            result=self.cli(*args,'devices','port-set','fixture switch','1','--reset','--allow-write')
            self.assertEqual(result.returncode,0,result.stderr);self.assertFalse(any(p['port_idx']==1 for p in state['writes'][-1][2]['port_overrides']))
            result=self.cli(*args,'clients','set-ip','AABB.CCDD.EE01','--ip','192.0.2.25','--allow-write')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(state['writes'][-1],('PUT','/proxy/network/api/s/default/rest/user/user-id',{'use_fixedip':True,'fixed_ip':'192.0.2.25','network_id':'network-id'}))
            result=self.cli(*args,'clients','remove-ip','aa:bb:cc:dd:ee:01','--network','fixture net','--allow-write')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(state['writes'][-1][2],{'use_fixedip':False})
            result=self.cli(*args,'clients','set-ip','aa:bb:cc:dd:ee:02','--ip','192.0.2.26','--network','fixture net','--allow-write')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(state['writes'][-1][0],'POST');self.assertEqual(state['writes'][-1][2]['mac'],'aa:bb:cc:dd:ee:02')
        finally:server.shutdown();server.server_close();thread.join()
    def test_topology_snapshot_pagination_uplinks_and_demo(self):
        site='00000000-0000-4000-8000-000000000001'
        first='00000000-0000-4000-8000-000000000002';second='00000000-0000-4000-8000-000000000003'
        base=f'/v1/sites/{site}'
        fixtures={('GET',base+'/devices?offset=0&limit=200'):{'data':[{'id':first},{'id':second}],'count':2,'totalCount':2},('GET',base+'/clients?offset=0&limit=200'):{'data':[{'id':'fixture-client','name':'fixture laptop','type':'WIRELESS','uplinkDeviceId':second}],'count':1,'totalCount':1},('GET',base+'/networks?offset=0&limit=200'):{'data':[],'count':0,'totalCount':0},('GET',base+'/devices/'+first):{'id':first,'name':'fixture gateway','state':'ONLINE'},('GET',base+'/devices/'+second):{'id':second,'name':'fixture access point','state':'ONLINE','uplink':{'deviceId':first}}}
        server=mock.MockServer(('127.0.0.1',0),self.spec,fixtures);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            args=['topology','--site',site,'--url',f'http://127.0.0.1:{server.server_port}']
            result=self.cli(*args)
            self.assertEqual(result.returncode,0,result.stderr);self.assertIn('  ├─ fixture access point',result.stdout);self.assertIn('    └─ fixture laptop',result.stdout)
            result=self.cli(*args,'--output','json')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(len(json.loads(result.stdout)['devices']),2)
            result=self.cli(*args,'--demo')
            self.assertEqual(result.returncode,0,result.stderr);self.assertNotIn('fixture',result.stdout)
            self.assertTrue(all(method=='GET' for method,path in server.requests))
        finally:server.shutdown();server.server_close();thread.join()
    def test_settings_merge_filters_demo_and_login_error(self):
        state={'writes':[],'fail':False}
        settings=[{'_id':'setting-id','site_id':'site-id','key':'teleport','enabled':True,'password':'fixture-secret','preserve':{'value':7},'auth':{'password':'nested-secret'},'servers':[{'token':'array-secret'}]}, {'key':'dpi','enabled':True}]
        class Handler(BaseHTTPRequestHandler):
            protocol_version='HTTP/1.1'
            def log_message(self,*args):pass
            def reply(self,data,cookie=False,status=200):
                body=json.dumps(data).encode();self.send_response(status)
                if cookie:self.send_header('Set-Cookie','TOKEN=failed-login; Path=/')
                self.send_header('Content-Length',str(len(body)));self.send_header('Connection','close');self.end_headers();self.wfile.write(body)
            def do_GET(self):
                if self.path.endswith('/rest/setting'):data=settings
                elif self.path.endswith('/rest/networkconf'):data=[{'_id':'a','purpose':'site-vpn','name':'fixture-vpn','preSharedKey':'fixture-secret'},{'_id':'b','purpose':'corporate'}]
                else:data=[{'mac':'aa:bb:cc:dd:ee:ff','use_fixedip':True},{'mac':'00:11:22:33:44:55'}]
                self.reply({'meta':{'rc':'ok'},'data':data})
            def do_PUT(self):
                body=json.loads(self.rfile.read(int(self.headers.get('Content-Length','0'))));state['writes'].append(body)
                self.reply({'password':'fixture-secret','auth':{'token':'nested-secret'}},status=400) if state['fail'] else self.reply({'meta':{'rc':'ok'},'data':[body]})
            def do_POST(self):
                self.rfile.read(int(self.headers.get('Content-Length','0')))
                self.reply({'meta':{'rc':'error'}},cookie=True)
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            args=['--url',f'http://127.0.0.1:{server.server_port}','--output','json']
            result=self.cli(*args,'vpn','settings','set','teleport','--field','enabled=false','--allow-write')
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(state['writes'][-1],{'enabled':False,'password':'fixture-secret','preserve':{'value':7},'auth':{'password':'nested-secret'},'servers':[{'token':'array-secret'}]})
            self.assertNotIn('fixture-secret',result.stdout)
            for field in ['auth={}', 'servers=[]', 'servers=[{}]']:
                result=self.cli(*args,'settings','set','teleport','--field',field,'--allow-write')
                self.assertEqual(result.returncode,2,result.stderr);self.assertIn('allow-secret-removal',result.stderr);self.assertEqual(len(state['writes']),1)
            result=self.cli(*args,'settings','set','teleport','--field','password=null','--allow-write')
            self.assertEqual(result.returncode,2);self.assertIn('allow-secret-removal',result.stderr);self.assertEqual(len(state['writes']),1)
            result=self.cli(*args,'settings','set','teleport','--field','password=null','--allow-write','--allow-secret-removal')
            self.assertEqual(result.returncode,0,result.stderr);self.assertIsNone(state['writes'][-1]['password'])
            result=self.cli(*args,'vpn','site-to-site','list')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(len(json.loads(result.stdout)),1);self.assertNotIn('fixture-secret',result.stdout)
            result=self.cli(*args,'vpn','site-to-site','list','--with-secrets')
            self.assertIn('fixture-secret',result.stdout)
            result=self.cli(*args,'vpn','site-to-site','list','--demo','--with-secrets')
            self.assertNotIn('fixture-secret',result.stdout);self.assertNotIn('fixture-vpn',result.stdout)
            result=self.cli(*args,'clients','reservations')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(len(json.loads(result.stdout)),1)
            result=self.cli(*args,'dpi','status')
            self.assertEqual(result.returncode,0,result.stderr);self.assertIs(json.loads(result.stdout),True)
            state['fail']=True
            result=self.cli(*args,'settings','set','teleport','--field','enabled=false','--allow-write')
            self.assertEqual(result.returncode,1,result.stderr);self.assertNotIn('fixture-secret',result.stderr);self.assertNotIn('nested-secret',result.stderr)
            result=self.cli(*args,'settings','set','teleport','--field','enabled=false','--allow-write','--with-secrets')
            self.assertIn('fixture-secret',result.stderr)
            state['fail']=False
            with tempfile.TemporaryDirectory() as tmp:
                cache=Path(tmp)/'login.json'
                result=self.cli(*args,'session','login','--allow-write','--cookie-file',str(cache),environment={'UNIFI_USERNAME':'fixture','UNIFI_PASSWORD':'fixture'})
                self.assertEqual(result.returncode,2,result.stderr);self.assertFalse(cache.exists())
        finally:server.shutdown();server.server_close();thread.join()
    def test_equivalent_cached_origins_are_accepted_without_network_io(self):
        with tempfile.TemporaryDirectory() as tmp:
            cache=Path(tmp)/'session.json'
            cache.write_text(json.dumps({'origin':'https://controller:443','cookie':'TOKEN=mock'}))
            result=self.cli('wifi','channels','--url','HTTPS://CONTROLLER','--cookie-file',str(cache),'--tls','invalid')
            self.assertEqual(result.returncode,2)
            self.assertIn('TLS backend must be',result.stderr)
            self.assertNotIn('different controller',result.stderr)
    def test_session_mfa_cache_origin_guard_and_logout(self):
        errors=[];requests=[]
        class Handler(BaseHTTPRequestHandler):
            protocol_version='HTTP/1.1'
            def log_message(self,*args):pass
            def reply(self,status,body,headers=()):
                data=json.dumps(body).encode()
                self.send_response(status)
                for name,value in headers:self.send_header(name,value)
                self.send_header('Content-Length',str(len(data)));self.send_header('Connection','close');self.end_headers();self.wfile.write(data)
            def do_POST(self):
                requests.append(('POST',self.path))
                try:
                    body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                    if self.path=='/api/auth/login':
                        assert self.headers.get('X-API-Key') is None
                        assert body['username']=='test-user' and body['password']==' \n test "password" 🦀\n '
                        if 'ubic_2fa_token' not in body:self.reply(499,{'token':'UBIC_2FA=challenge','errors':['ubic_2fa_token required']});return
                        assert body['ubic_2fa_token']=='123456'
                        assert self.headers.get('Cookie')=='UBIC_2FA=challenge'
                        self.reply(200,{},[('Set-Cookie','TOKEN=mock-session; Secure; HttpOnly'),('Set-Cookie','UBIC_2FA=; Max-Age=0'),('X-CSRF-Token','mock-csrf')])
                    elif self.path=='/api/auth/logout':
                        assert self.headers.get('Cookie')=='TOKEN=mock-session'
                        assert self.headers.get('X-CSRF-Token')=='mock-csrf'
                        self.reply(200,{})
                    else:raise AssertionError(self.path)
                except Exception as error:errors.append(error);self.reply(500,{'error':'fixture assertion'})
            def do_GET(self):
                requests.append(('GET',self.path))
                try:
                    assert self.path=='/proxy/network/api/s/default/stat/rogueap'
                    assert self.headers.get('Cookie')=='TOKEN=mock-session'
                    assert self.headers.get('X-API-Key') is None
                    self.reply(200,{'meta':{'rc':'ok'},'data':[{'ssid':'mock-neighbor'}]})
                except Exception as error:errors.append(error);self.reply(500,{'error':'fixture assertion'})
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                cache=Path(tmp)/'session.json'
                base=['--url',f'http://127.0.0.1:{server.server_port}','--cookie-file',str(cache)]
                env={'UNIFI_USERNAME':'test-user','UNIFI_PASSWORD':' \n test "password" 🦀\n ','UNIFI_TOTP':'123456'}
                result=self.cli(*base,'session','login',environment=env)
                self.assertEqual(result.returncode,2)
                self.assertEqual(requests,[])
                result=self.cli(*base,'session','login','--allow-write',environment=env)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual(cache.stat().st_mode & 0o777,0o600)
                self.assertNotIn('mock-session',result.stdout+result.stderr)
                self.assertNotIn('test-password',cache.read_text())
                result=self.cli(*base,'wifi','neighbors','--output','json')
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual(json.loads(result.stdout),[{'ssid':'mock-neighbor'}])
                count=len(requests)
                result=self.cli('wifi','neighbors','--url',f'http://127.0.0.2:{server.server_port}','--cookie-file',str(cache))
                self.assertEqual(result.returncode,2)
                self.assertIn('different controller',result.stderr)
                self.assertEqual(len(requests),count)
                result=self.cli(*base,'session','logout','--allow-write')
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertFalse(cache.exists())
                self.assertEqual(errors,[])
        finally:server.shutdown();server.server_close();thread.join()
    def test_cached_session_rotation_is_used_between_workflow_requests(self):
        state={'seen':[],'generation':0}
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def reply(self,clear=False):
                state['seen'].append((self.command,self.path,self.headers.get('Cookie'),self.headers.get('X-CSRF-Token')))
                state['generation']+=1;generation=state['generation'];data=b'{"meta":{"rc":"ok"},"data":[{"key":"test","enabled":true}]}'
                self.send_response(200);self.send_header('Content-Length',str(len(data)));self.send_header('Set-Cookie','TOKEN=; Max-Age=0' if clear else f'TOKEN=rotated-{generation}; Path=/');self.send_header('X-CSRF-Token',f'csrf-{generation}');self.end_headers();self.wfile.write(data)
            def do_GET(self):self.reply()
            def do_PUT(self):self.rfile.read(int(self.headers['Content-Length']));self.reply(True)
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                path=Path(directory)/'session.json';url=f'http://127.0.0.1:{server.server_port}';path.write_text(json.dumps({'origin':url,'cookie':'TOKEN=initial','csrf':'csrf-initial'}))
                args=['--url',url,'--cookie-file',str(path),'--site','fixture']
                result=self.cli(*args,'settings','list');self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual(state['seen'][-1][2:],('TOKEN=initial','csrf-initial'));self.assertEqual(json.loads(path.read_text())['cookie'],'TOKEN=rotated-1')
                before=path.read_text();result=self.cli(*args,'settings','list',environment={'UNIFI_COOKIE':'TOKEN=exported','UNIFI_CSRF':'csrf-exported'})
                self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(state['seen'][-1][2:],('TOKEN=exported','csrf-exported'));self.assertEqual(path.read_text(),before)
                result=self.cli(*args,'settings','set','test','--data','{"enabled":false}','--allow-write');self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual(state['seen'][-2][2:],('TOKEN=rotated-1','csrf-1'));self.assertEqual(state['seen'][-1][2:],('TOKEN=rotated-3','csrf-3'));self.assertFalse(path.exists())
                count=len(state['seen']);result=self.cli(*args,'settings','list');self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(len(state['seen']),count)
        finally:server.shutdown();server.server_close();thread.join()

    def test_vpn_families_and_wireguard_batch_workflows(self):
        state={'requests':[],'writes':[]}
        vpns=[{'_id':'s2s','name':'tunnel','vpn_type':'ipsec','purpose':'site-vpn','enabled':False,'x_ipsec_pre_shared_key':'fixture-secret','vendor':{'retained':True}},
              {'_id':'remote','name':'server','vpn_type':'wireguard','purpose':'remote-user-vpn'},
              {'_id':'client','name':'outbound','vpn_type':'wireguard-client','purpose':'vpn-client'}]
        peer={'_id':'peer','name':'laptop','interface_ip':'192.0.2.2','public_key':'fixture-public-key','preshared_key':'fixture-shared-key','private_key':'fixture-private-key','vendor':True}
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def reply(self,value):
                data=json.dumps(value).encode();self.send_response(200);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
            def do_GET(self):
                state['requests'].append((self.command,self.path))
                if self.path.endswith('/rest/networkconf'):self.reply({'meta':{'rc':'ok'},'data':vpns})
                elif self.path.endswith('/rest/setting'):self.reply({'meta':{'rc':'ok'},'data':[{'key':'peer_to_peer','enabled':True,'x_secret':'fixture-setting-secret'}]})
                elif '/wireguard/server/users' in self.path:self.reply([peer])
                else:self.reply([])
            def write(self):
                raw=self.rfile.read(int(self.headers.get('Content-Length','0')));data=json.loads(raw) if raw else None;state['writes'].append((self.command,self.path,data));state['requests'].append((self.command,self.path));self.reply({'meta':{'rc':'ok'},'data':[]})
            do_POST=write;do_PUT=write;do_DELETE=write
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            common=['--url',f'http://127.0.0.1:{server.server_port}','--site','fixture']
            auth={'UNIFI_COOKIE':'TOKEN=mock','UNIFI_CSRF':'mock-csrf'}
            for family,kind,purpose in [('site-to-site','ipsec','site-vpn'),('remote-access','wireguard','remote-user-vpn'),('clients','wireguard-client','vpn-client')]:
                result=self.cli(*common,'vpn',family,'create','--data',json.dumps({'name':'new','vpn_type':kind}),environment=auth)
                self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(state['requests'],[])
                result=self.cli(*common,'vpn',family,'create','--data',json.dumps({'name':'new','vpn_type':kind}),'--allow-write',environment=auth)
                self.assertEqual(result.returncode,0,result.stderr);body=state['writes'][-1][2];self.assertEqual(body['purpose'],purpose);self.assertTrue(body['enabled'])
                state['requests'].clear()
            result=self.cli(*common,'vpn','site-to-site','update','s2s','--data','{"name":"renamed"}','--allow-write',environment=auth)
            self.assertEqual(result.returncode,0,result.stderr);body=state['writes'][-1][2]
            self.assertEqual(body['x_ipsec_pre_shared_key'],'fixture-secret');self.assertEqual(body['vendor'],{'retained':True});self.assertFalse(body['enabled']);self.assertNotIn('_id',body)
            self.assertIn(('GET','/proxy/network/api/s/fixture/rest/networkconf'),state['requests'])
            before=len(state['writes'])
            result=self.cli(*common,'vpn','clients','update','s2s','--data','{"name":"wrong family"}','--allow-write',environment=auth)
            self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(before,len(state['writes']))
            result=self.cli(*common,'vpn','site-to-site','update','s2s','--data','{"x_ipsec_pre_shared_key":""}','--allow-write',environment=auth)
            self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(before,len(state['writes']))
            for family,valid,wrong in [('site-to-site','s2s','remote'),('remote-access','remote','client'),('clients','client','s2s')]:
                before=len(state['writes']);result=self.cli(*common,'vpn',family,'delete',wrong,'--allow-write',environment=auth)
                self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(len(state['writes']),before)
                result=self.cli(*common,'vpn',family,'delete',valid,'--allow-write',environment=auth)
                self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(state['writes'][-1],('DELETE','/proxy/network/api/s/fixture/rest/networkconf/'+valid,None))
            result=self.cli(*common,'vpn','peers','create','server','--data',json.dumps({k:peer[k] for k in ['name','interface_ip','public_key']}),'--allow-write',environment=auth)
            self.assertEqual(result.returncode,0,result.stderr);body=state['writes'][-1][2];self.assertIsInstance(body,list);self.assertEqual(len(body),1);self.assertEqual(body[0]['preshared_key'],'');self.assertEqual(body[0]['allowed_ips'],[])
            result=self.cli(*common,'vpn','peers','update','server','peer','--data','{"name":"renamed peer"}','--allow-write',environment=auth)
            self.assertEqual(result.returncode,0,result.stderr);body=state['writes'][-1][2]
            self.assertEqual(body[0]['_id'],'peer');self.assertEqual(body[0]['private_key'],'fixture-private-key');self.assertTrue(body[0]['vendor']);self.assertEqual(body[0]['name'],'renamed peer')
            result=self.cli(*common,'vpn','peers','delete','server','peer','--allow-write',environment=auth)
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(state['writes'][-1][2],['peer']);self.assertTrue(state['writes'][-1][1].endswith('/users/batch_delete'))
            result=self.cli(*common,'vpn','peers','list','server','--id-only',environment=auth)
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(result.stdout.strip(),'peer')
            result=self.cli(*common,'vpn','peers','get','server','peer','--output','json',environment=auth)
            self.assertEqual(result.returncode,0,result.stderr);body=json.loads(result.stdout);self.assertEqual(body['preshared_key'],'[REDACTED]');self.assertEqual(body['private_key'],'[REDACTED]')
            with tempfile.TemporaryDirectory() as tmp:
                saved=Path(tmp)/'peer.json'
                for secrets in [False,True]:
                    result=self.cli(*common,'vpn','peers','get','server','peer','--save',str(saved),*(['--with-secrets'] if secrets else []),environment=auth)
                    self.assertEqual(result.returncode,0,result.stderr);self.assertEqual('fixture-private-key' in saved.read_text(),secrets)
            before=len(state['writes']);result=self.cli(*common,'vpn','peers','update','server','peer','--data','{"preshared_key":""}','--allow-write',environment=auth)
            self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(len(state['writes']),before)
            self.assertIn(('GET','/proxy/network/v2/api/site/fixture/wireguard/server/users?networkId=server'),state['requests'])
            result=self.cli(*common,'vpn','settings','get','peer-to-peer','--output','json',environment=auth)
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(result.stdout)['key'],'peer_to_peer')
            result=self.cli(*common,'vpn','settings','patch','peer-to-peer','--data','{"fields":{"enabled":false}}','--allow-write',environment=auth)
            self.assertEqual(result.returncode,0,result.stderr);body=state['writes'][-1][2];self.assertNotIn('fields',body);self.assertFalse(body['enabled']);self.assertEqual(body['x_secret'],'fixture-setting-secret')
        finally:server.shutdown();server.server_close();thread.join()

    def test_dpi_station_queries_and_firmware_fallbacks(self):
        state={'mode':'modern','requests':[]}
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def reply(self,value):
                data=json.dumps(value).encode();self.send_response(200);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
            def do_GET(self):
                state['requests'].append((self.command,self.path,None))
                if '/traffic-flow-latest-statistics?' in self.path:self.reply({'top_all_traffic_by_application':[{'app':1}]} if state['mode']=='modern' else {})
                else:self.reply({'meta':{'rc':'ok'},'data':[{'app':3,'fallback':True}]})
            def do_POST(self):
                data=json.loads(self.rfile.read(int(self.headers['Content-Length'])));state['requests'].append((self.command,self.path,data))
                self.reply({'meta':{'rc':'ok'},'data':[{'by_cat':[{'app':2}]}] if state['mode']!='fallback' else []})
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            common=['stats','dpi','--url',f'http://127.0.0.1:{server.server_port}','--site','fixture','--output','json'];auth={'UNIFI_COOKIE':'TOKEN=mock','UNIFI_CSRF':'mock-csrf'}
            result=self.cli(*common,environment=auth);self.assertEqual(result.returncode,2);self.assertEqual(state['requests'],[])
            result=self.cli(*common,'--allow-write',environment=auth);self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(json.loads(result.stdout),[{'top_all_traffic_by_application':[{'app':1}]}]);self.assertEqual(len(state['requests']),1);self.assertIn('period=DAY&top=30',state['requests'][0][1])
            state['requests'].clear();result=self.cli(*common,'--group-by','category','--allow-write',environment=auth)
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(len(state['requests']),1);self.assertTrue(state['requests'][-1][1].endswith('/stat/sitedpi'));self.assertEqual(state['requests'][-1][2],{'type':'by_cat'})
            state['mode']='fallback';state['requests'].clear();result=self.cli(*common,'--allow-write',environment=auth)
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(len(state['requests']),3);self.assertTrue(state['requests'][-1][1].endswith('/stat/dpi'));self.assertTrue(json.loads(result.stdout)[0]['fallback'])
            state['mode']='legacy';state['requests'].clear();result=self.cli(*common,'--macs','02:00:00:00:00:01,02:00:00:00:00:02','--group-by','category','--allow-write',environment=auth)
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(len(state['requests']),1);self.assertTrue(state['requests'][0][1].endswith('/stat/stadpi'));self.assertEqual(state['requests'][0][2]['macs'],['02:00:00:00:00:01','02:00:00:00:00:02'])
            count=len(state['requests']);result=self.cli(*common,'--macs','invalid','--allow-write',environment=auth);self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(count,len(state['requests']))
        finally:server.shutdown();server.server_close();thread.join()

    def test_unified_paging_viewer_flags_help_and_optional_environment(self):
        site='00000000-0000-4000-8000-000000000001';path=f'/v1/sites/{site}/devices'
        fixtures={('GET',path+'?limit=1&offset=2'):{'data':[{'id':'fixture-id','name':'switch'}],'offset':2,'count':1,'totalCount':4}}
        server=mock.MockServer(('127.0.0.1',0),self.spec,fixtures);thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            args=['network','devices','list',site,'--url',f'http://127.0.0.1:{server.server_port}','--page-limit','1','--page-offset','2']
            result=self.cli(*args,'--output','json');self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(result.stdout)['totalCount'],4)
            result=self.cli(*args,'--hide-page','--output','json');self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(result.stdout),[{'id':'fixture-id','name':'switch'}])
            result=self.cli(*args);self.assertEqual(result.returncode,0,result.stderr);self.assertIn('1 of 4 items',result.stdout)
            before=len(server.requests);result=self.cli('countries','list','--minutes','3','--url',f'http://127.0.0.1:{server.server_port}')
            self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(len(server.requests),before)
        finally:server.shutdown();server.server_close();thread.join()
        protect=mock.MockServer(('127.0.0.1',0),json.loads((ROOT/'schemas/protect.json').read_text()));thread=threading.Thread(target=protect.serve_forever,daemon=True);thread.start()
        try:
            result=self.cli('protect','viewers','settings','viewer','--liveview','fixture-liveview','--url',f'http://127.0.0.1:{protect.server_port}','--allow-write')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(protect.request_bodies[-1][2]),{'liveview':'fixture-liveview'})
        finally:protect.shutdown();protect.server_close();thread.join()
        for args in [('wifi','create','--help'),('help','connection'),('completions','bash','--no-descriptions')]:
            result=self.cli(*args);self.assertEqual(result.returncode,0,result.stderr)
        result=self.cli('wifi','create','--help');self.assertIn('--passphrase',result.stdout);self.assertNotIn('--source-zone',result.stdout)
        result=self.cli('__complete','wifi','create');self.assertIn('--passphrase',result.stdout.splitlines());self.assertNotIn('--source-zone',result.stdout.splitlines())
        result=self.cli('__complete','--hidden','wifi','create');self.assertIn('--passphrase',result.stdout.splitlines())
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'.env';path.write_bytes(b'UNIFI_API_KEY=\xff')
            result=self.cli('system','info','--url',self.url,cwd=directory);self.assertEqual(result.returncode,2,result.stderr);self.assertIn('UTF-8',result.stderr)
            path.write_text('UNIFI_API_KEY='+'x'*8192)
            result=self.cli('system','info','--url',self.url,cwd=directory);self.assertEqual(result.returncode,2,result.stderr);self.assertIn('capacity',result.stderr)

    def test_help_and_operations(self):
        self.assertEqual(self.cli('--help').returncode,0)
        result=self.cli('operations','network');self.assertEqual(result.returncode,0)
        self.assertEqual(len(result.stdout.splitlines()),73)
    def test_context_completion_is_offline(self):
        result=self.cli('__complete','devices')
        self.assertEqual(result.returncode,0,result.stderr)
        words=result.stdout.splitlines()
        self.assertIn('list',words);self.assertIn('restart',words);self.assertNotIn('cameras',words)
        result=self.cli('__complete','--controller','https://unreachable.invalid','devices','list','--output')
        self.assertEqual(result.stdout.splitlines(),['table','json','json-compact','yaml','plain'])
        for shell in ['bash','zsh','fish','powershell','elvish']:
            result=self.cli('completions',shell)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertIn('unifi __complete',result.stdout)
            if shell=='bash':subprocess.run(['bash','-n'],input=result.stdout,text=True,check=True,capture_output=True)
    def test_all_follows_network_and_cloud_pages(self):
        site='00000000-0000-4000-8000-000000000001';path=f'/v1/sites/{site}/devices'
        fixtures={
            ('GET',path+'?offset=0&limit=1'):{'data':[{'id':'first'}],'totalCount':2},
            ('GET',path+'?offset=1&limit=1'):{'data':[{'id':'second'}],'totalCount':2},
        }
        server=mock.MockServer(('127.0.0.1',0),self.spec,fixtures)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            result=self.cli('devices','list','--site',site,'--url',f'http://127.0.0.1:{server.server_port}','--all','--limit','1','--output','json')
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(json.loads(result.stdout),[{'id':'first'},{'id':'second'}])
        finally:server.shutdown();server.server_close();thread.join()
        fixtures={('GET','/v1/hosts'):{'data':[{'id':'first'}],'nextToken':'next-page'},('GET','/v1/hosts?nextToken=next-page'):{'data':[{'id':'second'}]}}
        server=mock.MockServer(('127.0.0.1',0),json.loads((ROOT/'schemas/site-manager.json').read_text()),fixtures)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            result=self.cli('cloud','hosts','--url',f'http://127.0.0.1:{server.server_port}','--all','--output','json')
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(json.loads(result.stdout),[{'id':'first'},{'id':'second'}])
            fixtures[('GET','/v1/hosts?nextToken=next-page')]['nextToken']='next-page'
            result=self.cli('cloud','hosts','--url',f'http://127.0.0.1:{server.server_port}','--all','--output','json')
            self.assertEqual(result.returncode,2)
            self.assertEqual(result.stdout,'')
            self.assertIn('did not advance',result.stderr)
        finally:server.shutdown();server.server_close();thread.join()
    def test_profiles_are_atomic_private_and_redacted(self):
        with tempfile.TemporaryDirectory() as tmp:
            config=Path(tmp)/'nested/config.json'
            base=['--config',str(config)]
            result=self.cli(*base,'config','init','--controller',self.url)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(config.stat().st_mode & 0o777,0o600)
            result=self.cli(*base,'config','set','profiles.default.apiKey','secret-test-value')
            self.assertEqual(result.returncode,0,result.stderr)
            result=self.cli(*base,'config','use','default')
            self.assertEqual(result.returncode,0,result.stderr)
            result=self.cli(*base,'config','show')
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertNotIn('secret-test-value',result.stdout)
            self.assertEqual(json.loads(result.stdout)['profiles']['default']['apiKey'],'[REDACTED]')
            result=self.cli(*base,'system','info','--output','json')
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertIn('applicationVersion',json.loads(result.stdout))
            self.assertFalse(list(config.parent.glob('*.tmp-*')))
            result=self.cli(*base,'config','set','a.b.c.d.e.f.g.h.i','1')
            self.assertEqual(result.returncode,2,result.stderr)
            result=self.cli(*base,'config','init','--profile','prod.eu','--controller',self.url)
            self.assertEqual(result.returncode,0,result.stderr)
            result=self.cli(*base,'--profile','prod.eu','system','info','--output','json')
            self.assertEqual(result.returncode,0,result.stderr)
    def test_websocket_subscription_and_idle_heartbeat(self):
        errors=[]
        class Handler(BaseHTTPRequestHandler):
            protocol_version='HTTP/1.1'
            def log_message(self,*args):pass
            def do_GET(self):
                try:
                    assert self.path in ['/proxy/protect/integration/v1/subscribe/events','/proxy/network/wss/s/default/events']
                    assert self.headers['X-API-Key']=='mock-only-test-key'
                    key=self.headers['Sec-WebSocket-Key']
                    accept=base64.b64encode(hashlib.sha1((key+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
                    time.sleep(0.35) # handshake timeout must remain independent of the shorter heartbeat
                    self.send_response(101);self.send_header('Connection','Upgrade');self.send_header('Upgrade','websocket');self.send_header('Sec-WebSocket-Accept',accept);self.end_headers()
                    # Wait for an idle heartbeat, then reply and deliver one event.
                    self.connection.settimeout(5)
                    ping=self.rfile.read(6)
                    assert ping[:2]==b'\x89\x80',repr(ping)
                    payload=b'{"type":"add","item":{"id":"mock-event"}}'
                    if '/network/' in self.path:
                        skipped=b'{"meta":{"message":"events"},"data":[{"key":"EVT_IGNORE"}]}'
                        payload=b'{"meta":{"message":"events"},"data":[{"key":"EVT_TEST","id":"mock-event"}]}'
                        self.wfile.write(b'\x8a\0'+bytes([0x81,len(skipped)])+skipped+bytes([0x81,len(payload)])+payload)
                    else:self.wfile.write(b'\x8a\0'+bytes([0x81,len(payload)])+payload)
                    self.wfile.flush()
                    close=self.rfile.read(8)
                    assert close[:2]==b'\x88\x82',repr(close)
                except Exception as error:errors.append(error)
                finally:self.close_connection=True
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            result=self.cli('protect','subscribe','protect-events','--url',f'http://127.0.0.1:{server.server_port}','--count','1','--keep-alive-interval','100ms')
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(json.loads(result.stdout)['item']['id'],'mock-event')
            result=self.cli('events','watch','--types','EVT_TEST','--url',f'http://127.0.0.1:{server.server_port}','--count','1','--keep-alive-interval','0.1s')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(result.stdout)[0]['key'],'EVT_TEST')
            self.assertEqual(errors,[])
        finally:server.shutdown();server.server_close();thread.join()
        for value in ['1m30s','250ms','0.5s','+1h','2m0.5s']:
            result=self.cli('events','watch','--keep-alive-interval',value,'--help');self.assertEqual(result.returncode,0,(value,result.stderr))
        for value in ['0','-1s','1x','1msx','1h1s','1..5s']:
            result=self.cli('events','watch','--keep-alive-interval',value,'--help');self.assertEqual(result.returncode,2,(value,result.stderr))
    def test_read_over_real_tcp(self):
        result=self.cli('call','network','getInfo','--url',self.url)
        self.assertEqual(result.returncode,0,result.stderr+result.stdout)
        self.assertIn('applicationVersion',json.loads(result.stdout))
    def test_yaml_preserves_unicode_strings_and_nested_empty_values(self):
        values=[{'name':'🦀','items':['🦀',{},[],{'text':'a\u0085b\u2028c\u2029d\x7f'}]},'🦀',['🦀',{},[]]]
        for value in values:
            server=mock.MockServer(('127.0.0.1',0),self.spec,{('GET','/v1/info'):value})
            thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
            try:
                result=self.cli('system','info','--url',f'http://127.0.0.1:{server.server_port}','--output','yaml')
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual(yaml.safe_load(result.stdout),value)
            finally:server.shutdown();server.server_close();thread.join()
    def test_resource_commands_discovery_and_formats(self):
        site='00000000-0000-4000-8000-000000000001'
        fixture={'data':[{'id':site,'name':'fixture-site','internalReference':'default'}],'totalCount':1,'offset':0,'limit':200,'count':1}
        server=mock.MockServer(('127.0.0.1',0),self.spec,{('GET','/v1/sites'):fixture})
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            url=f'http://127.0.0.1:{server.server_port}'
            for fmt in ['json','json-compact','plain','yaml','table']:
                result=self.cli('devices','list','--url',url,'--site','fixture-site','--output',fmt)
                self.assertEqual(result.returncode,0,result.stderr+result.stdout)
                if fmt.startswith('json'):self.assertEqual(json.loads(result.stdout),[])
            self.assertIn(('GET',f'/v1/sites/{site}/devices'),server.requests)
            result=self.cli('sites','list','--url',url,'--output','yaml')
            self.assertIn('"name": "fixture-site"',result.stdout)
        finally:server.shutdown();server.server_close();thread.join()
    def test_protect_json_binary_and_multipart(self):
        spec=json.loads((ROOT/'schemas/protect.json').read_text())
        server=mock.MockServer(('127.0.0.1',0),spec)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            url=f'http://127.0.0.1:{server.server_port}'
            result=self.cli('protect','cameras','list','--url',url,'--output','json')
            self.assertEqual(result.returncode,0,result.stderr+result.stdout)
            self.assertEqual(json.loads(result.stdout),[])
            server.fixtures[('GET','/v1/cameras')]=[{'id':'fixture-camera','name':'fixture name'}]
            result=self.cli('protect','cameras','list','--url',url,'--id-only')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(result.stdout,'fixture-camera\n')
            result=self.cli('protect','cameras','list','--url',url,'--id-only','--output','json','--color','always')
            self.assertEqual(json.loads(result.stdout),['fixture-camera']);self.assertNotIn('\x1b',result.stdout)
            result=self.cli('protect','cameras','stream-create','camera','--url',url,'--low','--high','--allow-write')
            self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(server.request_bodies[-1][2]),{'qualities':['low','high']})
            result=self.cli('protect','cameras','stream-delete','camera','--url',url,'--low','--high','--allow-write')
            self.assertEqual(result.returncode,0,result.stderr);self.assertTrue(any('qualities=low%2Chigh' in path for method,path in server.raw_requests))
            result=self.cli('protect','cameras','snapshot','camera','--url',url,'--low-quality','--quiet')
            self.assertEqual(result.returncode,0,result.stderr);self.assertTrue(any('highQuality=false' in path for method,path in server.raw_requests))
            result=self.cli('protect','cameras','snapshot','camera','--url',url,'--jpeg-quality','101')
            self.assertEqual(result.returncode,2)
            if Path('/usr/bin/convert').exists():
                jpeg=subprocess.run(['/usr/bin/convert','-size','2x2','xc:blue','jpeg:-'],capture_output=True,check=True).stdout
                server.fixtures[('GET','/v1/cameras/camera/snapshot')]=jpeg
                result=self.cli('protect','cameras','snapshot','camera','--url',url,'--jpeg-quality','80',binary=True)
                self.assertEqual(result.returncode,0,result.stderr);self.assertTrue(result.stdout.startswith(b'\xff\xd8'))
                del server.fixtures[('GET','/v1/cameras/camera/snapshot')]
            with tempfile.TemporaryDirectory() as tmp:
                out=Path(tmp)/'snapshot.jpg'
                result=self.cli('protect','cameras','snapshot','camera','--url',url,'--save',str(out))
                self.assertEqual(result.returncode,0,result.stderr+result.stdout)
                self.assertEqual(out.read_bytes(),b'\xff\xd8mock-snapshot\xff\xd9')
                result=self.cli('protect','cameras','snapshot','camera','--url',url,'--quiet')
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertEqual(result.stdout,'')
                image=Path(tmp)/'upload.png';image.write_bytes(b'\x89PNG\0\xff')
                args=['protect','files','upload',str(image),'--url',url]
                before=len(server.requests)
                self.assertEqual(self.cli(*args).returncode,2)
                self.assertEqual(len(server.requests),before)
                result=self.cli(*args,'--allow-write')
                self.assertEqual(result.returncode,0,result.stderr+result.stdout)
                result=self.cli('protect','files','upload','--upload',str(image),'--url',url,'--allow-write');self.assertEqual(result.returncode,0,result.stderr)
                result=self.cli('protect','files','list','animations','--url',url);self.assertEqual(result.returncode,0,result.stderr)
                count=len(server.requests);result=self.cli('protect','files','list','unsupported','--url',url);self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(count,len(server.requests))
        finally:server.shutdown();server.server_close();thread.join()
    def test_write_is_only_exercised_on_loopback_mock(self):
        with tempfile.TemporaryDirectory() as tmp:
            body=Path(tmp)/'body.json';body.write_text('/* JSONC fixture */ {"action":"RESTART",}\n')
            args=['call','network','executeAdoptedDeviceAction','--param','siteId=test','--param','deviceId=test','--url',self.url,'--body',str(body)]
            self.assertEqual(self.cli(*args).returncode,2)
            result=self.cli(*args,'--allow-write')
            self.assertEqual(result.returncode,0,result.stderr+result.stdout)
            body.write_text('{"action":"INVALID"}')
            self.assertEqual(self.cli(*args,'--allow-write').returncode,1)
    def test_convenience_write_payloads_on_mock(self):
        site='00000000-0000-4000-8000-000000000001'
        result=self.cli('devices','adopt','AABB.CCDD.EEFF','--ignore-limit','--site',site,'--url',self.url,'--allow-write')
        self.assertEqual(result.returncode,0,result.stderr+result.stdout)
        method,path,body=self.server.request_bodies[-1]
        self.assertEqual(method,'POST')
        self.assertEqual(json.loads(body),{'ignoreDeviceLimit':True,'macAddress':'aa:bb:cc:dd:ee:ff'})
        self.assertEqual(body.count(b'ignoreDeviceLimit'),1)
        result=self.cli('hotspot','create','--name','mock voucher','--minutes','30','--count','3','--site',site,'--url',self.url,'--allow-write')
        self.assertEqual(result.returncode,0,result.stderr+result.stdout)
        payload=json.loads(self.server.request_bodies[-1][2])
        self.assertEqual(payload,{'name':'mock voucher','timeLimitMinutes':30,'count':3})
    def test_friendly_resource_lookup_follows_pages_and_rejects_ambiguity(self):
        site='00000000-0000-4000-8000-000000000001'
        first='00000000-0000-4000-8000-000000000002'
        second='00000000-0000-4000-8000-000000000003'
        path=f'/v1/sites/{site}/devices'
        fixtures={
            ('GET',path+'?offset=0&limit=200'):{'data':[{'id':first,'name':'first'}],'count':1,'totalCount':2},
            ('GET',path+'?offset=1&limit=200'):{'data':[{'id':second,'name':'fixture target'}],'count':1,'totalCount':2},
            ('GET',path+'/'+second):{'id':second,'name':'fixture target','state':'ONLINE'},
        }
        server=mock.MockServer(('127.0.0.1',0),self.spec,fixtures)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            args=['devices','get','fixture target','--site',site,'--url',f'http://127.0.0.1:{server.server_port}','--output','json']
            result=self.cli(*args)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(json.loads(result.stdout)['id'],second)
            self.assertEqual(len(server.requests),3)
            fixtures[('GET',path+'?offset=0&limit=200')]['data'][0]['name']='fixture target'
            before=len(server.requests)
            result=self.cli(*args)
            self.assertEqual(result.returncode,2)
            self.assertIn('ambiguous',result.stderr)
            self.assertEqual(len(server.requests)-before,2)
        finally:server.shutdown();server.server_close();thread.join()
    def test_missing_query_and_insecure_remote_fail(self):
        result=self.cli('call','network','getFirewallPolicyOrdering','--param','siteId=s','--url',self.url)
        self.assertEqual(result.returncode,2)
        result=self.cli('call','network','getInfo','--url','http://192.0.2.1')
        self.assertEqual(result.returncode,2)
    def test_cloud_legacy_commands_resolve_site_references_without_cookies(self):
        site='00000000-0000-4000-8000-000000000001';state={'requests':[],'sites':[{'id':'00000000-0000-4000-8000-000000000001','name':'fixture site','internalReference':'legacy-site'}]}
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_GET(self):
                state['requests'].append((self.path,self.headers.get('X-API-Key'),self.headers.get('Cookie')))
                if self.path=='/v1/hosts':value={'data':[{'id':'mock/console?value','type':'console','reportedState':{'state':'connected'}}]}
                elif '/integration/v1/sites?' in self.path:value={'data':state['sites'],'count':len(state['sites']),'totalCount':len(state['sites'])}
                else:value={'meta':{'rc':'ok'},'data':[{'subsystem':'wlan','status':'ok'}]}
                data=json.dumps(value).encode();self.send_response(200);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
        with tempfile.TemporaryDirectory() as directory:
            directory=Path(directory);key=directory/'key.pem';cert=directory/'cert.pem'
            subprocess.run(['openssl','req','-x509','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-keyout',str(key),'-out',str(cert),'-days','1','-subj','/CN=api.ui.com','-addext','subjectAltName=DNS:api.ui.com'],check=True,capture_output=True)
            (directory/'sessions').mkdir();(directory/'sessions/default.json').write_text('deliberately invalid unrelated direct-controller cache')
            server=ThreadingHTTPServer(('127.0.0.1',0),Handler);context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.load_cert_chain(cert,key);server.socket=context.wrap_socket(server.socket,server_side=True)
            thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
            try:
                args=['system','health','--url',f'https://API.UI.COM:{server.server_port}','--connect','127.0.0.1','--ca-file',str(cert),'--site',site,'--output','json']
                environment={'UNIFI_CONFIG':str(directory/'config.json')}
                result=self.cli(*args,environment=environment);self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(result.stdout),[{'subsystem':'wlan','status':'ok'}])
                self.assertTrue(state['requests'][-1][0].endswith('/api/s/legacy-site/stat/health'));self.assertIn('/mock%2Fconsole%3Fvalue/',state['requests'][-1][0]);self.assertTrue(all(key=='mock-only-test-key' and cookie is None for _,key,cookie in state['requests']))
                count=len(state['requests']);result=self.cli(*args,environment={**environment,'UNIFI_COOKIE':'TOKEN=unrelated'})
                self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(len(state['requests']),count)
                config=directory/'config.json';config.write_text(json.dumps({'activeProfile':'cloud','profiles':{'cloud':{'controller':args[3],'console':'mock/console?value'}}}))
                result=self.cli('cloud','switch','FIXTURE SITE',*args[2:],'--quiet',environment=environment)
                self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(config.read_text())['profiles']['cloud']['site'],'legacy-site')
                before=config.read_text();result=self.cli('cloud','switch','unknown',*args[2:],environment=environment)
                self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(config.read_text(),before)
                state['sites']=[{'id':site,'name':'home','internalReference':'lower'},{'id':'00000000-0000-4000-8000-000000000002','name':'HOME','internalReference':'upper'}]
                result=self.cli('cloud','switch','Home',*args[2:],environment=environment);self.assertEqual(result.returncode,2,result.stderr);self.assertEqual(config.read_text(),before)
                result=self.cli('cloud','switch','HOME',*args[2:],environment=environment);self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(config.read_text())['profiles']['cloud']['site'],'upper')
            finally:server.shutdown();server.server_close();thread.join()

    def test_verified_tls_and_hostname_rejection(self):
        with tempfile.TemporaryDirectory(prefix='unifi-tls-') as tmp:
            tmp=Path(tmp);key=tmp/'key.pem';cert=tmp/'cert.pem';der=tmp/'cert.der'
            subprocess.run(['openssl','req','-x509','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-keyout',str(key),'-out',str(cert),'-days','1','-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost,DNS:api.ui.com,IP:127.0.0.1'],check=True,capture_output=True)
            subprocess.run(['openssl','x509','-in',str(cert),'-outform','der','-out',str(der)],check=True,capture_output=True)
            hosts={'data':[{'id':'incomplete'},{'id':'wrong-type','type':42},{'id':'mock-console','type':'console','reportedState':{'state':'connected'}}]}
            server=mock.MockServer(('127.0.0.1',0),self.spec,{('GET','/v1/hosts'):hosts})
            context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.minimum_version=ssl.TLSVersion.TLSv1_3;context.load_cert_chain(cert,key)
            server.socket=context.wrap_socket(server.socket,server_side=True)
            thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
            try:
                args=['call','network','getInfo','--connect','127.0.0.1','--ca-cert',str(der)]
                result=self.cli(*args,'--url',f'https://localhost:{server.server_port}')
                self.assertEqual(result.returncode,0,result.stderr+result.stdout)
                result=self.cli('system','info','--ca-file',str(cert),'--url',f'https://localhost:{server.server_port}')
                self.assertEqual(result.returncode,0,result.stderr+result.stdout)
                profile=tmp/'profiles.json';profile_env={'UNIFI_CONFIG':str(profile)}
                result=self.cli('config','init','--profile','trust','--controller',f'https://localhost:{server.server_port}','--ca-file',str(cert),'--timeout','2',environment=profile_env)
                self.assertEqual(result.returncode,0,result.stderr);saved=json.loads(profile.read_text());self.assertEqual(saved['activeProfile'],'trust');self.assertEqual(saved['profiles']['trust']['caFile'],str(cert));self.assertEqual(saved['profiles']['trust']['timeout'],2)
                result=self.cli('system','info',environment=profile_env);self.assertEqual(result.returncode,0,result.stderr)
                result=self.cli('config','set','profiles.trust.insecure','true',environment=profile_env);self.assertEqual(result.returncode,0,result.stderr)
                result=self.cli('system','info','--controller',f'https://wrong.example:{server.server_port}','--connect','127.0.0.1',environment=profile_env);self.assertEqual(result.returncode,0,result.stderr)
                result=self.cli('system','info','--controller',f'https://wrong.example:{server.server_port}','--connect','127.0.0.1','--insecure','false',environment=profile_env);self.assertEqual(result.returncode,2,result.stderr)
                payload=tmp/'large-body.json';value={'action':'RESTART','extension':'x'*70000};payload.write_text(json.dumps(value))
                result=self.cli('call','network','executeAdoptedDeviceAction','--param','siteId=fixture','--param','deviceId=fixture','--body',str(payload),'--allow-write','--ca-file',str(cert),'--url',f'https://localhost:{server.server_port}')
                self.assertEqual(result.returncode,0,result.stderr);self.assertEqual(json.loads(server.request_bodies[-1][2]),value)
                result=self.cli('system','info','--ca-file',str(cert),'--url',f'https://127.0.0.1:{server.server_port}')
                self.assertEqual(result.returncode,0,result.stderr+result.stdout)
                result=self.cli('system','info','--ca-file',str(cert),'--connect','127.0.0.1','--url',f'https://API.UI.COM:{server.server_port}')
                self.assertEqual(result.returncode,0,result.stderr+result.stdout)
                self.assertIn(('GET','/v1/connector/consoles/mock-console/proxy/network/integration/v1/info'),server.raw_requests)
                result=self.cli('protect','subscribe','protect-events','--url','https://API.UI.COM','--console','mock-console')
                self.assertEqual(result.returncode,2,result.stderr+result.stdout)
                self.assertIn('WebSockets require a direct controller',result.stderr)
                result=self.cli('system','info','--url',f'https://localhost:{server.server_port}','--insecure')
                self.assertEqual(result.returncode,0,result.stderr+result.stdout)
                result=self.cli('system','info','--connect','127.0.0.1','--ca-file',str(cert),'--url',f'https://wrong.example:{server.server_port}')
                self.assertEqual(result.returncode,2)
                result=self.cli(*args,'--url',f'https://wrong.example:{server.server_port}')
                self.assertEqual(result.returncode,2)
                # A valid but unrelated trust anchor must also fail.
                other=tmp/'other.pem';other_key=tmp/'other-key.pem';other_der=tmp/'other.der'
                subprocess.run(['openssl','req','-x509','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-nodes','-keyout',str(other_key),'-out',str(other),'-days','1','-subj','/CN=Other'],check=True,capture_output=True)
                subprocess.run(['openssl','x509','-in',str(other),'-outform','der','-out',str(other_der)],check=True,capture_output=True)
                result=self.cli('call','network','getInfo','--connect','127.0.0.1','--ca-cert',str(other_der),'--url',f'https://localhost:{server.server_port}')
                self.assertEqual(result.returncode,2)
            finally:server.shutdown();server.server_close();thread.join()
if __name__=='__main__':unittest.main()
