import type { CSSProperties } from "react";
import { parseSequence } from "@/lib/docs/diagrams";

/**
 * A ```sequence block as a sequence diagram: one lifeline per actor and one row per step, laid out on a grid of
 * half-columns so arrows run exactly from lifeline to lifeline. It is an ordered list underneath, so it reads in
 * order without the drawing; on phones it becomes that list, with "From → To" on each step.
 */
export function Sequence({ source }: { source: string }) {
  const s = parseSequence(source);
  const n = s.actors.length;
  const cols = n * 2;
  // Grid line (1-based) of actor k's lifeline: between its two half-columns.
  const line = (k: number) => 2 * k + 2;
  const clampSpan = (start: number, width: number) => {
    const a = Math.max(1, Math.min(start, cols + 1 - width));
    return `${a} / ${a + width}`;
  };
  const style = { "--cols": cols } as CSSProperties;

  return (
    <figure className="dseq" style={style}>
      {s.title && <figcaption className="dseq__title">{s.title}</figcaption>}
      <div className="dseq__actors" aria-hidden="true">
        {s.actors.map((a, k) => (
          <span key={a} style={{ gridColumn: `${line(k) - 1} / ${line(k) + 1}` }}>
            {a}
          </span>
        ))}
      </div>
      <div className="dseq__body">
        <div className="dseq__lifelines" aria-hidden="true">
          {s.actors.map((a, k) => (
            <span key={a} style={{ gridColumn: `${line(k) - 1} / ${line(k) + 1}` }} />
          ))}
        </div>
        <ol className="dseq__steps">
          {s.steps.map((step, i) => {
            if (step.type === "divider") {
              return (
                <li key={`d${i}`} className="dseq__divider">
                  <span>{step.text}</span>
                </li>
              );
            }
            if (step.type === "note") {
              const k = step.actor;
              return (
                <li key={`n${i}`} className="dseq__note" data-key={step.key || undefined}>
                  <span className="dseq__box" style={{ gridColumn: clampSpan(line(k) - 2, 4) }}>
                    <span className="dseq__n">{step.n}</span>
                    <span className="dseq__who">{s.actors[k]}</span>
                    <span className="sr-only">: </span>
                    <span className="dseq__text">{step.text}</span>
                  </span>
                  <span className="dseq__pin" style={{ gridColumn: `${line(k) - 1} / ${line(k) + 1}` }} aria-hidden="true" />
                </li>
              );
            }
            const lo = Math.min(step.from, step.to);
            const hi = Math.max(step.from, step.to);
            return (
              <li
                key={`m${i}`}
                className="dseq__msg"
                data-dir={step.to >= step.from ? "right" : "left"}
                data-reply={step.reply || undefined}
                data-key={step.key || undefined}
              >
                <span className="dseq__label" style={{ gridColumn: clampSpan(line(lo) - 2, line(hi) - line(lo) + 4) }}>
                  <span className="dseq__n">{step.n}</span>
                  <span className="dseq__who">
                    {s.actors[step.from]} <span aria-hidden="true">→</span>
                    <span className="sr-only"> to </span> {s.actors[step.to]}
                  </span>
                  <span className="sr-only">: </span>
                  <span className="dseq__text">{step.text}</span>
                </span>
                <span className="dseq__arrow" style={{ gridColumn: `${line(lo)} / ${line(hi)}` }} aria-hidden="true" />
              </li>
            );
          })}
        </ol>
      </div>
    </figure>
  );
}
