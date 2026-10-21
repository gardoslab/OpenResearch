import { createContext, useContext, useEffect, useRef, useState } from "react";
import { m } from "../paraglide/messages.js";
import { WIDGET_BRIDGE_SCRIPT, clampWidgetHeight, parseWidgetMessage } from "../chatWidget";
import { assembleDocument } from "./HtmlPreview";

/** Where a widget's answer goes. Absent outside the chat (artifacts), where a
 * widget still renders but its submissions are ignored. */
export const ChatWidgetSubmitContext = createContext<((text: string) => void) | undefined>(undefined);

/** A streaming fence changes on every token; rebuilding the frame that often
 * would flash and restart the widget's scripts. */
const SETTLE_MS = 400;
const INITIAL_HEIGHT = 160;

/** An agent-written interactive widget (an ```orx-ui fence) rendered in the same
 * kind of frame as HtmlPreview: scripts run, but with no access to the
 * dashboard's origin, API or storage. The only way out is the bridge, and the
 * host decides what a message from it may do. */
export function ChatWidget({ html, resolveSrc }: {
  html: string;
  resolveSrc?: (src: string) => string | null;
}) {
  const submit = useContext(ChatWidgetSubmitContext);
  const submitRef = useRef(submit);
  submitRef.current = submit;
  const frame = useRef<HTMLIFrameElement>(null);
  const [source, setSource] = useState<string | null>(null);
  const [height, setHeight] = useState(INITIAL_HEIGHT);

  useEffect(() => {
    const controller = new AbortController();
    let cancelled = false;
    const timer = setTimeout(() => {
      assembleDocument(
        `<script>${WIDGET_BRIDGE_SCRIPT}</script>${html}`,
        resolveSrc ?? (() => null),
        controller.signal,
      ).then((next) => {
        if (!cancelled) setSource(next);
      });
    }, SETTLE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
      controller.abort();
    };
  }, [html, resolveSrc]);

  useEffect(() => {
    const onMessage = (event: MessageEvent) => {
      // Many frames can be open at once; a message only counts from this one.
      if (!frame.current || event.source !== frame.current.contentWindow) return;
      const message = parseWidgetMessage(event.data);
      if (!message) return;
      if (message.type === "resize") setHeight(clampWidgetHeight(message.height));
      else submitRef.current?.(message.text);
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, []);

  return (
    <iframe
      ref={frame}
      className="block w-full my-2.5 rounded-md border border-border bg-white"
      style={{ height }}
      title={m.chat_widget_title()}
      // Never allow-same-origin: see HtmlPreview. No downloads either — a
      // widget gets what the bridge gives it.
      sandbox="allow-scripts allow-popups"
      referrerPolicy="no-referrer"
      srcDoc={source ?? ""}
    />
  );
}
