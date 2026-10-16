#!/usr/bin/env python3
"""Generate human command routes for every pinned official API operation."""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
loader = importlib.util.spec_from_file_location('schema_generator', ROOT / 'tools/generate.py')
generator = importlib.util.module_from_spec(loader)
loader.loader.exec_module(generator)

NETWORK = {
'getCountries': 'countries list', 'getDpiApplications': 'dpi applications', 'getDpiApplicationCategories': 'dpi categories',
'getInfo': 'system info', 'getPendingDevicePage': 'devices pending', 'getSiteOverviewPage': 'sites list',
'getAclRulePage': 'acl list', 'createAclRule': 'acl create', 'getAclRuleOrdering': 'acl order', 'updateAclRuleOrdering': 'acl reorder',
'deleteAclRule': 'acl delete', 'getAclRule': 'acl get', 'updateAclRule': 'acl update',
'getConnectedClientOverviewPage': 'clients list', 'getConnectedClientDetails': 'clients get', 'executeConnectedClientAction': 'clients action',
'getDeviceTagPage': 'devices tags', 'getAdoptedDeviceOverviewPage': 'devices list', 'adoptDevice': 'devices adopt', 'removeDevice': 'devices remove',
'getAdoptedDeviceDetails': 'devices get', 'executeAdoptedDeviceAction': 'devices action', 'executePortAction': 'devices port-cycle',
'getAdoptedDeviceLatestStatistics': 'devices stats', 'getDnsPolicyPage': 'dns list', 'createDnsPolicy': 'dns create',
'deleteDnsPolicy': 'dns delete', 'getDnsPolicy': 'dns get', 'updateDnsPolicy': 'dns update',
'getFirewallPolicies': 'firewall policies list', 'createFirewallPolicy': 'firewall policies create',
'getFirewallPolicyOrdering': 'firewall policies order', 'updateFirewallPolicyOrdering': 'firewall policies reorder',
'deleteFirewallPolicy': 'firewall policies delete', 'getFirewallPolicy': 'firewall policies get',
'patchFirewallPolicy': 'firewall policies patch', 'updateFirewallPolicy': 'firewall policies update',
'getFirewallZones': 'firewall zones list', 'createFirewallZone': 'firewall zones create', 'deleteFirewallZone': 'firewall zones delete',
'getFirewallZone': 'firewall zones get', 'updateFirewallZone': 'firewall zones update',
'deleteVouchers': 'hotspot vouchers delete-all', 'getVouchers': 'hotspot vouchers list', 'createVouchers': 'hotspot vouchers generate',
'deleteVoucher': 'hotspot vouchers delete', 'getVoucher': 'hotspot vouchers get',
'getNetworksOverviewPage': 'networks list', 'createNetwork': 'networks create', 'deleteNetwork': 'networks delete',
'getNetworkDetails': 'networks get', 'updateNetwork': 'networks update', 'getNetworkReferences': 'networks references',
'getRadiusProfileOverviewPage': 'radius profiles', 'getLagPage': 'switching lags list', 'getLag': 'switching lags get',
'getMcLagDomainPage': 'switching mc-lag-domains list', 'getMcLagDomain': 'switching mc-lag-domains get',
'getSwitchStackPage': 'switching stacks list', 'getSwitchStack': 'switching stacks get',
'getTrafficMatchingLists': 'traffic-lists list', 'createTrafficMatchingList': 'traffic-lists create',
'deleteTrafficMatchingList': 'traffic-lists delete', 'getTrafficMatchingList': 'traffic-lists get', 'updateTrafficMatchingList': 'traffic-lists update',
'getVpnServerPage': 'vpn servers', 'getSiteToSiteVpnTunnelPage': 'vpn tunnels', 'getWansOverviewPage': 'wans list',
'getWifiBroadcastPage': 'wifi list', 'createWifiBroadcast': 'wifi create', 'deleteWifiBroadcast': 'wifi delete',
'getWifiBroadcastDetails': 'wifi get', 'updateWifiBroadcast': 'wifi update',
}
CLOUD = {
'ConnectorDelete': 'cloud connector delete', 'ConnectorGet': 'cloud connector get', 'ConnectorPatch': 'cloud connector patch',
'ConnectorPost': 'cloud connector post', 'ConnectorPut': 'cloud connector put',
'listDevices': 'cloud devices', 'listHosts': 'cloud hosts', 'getHostByID': 'cloud host',
'getISPMetrics': 'cloud isp get', 'queryISPMetrics': 'cloud isp query',
'listSDWanConfigs': 'cloud sdwan list', 'getSDWanConfigByID': 'cloud sdwan get', 'getSDWanConfigStatus': 'cloud sdwan status',
'listSites': 'cloud sites',
}

SESSION = {
    'listRogueAps':'wifi neighbors','listChannels':'wifi channels','getClientWifiExperience':'clients wifi','getClientRoams':'clients roams',
    'listAlarms':'alarms list','listEvents':'events list','listSettings':'settings list','listAdmins':'admin list',
    'getSysinfo':'system sysinfo','getHealth':'system health','listNatRules':'nat policies list','createNatRule':'nat policies create',
    'updateNatRule':'nat policies update','deleteNatRule':'nat policies delete',
    'listFirewallGroups':'firewall groups list','createFirewallGroup':'firewall groups create','updateFirewallGroup':'firewall groups update','deleteFirewallGroup':'firewall groups delete',
    'listIpsecSa':'vpn status','listVpnConnections':'vpn connections list','restartVpnConnection':'vpn connections restart',
    'listAllWireguardPeers':'vpn peers list','getWireguardSubnets':'vpn peers subnets','createWireguardPeers':'vpn peers create',
    'updateWireguardPeers':'vpn peers update','deleteWireguardPeers':'vpn peers delete','listMagicVpnConfigs':'vpn magic-site-to-site list',
    'suggestOpenvpnPorts':'vpn remote-access suggest-port','downloadOpenvpnConfiguration':'vpn remote-access download-config',
    'downloadBackup':'system backup download','reboot':'system reboot','poweroff':'system poweroff',
    'getSiteReport':'stats site','getDeviceReport':'stats device','getUserReport':'stats client','getGwReport':'stats gateway',
    'getSiteDpi':'stats dpi','subscribeEvents':'events watch','loginUnifiOs':'session login','logoutUnifiOs':'session logout',
}

def protect_command(path, method):
    special = {
        ('get','/v1/meta/info'): 'info',
        ('get','/v1/subscribe/devices'): 'subscribe device-events',
        ('get','/v1/subscribe/events'): 'subscribe protect-events',
        ('post','/v1/cameras/{id}/ptz/patrol/start/{slot}'): 'cameras ptz patrol-start',
        ('post','/v1/cameras/{id}/ptz/patrol/stop'): 'cameras ptz patrol-stop',
        ('post','/v1/cameras/{id}/ptz/goto/{slot}'): 'cameras ptz goto-position',
        ('post','/v1/alarm-manager/webhook/{id}'): 'alarm-manager webhook',
        ('get','/v1/nvrs'): 'nvrs',
        ('post','/v1/files/{fileType}'): 'files upload',
        ('get','/v1/files/{fileType}'): 'files list',
    }
    if (method,path) in special: return 'protect ' + special[(method,path)]
    parts = [part for part in path.split('/') if part and part != 'v1' and not part.startswith('{')]
    if 'rtsps-stream' in parts:
        return 'protect cameras stream-' + {'get':'get','post':'create','delete':'delete'}[method]
    if len(parts) == 1:
        action = {'get':'details' if '{id}' in path else 'list','post':'create','patch':'settings' if parts[0]=='viewers' else 'patch','delete':'delete'}[method]
        return 'protect ' + parts[0] + ' ' + action
    return 'protect ' + ' '.join(parts)

def route_rows():
    rows=[]
    def add(words, surface, operation, params, body=None):
        rows.append(dict(words=words.split(),surface=surface,operation=operation,params=params,body=body))
    for surface, mappings in [('network',NETWORK),('site-manager',CLOUD),('protect',None),('session-api',SESSION)]:
        spec=json.loads((ROOT/'schemas'/(surface+'.json')).read_text());found=set()
        for path,methods in spec['paths'].items():
            for method,operation in methods.items():
                if method not in ('get','post','put','patch','delete'):continue
                name=generator.operation_id(path,method,operation);found.add(name)
                params=[p for p in re.findall(r'\{([^}]+)\}',path) if p not in ('siteId','site','interval')]
                if '*' in path:params.append(path.split('*')[1])
                if surface=='site-manager' and name in ('getISPMetrics','queryISPMetrics'):params.remove('type')
                body={'action':'POWER_CYCLE'} if name=='executePortAction' else None
                if surface=='session-api' and name in ('reboot','poweroff','logoutUnifiOs','logoutStandalone','restartVpnConnection'):body={}
                if name in ('getSiteReport','getDeviceReport','getUserReport','getGwReport'):body={'attrs':['bytes','time','rx_bytes','tx_bytes','num_sta']}
                if name=='getSiteDpi':body={'type':'by_app'}
                words=(mappings.get(name,'session '+generator.snake(name).replace('_','-')) if mappings else protect_command(path,method))
                if surface=='network' and name=='adoptDevice':params=['$macAddress'];body={'ignoreDeviceLimit':False}
                if name in ('updateWireguardPeers','deleteWireguardPeers'):params.append('$_id')
                add(words,surface,name,params,body)
        if surface=='session-api' and not set(mappings)<=found:raise ValueError('unknown session route mapping')
        if mappings and surface!='session-api' and found!=set(mappings):raise ValueError(f'{surface}: incomplete route map')
    for words,operation,params in [
        ('clients find','getConnectedClientOverviewPage',['!search']),('countries','getCountries',[]),('dpi apps','getDpiApplications',[]),('networks refs','getNetworkReferences',['networkId']),
        ('hotspot list','getVouchers',[]),('hotspot get','getVoucher',['voucherId']),('hotspot create','createVouchers',[]),
        ('hotspot delete','deleteVoucher',['voucherId']),('hotspot purge','deleteVouchers',[]),
        ('vpn servers get','getVpnServerPage',['?id']),('vpn tunnels get','getSiteToSiteVpnTunnelPage',['?id']),
    ]:add(words,'network',operation,params)
    add('devices restart','network','executeAdoptedDeviceAction',['deviceId'],{'action':'RESTART'})
    add('clients unauthorize','network','executeConnectedClientAction',['clientId'],{'action':'UNAUTHORIZE_GUEST_ACCESS'})
    add('clients authorize','network','executeConnectedClientAction',['clientId'],{'action':'AUTHORIZE_GUEST_ACCESS'})
    for words,operation,params,body in [
        ('devices locate','executeDeviceCommand',['$mac'],{'cmd':'set-locate'}),
        ('devices upgrade','executeDeviceCommand',['$mac'],{'cmd':'upgrade'}),
        ('devices provision','executeDeviceCommand',['$mac'],{'cmd':'force-provision'}),
        ('devices speedtest','executeDeviceCommand',[],{'cmd':'speedtest'}),
        ('clients block','executeClientCommand',['$mac'],{'cmd':'block-sta'}),
        ('clients unblock','executeClientCommand',['$mac'],{'cmd':'unblock-sta'}),
        ('clients kick','executeClientCommand',['$mac'],{'cmd':'kick-sta'}),
        ('clients forget','executeClientCommand',['$@macs'],{'cmd':'forget-sta'}),
        ('clients reservations','listUsers',[],None),
        ('alarms archive','executeEventCommand',['$_id'],{'cmd':'archive-alarm'}),
        ('alarms archive-all','executeEventCommand',[],{'cmd':'archive-all-alarms'}),
        ('admin invite','executeSiteCommand',[],{'cmd':'invite-admin','role':'admin'}),
        ('admin revoke','executeSiteCommand',['$admin'],{'cmd':'revoke-admin'}),
        ('admin update','executeSiteCommand',['$admin'],{'cmd':'update-admin'}),
        ('sites create','executeSiteCommand',['$name'],{'cmd':'add-site'}),
        ('sites delete','executeSiteCommand',['$name'],{'cmd':'delete-site'}),
        ('system backup list','executeBackupCommand',[],{'cmd':'list-backups'}),
        ('system backup create','executeBackupCommand',[],{'cmd':'backup'}),
        ('system backup delete','executeBackupCommand',['$filename'],{'cmd':'delete-backup'}),
        ('firewall groups get','listFirewallGroups',['?id'],None),
        ('nat policies get','listNatRules',['?id'],None),
        ('settings get','listSettings',['?key'],None),('settings set','updateSetting',['key'],None),
        ('dpi status','listSettings',[],None),('dpi enable','updateSetting',[],{'enabled':True}),('dpi disable','updateSetting',[],{'enabled':False}),
        ('vpn settings list','listSettings',[],None),('vpn settings get','listSettings',['?key'],None),('vpn settings set','updateSetting',['key'],None),('vpn settings patch','updateSetting',['key'],None),('settings export','listSettings',[],None),
        ('vpn health','getHealth',[],None),
        ('vpn connections get','listVpnConnections',['?id'],None),
        ('vpn peers get','listWireguardPeers',['serverId','?id'],None),
        ('vpn magic-site-to-site get','listMagicVpnConfigs',['?id'],None),
    ]:add(words,'session-api',operation,params,body)
    for family in ['site-to-site','remote-access','clients']:
        for action,operation,params in [('list','listNetworkConf',[]),('get','listNetworkConf',['?id']),('create','createNetworkConf',[]),('update','updateNetworkConf',['id']),('delete','deleteNetworkConf',['id'])]:
            add('vpn '+family+' '+action,'session-api',operation,params)
    for words,op,params in [('cloud hosts get','getHostByID',['id']),('cloud isp','getISPMetrics',[]),('cloud sdwan','listSDWanConfigs',[])]:add(words,'site-manager',op,params)
    for words,op,params in [
        ('info','getInfo',[]),('sites list','getSiteOverviewPage',[]),('clients list','getConnectedClientOverviewPage',['siteId']),
        ('clients details','getConnectedClientDetails',['siteId','clientId']),('clients action','executeConnectedClientAction',['siteId','clientId','$action']),
        ('devices list','getAdoptedDeviceOverviewPage',['siteId']),('devices details','getAdoptedDeviceDetails',['siteId','deviceId']),
        ('devices action','executeAdoptedDeviceAction',['siteId','deviceId','$action']),
        ('devices port-action','executePortAction',['siteId','deviceId','portIdx','$action']),('devices stats','getAdoptedDeviceLatestStatistics',['siteId','deviceId']),
        ('vouchers list','getVouchers',['siteId']),('vouchers details','getVoucher',['siteId','voucherId']),
        ('vouchers generate','createVouchers',['siteId']),('vouchers delete-filter','deleteVouchers',['siteId']),('vouchers delete','deleteVoucher',['siteId','voucherId']),
    ]:add('network '+words,'network',op,params)
    return rows

def generate():
    out=['// @generated by tools/generate-cli.py\nuse unifi::{network as n, site_manager as s, protect as p, session_api as x, Operation};\n',
        '#[derive(Clone, Copy, PartialEq)] pub enum Surface { Network, Cloud, Protect, Session }\n',
        "pub struct Route { pub words: &'static [&'static str], pub operation: &'static Operation, pub params: &'static [&'static str], pub surface: Surface, pub body: Option<&'static [u8]> }\n",
        'pub static ROUTES: &[Route] = &[\n']
    for row in route_rows():
        prefix,surface={'network':('n','Network'),'site-manager':('s','Cloud'),'protect':('p','Protect'),'session-api':('x','Session')}[row['surface']]
        body='Some(b'+json.dumps(json.dumps(row['body'],separators=(',',':')))+')' if row['body'] is not None else 'None'
        out.append('Route { words: &['+','.join(map(json.dumps,row['words']))+'], operation: &'+prefix+'::'+generator.snake(row['operation']).upper()+', params: &['+','.join(map(json.dumps,row['params']))+'], surface: Surface::'+surface+', body: '+body+' },\n')
    out.append('];\n')
    options={}
    for cli in json.loads((ROOT/'docs/parity/reference-cli.json').read_text()).values():
        inherited=set(cli['commands'][0]['flags'])
        for command in cli['commands']:
            flags=set(command['flags'])-inherited
            options.setdefault(tuple(command['path']),set()).update(flags)
    out.append('pub static COMMAND_OPTIONS: &[(&[&str], &[&str])] = &[\n')
    for words,flags in sorted(options.items()):
        out.append('(&['+','.join(map(json.dumps,words))+'], &['+','.join(map(json.dumps,sorted(flags)))+']),\n')
    out.append('];\n')
    return subprocess.run(['rustfmt','--edition','2024'],input=''.join(out),text=True,capture_output=True,check=True).stdout

def main():
    parser = argparse.ArgumentParser(); parser.add_argument('--check', action='store_true'); args = parser.parse_args()
    path = ROOT / 'cli/src/routes.rs'; result = generate()
    if args.check:
        if path.read_text() != result: raise SystemExit('CLI routes are stale; run python3 tools/generate-cli.py')
    else: path.write_text(result)
    print('CLI routes cover Network, Site Manager and Protect')
if __name__ == '__main__': main()
