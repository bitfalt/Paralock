import { parse as parseYaml } from "yaml";

/**
 * The docs' two diagram languages, written as fenced code blocks so the Markdown stays plain text:
 *
 * - ```` ```timeline ````: a two-lane (or more) timeline in YAML. Lanes hold events at positions from 0 to 100; an
 *   event with `mark` is a deadline (N, T), one with `key: true` is where the secret moves. `spans` draw dimension
 *   lines under the lanes.
 * - ```` ```sequence ````: a sequence diagram in a line syntax close to Mermaid's:
 *   `actors: A, B, C`, `A -> B: message`, `A --> B: reply` (dashed), `A: note`, `== Phase ==`, and a trailing
 *   `[key]` on a step where the secret moves.
 *
 * The site renders them as components (components/docs/Timeline.tsx, Sequence.tsx). The Markdown endpoints,
 * llms-full.txt and the MCP server replace them with the plain-text versions built here (`diagramText`).
 */

export const DIAGRAM_LANGS = ["timeline", "sequence"] as const;
export type DiagramLang = (typeof DIAGRAM_LANGS)[number];

export const isDiagramLang = (lang: string | null | undefined): lang is DiagramLang =>
  (DIAGRAM_LANGS as readonly string[]).includes((lang ?? "").trim().toLowerCase());

// ---- Timeline -------------------------------------------------------------------------------------

/** One point on a lane: a step (`label`, e.g. "lock"), or a deadline (`mark`, e.g. "N"). */
export interface TimelineEvent {
  lane: number;
  /** Position from 0 to 100 (illustrative, not to scale). */
  at: number;
  label?: string;
  mark?: string;
  /** The full sentence, shown in the list under the diagram. */
  text: string;
  /** The secret moves here (drawn in the accent, joined to the next key event). */
  key: boolean;
}

export interface TimelineLane {
  name: string;
  unit?: string;
}

/** A dimension line under the lanes, from `from` to `to`. */
export interface TimelineSpan {
  from: number;
  to: number;
  label: string;
}

export interface Timeline {
  title?: string;
  caption?: string;
  lanes: TimelineLane[];
  /** Every event, sorted by position (then lane). */
  events: TimelineEvent[];
  spans: TimelineSpan[];
}

const str = (v: unknown, where: string): string => {
  if (typeof v === "string" && v.trim()) return v.trim();
  if (typeof v === "number") return String(v);
  throw new Error(`timeline: ${where} must be a non-empty string`);
};

const pos = (v: unknown, where: string): number => {
  if (typeof v !== "number" || !Number.isFinite(v) || v < 0 || v > 100) throw new Error(`timeline: ${where} must be a number from 0 to 100`);
  return v;
};

const optStr = (v: unknown, where: string) => (v === undefined || v === null ? undefined : str(v, where));

/** Parses a ```timeline block. Throws on a malformed one, so mistakes fail the build instead of shipping. */
export function parseTimeline(source: string): Timeline {
  const raw = parseYaml(source) as Record<string, unknown> | null;
  if (!raw || typeof raw !== "object") throw new Error("timeline: expected a YAML mapping");
  if (!Array.isArray(raw.lanes) || raw.lanes.length === 0) throw new Error("timeline: `lanes` must be a non-empty list");
  const lanes: TimelineLane[] = [];
  const events: TimelineEvent[] = [];
  raw.lanes.forEach((l: Record<string, unknown>, i: number) => {
    const name = str(l?.name, `lanes[${i}].name`);
    lanes.push({ name, unit: optStr(l.unit, `lanes[${i}].unit`) });
    if (!Array.isArray(l.events)) throw new Error(`timeline: lanes[${i}].events must be a list`);
    l.events.forEach((e: Record<string, unknown>, j: number) => {
      const where = `lanes[${i}].events[${j}]`;
      const label = optStr(e?.label, `${where}.label`);
      const mark = optStr(e?.mark, `${where}.mark`);
      if (!label && !mark) throw new Error(`timeline: ${where} needs a label or a mark`);
      events.push({ lane: i, at: pos(e.at, `${where}.at`), label, mark, text: str(e.text, `${where}.text`), key: e.key === true });
    });
  });
  events.sort((a, b) => a.at - b.at || a.lane - b.lane);
  const spans = (Array.isArray(raw.spans) ? raw.spans : []).map((s: Record<string, unknown>, i: number) => {
    const from = pos(s?.from, `spans[${i}].from`);
    const to = pos(s?.to, `spans[${i}].to`);
    if (to <= from) throw new Error(`timeline: spans[${i}] must end after it starts`);
    return { from, to, label: str(s.label, `spans[${i}].label`) };
  });
  return { title: optStr(raw.title, "title"), caption: optStr(raw.caption, "caption"), lanes, events, spans };
}

/** The timeline as plain text: its events in order, then the spans. */
export function timelineText(t: Timeline): string {
  const out: string[] = [];
  if (t.title) out.push(t.title, "");
  out.push(`Lanes: ${t.lanes.map((l) => (l.unit ? `${l.name} (${l.unit})` : l.name)).join(", ")}. In time order:`);
  t.events.forEach((e, i) => {
    const lane = t.lanes[e.lane]?.name ?? "";
    const tag = e.mark ? `${lane}, ${e.mark}` : lane;
    out.push(`${i + 1}. [${tag}] ${e.text}`);
  });
  if (t.spans.length) {
    out.push("", "Durations:");
    for (const s of t.spans) out.push(`- ${s.label}`);
  }
  if (t.caption) out.push("", t.caption);
  return out.join("\n");
}

// ---- Sequence -------------------------------------------------------------------------------------

export type SequenceStep =
  | { type: "message"; n: number; from: number; to: number; text: string; reply: boolean; key: boolean }
  | { type: "note"; n: number; actor: number; text: string; key: boolean }
  | { type: "divider"; text: string };

export interface Sequence {
  title?: string;
  actors: string[];
  steps: SequenceStep[];
}

const ARROW = /^(.+?)\s*(-->|->)\s*(.+?)\s*:\s*(.+)$/;
const NOTE = /^(.+?)\s*:\s*(.+)$/;
const DIVIDER = /^==\s*(.+?)\s*==$/;
const KEY_TAG = /\s*\[key\]\s*$/i;

/** Parses a ```sequence block. Throws on unknown actors or lines it can't read. */
export function parseSequence(source: string): Sequence {
  const actors: string[] = [];
  const steps: SequenceStep[] = [];
  let title: string | undefined;
  let n = 0;
  const index = (name: string, line: string) => {
    const i = actors.findIndex((a) => a.toLowerCase() === name.trim().toLowerCase());
    if (i < 0) throw new Error(`sequence: unknown actor "${name.trim()}" in "${line}" (declare it in \`actors:\`)`);
    return i;
  };
  for (const rawLine of source.split("\n")) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#")) continue;
    const head = /^(actors|title)\s*:\s*(.+)$/i.exec(line);
    if (head) {
      if (head[1].toLowerCase() === "title") title = head[2].trim();
      else actors.push(...head[2].split(",").map((a) => a.trim()).filter(Boolean));
      continue;
    }
    const div = DIVIDER.exec(line);
    if (div) {
      steps.push({ type: "divider", text: div[1] });
      continue;
    }
    const key = KEY_TAG.test(line);
    const body = line.replace(KEY_TAG, "");
    const arrow = ARROW.exec(body);
    if (arrow) {
      steps.push({ type: "message", n: ++n, from: index(arrow[1], line), to: index(arrow[3], line), text: arrow[4].trim(), reply: arrow[2] === "-->", key });
      continue;
    }
    const note = NOTE.exec(body);
    if (note) {
      steps.push({ type: "note", n: ++n, actor: index(note[1], line), text: note[2].trim(), key });
      continue;
    }
    throw new Error(`sequence: can't read "${line}"`);
  }
  if (actors.length < 2) throw new Error("sequence: declare at least two actors with `actors: A, B`");
  return { title, actors, steps };
}

/** The sequence as plain text: numbered steps, with the phases as headings. */
export function sequenceText(s: Sequence): string {
  const out: string[] = [];
  if (s.title) out.push(s.title, "");
  out.push(`Participants: ${s.actors.join(", ")}.`);
  for (const step of s.steps) {
    if (step.type === "divider") out.push("", `${step.text}:`);
    else if (step.type === "message") out.push(`${step.n}. ${s.actors[step.from]} → ${s.actors[step.to]}: ${step.text}`);
    else out.push(`${step.n}. ${s.actors[step.actor]}: ${step.text}`);
  }
  return out.join("\n");
}

/** The plain-text version of a diagram block (for the Markdown outputs and search). */
export function diagramText(lang: DiagramLang, source: string): string {
  return lang === "timeline" ? timelineText(parseTimeline(source)) : sequenceText(parseSequence(source));
}
