// Dependency-free validator for the explicitly used JSON Schema 2020-12 subset.
// Reject unknown keywords so additions cannot silently weaken conformance.
import assert from 'node:assert/strict';

const plain = x => x !== null && typeof x === 'object' && !Array.isArray(x);
export const canonical = x => Array.isArray(x) ? `[${x.map(canonical).join(',')}]`
  : plain(x) ? `{${Object.keys(x).sort().map(k => `${JSON.stringify(k)}:${canonical(x[k])}`).join(',')}}`
  : JSON.stringify(x);
const keywords = new Set(['$schema', '$id', '$defs', '$ref', 'title', 'description',
  'type', 'const', 'enum', 'oneOf', 'anyOf', 'allOf', 'properties', 'required',
  'additionalProperties', 'items', 'minItems', 'maxItems', 'uniqueItems', 'minLength',
  'maxLength', 'pattern', 'format', 'minimum', 'maximum']);

export function validateSchema(schema, value) {
  const resolve = ref => {
    assert.ok(ref.startsWith('#/'), 'external schema references unsupported');
    const node = ref.slice(2).split('/').reduce((v, k) => v?.[k.replaceAll('~1', '/').replaceAll('~0', '~')], schema);
    assert.ok(node !== undefined, `unresolved schema reference ${ref}`);
    return node;
  };
  function inspect(node) {
    if (typeof node === 'boolean') return;
    assert.ok(plain(node), 'schema must be an object');
    for (const key of Object.keys(node)) assert.ok(keywords.has(key), `unsupported keyword ${key}`);
    if (node.$ref) { resolve(node.$ref); assert.deepEqual(Object.keys(node), ['$ref']); }
    if (node.type) assert.ok(['object', 'array', 'string', 'integer', 'number', 'boolean', 'null'].includes(node.type));
    if (node.format) assert.equal(node.format, 'date-time');
    if (node.additionalProperties !== undefined) assert.equal(node.additionalProperties, false);
    for (const child of [...Object.values(node.$defs ?? {}), ...Object.values(node.properties ?? {}),
      ...(node.oneOf ?? []), ...(node.anyOf ?? []), ...(node.allOf ?? []), ...(node.items ? [node.items] : [])]) inspect(child);
  }
  inspect(schema);
  function check(node, x, at) {
    if (node === true) return [];
    if (node === false) return [`${at}: forbidden`];
    if (node.$ref) return check(resolve(node.$ref), x, at);
    const errors = [];
    if (node.oneOf && node.oneOf.filter(n => check(n, x, at).length === 0).length !== 1) errors.push(`${at}: oneOf`);
    if (node.anyOf && !node.anyOf.some(n => check(n, x, at).length === 0)) errors.push(`${at}: anyOf`);
    for (const n of node.allOf ?? []) errors.push(...check(n, x, at));
    if ('const' in node && canonical(x) !== canonical(node.const)) errors.push(`${at}: const`);
    if (node.enum && !node.enum.some(v => canonical(v) === canonical(x))) errors.push(`${at}: enum`);
    const matches = { object: plain(x), array: Array.isArray(x), string: typeof x === 'string',
      integer: Number.isSafeInteger(x), number: typeof x === 'number' && Number.isFinite(x),
      boolean: typeof x === 'boolean', null: x === null };
    if (node.type && !matches[node.type]) return [...errors, `${at}: type ${node.type}`];
    if (typeof x === 'string') {
      const length = [...x].length;
      if (node.minLength !== undefined && length < node.minLength) errors.push(`${at}: minLength`);
      if (node.maxLength !== undefined && length > node.maxLength) errors.push(`${at}: maxLength`);
      if (node.pattern && !new RegExp(node.pattern, 'u').test(x)) errors.push(`${at}: pattern`);
      if (node.format === 'date-time' && (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(x)
        || !Number.isFinite(Date.parse(x)) || new Date(x).toISOString().replace('.000Z', 'Z') !== x)) errors.push(`${at}: date-time`);
    }
    if (typeof x === 'number') {
      if (node.minimum !== undefined && x < node.minimum) errors.push(`${at}: minimum`);
      if (node.maximum !== undefined && x > node.maximum) errors.push(`${at}: maximum`);
    }
    if (Array.isArray(x)) {
      if (node.minItems !== undefined && x.length < node.minItems) errors.push(`${at}: minItems`);
      if (node.maxItems !== undefined && x.length > node.maxItems) errors.push(`${at}: maxItems`);
      if (node.uniqueItems && new Set(x.map(canonical)).size !== x.length) errors.push(`${at}: uniqueItems`);
      if (node.items) x.forEach((v, i) => errors.push(...check(node.items, v, `${at}[${i}]`)));
    }
    if (plain(x)) {
      for (const key of node.required ?? []) if (!Object.hasOwn(x, key)) errors.push(`${at}.${key}: required`);
      for (const [key, val] of Object.entries(x)) {
        if (Object.hasOwn(node.properties ?? {}, key)) errors.push(...check(node.properties[key], val, `${at}.${key}`));
        else if (node.additionalProperties === false) errors.push(`${at}.${key}: unknown field`);
      }
    }
    return errors;
  }
  return check(schema, value, '$');
}
