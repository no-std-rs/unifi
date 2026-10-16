#!/usr/bin/env python3
"""Local schema mock, not a device emulator. Never connects to a controller."""
import argparse
import copy
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import re
import ssl
from urllib.parse import urlsplit, unquote, parse_qs

ROOT=Path(__file__).resolve().parents[1]


def normalized(spec):
    """Explicit compatibility interpretations also used by generated bindings."""
    spec = copy.deepcopy(spec)

    def visit(value):
        if isinstance(value, list):
            for child in value:
                visit(child)
        elif isinstance(value, dict):
            # OAS 3.0 nullable is not a JSON Schema keyword.
            if value.get('nullable') and isinstance(value.get('type'), str):
                value['type'] = [value['type'], 'null']
            # Site Manager's nested string enums cannot describe valid strings.
            if value.get('type') == 'string' and any(isinstance(v, list) for v in value.get('enum', [])):
                del value['enum']
            if value.get('type') == 'integer' and value.get('enum') and all(isinstance(v, str) for v in value['enum']):
                value['enum'] = [int(v) for v in value['enum']]
            discriminator = value.get('discriminator', {})
            mapping = discriminator.get('mapping', {})
            enum = value.get('properties', {}).get(discriminator.get('propertyName'), {}).get('enum', [])
            if mapping and enum and set(mapping).isdisjoint(enum):
                discriminator['mapping'] = {wire: mapping[re.sub('[^A-Z0-9]+', '_', wire.upper())] for wire in enum if re.sub('[^A-Z0-9]+', '_', wire.upper()) in mapping}
            # Protect liveview wraps inherited fields with additionalProperties:
            # false but omits their names. Interpret those inherited names as
            # allowed, matching the SDK's flattened object model.
            if value.get('additionalProperties') is False and value.get('allOf'):
                properties, _ = object_shape(spec, value)
                value.setdefault('properties', {}).update({name: copy.deepcopy(field) for name, field in properties.items() if name not in value.get('properties', {})})
            for child in list(value.values()):
                visit(child)

    visit(spec)
    return spec


def object_shape(spec, schema, seen=()):
    """Resolve inherited properties together with required fields before sampling."""
    if '$ref' in schema:
        name=schema['$ref'].rsplit('/',1)[1]
        if name in seen:return {},set()
        return object_shape(spec,spec['components']['schemas'][name],(*seen,name))
    properties={};required=set(schema.get('required',[]))
    for parent in schema.get('allOf',[]):
        fields,needed=object_shape(spec,parent,seen)
        properties.update(fields);required.update(needed)
    properties.update(schema.get('properties',{}))
    return properties,required

def sample(spec,schema,seen=(),*,variant=None,include_optional=False):
    """Deterministic synthetic values; never use controller data or credentials."""
    if '$ref' in schema:
        name=schema['$ref'].rsplit('/',1)[1]
        if name in seen:return {}
        return sample(spec,spec['components']['schemas'][name],(*seen,name),variant=variant,include_optional=include_optional)
    if 'const' in schema:return copy.deepcopy(schema['const'])
    if 'enum' in schema:return copy.deepcopy(schema['enum'][0])
    if 'oneOf' in schema:return sample(spec,schema['oneOf'][0],seen,include_optional=include_optional)
    if 'anyOf' in schema:return sample(spec,schema['anyOf'][0],seen,include_optional=include_optional)
    properties,required=object_shape(spec,schema)
    kind=schema.get('type')
    if isinstance(kind,list):kind=next((k for k in kind if k!='null'),'null')
    if kind=='object' or properties:
        out={k:sample(spec,v,seen,include_optional=include_optional) for k,v in properties.items() if include_optional or k in required}
        mapping=schema.get('discriminator',{}).get('mapping',{})
        discriminator=schema.get('discriminator',{}).get('propertyName')
        if mapping:
            choices=[(tag,ref) for tag,ref in mapping.items() if ref.rsplit('/',1)[-1] not in seen]
            if variant is not None:choices=[(variant,mapping[variant])]
            if choices:
                value,ref=choices[0]
                target=sample(spec,{'$ref':ref},seen,include_optional=include_optional)
                if isinstance(target,dict):out.update(target)
                out[discriminator]=value
        # Paging fixtures remain consistent with their empty data array.
        if 'data' in out and isinstance(out['data'],list):
            for k in ('count','totalCount'):out[k]=len(out['data'])
        return out
    if 'allOf' in schema:
        value={}
        for part in schema['allOf']:
            value=sample(spec,part,seen,include_optional=include_optional)
        if isinstance(value,(int,float)) and not isinstance(value,bool):
            value=max(value,schema.get('minimum',value))
            value=min(value,schema.get('maximum',value))
        return value
    if kind=='array':
        count=max(schema.get('minItems',0),int(include_optional))
        return [sample(spec,schema.get('items',{}),seen,include_optional=include_optional) for _ in range(min(count,schema.get('maxItems',count)))]
    if kind=='boolean':return False
    if kind in ('integer','number'):
        value=schema.get('minimum',0)
        exclusive=schema.get('exclusiveMinimum')
        if exclusive is not None and not isinstance(exclusive,bool):value=max(value,exclusive+1)
        if exclusive is True:value+=1
        if 'maximum' in schema:value=min(value,schema['maximum'])
        return value
    if kind=='string':
        value={'uuid':'00000000-0000-4000-8000-000000000001','date-time':'2026-01-01T00:00:00Z','ipv4':'192.0.2.1','ipv6':'2001:db8::1','mac':'02:00:00:00:00:01'}.get(schema.get('format'),'mock')
        if schema.get('pattern')=='^[A-Z]{3}$':value='USA'
        if schema.get('pattern')=='^[0-9a-fA-F]+$':value='abcdef'
        value=value[:schema.get('maxLength',len(value))]
        if len(value)<schema.get('minLength',0):value+='x'*(schema['minLength']-len(value))
        return value
    if kind=='null':return None
    return {}

class MockServer(ThreadingHTTPServer):
    daemon_threads=True
    def __init__(self,address,spec,fixtures=None):
        self.spec=spec=normalized(spec)
        self.fixtures=fixtures or {}
        self.requests=[]
        self.raw_requests=[]
        self.request_bodies=[]
        self.routes=[]
        for path,methods in spec['paths'].items():
            pattern=re.escape(path)
            pattern=re.sub(r'\\\{[^}]+\\\}',r'[^/]+',pattern)
            pattern=re.sub(r'\\\*path',r'.+',pattern)
            self.routes.append((re.compile('^'+pattern+'$'),methods))
        super().__init__(address,Handler)

class Handler(BaseHTTPRequestHandler):
    protocol_version='HTTP/1.1'
    def log_message(self,*args):pass
    def respond(self,status,value):
        data=value if isinstance(value,bytes) else json.dumps(value,separators=(',',':')).encode() if value is not None else b''
        self.send_response(status);self.send_header('Content-Type','application/octet-stream' if isinstance(value,bytes) else 'application/json');self.send_header('Content-Length',str(len(data)));self.send_header('Connection','close');self.end_headers();self.wfile.write(data)
    def request(self):
        url=urlsplit(self.path);path=url.path
        self.server.raw_requests.append((self.command,self.path))
        path=re.sub(r'^/v1/connector/consoles/[^/]+','',path)
        for prefix in ('/proxy/network/integration','/proxy/protect/integration','/integration'):
            if path.startswith(prefix+'/'):path=path[len(prefix):];break
        self.server.requests.append((self.command,path))
        fixture=self.server.fixtures.get((self.command,path+('?' + url.query if url.query else '')),self.server.fixtures.get((self.command,path)))
        if fixture is not None:self.respond(200,fixture);return
        for pattern,methods in self.server.routes:
            if pattern.fullmatch(path):
                operation=methods.get(self.command.lower())
                if operation is None:self.respond(405,{'error':'method not allowed'});return
                break
        else:self.respond(404,{'error':'unknown operation'});return
        if not self.headers.get('X-API-Key'):
            self.respond(401,{'error':'missing API key'});return
        query=parse_qs(url.query,keep_blank_values=True)
        for param in operation.get('parameters',[]):
            if param['in']=='query' and param.get('required') and not query.get(param['name']):
                self.respond(400,{'error':'missing query parameter','parameter':param['name']});return
        try:
            length=int(self.headers.get('Content-Length','0'))
            if not 0<=length<=1048576:raise ValueError('body too large')
            data=self.rfile.read(length)
            self.server.request_bodies.append((self.command,path,data))
            if operation.get('requestBody',{}).get('required') and not data:raise ValueError('missing request body')
            if data:
                if self.headers.get_content_type() == 'multipart/form-data':
                    from email.parser import BytesParser
                    from email.policy import default
                    message=BytesParser(policy=default).parsebytes(b'Content-Type: '+self.headers['Content-Type'].encode()+b'\r\nMIME-Version: 1.0\r\n\r\n'+data)
                    files=list(message.iter_parts())
                    if len(files)!=1 or files[0].get_param('name',header='content-disposition')!='file' or not files[0].get_filename():raise ValueError('invalid multipart file')
                    body=None
                else:body=json.loads(data)
                schema=operation.get('requestBody',{}).get('content',{}).get('application/json',{}).get('schema')
                if schema:
                    import jsonschema
                    from referencing import Registry,Resource
                    from referencing.jsonschema import DRAFT202012
                    # Embed the document so local component refs resolve without network access.
                    document=copy.deepcopy(self.server.spec)
                    root_schema=schema
                    if '$ref' in schema:
                        root_schema=self.server.spec['components']['schemas'][schema['$ref'].rsplit('/',1)[1]]
                    discriminator=root_schema.get('discriminator',{})
                    mapping=discriminator.get('mapping',{})
                    if mapping:
                        selected=body.get(discriminator['propertyName']) if isinstance(body,dict) else None
                        if selected not in mapping:raise ValueError('unknown discriminator value')
                        schema={'allOf':[schema,{'$ref':mapping[selected]}]}
                    document['x-body-schema']=schema
                    registry=Registry().with_resource('urn:unifi:mock',Resource.from_contents(document,default_specification=DRAFT202012))
                    validator=jsonschema.Draft202012Validator({'$ref':'urn:unifi:mock#/x-body-schema'},registry=registry)
                    validator.validate(body)
        except Exception as error:
            self.respond(400,{'error':'invalid request body','detail':str(error)[:200]});return
        status,response=next(((int(k),v) for k,v in operation['responses'].items() if k.startswith('2')), (200,{}))
        schema=response.get('content',{}).get('application/json',{}).get('schema')
        if 'image/jpeg' in response.get('content',{}):
            data=b'\xff\xd8mock-snapshot\xff\xd9'
            self.send_response(status);self.send_header('Content-Type','image/jpeg');self.send_header('Content-Length',str(len(data)));self.send_header('Connection','close');self.end_headers();self.wfile.write(data)
        else:self.respond(status,sample(self.server.spec,schema) if schema else None)
    do_GET=request
    do_POST=request
    do_PUT=request
    do_PATCH=request
    do_DELETE=request

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--surface',choices=['network','site-manager','protect'],default='network')
    parser.add_argument('--port',type=int,default=4010)
    parser.add_argument('--cert');parser.add_argument('--key')
    args=parser.parse_args()
    server=MockServer(('127.0.0.1',args.port),json.loads((ROOT/'schemas'/f'{args.surface}.json').read_text()))
    if args.cert:
        context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.minimum_version=ssl.TLSVersion.TLSv1_3
        context.load_cert_chain(args.cert,args.key);server.socket=context.wrap_socket(server.socket,server_side=True)
    print(f'{args.surface} schema mock: {"https" if args.cert else "http"}://127.0.0.1:{server.server_port}',flush=True)
    try:server.serve_forever()
    except KeyboardInterrupt:pass
    finally:server.server_close()
if __name__=='__main__':main()
