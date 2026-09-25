/**
 * Reads Valve's text KeyValues format, which SteamCMD prints for
 * `app_info_print` and writes to every `appmanifest_<id>.acf`.
 *
 * @remarks
 * The grammar this reader takes:
 *
 * - A document is a run of pairs. A pair is a key, then a value or a table of
 *   pairs in braces.
 * - A key or a value is quoted, or bare: a run of characters up to whitespace,
 *   a quote, a brace or a bracket.
 * - Inside quotes, `\\`, `\"`, `\n` and `\t` decode, which is how Steam writes a
 *   Windows path. Any other backslash stays, with the character after it.
 * - `//` opens a comment wherever a token could start, up to the line's end.
 * - A conditional in brackets, such as `[$WIN32]`, `[!$X]` or `[$!X]`, joined
 *   with `||` or `&&`, can follow a key or a value. It is read and never
 *   evaluated, so its pair is kept.
 *
 * What comes back:
 *
 * - Every table has a null prototype, so a key named `__proto__` or
 *   `constructor` is data and never reaches `Object.prototype`.
 * - Every value is a string. A manifest gid is larger than
 *   `Number.MAX_SAFE_INTEGER`, and a number would round it into a gid Valve
 *   never served.
 * - A key seen twice in one table becomes an array of its values in order, so
 *   a reader can refuse the repeat rather than silently take one of the two.
 * - A key keeps its case as written.
 *
 * Binary KeyValues (`appinfo.vdf`, `shortcuts.vdf`) is another format, and a
 * NUL character in the input refuses it.
 *
 * @example
 * ```typescript
 * const root = parseVdf(await Bun.file(acf).text());
 * const state = vdfTable(root['AppState']);
 * const buildId = vdfString(state['buildid']);
 * ```
 */

/**
 * ///////////////////////////////////////////////
 * What a document parses into
 * ///////////////////////////////////////////////
 */

/** A value in a table: a string, or a table of its own. */
export type VdfValue = string | VdfTable;

/** A table of pairs. A key seen more than once holds its values in order. */
export type VdfTable = { [key: string]: VdfValue | VdfValue[] };

/** A refusal that names what was wrong with the input and where. */
export class VdfError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'VdfError';
  }
}

/**
 * The table an entry holds, or an empty one when it holds a string, a repeated
 * key or nothing.
 */
export function vdfTable(entry: VdfValue | VdfValue[] | undefined): VdfTable {
  return typeof entry === 'object' && !Array.isArray(entry) ? entry : emptyTable();
}

/** The string an entry holds, or null when it holds anything else. */
export function vdfString(entry: VdfValue | VdfValue[] | undefined): string | null {
  return typeof entry === 'string' ? entry : null;
}

/**
 * ///////////////////////////////////////////////
 * Parsing
 * ///////////////////////////////////////////////
 */

/**
 * Parse a KeyValues document into its root table.
 *
 * @param text - The document, with either line ending and an optional byte
 * order mark.
 * @returns The root table, holding every top-level pair.
 * @throws {@link VdfError} When the input is binary KeyValues, a quote or a
 * table never closes, a brace or a conditional stands where a key belongs, a
 * key has no value, or a conditional is not one this reader knows.
 */
export function parseVdf(text: string): VdfTable {
  const nul = text.indexOf('\0');
  if (nul !== -1) {
    throw located(text, nul, 'a NUL character marks binary KeyValues, which this reader does not parse');
  }

  const scanner = new Scanner(text);
  const root = emptyTable();
  const open: { parent: VdfTable; at: number }[] = [];
  let table = root;
  // A conditional may follow a value or a closing brace, and nothing else.
  let conditionalAllowed = false;

  for (;;) {
    const token = scanner.next();
    if (token.kind === 'conditional' && conditionalAllowed) {
      conditionalAllowed = false;
      continue;
    }
    conditionalAllowed = false;

    switch (token.kind) {
      case 'end': {
        const unclosed = open.at(-1);
        if (unclosed !== undefined) {
          throw located(text, unclosed.at, 'a table opened here never closes');
        }
        return root;
      }
      case 'close': {
        const closed = open.pop();
        if (closed === undefined) {
          throw located(text, token.at, 'a closing brace has no table to close');
        }
        table = closed.parent;
        conditionalAllowed = true;
        continue;
      }
      case 'open':
        throw located(text, token.at, 'an opening brace has no key before it');
      case 'conditional':
        throw located(text, token.at, 'a conditional has no key or value before it');
      case 'text':
        break;
    }

    let after = scanner.next();
    if (after.kind === 'conditional') {
      after = scanner.next();
    }
    if (after.kind === 'open') {
      const child = emptyTable();
      add(table, token.text, child);
      open.push({ parent: table, at: after.at });
      table = child;
    } else if (after.kind === 'text') {
      add(table, token.text, after.text);
      conditionalAllowed = true;
    } else {
      throw located(text, token.at, `the key ${JSON.stringify(token.text)} has no value`);
    }
  }
}

/**
 * ///////////////////////////////////////////////
 * Tokens
 * ///////////////////////////////////////////////
 */

/** One token, with the offset it starts at. */
type Token =
  | { readonly kind: 'text'; readonly text: string; readonly at: number }
  | { readonly kind: 'open' | 'close' | 'conditional' | 'end'; readonly at: number };

/** The characters that end a bare token without belonging to it. */
const DELIMITERS = new Set(['"', '{', '}', '[', ']']);

/** ASCII whitespace, which is all KeyValues skips between tokens. */
const WHITESPACE = new Set([' ', '\t', '\r', '\n', '\v', '\f']);

/** U+FEFF, which an editor can leave at the start of a file. */
const BYTE_ORDER_MARK = String.fromCodePoint(0xfeff);

/** What each escape inside quotes decodes to. */
const ESCAPES: ReadonlyMap<string, string> = new Map([
  ['\\', '\\'],
  ['"', '"'],
  ['n', '\n'],
  ['t', '\t'],
]);

/**
 * One conditional term such as `$WIN32`, `!$X` or `$!X`, then any more joined
 * with `||` or `&&`.
 */
const CONDITIONAL = /^!?\$!?[A-Za-z0-9_]+(?:\s*(?:\|\||&&)\s*!?\$!?[A-Za-z0-9_]+)*$/;

/** Splits a document into tokens, skipping whitespace and comments. */
class Scanner {
  private position: number;

  constructor(private readonly text: string) {
    this.position = text.startsWith(BYTE_ORDER_MARK) ? 1 : 0;
  }

  /** The next token, or `end` once the input runs out. */
  next(): Token {
    this.skipTrivia();
    const at = this.position;
    const char = this.text[at];
    switch (char) {
      case undefined:
        return { kind: 'end', at };
      case '{':
        this.position += 1;
        return { kind: 'open', at };
      case '}':
        this.position += 1;
        return { kind: 'close', at };
      case '[':
        this.conditional(at);
        return { kind: 'conditional', at };
      case ']':
        throw located(this.text, at, 'a closing bracket has no conditional to close');
      case '"':
        return { kind: 'text', text: this.quoted(at), at };
      default:
        return { kind: 'text', text: this.bare(), at };
    }
  }

  /** Step over whitespace and `//` comments. */
  private skipTrivia(): void {
    for (;;) {
      const char = this.text[this.position];
      if (char !== undefined && WHITESPACE.has(char)) {
        this.position += 1;
      } else if (char === '/' && this.text[this.position + 1] === '/') {
        const end = this.text.indexOf('\n', this.position);
        this.position = end === -1 ? this.text.length : end;
      } else {
        return;
      }
    }
  }

  /** A quoted token, decoded, with the scanner left after its closing quote. */
  private quoted(at: number): string {
    // Text between escapes is taken a run at a time, so a long value costs one
    // slice rather than one string per character.
    let decoded = '';
    let run = at + 1;
    this.position = run;
    for (;;) {
      const char = this.text[this.position];
      if (char === undefined) {
        throw located(this.text, at, 'a quote opened here never closes');
      }
      if (char === '"') {
        decoded += this.text.slice(run, this.position);
        this.position += 1;
        return decoded;
      }
      if (char === '\\') {
        const escaped = ESCAPES.get(this.text[this.position + 1] ?? '');
        if (escaped !== undefined) {
          decoded += this.text.slice(run, this.position) + escaped;
          this.position += 2;
          run = this.position;
          continue;
        }
      }
      this.position += 1;
    }
  }

  /** A bare token, which runs to whitespace or a delimiter. */
  private bare(): string {
    const start = this.position;
    for (;;) {
      const char = this.text[this.position];
      if (char === undefined || WHITESPACE.has(char) || DELIMITERS.has(char)) {
        return this.text.slice(start, this.position);
      }
      this.position += 1;
    }
  }

  /** Check a bracketed conditional, and leave the scanner after it. */
  private conditional(at: number): void {
    const close = this.text.indexOf(']', at);
    // The line break is looked for only up to the bracket, so a line of many
    // conditionals is not scanned to its end once per conditional.
    if (close === -1 || this.text.slice(at, close).includes('\n')) {
      throw located(this.text, at, 'a conditional opened here never closes on its line');
    }
    const body = this.text.slice(at + 1, close).trim();
    if (!CONDITIONAL.test(body)) {
      // JSON-quoted, so a control character from the input never reaches a log
      // line as itself.
      throw located(this.text, at, `the conditional ${JSON.stringify(body)} is not one this reader knows`);
    }
    this.position = close + 1;
  }
}

/**
 * ///////////////////////////////////////////////
 * Helpers
 * ///////////////////////////////////////////////
 */

/** A table no key can reach the prototype chain through. */
function emptyTable(): VdfTable {
  return Object.create(null) as VdfTable;
}

/** Put a value under a key, turning a repeated key into an array. */
function add(table: VdfTable, key: string, value: VdfValue): void {
  const existing = table[key];
  if (existing === undefined) {
    table[key] = value;
  } else if (Array.isArray(existing)) {
    existing.push(value);
  } else {
    table[key] = [existing, value];
  }
}

/** A {@link VdfError} naming the line and column of an offset. */
function located(text: string, at: number, message: string): VdfError {
  const before = text.slice(0, at);
  const line = before.split('\n').length;
  const column = at - before.lastIndexOf('\n');
  return new VdfError(`${message}, at line ${line}, column ${column}`);
}
