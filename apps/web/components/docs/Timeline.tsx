import type { CSSProperties } from "react";
import { parseTimeline, type TimelineEvent } from "@/lib/docs/diagrams";

/**
 * A ```timeline block: one lane per chain, drawn like the landing page's settlement diagram. Each lane shades its
 * lock's window (first event → deadline), steps are square nodes, deadlines are dashed marks (N, T), and the accent
 * line follows the secret from one key event to the next. The numbered list underneath is the same content as text.
 */
export function Timeline({ source }: { source: string }) {
  const t = parseTimeline(source);
  const lanes = t.lanes.length;
  const y = (lane: number) => ((lane + 0.5) / lanes) * 100;
  // Steps are numbered in time order; deadlines go by their mark (N, T).
  const number = new Map<TimelineEvent, number>(t.events.filter((e) => !e.mark).map((e, i) => [e, i + 1]));
  const keys = t.events.filter((e) => e.key);
  const keyLines = keys.slice(1).flatMap((b, i) => (keys[i].lane === b.lane ? [] : [{ a: keys[i], b }]));
  const v = (props: Record<string, number | string>) => props as CSSProperties;

  return (
    <figure className="dtl">
      {t.title && <p className="dtl__title">{t.title}</p>}
      {/* The list below carries the same content as text, so the drawing is hidden from assistive technology. */}
      <div className="dtl__diagram" aria-hidden="true">
        <div className="dtl__labels" style={v({ "--lanes": lanes })}>
          {t.lanes.map((l) => (
            <span key={l.name}>
              {l.name}
              {l.unit && <small>{l.unit}</small>}
            </span>
          ))}
        </div>
        <div className="dtl__field" style={v({ "--lanes": lanes })}>
          {t.lanes.map((lane, i) => {
            const events = t.events.filter((e) => e.lane === i);
            const steps = events.filter((e) => !e.mark);
            const start = Math.min(...events.map((e) => e.at));
            const end = Math.max(...events.map((e) => e.at));
            const lastStep = steps.length ? Math.max(...steps.map((e) => e.at)) : start;
            return (
              <div key={lane.name} className="dtl__lane" style={v({ "--y": `${y(i)}%` })} data-first={i === 0 || undefined}>
                <span className="dtl__window" style={v({ "--a": start / 100, "--b": end / 100 })} />
                <span className="dtl__rule" />
                <span className="dtl__bar" style={v({ "--a": start / 100, "--b": lastStep / 100 })} />
                {events.map((e) =>
                  e.mark ? (
                    <span key={`${e.at}-${e.mark}`} className="dtl__limit" style={v({ "--x": e.at / 100 })} data-end={e.at > 70 || undefined}>
                      <b>{e.mark}</b>
                      {e.label && <i>{e.label}</i>}
                    </span>
                  ) : (
                    <span key={`${e.at}-${e.label}`} className="dtl__node" style={v({ "--x": e.at / 100 })} data-key={e.key || undefined}>
                      <i>
                        <b>{number.get(e)}</b>
                        {e.label}
                      </i>
                    </span>
                  ),
                )}
              </div>
            );
          })}
          {keyLines.length > 0 && (
            <svg className="dtl__key" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
              {keyLines.map(({ a, b }) => (
                <line key={`${a.at}-${b.at}`} x1={a.at} y1={y(a.lane)} x2={b.at} y2={y(b.lane)} vectorEffect="non-scaling-stroke" />
              ))}
            </svg>
          )}
        </div>
        {t.spans.length > 0 && (
          <div className="dtl__spans">
            {t.spans.map((s) => (
              <div key={s.label} className="dtl__span" style={v({ "--a": s.from / 100, "--b": s.to / 100 })}>
                <span className="dtl__span-line" />
                <span className="dtl__span-label">{s.label}</span>
              </div>
            ))}
          </div>
        )}
      </div>
      <ol className="dtl__log" aria-label="Timeline, in order">
        {t.events.map((e) => (
          <li key={`${e.lane}-${e.at}`} data-key={e.key || undefined} data-mark={e.mark ? "" : undefined}>
            <span className="dtl__n">{e.mark ?? number.get(e)}</span>
            <span className="dtl__where">{t.lanes[e.lane]?.name}</span>
            <span className="dtl__text">{e.text}</span>
          </li>
        ))}
      </ol>
      {t.spans.length > 0 && <p className="sr-only">{t.spans.map((s) => s.label).join(". ")}.</p>}
      {t.caption && <figcaption className="dtl__caption">{t.caption}</figcaption>}
    </figure>
  );
}
