/** A small static TypeScript sample with a four-tone highlighter (keywords, strings, comments, calls). */
const KEYWORDS = new Set(["import", "from", "const", "new", "await", "try", "catch", "if", "instanceof", "return"]);
const TOKEN = /(\/\/.*$)|("[^"]*")|(\b\d[\d_]*n?\b)|([A-Za-z_$][\w$]*)(?=\s*\()|([A-Za-z_$][\w$]*)/gm;

function highlight(code: string) {
  const out: React.ReactNode[] = [];
  let last = 0;
  let i = 0;
  for (const m of code.matchAll(TOKEN)) {
    const at = m.index ?? 0;
    if (at > last) out.push(code.slice(last, at));
    const [text, comment, str, num, call, word] = m;
    const ident = call ?? word;
    const cls = comment ? "c" : str ? "s" : num ? "n" : ident && KEYWORDS.has(ident) ? "k" : call ? "f" : null;
    out.push(cls ? <span key={i++} className={`tok-${cls}`}>{text}</span> : text);
    last = at + text.length;
  }
  out.push(code.slice(last));
  return out;
}

export function CodeSample({ file, code }: { file: string; code: string }) {
  return (
    <div className="code">
      <div className="code__bar">
        <span>{file}</span>
        <span className="code__lang">TypeScript</span>
      </div>
      <pre className="code__body">
        <code>{highlight(code.trim())}</code>
      </pre>
    </div>
  );
}
