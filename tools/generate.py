#!/usr/bin/env python3
"""Regenerate bindings offline from pinned official schemas. Python stdlib only."""
import argparse
import hashlib
import json
import math
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
KEYWORDS = {'type','ref','self','Self','super','crate','match','loop','move','fn','mod','use','where','in','as','enum','struct','const','static','impl','trait','return','default','async','await','dyn','let','pub','for','if','else','while','break','continue','true','false','box','yield','try','gen'}

def snake(text):
    text = re.sub(r'([A-Z]+)([A-Z][a-z])', r'\1_\2', text)
    text = re.sub(r'([a-z0-9])([A-Z])', r'\1_\2', text)
    return re.sub(r'[^a-zA-Z0-9]+', '_', text).strip('_').lower()

def ident(text):
    text = snake(text) or 'empty'
    if text in KEYWORDS: return 'r#' + text
    if text[:1].isdigit(): text = '_' + text
    return text

def pascal(text):
    text = ''.join(s.capitalize() for s in snake(text).split('_'))
    return ('Model' if text[:1].isdigit() else '') + (text or 'Value')

def quoted(value): return json.dumps(value, ensure_ascii=False)
def doc(text): return '\n'.join('/// '+line for line in text.splitlines())+'\n'

def allows_null(schema):
    kind = schema.get('type')
    return schema.get('nullable', False) or isinstance(kind, list) and 'null' in kind

def wire_discriminator(schema):
    discriminator=schema.get('discriminator',{})
    mapping=discriminator.get('mapping',{})
    enum=schema.get('properties',{}).get(discriminator.get('propertyName'),{}).get('enum',[])
    # Named protocols publish Java enum constants as discriminator keys but
    # lower-case wire names (including dots/hyphens) in the property enum.
    if mapping and enum and all(isinstance(v,str) for v in enum) and set(mapping).isdisjoint(enum):
        mapping={name:mapping[re.sub(r'[^A-Z0-9]+','_',name.upper())] for name in enum if re.sub(r'[^A-Z0-9]+','_',name.upper()) in mapping}
        return discriminator.get('propertyName'),mapping,[name for name in enum if name not in mapping]
    return discriminator.get('propertyName'),mapping,[]

def operation_id(path, method, operation):
    """Protect omits operationId; derive a stable name from method and path."""
    return operation.get('operationId') or method.title() + ''.join(
        ('By' + pascal(part[1:-1])) if part.startswith('{') else pascal(part)
        for part in path.split('/') if part and part != 'v1')

def generate(surface, request_models=None, model_names=None):
    spec = json.loads((ROOT/'schemas'/f'{surface}.json').read_text())
    schemas = dict(spec.get('components',{}).get('schemas',{}))
    # Site Manager uses inline schemas; promote those to named response/request views.
    for path, methods in spec['paths'].items():
        for method, op in methods.items():
            if method not in ('get','post','put','patch','delete'): continue
            op['operationId'] = operation_id(path, method, op)
            for status, response in op.get('responses',{}).items():
                schema = response.get('content',{}).get('application/json',{}).get('schema')
                if schema and '$ref' not in schema:
                    key = op['operationId']+' response '+status
                    schemas[key] = schema
                    response['content']['application/json']['schema'] = {'$ref':'#/components/schemas/'+key}
            body = op.get('requestBody',{}).get('content',{}).get('application/json',{})
            if body.get('schema') and '$ref' not in body['schema']:
                key = op['operationId']+' request'
                schemas[key] = body['schema']
                body['schema'] = {'$ref':'#/components/schemas/'+key}

    def flatten(schema):
        out = {}; required = set()
        if '$ref' in schema: return flatten(schemas[schema['$ref'].rsplit('/',1)[1]])
        for parent in schema.get('allOf',[]):
            props, req = flatten(parent); out.update(props); required.update(req)
        out.update(schema.get('properties',{})); required.update(schema.get('required',[]))
        return out, required

    # Promote nested inline objects to keep all declared fields typed.
    done=set()
    while len(done) < len(schemas):
        for name in list(schemas):
            if name in done: continue
            done.add(name)
            props, _ = flatten(schemas[name])
            for field, schema in props.items():
                value = schema.get('items',{}) if schema.get('type') == 'array' else schema
                if 'properties' in value and '$ref' not in value:
                    key = name+' '+field+' value'
                    if key not in schemas: schemas[key] = dict(value)
                    value.clear(); value['$ref']='#/components/schemas/'+key
    names={k:pascal(k) for k in schemas}
    used_names=set()
    for key,name in names.items():
        candidate=name
        while candidate in used_names: candidate += 'Model'
        names[key]=candidate;used_names.add(candidate)
    if model_names is not None:model_names.update(names)

    def ancestors(schema, seen=()):
        out=set()
        if '$ref' in schema:
            key=schema['$ref'].rsplit('/',1)[1]
            if key in seen:return out
            out.add(key);out.update(ancestors(schemas[key],(*seen,key)))
        for parent in schema.get('allOf',[]):out.update(ancestors(parent,seen))
        return out

    def inherited_tags(key):
        """Constrain inherited dispatch tags without recursively re-dispatching.

        Calling a parent's public validator would dispatch straight back into
        the concrete subtype. Check the mapped tag subset locally instead.
        """
        lineage=ancestors(schemas[key])|{key}
        guards=[]
        for parent in sorted(lineage-{key}):
            field,mapping,unmapped=wire_discriminator(schemas[parent])
            if not mapping:continue
            allowed=[]
            for tag,ref in mapping.items():
                target=ref.rsplit('/',1)[1]
                if target in lineage or key in ancestors(schemas[target]):allowed.append(tag)
            # An inherited wrapper not itself named by the dispatch map still
            # obeys the parent map, without claiming a particular subtype.
            allowed=(allowed or list(mapping))+unmapped
            guards.append('{ let tag=value.required('+quoted(field)+')?.string()?; if !('+ ' || '.join('tag.equals('+quoted(tag)+')' for tag in allowed)+') {return Err(JsonError::Type);} }\n')
        return ''.join(guards)

    def field_type(schema):
        if '$ref' in schema:
            return names[schema['$ref'].rsplit('/',1)[1]]+"<'a>", '::from_json'
        kind = schema.get('type')
        if kind == 'string': return "JsonString<'a>", '.string'
        if kind == 'boolean': return 'bool', '.boolean'
        if kind == 'integer': return 'i64', '.integer'
        if kind == 'number': return 'f64', '.number'
        if kind == 'array':
            if '$ref' in schema.get('items',{}):
                model = names[schema['items']['$ref'].rsplit('/',1)[1]]
                return f"Array<'a, {model}<'a>>", '::new'
            return "Items<'a>", '.items'
        return "Json<'a>", ''

    def convert(ty, conv, value):
        if conv.startswith('::'): return f'{ty.split("<")[0]}{conv}({value})'
        if conv: return f'{value}{conv}()'
        return f'Ok({value})'

    def validation(schema, expression='value'):
        """Generate wire-type and schema constraints; no runtime schema or allocator."""
        if schema is True or schema == {}: return ''
        if schema is False: return 'return Err(JsonError::Type);\n'
        module = surface.replace('-','_')
        if '$ref' in schema:
            target = names[schema['$ref'].rsplit('/',1)[1]]
            return f'crate::{module}::models::{target}::validate({expression})?;\n'
        out=['{\n'+('let value = '+expression+';\n' if expression!='value' else '')+'let _ = value;\n']
        kind=schema.get('type')
        nullable=allows_null(schema)
        if nullable: out.append('if !value.is_null() {\n')
        if isinstance(kind,list):
            kinds=[k for k in kind if k!='null']
            if not kinds:kind='null'
            elif len(kinds)==1:kind=kinds[0]
            elif len(kinds)>1:
                choices=[]
                for k in kinds:choices.append('{ let check = || -> Result<(),JsonError> {'+validation({**schema,'type':k,'nullable':False})+'Ok(()) }; check().is_ok() }')
                out.append('if !('+ ' || '.join(choices)+') { return Err(JsonError::Type); }\n');kind=None
        props,required=flatten(schema)
        if kind=='object' or kind is None and (props or required):
            out.append('value.object()?;\n')
            for field,child in props.items():
                if field in required:out.append('{ let field=value.required('+quoted(field)+')?; let _ = field;\n'+validation(child,'field')+'}\n')
                else:out.append('if let Some(field)=value.get('+quoted(field)+')? { let _ = field;\n'+validation(child,'field')+'}\n')
            for field in sorted(required-set(props)):out.append('value.required('+quoted(field)+')?;\n')
            additional=schema.get('additionalProperties')
            if additional is False or isinstance(additional,dict) and additional:
                keys=','.join(quoted(k) for k in props)
                out.append('for (name,field) in value.fields()? { let _ = field; if !(&['+keys+'] as &[&str]).iter().any(|key|name.equals(key)) {\n')
                out.append('return Err(JsonError::Type);\n' if additional is False else validation(additional,'field'))
                out.append('} }\n')
        elif kind=='array':
            out.append('let items=value.items()?; let _ = &items;\n')
            for key,op in [('minItems','<'),('maxItems','>')]:
                if key in schema and not (key=='minItems' and schema[key]==0):out.append(f'if items.clone().count() {op} {schema[key]} {{ return Err(JsonError::Type); }}\n')
            if schema.get('uniqueItems'):out.append('crate::validation::unique(value)?;\n')
            child=validation(schema.get('items',{}),'item')
            if child:out.append('for item in items {\n'+child+'}\n')
        elif kind=='string':
            out.append('let text=value.string()?; let _ = text;\n')
            for key,op in [('minLength','<'),('maxLength','>')]:
                if key in schema and not (key=='minLength' and schema[key]==0):out.append(f'if text.chars().count() {op} {schema[key]} {{ return Err(JsonError::Type); }}\n')
            pattern=schema.get('pattern')
            if pattern=='^[A-Z]{3}$':out.append('if text.chars().count()!=3 || !text.chars().all(|c|c.is_ascii_uppercase()) {return Err(JsonError::Type);}\n')
            elif pattern=='^[0-9a-fA-F]+$':out.append('if text.chars().next().is_none() || !text.chars().all(|c|c.is_ascii_hexdigit()) {return Err(JsonError::Type);}\n')
            elif pattern:raise ValueError('unsupported pattern: '+pattern)
        elif kind in ('integer','number'):
            conversion='integer' if kind=='integer' else 'number'
            out.append(f'let number=value.{conversion}()?; let _ = number;\n')
            if kind=='number':out.append('if !number.is_finite() { return Err(JsonError::Type); }\n')
            rusttype='i64' if kind=='integer' else 'f64'
            for key,op in [('minimum','<'),('maximum','>'),('exclusiveMinimum','<='),('exclusiveMaximum','>=')]:
                if key not in schema:continue
                bound=schema[key]
                if isinstance(bound,bool):
                    base='minimum' if key=='exclusiveMinimum' else 'maximum'
                    if not bound or base not in schema:continue
                    bound=schema[base]
                if kind=='integer':bound=math.ceil(bound) if key in ('minimum','exclusiveMaximum') else math.floor(bound)
                out.append(f'if number {op} {bound}_{rusttype} {{ return Err(JsonError::Type); }}\n')
        elif kind=='boolean':out.append('value.boolean()?;\n')
        elif kind=='null':out.append('if !value.is_null() {return Err(JsonError::Type);}\n')
        # JSON Schema bounds also apply beside allOf/$ref wrappers that omit
        # their own type. Other JSON types are unaffected by numeric keywords.
        if kind not in ('integer','number') and any(key in schema for key in ('minimum','maximum','exclusiveMinimum','exclusiveMaximum')):
            out.append('if let Ok(number)=value.number() {\nif !number.is_finite() {return Err(JsonError::Type);}\n')
            for key,op in [('minimum','<'),('maximum','>'),('exclusiveMinimum','<='),('exclusiveMaximum','>=')]:
                if key not in schema:continue
                bound=schema[key]
                if isinstance(bound,bool):
                    base='minimum' if key=='exclusiveMinimum' else 'maximum'
                    if not bound or base not in schema:continue
                    bound=schema[base]
                out.append(f'if number {op} {bound}_f64 {{return Err(JsonError::Type);}}\n')
            out.append('}\n')
        if not props and not required:
            for part in schema.get('allOf',[]):out.append(validation(part))
        discriminator_name,mapping,unmapped_tags=wire_discriminator(schema)
        if mapping:
            out.append('let tag=value.required('+quoted(discriminator_name)+')?.string()?;\n')
            grouped={}
            for tag,ref in mapping.items():grouped.setdefault(ref,[]).append(tag)
            for i,(ref,tags) in enumerate(grouped.items()):
                out.append(('if' if i==0 else 'else if')+' '+ ' || '.join('tag.equals('+quoted(tag)+')' for tag in tags)+' {\n'+validation({'$ref':ref})+'}\n')
            if unmapped_tags:out.append('else if '+ ' || '.join('tag.equals('+quoted(tag)+')' for tag in unmapped_tags)+' {}\n')
            out.append('else {return Err(JsonError::Type);}\n')
        else:
            for key in ['oneOf','anyOf']:
                choices=schema.get(key)
                if choices:
                    out.append('let mut matches=0usize;\n')
                    for child in choices:out.append('{ let variant = || -> Result<(),JsonError> {\n'+validation(child)+'Ok(()) }; if variant().is_ok() {matches+=1;} }\n')
                    out.append('if '+('matches!=1' if key=='oneOf' else 'matches==0')+' {return Err(JsonError::Type);}\n')
        if nullable:out.append('}\n')
        enum=schema.get('enum')
        # Network documents some integer enums as quoted decimal values while
        # its examples and declared wire type use JSON numbers.
        if schema.get('type')=='integer' and enum and all(isinstance(v,str) and re.fullmatch(r'-?[0-9]+',v) for v in enum):
            enum=[int(v) for v in enum]
        malformed_string_enum=schema.get('type')=='string' and enum is not None and any(isinstance(v,list) for v in enum)
        if enum is not None and not malformed_string_enum:
            checks=['crate::validation::equivalent(value,Json::parse(b'+json.dumps(json.dumps(v,ensure_ascii=True,separators=(',',':')))+')?)' for v in enum]
            out.append('if !('+(' || '.join(checks) or 'false')+') {return Err(JsonError::Type); }\n')
        if 'const' in schema:
            literal=json.dumps(json.dumps(schema['const'],ensure_ascii=True,separators=(',',':')))
            out.append('if !crate::validation::equivalent(value,Json::parse(b'+literal+')?) {return Err(JsonError::Type); }\n')
        out.append('}\n');return ''.join(out)

    out=['// @generated by tools/generate.py; edit the generator, not this file.\n',
         '#![allow(unused_imports)]\n',
         'use crate::{BuildError, Call, Method, Operation, QueryParameter};\n',
         'use crate::json::{Array, Items, Json, JsonError, JsonString, View};\n']
    out.append('pub mod models {\nuse super::*;\n')
    for key, schema in schemas.items():
        name=names[key]; props, required = flatten(schema)
        out.append(doc(key + '. ' + schema.get('description','')))
        if allows_null(schema): out.append(doc('This view permits JSON null; check json().is_null() before reading object fields.'))
        out.append(f"#[derive(Clone, Copy, Debug)]\npub struct {name}<'a>(Json<'a>);\n")
        out.append(f"impl<'a> View<'a> for {name}<'a> {{ fn from_json(value: Json<'a>) -> Result<Self, JsonError> {{ ")
        if props or schema.get('type')=='object':
            out.append('if !value.is_null() { value.object()?; } ' if allows_null(schema) else 'value.object()?; ')
        out.append('Ok(Self(value)) } }\n')
        out.append(f"impl<'a> {name}<'a> {{\npub fn from_json(value: Json<'a>) -> Result<Self, JsonError> {{ <Self as View>::from_json(value) }}\npub fn json(self) -> Json<'a> {{ self.0 }}\n")
        out.append(doc('Validate wire types, nested fields, variants, enum/range and container constraints. Format annotations are not validated.'))
        out.append("pub fn validate(value: Json<'_>) -> Result<(), JsonError> {\nlet _ = value;\n"+validation(schema)+inherited_tags(key)+'Ok(())\n}\n')
        used={'json','from_json','validate'}
        for field, value in props.items():
            method=ident(field)
            if method in used: method += '_field'
            if method in used: raise ValueError('field collision')
            used.add(method)
            ty, conv=field_type(value)
            description=value.get('description',field)
            if 'enum' in value: description+=' Values: '+', '.join(str(x) for x in value['enum'])+'.'
            out.append(doc(description))
            if field in required:
                out.append(f'pub fn {method}(self) -> Result<{ty}, JsonError> {{\nlet value = self.0.required({quoted(field)})?;\n{convert(ty,conv,"value")}\n}}\n')
            else:
                if not conv:
                    expression=f'self.0.optional({quoted(field)})'
                else:
                    callback=(ty.split('<')[0]+conv) if conv.startswith('::') else ('Json::'+conv[1:])
                    expression=f'self.0.optional({quoted(field)})?.map({callback}).transpose()'
                out.append(f'pub fn {method}(self) -> Result<Option<{ty}>, JsonError> {{\n{expression}\n}}\n')
        if props:
            out.append(f"pub fn builder(buffer: &'a mut [u8]) -> Result<builders::{name}<'a>, JsonError> {{ builders::{name}::new(buffer) }}\n")
        out.append('}\n')
    out.append('pub mod builders {\nuse crate::json::{Json, JsonError, ObjectWriter};\n')
    for key,schema in schemas.items():
        props,required=flatten(schema)
        if not props: continue
        if len(props)>128: raise ValueError('too many properties for required field mask')
        name=names[key]
        mask=sum(1<<i for i,field in enumerate(props) if field in required)
        out.append(doc('Allocation-free '+key+' body builder. Checks required fields, nested wire shapes, enum/range and container constraints.'))
        out.append(f"pub struct {name}<'a> {{ writer: ObjectWriter<'a>, present: u128 }}\nimpl<'a> {name}<'a> {{\n")
        out.append("pub fn new(buffer: &'a mut [u8]) -> Result<Self, JsonError> { Ok(Self { writer: ObjectWriter::new(buffer)?, present: 0 }) }\n")
        used={'new','finish','extra_field'}
        for i,(field,value) in enumerate(props.items()):
            method=ident(field)
            while method in used: method += '_field'
            used.add(method)
            kind=value.get('type')
            param={'string':'&str','boolean':'bool','integer':'i64','number':'f64'}.get(kind if isinstance(kind,str) else None,"Json<'_>")
            out.append(doc(value.get('description',field)))
            out.append(f'pub fn {method}(&mut self, value: {param}) -> Result<&mut Self, JsonError> {{\n')
            if kind=='string':
                if value.get('enum') and all(isinstance(v,str) for v in value['enum']): out.append('if !['+','.join(quoted(v) for v in value['enum'])+'].contains(&value) { return Err(JsonError::Type); }\n')
                if 'minLength' in value: out.append(f"if value.chars().count() < {value['minLength']} {{ return Err(JsonError::Type); }}\n")
                if 'maxLength' in value: out.append(f"if value.chars().count() > {value['maxLength']} {{ return Err(JsonError::Type); }}\n")
            if kind in ('integer','number'):
                if kind=='number': out.append('if !value.is_finite() { return Err(JsonError::Type); }\n')
                if 'minimum' in value:out.append(f"if value < {value['minimum']}_{param} {{ return Err(JsonError::Type); }}\n")
                if 'maximum' in value:out.append(f"if value > {value['maximum']}_{param} {{ return Err(JsonError::Type); }}\n")
            if param=="Json<'_>":out.append(validation(value)+f'self.writer.raw_field({quoted(field)}, value)?;\n')
            else:out.append(f'self.writer.field({quoted(field)}, &value)?;\n')
            out.append(f'self.present |= 1u128 << {i}; Ok(self)\n}}\n')
        out.append(doc('Add an extension/variant field or an explicit nullable value. The complete model is validated by finish.'))
        out.append("pub fn extra_field(&mut self, name: &str, value: Json<'_>) -> Result<&mut Self, JsonError> {\nlet bit = match name {\n")
        for i,field in enumerate(props):out.append(quoted(field)+f' => 1u128 << {i},\n')
        out.append('_ => 0, }; self.writer.raw_field(name,value)?; self.present |= bit; Ok(self)\n}\n')
        check = f'if self.present & {mask}u128 != {mask}u128 {{ return Err(JsonError::Missing); }} ' if mask else ''
        out.append(f"pub fn finish(self) -> Result<&'a [u8], JsonError> {{ {check}let bytes=self.writer.finish()?; super::{name}::validate(Json::parse(bytes)?)?; Ok(bytes) }}\n}}\n")
    out.append('}\n')
    out.append('}\n')
    ops=[]
    for path, methods in spec['paths'].items():
        for method, op in methods.items():
            if method not in ('get','post','put','patch','delete'):continue
            id=op['operationId']; constant=snake(id).upper(); method_name=ident(id)
            query=op.get('parameters',[])
            query=[x for x in query if x['in']=='query']
            request=op.get('requestBody',{}).get('content',{}).get('application/json',{}).get('schema',{}).get('$ref')
            if request and request_models is not None:
                request_models[(path,method)]=names[request.rsplit('/',1)[1]]
            responses=op.get('responses',{})
            response=next((v.get('content',{}).get('application/json',{}).get('schema',{}).get('$ref') for k,v in responses.items() if k.startswith('2')),None)
            out.append(f'pub static {constant}: Operation = Operation {{ id: {quoted(id)}, method: Method::{method.upper()}, path: {quoted(path)}, summary: {quoted(op.get("summary",""))}, body_required: {str(op.get("requestBody",{}).get("required",False)).lower()}, query: &[\n')
            for p in query:out.append(f'QueryParameter {{ name: {quoted(p["name"])}, required: {str(p.get("required",False)).lower()} }},\n')
            out.append('], request_schema: '+ ('Some('+quoted(request.rsplit('/',1)[1])+')' if request else 'None')+', response_schema: '+('Some('+quoted(response.rsplit('/',1)[1])+')' if response else 'None')+' };\n')
            path_params=re.findall(r'\{([^}]+)\}',path)
            if '*' in path:path_params.append(path.split('*')[1])
            args=', '.join(ident(p)+': &str' for p in path_params)
            bindings=', '.join(f'({quoted(p)}, {ident(p)})' for p in path_params)
            out.append(doc(op.get('summary',id)+'. '+method.upper()+' '+path))
            if request:out.append(doc('Request schema: models::'+names[request.rsplit('/',1)[1]]+'.'))
            if response:out.append(doc('Response schema: models::'+names[response.rsplit('/',1)[1]]+'.'))
            out.append(f'pub fn {method_name}({args}) -> Result<Call, BuildError> {{ {constant}.bind(&[{bindings}]) }}\n')
            ops.append(constant)
    out.append('pub static OPERATIONS: &[&Operation] = &[\n'+',\n'.join('&'+x for x in ops)+'\n];\n')
    result=subprocess.run(['rustfmt','--edition','2024'],input=''.join(out),text=True,capture_output=True)
    if result.returncode: raise ValueError(result.stderr)
    formatted=result.stdout
    return formatted,len(ops),len(schemas)

def main():
    surfaces=('network','site-manager','protect','session-api')
    parser=argparse.ArgumentParser();parser.add_argument('--check',action='store_true')
    parser.add_argument('--surface',action='append',choices=surfaces,help='Generate an explicit API surface; defaults to checked-in modules')
    args=parser.parse_args()
    selected=args.surface or [surface for surface in surfaces if (ROOT/'src'/(surface.replace('-','_')+'.rs')).exists()]
    if not selected:parser.error('select a surface with --surface')
    for surface in selected:
        content,ops,models=generate(surface)
        path=ROOT/'src'/(surface.replace('-','_')+'.rs')
        if args.check:
            if not path.exists() or path.read_text()!=content:raise SystemExit(f'{path} is stale; run python3 tools/generate.py')
        else:path.write_text(content)
        print(f'{surface}: {ops} operations, {models} model views')
if __name__=='__main__':main()
