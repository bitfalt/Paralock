/**
 * Static code samples with a four-tone highlighter (keywords, strings, comments, calls) for TypeScript, Rust, shell
 * and JSON. Server-rendered: no highlighting library ships to the browser.
 */
export type CodeLang = "ts" | "rust" | "sh" | "json";

const LANG_NAME: Record<CodeLang, string> = { ts: "TypeScript", rust: "Rust", sh: "Shell", json: "JSON" };

const KEYWORDS: Record<"ts" | "rust", Set<string>> = {
  ts: new Set(["import", "from", "const", "let", "new", "await", "async", "try", "catch", "if", "instanceof", "return", "export"]),
  rust: new Set(["use", "fn", "let", "mut", "pub", "self", "struct", "impl", "match", "if", "else", "return", "as", "dyn", "Box"]),
};

const CODE_TOKEN: Record<"ts" | "rust", RegExp> = {
  ts: /(\/\/.*$)|("(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*'|`[^`]*`)|(\b\d[\d_]*n?\b)|([A-Za-z_$][\w$]*)(?=\s*\()|([A-Za-z_$][\w$]*)/gm,
  rust: /(\/\/.*$)|("(?:[^"\\\n]|\\.)*")|(\b\d[\d_]*(?:u8|u16|u32|u64|usize)?\b)|([A-Za-z_]\w*!?)(?=\s*\()|([A-Za-z_]\w*)/gm,
};

type Piece = string | { cls: "k" | "s" | "c" | "f" | "n"; text: string };

function tokenizeCode(code: string, lang: "ts" | "rust"): Piece[] {
  const out: Piece[] = [];
  let last = 0;
  for (const m of code.matchAll(CODE_TOKEN[lang])) {
    const at = m.index ?? 0;
    if (at > last) out.push(code.slice(last, at));
    const [text, comment, str, num, call, word] = m;
    const ident = call ?? word;
    const cls = comment
      ? "c"
      : str
        ? "s"
        : num
          ? "n"
          : ident && KEYWORDS[lang].has(ident)
            ? "k"
            : call
              ? "f"
              : null;
    out.push(cls ? { cls, text } : text);
    last = at + text.length;
  }
  out.push(code.slice(last));
  return out;
}

/** Shell: comment lines are dim, the command is bright, flags are dim, quoted strings are warm. */
function tokenizeShell(code: string): Piece[] {
  const out: Piece[] = [];
  code.split("\n").forEach((line, i) => {
    if (i > 0) out.push("\n");
    if (/^\s*#/.test(line)) {
      out.push({ cls: "c", text: line });
      return;
    }
    if (/^\s*[{[]/.test(line)) {
      out.push(...tokenizeJson(line));
      return;
    }
    let first = true;
    let last = 0;
    for (const m of line.matchAll(/("[^"]*"|'[^']*')|(\s--?[A-Za-z][\w-]*)|(\S+)/g)) {
      const at = m.index ?? 0;
      if (at > last) out.push(line.slice(last, at));
      const [text, str, flag] = m;
      if (str) out.push({ cls: "s", text });
      else if (flag) out.push(" ", { cls: "k", text: text.trimStart() });
      else if (first) out.push({ cls: "f", text });
      else out.push(text);
      first = false;
      last = at + text.length;
    }
    out.push(line.slice(last));
  });
  return out;
}

/** JSON: keys bright, string values warm. */
function tokenizeJson(code: string): Piece[] {
  const out: Piece[] = [];
  let last = 0;
  for (const m of code.matchAll(/("(?:[^"\\\n]|\\.)*")(\s*:)?/g)) {
    const at = m.index ?? 0;
    if (at > last) out.push(code.slice(last, at));
    const [text, str, colon] = m;
    out.push({ cls: colon ? "n" : "s", text: str ?? text });
    if (colon) out.push(colon);
    last = at + text.length;
  }
  out.push(code.slice(last));
  return out;
}

/** The highlighted code as React nodes. */
export function highlight(code: string, lang: CodeLang = "ts"): React.ReactNode[] {
  const pieces = lang === "sh" ? tokenizeShell(code) : lang === "json" ? tokenizeJson(code) : tokenizeCode(code, lang);
  return pieces.map((p, i) => (typeof p === "string" ? p : <span key={i} className={`tok-${p.cls}`}>{p.text}</span>));
}

/** A code block with a file name bar. `actions` (e.g. a copy button) sit at the end of the bar. */
export function CodeSample({
  file,
  code,
  lang = "ts",
  actions,
}: {
  file: string;
  code: string;
  lang?: CodeLang;
  actions?: React.ReactNode;
}) {
  return (
    <div className="code">
      <div className="code__bar">
        <span>{file}</span>
        <span className="code__end">
          <span className="code__lang">{LANG_NAME[lang]}</span>
          {actions}
        </span>
      </div>
      <pre className="code__body" tabIndex={0}>
        <code>{highlight(code.trim(), lang)}</code>
      </pre>
    </div>
  );
}
