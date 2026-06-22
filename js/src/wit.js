// A parser for the structural, inlined subset of WIT that wRPC servers render
// with `wasm-wave`'s `DisplayType` (records as `record { a: u8 }`, variants as
// `variant { off, on(u8) }`, …). It turns that text back into the `Type` tree
// the value codec walks, so a client can fetch a served interface description
// and invoke it without statically generated bindings.
//
// This handles inlined, self-contained type expressions and a flat
// `interface { name: func(...) -> ...; }` document. It does not resolve named
// type aliases or `use` statements; build those types with `types.js` instead.

// Identifiers, the `->` arrow, version/number runs (e.g. `0.2.0-draft2` in a
// package declaration), and single-character punctuation.
const TYPE_TOKEN = /\s*(->|[A-Za-z][A-Za-z0-9-]*|[0-9][A-Za-z0-9.+-]*|[{}()<>,:;_/.@])/y;

/** @param {string} src @returns {string[]} */
function tokenize(src) {
  const tokens = [];
  let pos = 0;
  while (pos < src.length) {
    TYPE_TOKEN.lastIndex = pos;
    const m = TYPE_TOKEN.exec(src);
    if (!m) {
      if (/\s/.test(src[pos])) {
        pos += 1;
        continue;
      }
      throw new SyntaxError(`unexpected character \`${src[pos]}\` in WIT`);
    }
    tokens.push(m[1]);
    pos = TYPE_TOKEN.lastIndex;
  }
  return tokens;
}

const PRIMITIVES = new Set([
  "bool", "s8", "u8", "s16", "u16", "s32", "u32", "s64", "u64",
  "f32", "f64", "char", "string",
]);

/** A token cursor shared by the type and document parsers. */
class TokenStream {
  /** @param {string[]} tokens */
  constructor(tokens) {
    this.tokens = tokens;
    this.pos = 0;
  }
  peek() {
    return this.tokens[this.pos];
  }
  next() {
    if (this.pos >= this.tokens.length) throw new SyntaxError("unexpected end of WIT");
    return this.tokens[this.pos++];
  }
  /** @param {string} tok */
  expect(tok) {
    const got = this.next();
    if (got !== tok) throw new SyntaxError(`expected \`${tok}\`, got \`${got}\``);
    return got;
  }
  eof() {
    return this.pos >= this.tokens.length;
  }
}

/**
 * Parse a single type expression from `ts` into a `Type`.
 * @param {TokenStream} ts
 * @returns {object}
 */
function parseTypeFrom(ts) {
  const tok = ts.next();
  if (PRIMITIVES.has(tok)) return { kind: tok };
  switch (tok) {
    case "list": {
      ts.expect("<");
      const elem = parseTypeFrom(ts);
      ts.expect(">");
      return { kind: "list", elem };
    }
    case "option": {
      ts.expect("<");
      const some = parseTypeFrom(ts);
      ts.expect(">");
      return { kind: "option", some };
    }
    case "result": {
      if (ts.peek() !== "<") return { kind: "result", ok: null, err: null };
      ts.expect("<");
      const ok = ts.peek() === "_" ? (ts.next(), null) : parseTypeFrom(ts);
      let err = null;
      if (ts.peek() === ",") {
        ts.next();
        err = parseTypeFrom(ts);
      }
      ts.expect(">");
      return { kind: "result", ok, err };
    }
    case "tuple": {
      ts.expect("<");
      const elems = [];
      if (ts.peek() !== ">") {
        elems.push(parseTypeFrom(ts));
        while (ts.peek() === ",") {
          ts.next();
          elems.push(parseTypeFrom(ts));
        }
      }
      ts.expect(">");
      return { kind: "tuple", elems };
    }
    case "record": {
      ts.expect("{");
      const fields = [];
      if (ts.peek() !== "}") {
        do {
          const name = ts.next();
          ts.expect(":");
          fields.push({ name, ty: parseTypeFrom(ts) });
        } while (ts.peek() === "," && ts.next());
      }
      ts.expect("}");
      return { kind: "record", fields };
    }
    case "variant": {
      ts.expect("{");
      const cases = [];
      if (ts.peek() !== "}") {
        do {
          const name = ts.next();
          let ty = null;
          if (ts.peek() === "(") {
            ts.next();
            ty = parseTypeFrom(ts);
            ts.expect(")");
          }
          cases.push({ name, ty });
        } while (ts.peek() === "," && ts.next());
      }
      ts.expect("}");
      return { kind: "variant", cases };
    }
    case "enum": {
      ts.expect("{");
      const names = [];
      if (ts.peek() !== "}") {
        do {
          names.push(ts.next());
        } while (ts.peek() === "," && ts.next());
      }
      ts.expect("}");
      return { kind: "enum", names };
    }
    case "flags": {
      ts.expect("{");
      const names = [];
      if (ts.peek() !== "}") {
        do {
          names.push(ts.next());
        } while (ts.peek() === "," && ts.next());
      }
      ts.expect("}");
      return { kind: "flags", names };
    }
    case "borrow": {
      ts.expect("<");
      const name = ts.next();
      ts.expect(">");
      return { kind: "borrow", name };
    }
    case "own": {
      ts.expect("<");
      const name = ts.next();
      ts.expect(">");
      return { kind: "own", name };
    }
    default:
      // A bare identifier is an owned resource handle (`own<utxo>` is rendered
      // as just `utxo`).
      return { kind: "own", name: tok };
  }
}

/**
 * Parse a single inlined WIT type expression.
 * @param {string} text
 * @returns {object}
 */
export function parseType(text) {
  return parseTypeFrom(new TokenStream(tokenize(text)));
}

/**
 * Parse a flat WIT document: an optional `package` declaration and one
 * `interface` whose body is a list of `name: func(params) -> results;`.
 * @param {string} text
 * @returns {{ package: string | null, interface: string, funcs: { name: string, params: { name: string, ty: object }[], results: object[] }[] }}
 */
export function parseWit(text) {
  const ts = new TokenStream(tokenize(text));

  let pkg = null;
  if (ts.peek() === "package") {
    ts.next();
    const parts = [];
    while (ts.peek() !== ";" && !ts.eof()) parts.push(ts.next());
    ts.expect(";");
    pkg = parts.join("");
  }

  ts.expect("interface");
  const iface = ts.next();
  ts.expect("{");

  const funcs = [];
  while (ts.peek() !== "}" && !ts.eof()) {
    const name = ts.next();
    ts.expect(":");
    ts.expect("func");
    ts.expect("(");
    const params = [];
    if (ts.peek() !== ")") {
      do {
        const pname = ts.next();
        ts.expect(":");
        params.push({ name: pname, ty: parseTypeFrom(ts) });
      } while (ts.peek() === "," && ts.next());
    }
    ts.expect(")");

    const results = [];
    if (ts.peek() === "->") {
      ts.next();
      if (ts.peek() === "(") {
        ts.next();
        if (ts.peek() !== ")") {
          do {
            results.push(parseTypeFrom(ts));
          } while (ts.peek() === "," && ts.next());
        }
        ts.expect(")");
      } else {
        results.push(parseTypeFrom(ts));
      }
    }
    ts.expect(";");
    funcs.push({ name, params, results });
  }
  ts.expect("}");

  return { package: pkg, interface: iface, funcs };
}
