"""Ensure every public command supplies exactly the parameters its endpoint accepts."""
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import unittest

ROOT=Path(__file__).resolve().parents[1]
loader=importlib.util.spec_from_file_location('cli_generator',ROOT/'tools/generate-cli.py')
generator=importlib.util.module_from_spec(loader);loader.loader.exec_module(generator)

class Routes(unittest.TestCase):
    def test_every_captured_option_is_recognized_offline(self):
        reference=json.loads((ROOT/'docs/parity/reference-cli.json').read_text())
        contexts={}
        for cli in reference.values():
            for row in cli['commands']:
                for flag in row['flags']:contexts.setdefault(flag,row['path'])
        switches={'help','version','insecure','yes','quiet','verbose','demo','no-cache','no-effects','debug','trace','all','id-only','hide-page','with-clients','with-secrets','allow-secret-removal','reset','low','medium','high','package','low-quality','ignore-limit','get','force','after-system','hidden','band-steering','fast-roaming','dhcp','isolated','internet','logging','allow-return-traffic','unarchived','no-descriptions'}
        values={'color':'never','output':'json','timeout':'1','count':'1','jpeg-quality':'90','keep-alive-interval':'1s','field':'fixture=true','param':'fixture=value','query':'fixture=value','on':'true','enabled':'true'}
        for flag,path in contexts.items():
            arguments=[str(ROOT/'target/debug/unifi'),*path,'--'+flag]
            if flag not in switches:arguments.append(values.get(flag,'fixture'))
            arguments.append('--help')
            result=subprocess.run(arguments,capture_output=True,text=True,timeout=5,cwd=ROOT)
            self.assertEqual(result.returncode,0,(flag,path,result.stderr))

    def test_captured_reference_actions_have_commands(self):
        local=(ROOT/'cli/src/ui.rs').read_text().split('const LOCAL:')[1].split('const FLAGS:')[0]
        commands={tuple(row['words']) for row in generator.route_rows()}
        commands.update(tuple(re.findall(r'"([^"\n]+)"',row)) for row in re.findall(r'&\[([^]\n]+)\]',local))
        reference=json.loads((ROOT/'docs/parity/reference-cli.json').read_text())
        for cli,snapshot in reference.items():
            for row in snapshot['commands']:
                path=tuple(row['path'])
                if not path or row['children'] and not row.get('default_action'):continue
                if path[0]=='completion':continue # shell argument of the local completion command
                self.assertIn(path,commands,(cli,path))

    def test_route_parameters_match_endpoint_contracts(self):
        operations={}
        for surface in ['network','site-manager','protect','session-api']:
            spec=json.loads((ROOT/'schemas'/f'{surface}.json').read_text())
            for path,methods in spec['paths'].items():
                for method,operation in methods.items():
                    if method not in ['get','post','put','patch','delete']:continue
                    operations[(surface,generator.generator.operation_id(path,method,operation))]=path
        seen=set()
        for row in generator.route_rows():
            command=tuple(row['words'])
            self.assertNotIn(command,seen,command);seen.add(command)
            path=operations[(row['surface'],row['operation'])]
            required=set(re.findall(r'\{([^}]+)\}',path))
            if '*' in path:required.add(path.split('*')[1])
            supplied={p for p in row['params'] if not p.startswith(('$','?','!'))}
            self.assertFalse(supplied-required,(command,supplied-required))
            automatic={'siteId','site','interval'}
            if row['operation'] in ('getISPMetrics','queryISPMetrics'):automatic.add('type')
            if row['words'][0]=='dpi':automatic.add('key')
            self.assertTrue(required-supplied <= automatic,(command,required-supplied))

if __name__=='__main__':unittest.main()
