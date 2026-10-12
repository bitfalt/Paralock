import { createHighlighter, type Highlighter, type ThemeRegistration } from "shiki";

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
  return (highlighter ??= createHighlighter({ themes: [CODE_THEME], langs: [...LANGS] }));
}

/** Resolves a fence's language tag to a loaded grammar ("text" when unknown or absent). */
export function resolveLang(tag: string | null | undefined): string {
  const t = (tag ?? "").trim().toLowerCase();
  if (!t) return "text";
  const id = ALIASES[t] ?? t;
  return (LANGS as readonly string[]).includes(id) ? id : "text";
}

/** The label shown in a code block's header. */
export function langLabel(lang: string): string {
  return LABELS[lang] ?? lang.toUpperCase();
}
