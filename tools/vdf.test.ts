import { describe, expect, test } from 'bun:test';
import { parseVdf, type VdfTable, type VdfValue, VdfError, vdfString, vdfTable } from './vdf.ts';

/** Every table under a root, the root first. */
function tables(root: VdfTable): VdfTable[] {
  const found: VdfTable[] = [root];
  for (const entry of Object.values(root)) {
    for (const value of Array.isArray(entry) ? entry : [entry]) {
      if (typeof value === 'object') {
        found.push(...tables(value));
      }
    }
  }
  return found;
}

describe('parseVdf, the grammar', () => {
  test.each<[string, string, unknown]>([
    ['a quoted pair', '"a" "b"', { a: 'b' }],
    ['bare tokens', 'key value', { key: 'value' }],
    ['a bare value with dots, a colon and a slash', 'ver 1.2:3/x', { ver: '1.2:3/x' }],
    ['a bare value that starts with one slash', 'path /usr/bin', { path: '/usr/bin' }],
    ['a nested table', '"a"\n{\n\t"b"\t\t"c"\n}\n', { a: { b: 'c' } }],
    ['braces on the key line', '"a" { "b" "c" }', { a: { b: 'c' } }],
    ['no whitespace between tokens', '"a"{"b""c""d"{"e""f"}}', { a: { b: 'c', d: { e: 'f' } } }],
    ['an empty table', '"a" {}', { a: {} }],
    ['an empty value', '"a" ""', { a: '' }],
    ['an empty key', '"" "b"', { '': 'b' }],
    ['several top-level pairs', '"a" "1"\n"b" { }\n"c" "3"', { a: '1', b: {}, c: '3' }],
    ['comments on their own line and after a pair', '// top\n"a" "b" // after\n// end', { a: 'b' }],
    ['a comment where a key would start', '"a" { // opens\n"b" "c" }', { a: { b: 'c' } }],
    ['a comment marker inside quotes is text', '"url" "http://x//y"', { url: 'http://x//y' }],
    ['a comment marker inside a bare token is text', 'url http://x', { url: 'http://x' }],
    ['CRLF line endings', '"a"\r\n{\r\n\t"b"\t"c"\r\n}\r\n', { a: { b: 'c' } }],
    ['a byte order mark', `${String.fromCodePoint(0xfeff)}"a" "b"`, { a: 'b' }],
    ['a quoted value across lines', '"a" "one\ntwo"', { a: 'one\ntwo' }],
    ['every ASCII space between tokens', '"a"\t\v\f "b"', { a: 'b' }],
    ['an empty document', '', {}],
    ['a document of comments and whitespace', '  // nothing\n\t\n', {}],
  ])('%s', (_name, text, expected) => {
    expect(parseVdf(text)).toEqual(expected as VdfTable);
  });
});

describe('parseVdf, values', () => {
  /**
   * A manifest gid is up to twenty digits and passes Number.MAX_SAFE_INTEGER,
   * so a number would round it (upstream vdf-parser issue 6).
   */
  test.each(['18446744073709551615', '007', '1.50', '-3', 'true', 'FALSE'])('%s stays the string it was', (value) => {
    expect(parseVdf(`"v" "${value}"`)['v']).toBe(value);
  });

  test.each<[string, string, VdfValue[]]>([
    ['two strings', '"k" "1"\n"k" "2"', ['1', '2']],
    ['three strings, in order', '"k" "1" "k" "2" "k" "3"', ['1', '2', '3']],
    ['two tables', '"k" { "a" "1" } "k" { "b" "2" }', [{ a: '1' }, { b: '2' }]],
    ['a string then a table', '"k" "1" "k" { "b" "2" }', ['1', { b: '2' }]],
  ])('a key seen twice holds %s', (_name, text, expected) => {
    expect(parseVdf(text)['k']).toEqual(expected);
  });

  test('the same key in two tables is two keys', () => {
    expect(parseVdf('"a" { "k" "1" } "b" { "k" "2" }')).toEqual({ a: { k: '1' }, b: { k: '2' } });
  });

  test('a key keeps its case', () => {
    expect(parseVdf('"Key" "1" "key" "2"')).toEqual({ Key: '1', key: '2' });
  });
});

describe('parseVdf, escapes', () => {
  // Each input is the file's text, so a backslash in it is doubled once here.
  test.each<[string, string, string]>([
    ['a Windows path', '"p" "C:\\\\Program Files\\\\Steam"', 'C:\\Program Files\\Steam'],
    // Upstream vdf-parser pull request 3: the escaped backslash ends the value.
    ['a value ending in an escaped backslash', '"p" "/Hexalitos\\\\"', '/Hexalitos\\'],
    ['an escaped quote', '"p" "say \\"hi\\""', 'say "hi"'],
    ['a newline and a tab', '"p" "a\\nb\\tc"', 'a\nb\tc'],
    ['a backslash no escape names', '"p" "\\d\\q"', '\\d\\q'],
    ['a doubled backslash before a letter', '"p" "\\\\n"', '\\n'],
    ['text on both sides of each escape', '"p" "ab\\\\cd\\"ef\\ngh"', 'ab\\cd"ef\ngh'],
  ])('%s decodes', (_name, text, expected) => {
    expect(parseVdf(text)['p']).toBe(expected);
  });

  test('an escaped quote in a key', () => {
    expect(Object.keys(parseVdf('"a\\"b" "c"'))).toEqual(['a"b']);
  });

  test('the pairs after a value ending in an escaped backslash still parse', () => {
    const text = ['"friends"', '{', '  "1" "/Hexalitos\\\\"', '  "2" "put in"', '}'].join('\n');
    expect(parseVdf(text)).toEqual({ friends: { '1': '/Hexalitos\\', '2': 'put in' } });
  });
});

describe('parseVdf, text in any script', () => {
  // Upstream vdf-parser issue 1 reported a localconfig.vdf name it choked on.
  test.each([
    'DMR ALİ',
    '日本語の名前',
    'emoji 🎮 in a name',
    `no${String.fromCodePoint(0x00a0)}break space`,
    `a line${String.fromCodePoint(0x2028)}separator`,
    'e\u0301 combining',
  ])('%s survives as a key and a value', (name) => {
    const root = parseVdf(`"${name}" "${name}"`);
    expect(Object.keys(root)).toEqual([name]);
    expect(root[name]).toBe(name);
  });
});

describe('parseVdf, conditionals', () => {
  // Upstream vdf-parser issue 2: a TF2 localization file uses [$!ENGLISH].
  test.each([
    ['after a value', '"a" "b" [$WIN32]', { a: 'b' }],
    ['negated before the dollar', '"a" "b" [!$X360]', { a: 'b' }],
    ['negated after the dollar', '"a" "b" [$!ENGLISH]', { a: 'b' }],
    ['joined with or', '"a" "b" [$WIN32||$POSIX]', { a: 'b' }],
    ['joined with and, spaced', '"a" "b" [ $WIN32 && !$X360 ]', { a: 'b' }],
    ['between a key and its table', '"a" [$WIN32] { "b" "c" }', { a: { b: 'c' } }],
    ['between a key and its value', '"a" [$WIN32] "b"', { a: 'b' }],
    ['after a closing brace', '"a" { "b" "c" } [$WIN32]\n"d" "e"', { a: { b: 'c' }, d: 'e' }],
  ])('%s is read and its pair kept', (_name, text, expected) => {
    expect(parseVdf(text)).toEqual(expected as VdfTable);
  });
});

describe('parseVdf, refusals', () => {
  test.each([
    // Upstream vdf-parser issue 7: shortcuts.vdf is binary KeyValues.
    [
      'binary KeyValues',
      '\0shortcuts\0\x010\0',
      'binary KeyValues, which this reader does not parse, at line 1, column 1',
    ],
    ['a quote that never closes', '"a" "b', 'a quote opened here never closes, at line 1, column 5'],
    ['a backslash before the end', '"a" "b\\', 'a quote opened here never closes, at line 1, column 5'],
    ['a table that never closes', '"a"\n{\n"b" "c"', 'a table opened here never closes, at line 2, column 1'],
    ['a closing brace with nothing open', '"a" "b"\n}', 'a closing brace has no table to close, at line 2, column 1'],
    ['a key at the end of the input', '"a" "b"\n"c"', 'the key "c" has no value, at line 2, column 1'],
    ['a key before a closing brace', '"t" { "a" }', 'the key "a" has no value, at line 1, column 7'],
    ['an opening brace first', '{ "a" "b" }', 'an opening brace has no key before it, at line 1, column 1'],
    ['a conditional first', '[$WIN32] "a" "b"', 'a conditional has no key or value before it, at line 1, column 1'],
    [
      'two conditionals in a row',
      '"a" "b" [$X] [$Y]',
      'a conditional has no key or value before it, at line 1, column 14',
    ],
    [
      'a conditional that never closes',
      '"a" "b" [$WIN32\n"c" "d"',
      'a conditional opened here never closes on its line',
    ],
    [
      'a conditional whose bracket closes on a later line',
      '"a" "b" [$WIN32\n"c" "d" [$X]',
      'a conditional opened here never closes on its line, at line 1, column 9',
    ],
    ['a conditional with no dollar', '"a" "b" [WIN32]', 'the conditional "WIN32" is not one this reader knows'],
    ['an empty conditional', '"a" "b" []', 'the conditional "" is not one this reader knows'],
    ['a closing bracket with nothing open', '"a" "b" ]', 'a closing bracket has no conditional to close'],
  ])('%s is refused with where it is', (_name, text, message) => {
    expect(() => parseVdf(text)).toThrow(VdfError);
    expect(() => parseVdf(text)).toThrow(message);
  });

  /**
   * A refusal reaches a CI log, where a line starting `::` is a workflow
   * command, so what the input carried is quoted and never printed raw.
   */
  test('a control character in a refused conditional reaches the message escaped', () => {
    const text = '"a" "b" [$X\r::notice title=probe::line\r\u001b[31m_]';
    let message = '';
    try {
      parseVdf(text);
    } catch (error) {
      message = (error as Error).message;
    }
    expect(message).toContain('is not one this reader knows');
    expect(message).not.toMatch(/[\r\u001b]/);
    expect(message).toContain('\\r::notice');
  });
});

describe('parseVdf, keys that name a property of Object', () => {
  test.each([
    ['a __proto__ table', '"__proto__" { "pwned" "yes" }', '__proto__'],
    ['a __proto__ string', '"__proto__" "pwned"', '__proto__'],
    ['a repeated __proto__ table', '"__proto__" { "a" "b" } "__proto__" { "pwned" "yes" }', '__proto__'],
    ['a constructor with a prototype', '"constructor" { "prototype" { "pwned" "yes" } }', 'constructor'],
    ['a toString value', '"toString" "pwned"', 'toString'],
    ['a hasOwnProperty table', '"hasOwnProperty" { "pwned" "yes" }', 'hasOwnProperty'],
  ])('%s is data, and Object.prototype is left alone', (_name, text, key) => {
    const root = parseVdf(text);
    expect(({} as Record<string, unknown>)['pwned'], 'a key wrote onto Object.prototype').toBeUndefined();
    expect(Object.hasOwn(Object.prototype, 'pwned')).toBe(false);
    expect(Object.hasOwn(root, key), `the ${key} key was not kept as data`).toBe(true);
  });

  test('the same keys one table down are data too', () => {
    const root = parseVdf('"a" { "__proto__" { "pwned" "yes" } "constructor" { "prototype" { "pwned" "yes" } } }');
    expect(({} as Record<string, unknown>)['pwned']).toBeUndefined();
    const inner = vdfTable(root['a']);
    expect(vdfTable(inner['__proto__'])['pwned']).toBe('yes');
    expect(vdfTable(vdfTable(inner['constructor'])['prototype'])['pwned']).toBe('yes');
  });

  test('every table has a null prototype', () => {
    const root = parseVdf('"a" { "b" { "c" "d" } "e" { } "e" { "f" "g" } }');
    const all = tables(root);
    expect(all.length).toBe(5);
    for (const table of all) {
      expect(Object.getPrototypeOf(table)).toBeNull();
    }
  });
});

describe('parseVdf, depth', () => {
  /**
   * A hostile file nests without end. A recursive reader under Bun overflows
   * its stack well short of this depth, measured at 50,000.
   */
  test('a hundred thousand nested tables parse', () => {
    const depth = 100_000;
    const text = `${'"k" {'.repeat(depth)}"leaf" "yes"${'}'.repeat(depth)}`;
    let table = parseVdf(text);
    for (let level = 0; level < depth; level += 1) {
      table = vdfTable(table['k']);
    }
    expect(table['leaf']).toBe('yes');
  });
});

describe('vdfTable and vdfString', () => {
  const root = parseVdf('"t" { "a" "b" } "s" "text" "r" "1" "r" "2"');

  test('a table entry is the table itself', () => {
    expect(vdfTable(root['t'])).toBe(root['t'] as VdfTable);
  });

  test.each([
    ['a string', 's'],
    ['a repeated key', 'r'],
    ['a missing key', 'missing'],
  ])('%s reads as an empty table with a null prototype', (_name, key) => {
    const table = vdfTable(root[key]);
    expect(Object.keys(table)).toEqual([]);
    expect(Object.getPrototypeOf(table)).toBeNull();
  });

  test('a string entry is the string', () => {
    expect(vdfString(root['s'])).toBe('text');
  });

  test.each([
    ['a table', 't'],
    ['a repeated key', 'r'],
    ['a missing key', 'missing'],
  ])('%s reads as no string', (_name, key) => {
    expect(vdfString(root[key])).toBeNull();
  });
});
