/** The protocol between an agent-written chat widget (a sandboxed iframe) and the
 * dashboard. The frame runs in an opaque origin, so everything it says is
 * untrusted input: this module is the single place that decides what of it is
 * allowed to reach the composer. */

/** Language of the fenced block an agent writes to render a widget. */
export const WIDGET_FENCE = "orx-ui";

const MAX_TEXT_CHARS = 4_000;
const MAX_VALUE_ENTRIES = 50;
const MIN_HEIGHT = 80;
const MAX_HEIGHT = 720;

export type WidgetMessage =
  | { type: "submit"; text: string }
  | { type: "resize"; height: number };

/** Injected ahead of the agent's HTML. Defines `orx.submit(textOrValues)` and
 * reports the content height so the host can size the frame. The target origin
 * is "*" because an opaque origin can't be named; the host checks `source`. */
export const WIDGET_BRIDGE_SCRIPT = `(function(){
var post=function(m){m.orx="widget";parent.postMessage(m,"*");};
window.orx={submit:function(v){post(typeof v==="string"?{type:"submit",text:v}:{type:"submit",values:v});}};
var last=0;
function size(){var h=Math.ceil(document.documentElement.getBoundingClientRect().height);if(h!==last){last=h;post({type:"resize",height:h});}}
new ResizeObserver(size).observe(document.documentElement);
addEventListener("load",size);
size();
})();`;

const clip = (value: string) => value.slice(0, MAX_TEXT_CHARS);

function describeValue(value: unknown): string {
  if (typeof value === "string") return clip(value);
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  if (Array.isArray(value)) return clip(value.map(describeValue).join(", "));
  return "";
}

/** Reduce an arbitrary `postMessage` payload to a message the host understands,
 * or `null`. Never trusts shape: anything unexpected is dropped, not coerced. */
export function parseWidgetMessage(data: unknown): WidgetMessage | null {
  if (typeof data !== "object" || data === null) return null;
  const message = data as Record<string, unknown>;
  if (message.orx !== "widget") return null;
  if (message.type === "resize") {
    const height = message.height;
    if (typeof height !== "number" || !Number.isFinite(height)) return null;
    return { type: "resize", height: clampWidgetHeight(height) };
  }
  if (message.type !== "submit") return null;
  if (typeof message.text === "string") {
    const text = clip(message.text).trim();
    return text ? { type: "submit", text } : null;
  }
  const values = message.values;
  if (typeof values !== "object" || values === null || Array.isArray(values)) return null;
  const lines = Object.entries(values as Record<string, unknown>)
    .slice(0, MAX_VALUE_ENTRIES)
    .map(([key, value]) => [clip(key).replace(/\s+/g, " ").trim(), describeValue(value).trim()] as const)
    .filter(([key, value]) => key && value)
    .map(([key, value]) => `- ${key}: ${value.replace(/\s*\n\s*/g, " ")}`);
  return lines.length ? { type: "submit", text: `Widget response:\n${lines.join("\n")}` } : null;
}

export function clampWidgetHeight(height: number): number {
  return Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, Math.ceil(height)));
}

/** Append a widget's answer to whatever the user was already typing, so a
 * submit never discards a draft. */
export function appendToDraft(draft: string, text: string): string {
  return draft.trim() ? `${draft.replace(/\s+$/, "")}\n\n${text}` : text;
}
