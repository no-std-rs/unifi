#!/usr/bin/env python3
"""Cross-check SDK request validators against independent JSON Schema fixtures.

Development only: jsonschema is never a Rust/runtime dependency. Samples contain
synthetic identifiers and addresses. Pinned upstream documents remain unchanged.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import subprocess

import jsonschema

from mock_server import normalized, sample

ROOT = Path(__file__).resolve().parents[1]
loader = importlib.util.spec_from_file_location('generate', ROOT / 'tools/generate.py')
generator = importlib.util.module_from_spec(loader)
loader.loader.exec_module(generator)


def resolve(spec, schema):
    while '$ref' in schema:
        schema = spec['components']['schemas'][schema['$ref'].rsplit('/', 1)[1]]
    return schema


def validator(spec, schema, value):
    root = resolve(spec, schema)
    mapping = root.get('discriminator', {}).get('mapping', {})
    if mapping:
        tag = value.get(root['discriminator']['propertyName']) if isinstance(value, dict) else None
        if not isinstance(tag, str) or tag not in mapping:
            return None
        schema = {'allOf': [schema, {'$ref': mapping[tag]}]}
    document = {**spec, 'x-request-fixture': schema, '$ref': '#/x-request-fixture'}
    return jsonschema.Draft202012Validator(document)


def generate():
    rows = []
    operations = 0
    for surface in ('network', 'site-manager', 'protect'):
        spec = normalized(json.loads((ROOT / 'schemas' / (surface + '.json')).read_text()))
        models = {}
        names = {}
        generator.generate(surface, models, names)
        for path, methods in spec['paths'].items():
            for method, operation in methods.items():
                if (path, method) not in models:
                    continue
                operations += 1
                schema = operation['requestBody']['content']['application/json']['schema']
                root = resolve(spec, schema)
                variants = list(root.get('discriminator', {}).get('mapping', {})) or [None]
                seen = set()

                def add(value, expected, label, model=None, shape=None):
                    encoded = json.dumps(value, ensure_ascii=True, separators=(',', ':'))
                    key = encoded, expected, model
                    if key in seen:
                        return
                    seen.add(key)
                    check = validator(spec, shape or schema, value)
                    actual = check is not None and check.is_valid(value)
                    if actual != expected:
                        errors = [] if check is None else [e.message for e in check.iter_errors(value)]
                        raise ValueError(f'{surface} {method} {path} {label}: expected {expected}, schema says {actual}: {errors[:3]}')
                    rows.append((f'{surface} {method} {path} {label}', surface.replace('-', '_'), model or models[path, method], encoded, expected))

                for variant in variants:
                    for optional in (False, True):
                        value = sample(spec, schema, variant=variant, include_optional=optional)
                        label = f'{variant or "default"} {"all" if optional else "required"}'
                        add(value, True, label)
                        if variant is not None:
                            discriminator = root['discriminator']
                            ref = discriminator['mapping'][variant]
                            concrete = names[ref.rsplit('/', 1)[1]]
                            tags = [tag for tag, target in discriminator['mapping'].items() if target == ref]
                            tag_field = discriminator['propertyName']
                            derived = {'allOf': [schema, {'$ref': ref}, {'required': [tag_field], 'properties': {tag_field: {'enum': tags}}}]}
                            add(value, True, label + ' concrete model', concrete, derived)
                            for wrong in [*discriminator['mapping'], '__invalid_discriminator__']:
                                if wrong not in tags:
                                    add({**value, tag_field: wrong}, False, label + ' concrete wrong tag', concrete, derived)
                        # Mutations are kept only when an independent validator
                        # proves them invalid; no duplicated SDK validation logic.
                        candidates = [('null', None), ('array', []), ('scalar', 42)]
                        if isinstance(value, dict):
                            for key in value:
                                removed = {k: v for k, v in value.items() if k != key}
                                candidates.append((f'missing {key}', removed))
                                for replacement in (None, False, -1, 0, 42, [], {}, '__invalid_enum__'):
                                    changed = {**value, key: replacement}
                                    candidates.append((f'invalid {key}', changed))
                        for suffix, changed in candidates:
                            check = validator(spec, schema, changed)
                            if check is None or not check.is_valid(changed):
                                add(changed, False, label + ' ' + suffix)

    out = ['// @generated by tools/generate-request-fixtures.py; synthetic data only.\n',
           'use unifi::{Json, json::JsonError};\n',
           'type Validator = fn(Json<\'_>) -> Result<(), JsonError>;\n',
           '#[test]\nfn official_request_schema_corpus() {\n',
           'let fixtures: &[(&str, Validator, &[u8], bool)] = &[\n']
    for label, module, model, encoded, expected in rows:
        out.append(f'({json.dumps(label)}, unifi::{module}::models::{model}::validate, b{json.dumps(encoded)}, {str(expected).lower()}),\n')
    out.append('];\nfor (label, validate, bytes, expected) in fixtures {\nlet result=validate(Json::parse(bytes).unwrap());\nassert_eq!(result.is_ok(), *expected, "{label}: {result:?}");\n}\n}\n')
    result = subprocess.run(['rustfmt', '--edition', '2024'], input=''.join(out), text=True, capture_output=True, check=True)
    return result.stdout, operations, sum(row[-1] for row in rows), len(rows)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    content, operations, positive, total = generate()
    path = ROOT / 'tests/request_fixtures.rs'
    if args.check:
        if not path.exists() or path.read_text() != content:
            raise SystemExit('Request corpus is stale; run python3 tools/generate-request-fixtures.py')
    else:
        path.write_text(content)
    print(f'{operations} JSON request operations: {positive} valid and {total-positive} invalid schema fixtures')


if __name__ == '__main__':
    main()
