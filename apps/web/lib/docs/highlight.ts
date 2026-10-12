import { createHighlighter, type Highlighter, type ThemeRegistration } from "shiki";
import { ZSCRIPT_GRAMMAR } from "@/lib/docs/zscript";

/**
 * Build-time syntax highlighting (shiki) with a theme drawn from the site's palette: mostly grayscale, with keywords
 * and punctuation receding, names and values in the foreground, and strings in the warm off-white used on the landing
 * page's code sample. No client-side cost: pages ship the colored spans as static HTML.
 */

const FG = "#ededea";
const FG2 = "#a3a39f";
const FG3 = "#7e7f7b";
const SOFT = "#c6c6c0";
const STRING = "#d9d4c7";
const COMMENT = "#63645f";
/** Script placeholders (`<hashlock>`) take the accent, a little muted so they don't shout. */
const PLACEHOLDER = "#d9a752";
const PLACEHOLDER_EDGE = "#e8b04b80";

export const CODE_THEME: ThemeRegistration = {
  name: "paralock",
  type: "dark",
  colors: { "editor.background": "#0d0e10", "editor.foreground": FG2 },
  settings: [
    { settings: { foreground: FG2, background: "#0d0e10" } },
    { scope: ["comment", "punctuation.definition.comment", "string.comment"], settings: { foreground: COMMENT, fontStyle: "italic" } },
    {
      scope: [
        "keyword",
        "storage",
        "storage.type",
        "storage.modifier",
        "keyword.control",
        "keyword.operator.new",
        "keyword.operator.expression",
        "keyword.operator.word",
        "variable.language.this",
      ],
      settings: { foreground: FG3 },
    },
    { scope: ["punctuation", "meta.brace", "keyword.operator", "punctuation.separator", "punctuation.terminator"], settings: { foreground: FG3 } },
    {
      scope: ["string", "string.quoted", "string.template", "punctuation.definition.string", "string.unquoted", "markup.inline.raw"],
      settings: { foreground: STRING },
    },
    {
      scope: ["punctuation.definition.template-expression", "punctuation.section.embedded"],
      settings: { foreground: FG3 },
    },
    {
      scope: ["constant.numeric", "constant.language", "constant.character", "support.constant", "keyword.other.unit", "constant.other"],
      settings: { foreground: FG },
    },
    {
      scope: [
        "entity.name.function",
        "support.function",
        "variable.function",
        "meta.function-call.generic",
        "entity.name.command",
        "support.function.builtin",
      ],
      settings: { foreground: FG },
    },
    {
      scope: [
        "entity.name.type",
        "entity.name.class",
        "entity.name.interface",
        "support.type",
        "support.class",
        "entity.other.inherited-class",
        "storage.type.primitive",
        "storage.type.built-in",
        "entity.name.type.module",
        "entity.name.namespace",
      ],
      settings: { foreground: SOFT },
    },
    {
      scope: ["support.type.property-name", "support.type.property-name.json", "entity.name.tag", "meta.object-literal.key"],
      settings: { foreground: SOFT },
    },
    { scope: ["variable", "variable.other", "variable.parameter", "support.variable.property", "meta.definition.variable"], settings: { foreground: FG2 } },
    { scope: ["variable.other.constant", "variable.other.enummember"], settings: { foreground: FG } },
    { scope: ["constant.other.option", "variable.parameter.option"], settings: { foreground: FG2 } },
    { scope: ["markup.inserted", "punctuation.definition.inserted"], settings: { foreground: FG } },
    { scope: ["markup.deleted", "punctuation.definition.deleted"], settings: { foreground: COMMENT } },
    { scope: ["markup.heading", "markup.bold"], settings: { foreground: FG, fontStyle: "bold" } },
    { scope: ["markup.italic"], settings: { fontStyle: "italic" } },
    { scope: ["meta.diff.header", "meta.diff.range"], settings: { foreground: FG3 } },
    // Rust: macros and attributes read as names; lifetimes recede.
    { scope: ["entity.name.function.macro.rust", "support.macro.rust"], settings: { foreground: FG } },
    { scope: ["meta.attribute.rust", "punctuation.definition.attribute.rust"], settings: { foreground: FG3 } },
    { scope: ["entity.name.type.lifetime.rust", "punctuation.definition.lifetime.rust", "storage.modifier.lifetime.rust"], settings: { foreground: FG3 } },
    { scope: ["keyword.operator.namespace.rust", "keyword.operator.access.dot.rust"], settings: { foreground: FG3 } },
    // TOML: table headers bright, keys soft.
    { scope: ["entity.name.section.toml", "support.type.property-name.table.toml", "punctuation.definition.table.toml"], settings: { foreground: FG } },
    { scope: ["support.type.property-name.toml", "entity.name.tag.toml", "keyword.key.toml"], settings: { foreground: SOFT } },
    // Zcash script (lib/docs/zscript.ts).
    { scope: ["keyword.control.flow.zscript"], settings: { foreground: FG3 } },
    { scope: ["keyword.operator.opcode.zscript"], settings: { foreground: SOFT } },
    { scope: ["support.function.check.zscript"], settings: { foreground: FG } },
    { scope: ["constant.numeric.zscript"], settings: { foreground: FG } },
    { scope: ["variable.parameter.placeholder.zscript"], settings: { foreground: PLACEHOLDER } },
    { scope: ["punctuation.definition.placeholder.begin.zscript", "punctuation.definition.placeholder.end.zscript"], settings: { foreground: PLACEHOLDER_EDGE } },
  ],
};

const LANGS = [
  "typescript",
  "tsx",
  "javascript",
  "jsx",
  "shellscript",
  "shellsession",
  "json",
  "jsonc",
  "solidity",
  "yaml",
  "toml",
  "diff",
  "dotenv",
  "ini",
  "sql",
  "http",
  "proto",
  "rust",
  "go",
  "python",
  "html",
  "css",
  "markdown",
] as const;

const ALIASES: Record<string, string> = {
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  sh: "shellscript",
  bash: "shellscript",
  shell: "shellscript",
  zsh: "shellscript",
  console: "shellsession",
  terminal: "shellsession",
  yml: "yaml",
  sol: "solidity",
  md: "markdown",
  env: "dotenv",
  py: "python",
  rs: "rust",
  protobuf: "proto",
  script: "zscript",
  btcscript: "zscript",
  "bitcoin-script": "zscript",
  "zcash-script": "zscript",
  txt: "text",
  plaintext: "text",
};

const LABELS: Record<string, string> = {
  typescript: "TypeScript",
  tsx: "TSX",
  javascript: "JavaScript",
  jsx: "JSX",
  shellscript: "Shell",
  shellsession: "Terminal",
  json: "JSON",
  jsonc: "JSON",
  solidity: "Solidity",
  yaml: "YAML",
  toml: "TOML",
  diff: "Diff",
  dotenv: ".env",
  ini: "INI",
  sql: "SQL",
  http: "HTTP",
  proto: "Protobuf",
  rust: "Rust",
  zscript: "Zcash script",
  go: "Go",
  python: "Python",
  html: "HTML",
  css: "CSS",
  markdown: "Markdown",
  text: "Text",
};

let highlighter: Promise<Highlighter> | undefined;

/** The shared highlighter (created once per process). */
export function getHighlighter(): Promise<Highlighter> {
  return (highlighter ??= createHighlighter({ themes: [CODE_THEME], langs: [...LANGS, ZSCRIPT_GRAMMAR] }));
}

/** Every language a fence can name, grammars and aliases. */
const KNOWN = new Set<string>([...LANGS, ZSCRIPT_GRAMMAR.name, "text"]);

/** Resolves a fence's language tag to a loaded grammar ("text" when unknown or absent). */
export function resolveLang(tag: string | null | undefined): string {
  const t = (tag ?? "").trim().toLowerCase();
  if (!t) return "text";
  const id = ALIASES[t] ?? t;
  return KNOWN.has(id) ? id : "text";
}

/** The label shown in a code block's header. */
export function langLabel(lang: string): string {
  return LABELS[lang] ?? lang.toUpperCase();
}
